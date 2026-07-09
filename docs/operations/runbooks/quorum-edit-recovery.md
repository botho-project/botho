# Runbook: Quorum Edit Recovery

Procedure to recover from a liveness stall caused by an operator-signed quorum
edit, and to rotate or revoke operator signing keys.

**Target RTO:** 5-30 minutes
**Severity:** High
**Owner:** Infrastructure

---

## Scope

Operator-signed quorum curation (the `operator_submitAction` write path, #709)
lets an operator mutate a node's `[network.quorum]` inputs remotely, without SSH.
Every edit is routed through the existing promotion gate, so a **safety-bad edit
(a fork-capable disjoint quorum) is refused before it is applied** — it can never
take effect. The residual failure class this runbook addresses is **liveness**:
a gate-accepted edit that leaves a node unable to externalize (a stall), *not* a
fork.

Authoritative design: `docs/security/quorum-write-path.md` (§7 failure modes,
§2 key lifecycle, §3 envelope + degenerate/solo-floor rules, §6 audit log).

This runbook covers:

1. The §7.2 **recovery ladder** (three rungs, in order) for a stalled node.
2. Operator key **rotation** and **revocation** (SSH-only, off the signed-action
   surface).
3. A **symptom-to-action map** keyed on `slotStalled` and the `/network`
   dashboard.

> All `curl` examples target the local RPC port (`7101` mainnet, `17101`
> testnet). Read RPCs (`operator_getQuorumInfo`, `operator_getAuditLog`) require
> an operator read token minted with `botho operator mint-read-link`, passed as
> the `token` param. The write RPC (`operator_submitAction`) requires a signed
> envelope and reads no read token.

---

## Detection

### Symptoms

- The `/network` dashboard shows a node **stalled** (SCP slot active but not
  externalizing).
- `node_getStatus` reports `"slotStalled": true` (with `slotStallSeconds`
  climbing) while `scpSlotActive` is `true`.
- Chain height is not advancing on the affected node while peers advance.
- `mintingActive` may drop as the node stops participating.

### Confirm the stall

```bash
# slotStalled is the derived stall verdict (#653): slot ACTIVE but no
# externalization past the stall window. An idle node is never "stalled".
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  | jq '{slotStalled, slotStallSeconds, scpSlotActive, chainHeight, quorumDegenerate, quorumGateIntersectionRefused}'
```

- `slotStalled: true` + `scpSlotActive: true` → a genuine stall. Proceed to the
  recovery ladder.
- `quorumGateIntersectionRefused: true` → the gate is *refusing* the current
  candidate and keeping the previous safe set. This is a **safety refusal, not a
  stall from your edit** — your edit did not apply; see step 4 of the gate flow
  in the design (§4). Read the audit log for the refused action rather than
  compensating blindly.

### Read the operator audit log (what edit caused this)

```bash
# operator_getAuditLog returns recent authenticated outcomes (applied /
# gate_refused / verify_refused). Read-token gated.
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"operator_getAuditLog","params":{"token":"<READ_TOKEN>","limit":20},"id":1}' \
  | jq '.result.entries'
```

Look for the most recent `"outcome":"applied"` entry: its `action`, `params`,
`signerKeyId`, and `newQuorum` tell you exactly what changed and who signed it.
That is the edit to invert.

### Inspect the live quorum posture

```bash
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"operator_getQuorumInfo","params":{"token":"<READ_TOKEN>"},"id":1}' \
  | jq '.result.quorum'
# -> { mode, faultModel, threshold, members, minPeers, maxAutoMembers }
```

---

## Recovery Ladder

Apply the rungs **in order**. Rung 1 is the designed no-SSH path and resolves
the common cases; escalate only if it cannot.

### Rung 1 — Compensating signed action over RPC (no SSH)

A stalled SCP slot does **not** stop the RPC server or the node's event loop, so
`operator_submitAction` still works on a stalled node. Submit the **inverse** of
the offending edit:

| Offending edit | Compensating action |
|---|---|
| `quorum.pin_member` (pinned a now-dead peer) | `quorum.unpin_member` (same `peerId`) |
| `quorum.set_max_auto_members` (lowered the cap below need) | `quorum.set_max_auto_members` (restore the previous cap) |

The v1 action allowlist is exactly `quorum.pin_member`,
`quorum.unpin_member`, and `quorum.set_max_auto_members` (mode/threshold flips
are **not** in v1 — they stay SSH-only, §3).

**Compose and sign the envelope** (normally done by the dashboard's action
composer; the shape below is the canonical envelope that gets signed). The
signed payload is the source of truth — `dryRun` is a *signed field*, never an
RPC parameter:

```json
{
  "action": "quorum.unpin_member",
  "dryRun": false,
  "expiresAt": 1783450000,
  "issuedAt": 1783449700,
  "nonce": "<128-bit random hex>",
  "params": { "peerId": "12D3KooW..." },
  "signerKeyId": "<8-byte fingerprint hex>",
  "targetNode": "<this node's base58 PeerId>",
  "v": 1
}
```

Canonicalization / signing rules (must match, or the node refuses at step 3):

- Canonical JSON: UTF-8, keys sorted lexicographically, no insignificant
  whitespace, integers only (no floats).
- Signature: `Ed25519-sign(sk, "botho-operator-action-v1" || canonical_bytes)`,
  detached, lowercase hex (128 chars).
- `expiresAt - issuedAt <= 300` seconds; the node allows ±30 s clock skew, so
  keep node clocks on NTP.
- `nonce` is single-use per node; `targetNode` binds the envelope to exactly one
  node (fleet-wide fixes are N envelopes, one per node).

**Dry-run first** (recommended). Sign a second envelope identical except
`"dryRun": true` with its own fresh `nonce`, submit it, and read the gate
verdict for the hypothetical result before committing the real apply. A dry run
never mutates, persists, or consumes its nonce.

**Submit** (the RPC takes exactly one argument object — `envelope` (the canonical
string, verbatim) and `signature`; nothing else is read):

```bash
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"operator_submitAction","params":{"envelope":"<CANONICAL_JSON_STRING>","signature":"<HEX_SIG>"},"id":1}'
```

**Read the outcome from the node** (the response is the truth — never assume
success). On apply you get a `success` result:

```json
{
  "outcome": "applied",
  "dryRun": false,
  "signerKeyId": "...",
  "action": "quorum.unpin_member",
  "resultingQuorum": { "mode": "recommended", "members": [...], "maxAutoMembers": 8 },
  "gate": { "intersectionRefused": false, "curatedMembers": 4, "autoMembers": 1,
            "suppressedPeers": 0, "maxAutoMembers": 8,
            "faultTolerant": true, "degenerate": false }
}
```

A refusal comes back as a JSON-RPC **error** with the structured outcome in
`data`:

- `outcome: "gate_refused"` — the gate rejected the compensating edit
  (intersection would break); the previous set stays live, nothing persisted.
  Choose a different corrective edit.
- `outcome: "verify_refused"` with an `audit_tag` — verification/policy failed
  (see the tags below). Common ones during recovery:
  - `verify_refused:invalid_payload` — often the degenerate/solo-floor policy
    (see Rung 2), a bad `peerId`, or `value` out of the `0..=64` cap range.
  - `verify_refused:stale` — the envelope expired or clocks are skewed; re-sign.
  - `verify_refused:replayed_nonce` — reuse a **fresh** nonce.
  - `verify_refused:not_configured` — the node has no `action_public_keys`; this
    node has no write surface, go to Rung 3.

If the compensating action applies and the gate accepts, the node resumes
externalizing within seconds. Verify (below) and stop.

### Rung 2 — Degenerate-posture edit (deliberate, attributable)

If restoring liveness on a small fleet requires **shrinking the quorum below 4
nodes** (the BFT floor — below it the quorum degenerates to n-of-n crash-only
tolerance, #509), the node **refuses the action unless the signed envelope
carries `"acknowledgeDegenerate": true`**. This is deliberate and non-repudiable:
the acknowledgment is inside the signed payload, so the audit log attributes it
to the signer.

Add the field to the envelope before signing:

```json
{
  "action": "quorum.unpin_member",
  "acknowledgeDegenerate": true,
  "dryRun": false,
  "...": "..."
}
```

Without it, an edit whose resulting membership is `< 4` is refused with
`verify_refused:invalid_payload`.

> **Hard floor — membership 1 has NO override.** An action whose resulting
> membership would be **1 (the node alone)** is refused **outright**;
> `acknowledgeDegenerate` does **not** override it. A 1-of-1 quorum trivially
> passes the intersection check yet lets the node self-externalize and diverge
> from the fleet — a self-fork the node-local gate cannot see. **Do not attempt
> a recovery edit that drops a node to solo** — it will always be refused. If a
> node genuinely must run alone, that is an SSH-only operation (Rung 3).

### Rung 3 — SSH + `config.toml` + restart (root recovery)

The recovery that needs **no cooperation from the write path at all**. Use it
when a node has no write surface (`not_configured`), when no signed key is
available, or when Rungs 1-2 cannot express the fix (mode/threshold change, or a
legitimate solo posture).

```bash
# SSH to the affected node
ssh ec2-user@<node-host>

# Inspect the current quorum config
grep -A8 '\[network.quorum\]' ~/.botho/config.toml

# Edit [network.quorum] to the desired posture. Relevant keys:
#   mode             = "recommended" | "explicit"
#   threshold        = <u32>          (explicit mode)
#   members          = ["<base58 PeerId>", ...]   (curated members)
#   min_peers        = <u32>
#   max_auto_members = <u32>          (auto-promotion cap)
# e.g. remove a dead pinned peer from `members`, or restore max_auto_members.

# Restart the service
sudo systemctl restart botho.service

# Watch it re-seed and heal
sudo journalctl -u botho.service -f
```

On startup the promotion gate **re-seeds from config + connected peers** (#427),
and the under-connected re-dial fix (#690/#692) makes post-restart mesh healing
reliable — validated live in the v0.3.2 rolling deploy. A restarted node
re-forms quorum over the peers it reaches; no manual re-peering is normally
needed.

> Note: the write path also persists accepted edits via `Config::save`, so a
> node that stalled after a Rung-1-recoverable edit will converge to the same
> (bad) state on a plain restart. If the stall was caused by a persisted edit,
> **fix `config.toml` before restarting**, or the restart reloads the stall.

---

## Operator Key Rotation and Revocation

Operator signing keys are Ed25519. The **private key never lives on a node** —
only the public keys do, in `[rpc.operator] action_public_keys`. Key-list
changes are **SSH/config operations only**, deliberately off the signed-action
surface: a stolen key must not be able to enroll or revoke keys
(non-self-referential, §2).

### Rotation (two-step overlap, no flag-day)

1. **Generate** a new keypair on the operator workstation:

   ```bash
   botho operator keygen
   # Prompts for a mandatory passphrase (private key encrypted at rest).
   # Prints:
   #   public_key   <hex pubkey>
   #   fingerprint  <8-byte signerKeyId hex>
   # Writes ./operator-key.json (override with --output <path>).
   ```

2. **Add** the new public key to `action_public_keys` on **every** node (SSH),
   keeping the old key in place:

   ```bash
   ssh ec2-user@<node-host>
   # Edit ~/.botho/config.toml:
   #   [rpc.operator]
   #   action_public_keys = ["<old hex pubkey>", "<new hex pubkey>"]
   sudo systemctl restart botho.service
   ```

3. **Confirm** a new-key action verifies (e.g. a `dryRun` action signed with the
   new key returns a non-`unknown_signer` outcome on each node).

4. **Remove** the old public key from `action_public_keys` on every node (SSH)
   and restart. The list form makes this a rolling two-step overlap rather than
   a synchronized cutover.

### Revocation (immediate, node-local, works mid-abuse)

Remove the compromised public key from `action_public_keys` on **every** node
via SSH and restart `botho.service`. This is effective **even while the key is
being actively abused**, because config changes ride the SSH trust domain the
attacker (by assumption §8.5) does not hold. A node with an empty/absent
`action_public_keys` has **no write surface at all** (fail-closed:
`operator_submitAction` returns "operator actions not configured").

```bash
ssh ec2-user@<node-host>
# Edit ~/.botho/config.toml: drop the revoked pubkey from action_public_keys.
sudo systemctl restart botho.service
```

### Post-revocation forensics

The audit log records the **signer fingerprint** (`signerKeyId`) of every
authenticated action — applied and refused — so you can attribute any damage the
revoked key did:

```bash
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"operator_getAuditLog","params":{"token":"<READ_TOKEN>","limit":1000},"id":1}' \
  | jq '[.result.entries[] | select(.signerKeyId == "<revoked fingerprint>")]'
```

Each entry carries `ts`, `action`, `params`, `dryRun`, `outcome`,
`envelopeHash`, and (for applies) `prevQuorum` / `newQuorum`. The log is also
mirrored to `warn!`-level tracing (journald / CloudWatch) and appended to
`<data-dir>/operator-audit.jsonl`, so attribution survives even if one node's
JSONL is tampered with (the log is node-local and append-only by convention, not
tamper-proof — §6). Cross-check the same fingerprint across **every** node's log
for fleet-wide scope.

---

## Symptom → Action Map

| Observed symptom | Likely cause | Rung / action |
|---|---|---|
| `/network` dashboard shows a node stalled; `slotStalled: true`, `scpSlotActive: true` after a recent operator edit | A gate-accepted liveness-harming edit (pinned dead peer / cap too low) | **Rung 1** — submit the inverse `operator_submitAction` |
| Rung-1 apply returns `outcome: "gate_refused"` | The corrective edit would break intersection | Pick a different corrective edit; if the fleet needs it, **Rung 3** |
| Rung-1 apply returns `verify_refused:invalid_payload` and membership would be `< 4` | Degenerate posture not acknowledged | **Rung 2** — re-sign with `acknowledgeDegenerate: true` |
| Recovery seems to require membership `= 1` | Solo-quorum hard floor (no override) | **Do not attempt via RPC**; if truly required, **Rung 3** (SSH) |
| `operator_submitAction` returns `verify_refused:not_configured` | Node has no `action_public_keys` (no write surface) | **Rung 3** — SSH + config + restart |
| `verify_refused:stale` / `verify_refused:replayed_nonce` | Expired/skewed envelope, or reused nonce | Re-sign with fresh `issuedAt`/`expiresAt`/`nonce`; check NTP |
| `verify_refused:unknown_signer` | Signing key not (yet) in `action_public_keys`, or already revoked | Provision the pubkey (rotation), or use a valid key |
| `quorumGateIntersectionRefused: true`, node kept its previous set | A candidate edit was refused for safety — it did **not** apply | Read the audit log; no compensating action needed for the refusal itself |
| `operatorRejectedRequests` climbing in `node_getStatus` | Unauthenticated / pre-signature `operator_submitAction` probes | Not a stall; investigate as a security signal (these are never audit-logged) |
| Stall persists after any RPC fix, or no signed key available | Write path cannot express or reach the fix | **Rung 3** — SSH + `config.toml` + `systemctl restart botho.service` |

---

## Verification

After any rung, confirm the node resumed externalizing:

```bash
# 1. slotStalled should return to false and height should advance.
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  | jq '{slotStalled, slotStallSeconds, chainHeight, mintingActive, quorumFaultTolerant}'

sleep 30

# 2. chainHeight must be strictly higher than the reading above.
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' \
  | jq '.result.chainHeight'

# 3. Confirm the applied edit is what you intended (and attributable).
curl -s http://localhost:7101/ \
  -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"operator_getQuorumInfo","params":{"token":"<READ_TOKEN>"},"id":1}' \
  | jq '.result.quorum'
```

For a fleet-wide edit, repeat against **every** node — per-node targeting means
configs can diverge, and the trust dashboard (#707) surfaces the divergence.

---

## Related Documentation

- [Quorum Write-Path Security Design](../../security/quorum-write-path.md) — authoritative (§7 recovery ladder, §2 key lifecycle, §3 envelope, §6 audit log)
- [Network Partition Recovery](network-partition.md) — quorum recovery on peer loss
- [Seed Node Recovery](seed-node-recovery.md) — full node rebuild
- [Configuration Reference](../configuration.md) — `[network.quorum]` and `[rpc.operator]` settings
