# Runbook: Bridge Order Engine Recovery

Procedures for operating the BTH bridge service (`bth-bridge`): the circuit
breaker / kill switch, crash and restart behavior, stuck-order triage,
peg-drift incident response, signer outages, key rotation, and backup/restore
of the bridge database.

**Target RTO:** 5-30 minutes
**Severity:** Critical (custody of the locked BTH reserve)
**Owner:** Infrastructure

---

## Scope and design recap

The bridge engine (#816) drives orders through a strict state machine,
enforced at the database layer (#839 — no writer can skip a state or clobber
a terminal state):

```
Mint (BTH -> wBTH):  awaiting_deposit -> deposit_detected -> deposit_confirmed
                     -> mint_pending -> completed
Burn (wBTH -> BTH):  burn_detected -> burn_confirmed -> release_pending -> released
Error states:        failed (any non-terminal), expired (pre-deposit only)
```

Exactly-once machinery (all in the SQLite bridge DB):

- `mints` — one row per order that ever had a mint tx prepared; written
  BEFORE first broadcast. The contract-side order-id guard (#826) is the
  on-chain backstop.
- `release_claims` — claim taken before signing; signed tx hash + raw bytes
  recorded before first broadcast. BTH has **no on-chain guard**: this table
  is the only double-release protection. Never delete rows from it.
- `processed_deposits` / `processed_burns` / `watcher_cursors` — watcher
  idempotency + resume points.
- `reserve_ledger` — the locked reserve, derived from outputs (never a
  mutable counter); reconciled against on-chain supply every
  `reserve.reconcile_interval_secs` (#825).

**Crash-safety guarantee (verified by the chaos suite in
`bridge/service/src/chaos_tests.rs`):** killing the service at ANY point and
restarting never double-mints, never double-releases, and never strands an
order — startup recovery (`recover_on_startup`) rolls forward orders caught
in the deposit-detection window (#843), and the submit stages resume from
the idempotency tables. **A plain restart is therefore always safe** and is
the first response to most anomalies.

---

## Monitoring surface

All endpoints on `[reserve] api_listen` (default `127.0.0.1:9741` —
localhost-only; reaching it means you are an operator):

| Endpoint | Purpose |
|---|---|
| `GET /health` | `200 OK` operating; `503 paused: <reason>` when the breaker is tripped |
| `GET /api/status` | Pause state, order counts per status, actionable backlog, stuck-order count, component health, latest peg verdict |
| `GET /metrics` | Prometheus text for `bridge/service/alerts.yml` |
| `POST /api/breaker` | Kill-switch toggle (below) |
| `GET /api/reserve/proof` | Latest proof-of-reserves snapshot (`pegHealthy`, `reserveBalanceChecked`, drift) |

```bash
curl -s http://127.0.0.1:9741/api/status | jq
```

The audit trail lives in the `audit_log` table; key alert actions:
`breaker_tripped`, `breaker_resumed`, `stuck_order_alert`, `rate_limited`,
`reserve_drift_alert`, `deposit_recovered`.

---

## Kill switch / circuit breaker

Pausing halts the **submit** stages (no new mint or release leaves the
bridge). **Confirm** stages keep running so already-broadcast transactions
settle to a durable terminal state. The pause state persists in the DB
(`bridge_state`) across restarts.

### Pause (freeze the bridge)

```bash
curl -s -X POST http://127.0.0.1:9741/api/breaker \
  -H "Content-Type: application/json" \
  -d '{"paused": true, "reason": "incident <ticket>"}'
```

Alternatives when the API is unreachable:

- Config: set `paused = true` under `[bridge]` and restart — the service
  starts paused.
- On-chain (strongest, mint side): pause the `WrappedBTH` contract itself
  (`pause()` via the Gnosis Safe, #826) — stops mints even if the service
  host is compromised. Contract pause and service pause are independent
  layers; for a custody incident engage **both**.

### Automatic trips (fail closed)

| Trigger | Source |
|---|---|
| Reserve drift alert (`pegHealthy = false`) | reconciler, every pass |
| Actionable backlog > `bridge.max_pending_orders` (default 1000) | engine tick |
| Global daily volume cap reached (`bridge.global_daily_limit`) | rate limiter |
| `bridge.paused = true` in config | startup |

Trips are audited (`breaker_tripped`) exactly once. Per-address and
per-order rate-limit rejections do **not** trip the breaker; they defer the
individual order (audited `rate_limited`).

### Resume — safety checklist

1. Root cause identified and fixed (see the incident sections below).
2. `GET /api/reserve/proof` shows `pegHealthy: true` on a FRESH pass
   (`takenAt` newer than the fix). The reconciler re-trips on the next
   unhealthy pass, so resuming with a bad peg just re-pauses.
3. `GET /api/status` shows the expected backlog and no unexplained
   `stuckOrders`.
4. Resume:

```bash
curl -s -X POST http://127.0.0.1:9741/api/breaker -d '{"paused": false}' \
  -H "Content-Type: application/json"
```

5. Watch `bridge_actionable_backlog` drain and the first orders complete.

---

## Incident: peg drift alert (`reserve_drift_alert`)

The reconciler compares on-chain wBTH supply vs the ledger-locked reserve vs
(when the transport is wired) the actual reserve-address balance.

1. **Freeze.** The breaker trips automatically; verify with `GET /health`.
   For positive drift (unbacked supply — possible mint-authority
   compromise) ALSO pause the `WrappedBTH` contract via the Safe.
2. **Read the evidence.** The full per-chain detail is in the audit log:

   ```bash
   sqlite3 bridge.db \
     "SELECT created_at, details FROM audit_log
      WHERE action IN ('reserve_drift_alert','reserve_reconcile')
      ORDER BY id DESC LIMIT 5;"
   ```

   - `drift > 0` (supply > locked): unbacked wBTH. Suspect an unauthorized
     mint (check the wBTH contract's event log against the `mints` table)
     or a missed deposit unlock. Custody/key incident until proven
     otherwise — see key rotation below.
   - `drift < 0` beyond the in-flight allowance: supply that should exist
     does not, or the ledger overcounts (e.g. a failed-mint unlock that
     could not cover — audited as `reserve_unlock_failed`).
   - `reserveBalanceChecked: true` with a shortfall: BTH left the reserve
     address without a burn — custody incident, rotate reserve keys.
3. **Reconcile.** Fix the ledger only with recorded evidence (every ledger
   mutation is audited: `reserve_locked`, `reserve_spent`,
   `reserve_unlocked`, `reserve_spend_failed`, `reserve_unlock_failed`).
4. **Resume** per the checklist above (fresh healthy pass required).

---

## Incident: stuck orders (`stuck_order_alert`)

Orders past `deposit_detected` never auto-expire — funds have moved, an
operator must act. Only `awaiting_deposit` orders expire automatically.

| Stuck status | Meaning | Action |
|---|---|---|
| `deposit_detected` | #843 crash window (should be recovered at startup) | Restart the service — `recover_on_startup` rolls it forward; check `deposit_recovered` in the audit log |
| `deposit_confirmed` | Mint submission failing | Check `minter:*` in `/api/status` components (config), attestation health, and `rate_limited` audits (deferred by a cap: passes at the next UTC day window) |
| `mint_pending` | Mint tx not confirming | Check the tx on the destination chain. Dropped/reorged txs unwind automatically; a tx that reverted marks the order `failed` and unlocks its backing |
| `burn_confirmed` | Release submission failing | Check `releaser:bth` component health, attestation health, `rate_limited` audits |
| `release_pending` | Release tx not confirming | Check the BTH tx. **Never** delete the `release_claims` row to force a re-sign — a merely-unseen tx can still land; only the engine's provably-dropped path may unwind it |
| `failed` releases | Landed-but-wrong release tx | Manual review; the claim is kept deliberately so any retry reuses the recorded tx |

A stuck order alerts exactly once (audit `stuck_order_alert`); the
`bridge_stuck_orders` gauge stays up until resolved.

---

## Incident: signer / component outage

`/api/status` `components` reports `attestation`, `minter:ethereum`,
`minter:solana`, `releaser:bth`. The engine fails closed: affected orders
stay retryable in their current state — nothing is dropped or failed.

- `attestation` unhealthy: the federation config is invalid
  (`[bridge] attestation_*_key_file`, `mint_signers` / `release_signers` /
  thresholds). No mint or release is authorized until fixed. Fix config,
  restart, confirm the component turns healthy.
- `minter:*` / `releaser:bth` unhealthy: missing Safe address / contract
  address / reserve address. Same treatment.

---

## Key rotation

Bridge-relevant keys and where they live:

| Key | Location | Rotation |
|---|---|---|
| Attestation Ed25519 (BTH release + Solana mint federation) | `[bridge] attestation_ed25519_key_file`; pubkeys in every node's `release_signers` / `mint_signers` | Two-step overlap: add the new pubkey to the signer lists fleet-wide, restart, verify a release authorizes, then remove the old key. Thresholds must hold at every step |
| Attestation secp256k1 (Ethereum mint / Safe owner) | `[bridge] attestation_secp256k1_key_file`; owner set on the Gnosis Safe | Rotate via the Safe's owner-management transactions (`addOwnerWithThreshold` / `removeOwner`), then update `ethereum.mint_signers` |
| Reserve wallet spend key | `[bth] spend_key_file` | A compromise is a custody incident: **pause first**, move the reserve to a fresh address, update `reserve_address`, reconcile the ledger before resuming |
| Ethereum submitter key | `[ethereum] private_key_file` | Low privilege (pays gas); rotate freely |

After any rotation: restart, check `/api/status` components, and run one
end-to-end testnet order in each direction before resuming mainnet flow.

---

## Backup / restore of the bridge DB

The bridge DB is the custody-critical state: the reserve ledger, the
exactly-once tables, and the audit log. Losing `release_claims` in
particular could allow a double release on restore-then-resume.

**Backup** (safe while the service runs — SQLite online backup):

```bash
sqlite3 bridge.db ".backup 'bridge-$(date -u +%Y%m%dT%H%M%SZ).db'"
# Also back up the attestation nonce store (replay protection):
cp bridge.db.attestation-nonces.json bridge-nonces-$(date -u +%Y%m%dT%H%M%SZ).json
```

Schedule at least hourly in production and before every deploy.

**Restore procedure:**

1. Stop the service; keep the corrupt DB aside for forensics.
2. Restore the newest backup pair (DB + nonce store).
3. Start the service **paused** (`paused = true` in `[bridge]`).
4. The watchers rescan from the persisted cursors; the idempotency tables
   absorb the replay. Startup recovery rolls stranded orders forward.
5. **Cross-check before resuming:** for every `release_pending` /
   `mint_pending` order, compare the recorded tx against the chain. A
   release broadcast AFTER the backup was taken will not be in the restored
   `release_claims` — this is the one restore hazard. Reconcile manually
   (insert the claim row from chain evidence) before resuming.
6. Wait for a fresh `pegHealthy: true` reconciliation, then resume.

> Testnet deployments are ephemeral — skip the backup ceremony there.

---

## Symptom → Action map

| Symptom | Likely cause | Action |
|---|---|---|
| `/health` returns `503 paused: reserve drift alert...` | Peg incident | Drift incident procedure above |
| `/health` returns `503 paused: actionable backlog...` | Order flood / settlement stall | Check destination-chain RPC health; drain or raise `max_pending_orders`; resume |
| `bridge_stuck_orders > 0` | See stuck-order table | Per-status triage above |
| Order stuck at `deposit_detected` | #843 window (pre-recovery build or recovery failed) | Restart; verify `deposit_recovered` audit |
| `rate_limited` audits accumulating | Caps sized below real traffic | Raise `[bridge]` caps deliberately; per-order-cap mints can never pass — refund path via operator |
| Service crash-loops | Bad config / DB corruption | `bth-bridge --migrate` to test the DB; restore-from-backup procedure |
| Repeated `breaker_tripped` right after resume | Root cause not fixed (reconciler re-trips on every unhealthy pass) | Fix first; the breaker is doing its job |

---

## Related documentation

- `bridge/service/alerts.yml` — Prometheus alert rules for this runbook
- `docs/decisions/` ADR 0002 (custody), 0003 (peg/reserve), 0005 (per-chain invariant)
- [Quorum Edit Recovery](quorum-edit-recovery.md) — operator-signed-action recovery pattern this runbook mirrors
- [Disaster Recovery](../disaster-recovery.md) — node-level procedures
