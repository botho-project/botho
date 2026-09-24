# Snow fork deployment and stress campaign — September 24, 2026

**Status at September 24, 00:57:23 UTC: fleet rollout complete; fresh setup is running with one reconciled funding grant. The 72-hour workload has not started.**

This experiment tests the reviewed Snow AEAD migration on the public testnet while upstream integration remains pending. It continues dependency work tracked in [#1429](https://github.com/botho-project/botho/issues/1429). Successful deployment or funding alone does not establish a completed stress campaign or resolve the remaining upstream dependency work.

## Software and validation

| Component | Exact revision or artifact |
| --- | --- |
| Botho node source | `c4a8fa5ae38a0cb0702daa1d29caadfb5f132d23` |
| Source base | Main `85039ef0`, including the nonblocking minter shutdown fix [#1420](https://github.com/botho-project/botho/pull/1420) |
| Snow fork | `rjwalters/snow` at `8192d73c676ecdc93f91c5621caaa95245969ccc` |
| Node executable SHA-256 | `cbb521b49b3682ff0b3fd8eecc3b815157f36ff345036eea659c6648d5117630` |
| Previous deployed Botho source | `6dbcb92463214694f3122a05c9c48986d4f4f1b7` |
| Previous executable SHA-256 | `88c169423fdc003a5dbc6d688852fdbcda26b78f85cd371de29d9192571c674f` |
| Stress controller | `3e6dc9cb7dfb674fc3f6bd7bbbb32f5ce1ca277a`, reviewed in [#1430](https://github.com/botho-project/botho/pull/1430) |
| Native wallet adapter | `57d647597b5d438ac281ae66cc3593ca7ebcc259` |
| Adapter executable SHA-256 | `23ba0ef227697d40970f3d7b55669635b617d8317a66f2a39e8ad1fe627f6c93` |

The [Linux build and focused test run](https://github.com/botho-project/botho/actions/runs/35939330921) passed **18 network tests and 11 minter tests**, with **2 minter tests ignored**. These are focused checks, not a claim that the entire repository test suite or all platform configurations passed.

The Snow changes update AES-GCM alongside ChaCha20-Poly1305, migrate the resolver to the AEAD 0.11 `AeadInOut` interface, and preserve detached-tag handling for AES-GCM, ChaCha20-Poly1305, and XChaCha20-Poly1305. Local validation passed default, sparse-feature, ring-resolver, and ring-accelerated configurations, including the Cacophony, Snow, and extended vector corpora. Full upstream platform/MSRV CI remains separate. The contribution is [snow#219](https://github.com/mcginty/snow/pull/219), following [snow#214](https://github.com/mcginty/snow/pull/214).

The controller pacing fix passed **25 Python tests**. A funding offer now waits when the previous actual submission was less than 900 seconds ago, while retaining its original 120-second admission window. An expired offer is skipped; it is not replayed. The 24-grant count, three grants per recipient per day, chain-write budget, durable submission marker, and original setup deadline remain enforced.

## Fleet rollout

Deployment proceeds sequentially: seed, AP, EU, seed2, then faucet. Before each replacement, the operator verifies the installed and running executable, expected source, node identity, shared genesis, common-block ancestry, and a fleet height spread of at most three blocks. Each host retains its original executable and gets a persistent 30-minute rollback timer before its replacement is installed. Promotion requires the new executable to be running and synced; promotion and rollback share a lock to prevent a queued timer from undoing a completed promotion.

Pre-deployment checks found a separate existing sync failure, tracked in [#1431](https://github.com/botho-project/botho/issues/1431): nodes could report `stalled` and 100% sync progress despite reaching the common chain tip. Timeouts from earlier requests can unconditionally enter the failed sync state; request correlation and overlapping status probes need investigation. This was observed on the previous deployment before the fleet upgrade. It is not established as a Snow regression, and #1420 does not fix that sync state machine.

One ledger-preserving faucet restart briefly restored full sync, but the old-base symptom recurred. The reviewed deployment recovery option permits an unsynced **old-base** node only when it has at least three peers and the fleet still has at least three synced nodes. Common ancestry, identity, source, and height checks remain mandatory. This exception never applies to the new source, the final fleet gate, or the stress controller.

A transient TLS read interrupted orchestration after seed, AP, EU, and seed2 had been promoted and before faucet deployment. Resuming verified the exact source and running executable on the four completed hosts and did not restart them again. Faucet subsequently passed its post-restart gate and was promoted.

The rollout completed at **September 24, 00:51:47 UTC**. The final strict gate verified all five nodes synced at height **2656**, running the source and executable hash above, with three or four connected peers each. All agreed on common block **2655**, hash `f17e584ffd7529554cd2813d4d1c7bca356b52dcd9352100babc49e9fdea6b8a`, and retained the baseline ancestry and identities. No rollout rollback timer remained active. This establishes deployment health at the recorded checkpoint, not a resolution of #1431 or completed stress acceptance.

The deployment directly replaces executables and adds rollback units; it does not rewrite wallets, node identities, economic configuration, or ledger data. Ordinary chain processing continues throughout. Protected operator evidence retains the before/after snapshots, binary hashes, promotion records, rollback metadata, and resume decisions. Wallet addresses, wallet recovery material, and private keys are excluded from this report.

## Fresh setup and workload activation

The [September 22 setup attempt](testnet-stress-launch-2026-09-22.md) remains incomplete. Its journal, grants, deadlines, and evidence are retained. This experiment prepares a separate controller state, eight newly generated wallets, and a new observer namespace.

Fresh preflight passed at **September 24, 00:53:00 UTC**. It verified all five deployed identities and executable hashes, recent resource observations, matching genesis and chain checkpoint, and all eight wallets' zero opening balances. The independent scan covered **2,657 blocks**, through height **2656**; exact accounting difference was **0**. The native scan request was **7,400,628 bytes**, below the 24-MiB preflight headroom threshold and 32-MiB signer limit. A background monitor hold or stale observation invalidates preflight success. Activation binds the checked launch template and plan, requires recent preflight evidence, and refuses an existing finalized launch or journal.

The fresh observers started after deployment at **September 24, 00:51:54 UTC**, with a fixed end of **September 27, 10:51:54 UTC**. Preflight and activation require enough remaining time within this 82-hour window for the eight-hour setup, 72-hour workload, 30-minute drain, and scheduled setup start. Existing observer deadlines are not extended.

The new controller was activated once on `loom-worker-1` as `botho-stress-snow-20260924.service`, using run ID `botho-stress-snow-20260924`. Setup start is **September 24, 00:54:29.235 UTC**, with the immutable deadline **September 24, 08:54:29.235 UTC**. The initial service checkpoint recorded PID **3577294**, **0 restarts**, and enforced cgroup limits of **one CPU, 1 GiB memory, and 64 tasks**. No workload T0 or end had been assigned at that checkpoint.

**Setup start and workload T0 are distinct.** Once explicitly activated, the controller has a fixed eight-hour setup window to obtain eligible funding, bootstrap inventory, and verify each wallet's four-input signing shape. It records `activated.json`, T0, and the immutable 72-hour end only after those gates pass. A running controller service or a confirmed faucet grant does not mean the 72-hour workload has begun.

The existing [72-hour plan](../../scripts/stress/testnet-72h-plan.json) offers 692 campaign payments, with at most 24 funding grants and 64 bootstrap transfers. The absolute limit is 800 unique chain writes. Signed fees remain capped at 5,000,000,000 picocredits per transaction and 500,000,000,000 in total; faucet fees are recorded separately. Inputs require the production maturity floor and sufficient age-matched decoys. Resource, sync, identity, confirmation, and exact-accounting breakers remain active, with no automatic write retries or deadline extension.

The first ordinary funding transaction, `5136ad79a97634e819e64ff4a949814e480154ae0ba7b76b7e9b09588e839a07`, was submitted at **September 24, 00:54:35.866 UTC** and reconciled at **00:56:54.185 UTC**, **138.320 seconds** later. All five nodes confirmed inclusion in block **2657**, hash `76d17f39626a27ff3bd5a22ec1ac4844d6491e5a2e7ec7bb089e8ad49c662457`. Recipient ownership/value checks passed. Opening balance was **0**, funding and ending balance were each **1,000,000,000,000 picocredits (1 BTH)**, harness-signed fees were **0**, and accounting difference was **0**. The faucet fee was **100,000,000 picocredits (0.0001 BTH)**, recorded separately.

The subsequent **00:57:23 UTC** controller checkpoint remained in `setup`, with no hold and zero controller restarts. Its recent fleet observation showed all five nodes synced at height **2658**, tip `5ade93fcd3be85292d9bfcfd53c6fbb0df7f73583975dbb09684118db9185204`. No wallet yet had an output meeting both maturity and decoy requirements. Workload T0 and end remained unset. The [sanitized machine-readable evidence](testnet-stress-evidence/2026-09-24-snow-launch.json) records the deployment, preflight, activation, receipt, accounting, and observation checkpoints.

A known setup constraint remains under observation: the sole faucet minter's idle pause may limit block progression needed for input maturity and 19 age-matched decoys. Bootstrap transfers themselves require an already eligible input. Wall-clock waiting alone does not mature outputs. This has not established a readiness failure in the current run. As the [stress program](testnet-stress-program.md) specifies, inability to qualify within the existing setup window must be recorded as a product constraint; no extra grants, unplanned minter, or weaker eligibility rules may manufacture readiness.

Further setup results, workload activation, and final disposition must be recorded after they are observed. This funding receipt does not establish native spending, load, browser, Snap, confidential-amount, or external-audit acceptance.

## Read-only observation

Inspect the service and a selected journal summary without opening the controller's exclusive lock or invoking its report command:

```sh
ssh loom-worker-1 'systemctl --user show botho-stress-snow-20260924.service -p ActiveState -p SubState -p MainPID -p NRestarts'

ssh loom-worker-1 'python3 -' <<'PY'
import json
from pathlib import Path
import sqlite3

state = Path('/home/ubuntu/.local/share/botho-stress-snow-20260924')
db = sqlite3.connect((state / 'journal.sqlite').as_uri() + '?mode=ro', uri=True)
db.execute('PRAGMA query_only=ON')
db.execute('BEGIN')
keys = ('status', 'reason', 'setup_start', 'start', 'end', 'stopped_at')
summary = {}
for key in keys:
    row = db.execute('SELECT value FROM meta WHERE key=?', (key,)).fetchone()
    summary[key] = json.loads(row[0]) if row else None
summary['counts'] = [dict(kind=kind, state=status, count=count)
    for kind, status, count in db.execute(
        'SELECT kind,state,COUNT(*) FROM intents GROUP BY kind,state')]
summary['workload_activation_recorded'] = (state / 'activated.json').exists()
db.rollback()
db.close()
print(json.dumps(summary, indent=2))
PY
```

The journal is authoritative for current status; a prior report file may lag a hold. A missing workload start or `activated.json` means setup has not yet qualified the 72-hour run. Investigate any hold before further action; restarting must not reset the journal, clear reservations, replay offers, or extend deadlines.
