# Bounded testnet stress controller

Implementation tracker: #1404. Design: #1401/#1403. Offline native signer: #1402/#1405.

## Operation

The controller runs as a dedicated unprivileged systemd user service (lingering enabled) on `loom-worker-1`, separately from all five testnet nodes. Its state is `/home/ubuntu/.local/share/botho-stress-1404`. Code and the signer are installed under a versioned `/home/ubuntu/.local/lib/botho-stress-1404-*` directory with source hashes checked at startup. Local recovery material is protected outside the repository under `~/.local/state/botho-stress-1404/`.

The launch configuration pins the node and signer hashes, controller source file digests, endpoint identities, wallet allowlist and setup start. The durable SQLite journal pins its configuration digest and every submitted intent. Signed artifacts, wallet-linked inventory and private recovery material are not public evidence.

Setup has eight hours, at most 24 ordinary one-BTH grants spaced 15 minutes apart, and at most 64 bootstrap self-transfers. The first grant is not a proof of wallet spending. Bootstrap transactions split wallet outputs as eligible inputs become available. Setup must establish four mature outputs per wallet, production decoy availability, successful native four-input preparation probes, full rescan agreement and exact accounting. It then records immutable T0/end timestamps and automatically starts the 692-offer, 72-hour schedule. If these gates cannot be met, the run stops as incomplete.

Input selection uses the largest-value eligible outputs in deterministic order.
For a payment or four-input readiness probe, every ring must retain at least 19
canonical, age-matched decoys after excluding **all selected real target keys**.
The per-wallet inventory count describes individually eligible outputs; it does
not establish that a four-input selection is feasible. Setup checks all eight
complete selections before any probe and repeats that check after its full
rescan, using the refreshed height. An insufficient pool waits within the
original eight-hour deadline without signing or paying for redundant splits.
This deliberately conservative policy does not search alternative input
combinations if the largest-value prefix is infeasible. The native signer
remains the final gate; unrelated signer failures still stop the run.

No missed-slot replay or automatic transaction rebroadcast occurs. A crash after the submission marker retains the hash, inputs and fee reservation and reconciles by hash. A crash before the marker can make its first submission only after querying all five nodes and only within its original slot. Orphaned/partial prepared files cause a hold. Uncertain grant responses consume their funding slot.

The public recovery stage restores a clean wallet profile and exercises controller exits after durable preparation and after recorded acceptance. The latter keeps acceptance evidence; an actually lost HTTP response is tested in the journal recovery tests and remains a hold during public operation. No public node outage, partition, miner change, CT activation, wallet application deployment or chain reset is performed.

## Observation and stop behavior

Each node runs `botho-stress-observer-1404.service`, independently of RPC. It samples resources every five seconds, exports the last 30 records, and retains bounded rotated local logs. This is deliberately more frequent than the design's idle minute: resource collection makes no RPC calls, and its measured overhead is recorded in the launch evidence. Collection expires after 82 hours.

The controller holds only a dedicated SSH key restricted to the fixed observer export command. The unit requests a private user/mount namespace and home masking, but the worker did not apply that isolation (the live process retained the host mount namespace). Files are therefore protected by owner-only permissions under the existing Ubuntu account; this is not a separate OS security principal. No operator private SSH key is present on the worker or copied by deployment; outbound observer SSH uses `IdentitiesOnly=yes`. Shared per-endpoint RPC quotas include all controller reads/writes and persist across restarts. Every 15 seconds it checks node identity, independent resources, sync and common-height block hashes. Receipt polls run every 30 seconds. All money arithmetic uses integers.

A health/accounting/identity failure pauses admission and requires explicit investigation. A signed payment unresolved for 15 minutes, the eight-hour setup deadline, or the 72-hour end stops admission. Read-only reconciliation is bounded to another 30 minutes. A stopped run cannot silently move its end or refill missed events. The service itself has an 82-hour runtime ceiling and bounded CPU/memory; the controller also checks its own disk headroom.

Accounting records `legacy_lottery` evidence when distinct owned outpoints share
a key image and at least one is a lottery award in the checked output history.
It reports affected outpoints, award amounts, spent status and the subtotal of
awards whose shared image is already spent. The original exact `difference`
remains unchanged. Aliased awards block qualification even when that difference
is zero, because their values are not independently spendable. Reports containing
this evidence cannot mark final reconciliation verified. Archived reports without
the new field retain their existing interpretation; absence is not proof that
legacy payouts were checked. This diagnostic does not repair legacy claims or
activate LotteryV2 (#1286).

To stop admission, create an owner-readable `STOP` file in the state directory. Do not delete the journal, edit the launch manifest, rebuild a transaction, or restart the service as a means of clearing a hold. Review the evidence and create a separately identified decision/run if continuation is warranted.

## Results

- `report.json`: current sanitized status, phase counts, fees, accounting, latency, fleet and resource snapshots.
- `reports/`: six-hour snapshots; the current report is also updated on hold, phase transition and completion.
- `activated.json`: exists only once setup actually passes; immutable workload start/end.
- `journal.sqlite`: durable intent/input/request/transition records (protected).
- `artifacts/`, `probes/`, `wallets/`, `inventory.json`, `blocks.json`: protected operational state.

The controller reports actual coverage. Native headless signing does not establish web wallet, MetaMask Snap or confidential-amount acceptance. The full showcase gates remain open.

## Verification before activation

`python3 -m unittest discover -s scripts/stress -p 'test_*.py'` includes offline plan validation, a fake-clock traversal of all 692 offers, input/fee/submission/RPC bounds, durable crash markers, missed-slot/phase holds, and resource/network breakers. The signer has separate production loopback RPC/mempool/ledger receipt → spend → restore → spend acceptance and protected artifact tests. A live read-only preflight verifies five identities/checkpoints, restricted observer access, full-chain scans and zero opening wallet balances.

The deployment script has explicit `observers` and `controller` operations. It uses operator credentials only on the local machine, refuses to overwrite an existing experiment journal, stages owner-only private files, and installs versioned units. Invoking its controller operation activates setup; it must follow the acceptance gates above.

## Fresh isolated campaign profiles

Historical launch files without `targets` retain the original five destinations.
A fresh profile must explicitly map all five **logical roles** in this order:
`seed.botho.io`, `seed2.botho.io`, `faucet.botho.io`, `eu.seed.botho.io`,
`ap.seed.botho.io`. These become journal role labels, not network destinations.
The faucet role must point to the new funded producer. Each entry is:

```json
{"role":"seed.botho.io","rpc_url":"https://n1.example.internal/rpc","observer_ssh_target":"botho@n1.example.internal"}
```

The five HTTPS URLs must be distinct and exactly equal `plan.endpoints` in order;
all five SSH targets must also be distinct. URLs cannot carry credentials,
queries, fragments or alternate paths. HTTPS supports an explicit port; observer
SSH uses port 22. Missing or partial explicit profiles fail before journal or
network access. Changing only `plan.endpoints` never retargets a legacy run.
New profiles disable environment HTTP proxies, retain redirect refusal and TLS
hostname/certificate verification, and include the actual mapping in reports.
They require `known_hosts_sha256` alongside `known_hosts`; strict host-key checking
and `IdentitiesOnly=yes` remain enabled. `tls_ca_file` plus `tls_ca_sha256` can pin
a private CA bundle for this process only; neither global trust nor TLS checks
are changed. Launch/profile digests still prevent editing an initialized run.

`deploy.py stage --profile /private/profile.json --output /private/fresh-bundle`
validates every input and prepares protected local bundles without SSH. Adding
`--execute` installs **inactive, disabled** services using explicit operator SSH
credentials; it never starts nodes, observers, controllers or funding. Both the
output directory and every remote campaign state/package/unit/control-file leaf
must be absent. Existing parent directories are allowed; control parents must
be root-owned and not writable by other users. Packages, units, authorized keys
and export wrappers are root-owned. State and wallets belong to the configured
service account. Operator credentials are never shipped. This mode cannot fall
back to the legacy deploy target or old binary pins. Legacy `observers` and
`controller` deployment commands retain their original behavior and are **not**
the fresh isolated launch path.

The deployment profile is version 1 and contains these fields:

- `run_id`: unique lowercase campaign name; every new state/package leaf and
  unit name contains it.
- `plan_file`, `signer`, `adapter_source`, `node_sha256`: reviewed local inputs
  and explicit source/artifact identities. Update the copied plan's `node_commit`
  and endpoint list for the new cohort; keep budgets, schedule and genesis honest.
- `setup_start`, `end`: fixed UNIX seconds. `end` is the independently enforced
  infrastructure cutoff and must leave 80.5–82 hours after `setup_start` for the
  eight-hour setup, 72-hour workload and 30-minute drain. If prewarming for 30
  minutes, `setup_start = cohort_activation + 1800`; freeze the host cutoff from
  provisioning and separately verify enough time remains. The preparer does not
  extend the host cutoff or deploy its enforcement.
- `targets`: the five ordered mappings above.
- `controller`: `deploy_ssh_target` (operator `user@host`), unprivileged service
  `user`, fresh `state`, root-owned `package` under `/opt/` or `/usr/local/lib/`,
  and fresh `.service` `unit`.
- `observers`: five entries in role order, with the same component fields plus
  `role`, `node`, `export_wrapper`, and `authorized_keys_file`. `node` contains
  **all** of `node_unit`, `binary_path`, `data_dir`, `config_path`, `node_key_path`.
  The observer service user must equal its configured SSH export user and must
  be able to read its own node's process/config/key metadata. Sharing the node's
  dedicated account permits this without sudo. Infrastructure must separately
  ensure the node unit can write only node data, not observer evidence.
- `operator_identity`, `operator_known_hosts`: local operator access only.
- `observer_key`, `observer_public_key`, `observer_known_hosts`: fresh restricted
  export credentials/trust. Verify the public/private key pair before staging.
  Staging installs a root-owned fixed export wrapper under `/usr/local/libexec/`
  and an exclusive root-owned authorized-keys file under `/etc/ssh/authorized_keys/`.
  The key is restricted to `controller_ip` and the wrapper; the controller passes
  no remote command. Infrastructure must configure sshd to use this authorized
  keys file, disable forwarding/TTY, and enforce the same fixed command.
- `controller_ip`: the private source address allowed for observer export.
- `hosts_file`: a private-address hosts file covering every RPC/SSH target name.
  Its hash is pinned and it is mounted read-only as `/etc/hosts` **inside the
  controller unit only**. No host-wide or public DNS aliases are changed.
- `wallets`: eight distinct `{address,key}` records for fresh owner-only mnemonic
  files. Staging never adopts old wallets, journals or signed artifacts.
- Optional `tls_ca_file`, `tls_ca_sha256`: the verified private CA described above.

OS accounts, private routing/firewalls, TLS servers, node deployment and absolute
host shutdown timers are infrastructure prerequisites, not effects of this
staging command. Verify the staged source/binary/profile hashes, forced export,
private TLS identities, five-node genesis/quorum readiness and zero opening
wallet balances before separately activating observer and controller units.
Activate before the frozen setup time; do not modify staged launch files or
reuse a missed attempt. The service's `RuntimeMaxSec` is defense in depth and
resets on restart; the independent absolute host deadline remains required.
Controller recovery-stage exits need `Restart=on-failure`, while a held journal
must remain held. A 72-hour run starts only when the existing inventory, decoy,
full-rescan, native four-input and exact-accounting gates pass.

A fresh isolated cohort with stock testnet genesis is a separate operational
campaign, not a new cryptographic network or historical-chain recovery. Its
results must identify the new endpoints and effective quorum. Continuous mining
does not establish idle-memory coverage; the existing observer/controller checks
must actually observe mining stopped. No unrelated issue-closure requirement is
introduced by this profile.

## Opt-in discovery campaign (schema 2)

`testnet-discovery-v2.json` replaces sparse endurance scheduling for **fresh,
isolated** campaigns. It does not alter, migrate, resume, or reinterpret retained
schema-1 journals. The purpose is finding failures under repeated work, mixed
transaction shapes, restored wallets, and changing load. A nominal schedule is
not a measured capacity result.

| Hours after T0 | Work |
| --- | --- |
| 0–1 | 4 payments/minute; explicit 1-, 2-, and 4-input probes at 1x/2x/4x fees |
| 1–24 | Continuous 4 payments/minute |
| 24–30 | The one long idle window; monitoring and receipt reconciliation continue |
| 30–42 | 12 payments/minute, with the final five minutes of each hour at 30/minute |
| 42–48 | 4 payments/minute after an independent wallet restore check |
| 48–72 | Repeat at 4 payments/minute, then final reconciliation |

The default expands to **22,680 campaign offers**, plus **540 rehearsal offers**.
A pinned integer seed chooses wallet order, recipients, amounts, and submission
ingresses reproducibly. Three adjacent fee-tier offers share their amount;
the first nine cover matched 1/2/4-input cohorts. Subsequent payments can combine
up to four inputs. The selector searches at most 2,516 combinations among the
largest sixteen canonical eligible outputs. It excludes the entire selected
set from every ring and preserves the production ten-block age floor and 19
decoys. Structured selection evidence distinguishes maturity, unavailable or
reserved inputs, insufficient decoys, and value/complete-set failures. The seed,
intent, selected inputs, native artifact, and failure transition preserve a
counterexample for replay **offline**, never resubmission.

Phase boundaries allow up to 120 seconds for the preceding phase's natural
inflight tail to reconcile. Offers keep their original times and deadlines;
there is no catch-up or clock shift. Missed offers, accounting differences,
unresolved spends, node identity changes, and disagreeing receipts still prevent
escalation. Existing journal crash/ambiguous-submit checks apply to both profiles.
V2 restores a wallet but does not automatically kill validators or restart a
controller; those fault semantics continue to have offline fault-injection tests.

### Rehearsal-only live qualification

Before committing to a new 72-hour campaign, use a **fresh** isolated v2 plan
and deployment profile with `"execution_mode": "rehearsal_only"` in both.
Set the plan's `setup_deadline_hours` to **1**. Keep the 72-hour phase blueprint
for validation; the planner returns **zero campaign events** in this mode and
budgets only the rehearsal's writes (540 with the default cadence).
Omitting `execution_mode` preserves the existing full-campaign behavior.
Unknown modes, a mode mismatch between documents, legacy rehearsal-only plans,
and the wrong setup duration are rejected.

Set the deployment profile's absolute `end` to **1.5–2 hours after `setup_start`**.
This allows at most one hour for admission/setup and a 30-minute failure drain;
controller and observer units receive that short runtime ceiling. Install an
independent absolute host shutdown deadline at the same infrastructure cutoff;
systemd runtime ceilings alone reset on a service restart and do not stop EC2
billing. Complete provisioning, prefunding, output maturity, signer installation,
RPC/observer access and artifact pinning **before** this short qualification
window. Stage into new run-specific paths/accounts, never an old journal.

All ordinary rehearsal gates remain: measured submission cadence and receipts,
exact full-scan accounting, post-rehearsal input/decoy inventory, fresh fee
observations from all five nodes, and the final health/admission gate. A pass
ends with `status: rehearsal_complete` and `qualification_status: passed`.
It creates no `start`, `end`, or `activated.json` and makes no campaign offers.
The controller exits cleanly; restarting it remains terminal. Reported campaign
`workload_delivery` is `not_requested`, and fee/72-hour `coverage_status` remains
`incomplete`. A failure remains held/incomplete and cannot become a qualification
pass merely because its payments later reconcile.

Promotion is a separate fresh full-campaign launch with its own fixed deadlines,
opening inventory and required rehearsal. Changing either document in a persisted
qualification journal fails its immutable digest check. A qualification pass is
initial generator evidence, not proof of sustained 72-hour capacity or production
congestion/priority coverage.

### Inventory and short rehearsal prerequisites

Use the existing explicit `deploy.py stage --profile ... --output ...` path,
with the new plan file, `isolated_discovery: true`, and `opening_balances` containing
one exact integer balance per wallet. Provision **32 separate isolated wallets**
and their funds before launch. The controller scans the real chain, verifies the
pinned opening balances, includes them in exact accounting, and never invokes
the public faucet or manufactures funding outputs. All campaign packages,
including the new Python modules, are pinned by staging. Install a signer that
implements #1474's `fee_multiplier`, `baseline_fee`, and actual signed-fee
response before attempting this profile.

Every wallet needs at least sixteen mature, independently spendable canonical
outputs and a feasible four-input selection with **four spare decoys in every
ring after excluding all four real inputs**. This is initial headroom, not a
claim that a finite inventory can sustain 72 hours. Funding distribution must
also cover amounts plus the maximum signed fee; large balances alone do not
establish suitable denominations or age-band liquidity.

Before T0, the controller actually signs, submits, and reconciles 18 minutes at
30 offers/minute, followed by two minutes with no new offers and at most five
additional minutes of bounded receipt drain. All 540 offers must reconcile;
the controller then performs a full rescan, exact accounting, fresh fee reads
from all five nodes, and the inventory/headroom checks again. A failed rehearsal
records `generator_limited` and cannot activate a 72-hour clock. Calibration
checks the original offer timestamps and finite, ordered submission/receipt
records. Each submission must follow its offer by at most four seconds at the
default two-second cadence (in general, twice the interval, bounded to 1–5
seconds and never beyond offer grace). Every rolling approximately 60-second
submission span and the whole trial must match their nominal span within **one
second** of absolute jitter. This tolerance accommodates small RPC/scheduler
jitter; it cannot accumulate into a percentage throughput allowance. Reports
calculate achieved rate from actual first/last submission times and provide
rolling-window rates, maximum offer lag, cutoffs, and specific failure reasons.
A 2.2-second generator is measured at 27.27/minute and rejected even if every
receipt eventually arrives. Catch-up batches, changed offer times, non-finite
or future timestamps, and submissions beyond the immutable cutoff also fail.
Receipt drain time never enters the achieved generation-rate denominator.
Signed artifacts
and reservations remain preserved. Setup still has an absolute eight-hour limit,
and the 72-hour run plus final drain must fit the original infrastructure cutoff.

This synchronous, subprocess-based scanner/signer has **not been qualified at
30/minute on the dedicated hosts**. RPC quotas include all observation, scan,
receipt, and submission calls, using at most half each endpoint's advertised
quota. The old default of 50 calls/minute can therefore be the rehearsal's
limiting factor. A rehearsal failure is useful evidence about the generator,
inventory, or configured quotas; it is not evidence of network saturation.

For an isolated qualification deployment, each node can explicitly provision a
larger ordinary RPC quota in its `config.toml`:

```toml
[rpc]
requests_per_minute = 600
```

The default remains 100. Values must be integers in `1..=10000`; invalid values
fail configuration loading. This sets the actual default per-API-key bucket
quota, including the shared anonymous bucket, and the HTTP handler advertises
that same enforced value in `X-RateLimit-Limit`. It does not enable dev RPC or
change authentication. The existing explicit testnet dev-RPC override still
takes precedence when enabled; leave it disabled for qualification. Restart the
node to apply configuration and verify its response headers before rehearsal.
With 600 advertised, the controller still uses at most 300 calls/minute per
endpoint, subject to its own pinned profile limit. Increasing this quota does
not establish transaction throughput or congestion coverage.

Profile ceilings are enforced in expansion, admission, durable preparation,
submission, and RPC reservation: 64 wallets/inflight, 128 queued offers,
60 ordinary offers/minute, 100,000 signed attempts, 600 RPC calls per endpoint
per minute, 256 KiB per signed artifact, and 32 MiB wire bytes/minute. Actual
profile limits can be lower. Rehearsal consumes the same immutable write and fee
budgets; the planner reserves the worst-case fee for every offer. The shipped
profile caps individual fees at 1,000,000,000 picocredits, total fees at
100,000,000,000,000, and opening principal at 200,000,000,000,000.
Increasing workload requires a separately pinned profile, sufficient isolated
infrastructure and wallet inventory, and a successful rehearsal at its peak rate.
It cannot retrofit a stopped campaign or raise an existing journal's limits.

### Fee evidence and honest coverage

The signer receives a 1x, 2x, or 4x multiplier **after** the production minimum
fee is applied. Reports distinguish the baseline, multiplier, actual signed and
paid fee (which can absorb dust), and raw serialized-byte density. Raw wire
fee/byte is **not** production mempool priority density: production also uses
estimated size and the cluster factor. The controller quotes the actual chosen
ingress before signing, observes `fee_getRate` on all five nodes, and journals
per-block transaction counts/hashes and bounded observation gaps. Receipts verify
the signed fee, recipient, change, and spent inputs on every node.

These are separate claims:

- Signed fee tiers require reconciled matched input/output-shape cohorts.
- Dynamic activation requires observed 3-second slots, EMA fullness above 75%,
  active adjustment, and a quote above its floor.
- Quote recovery requires a later block on the **same node**, reduced fullness,
  and the baseline quote. One hot node and a different cold node cannot pass it.
- Priority remains **not exercised** without a trace proving that competing
  transactions were available in the same proposal opportunity. Different fees
  or different confirmation latencies do not establish priority.

Default production capacity is 100 transactions/slot. Sustaining >75% fullness
at three seconds requires roughly **25+ transactions/second**. Even this
controller's 60/minute hard ceiling is insufficient. Raising the JSON rate does
not solve it: a separately reviewed, parallel generator and enough mature
age-matched inventory, proposer-side contention/selection instrumentation, RPC
capacity, and resource headroom are required. Production integration coverage
in #1476 complements these observations but cannot establish live coverage.
Fee quotes can also rise without changing signed fees because `MIN_TX_FEE`
(100,000,000 picocredits) masks the change.

Reports separate workload delivery, discovery coverage, and node health. If all
22,680 payments finish but required production fee behavior is unexercised, the
workload delivery says complete while overall coverage and run status remain
**incomplete**, with the missing behavior explained. A healthy fleet or a larger
signed fee never silently passes unexercised fee tests. A new report derives
planned/offered/submitted/reconciled/skipped counts from the actual manifest and
journal, including separate rehearsal results; it does not use the legacy 692
constant.

Run all offline checks with:

```sh
python3 -m unittest discover -s scripts/stress -p 'test_*.py'
python3 scripts/stress/plan.py --plan scripts/stress/testnet-discovery-v2.json
```

CI runs this entire suite. It includes sparse-decoy/value starvation replay,
complete-selection exclusion, input reuse and crash-marker faults, quota and
fee-budget overruns, skipped-offer and inflight phase boundaries, seeded
reproducibility, stale/missing fee evidence, and one-picocredit accounting faults.
No live rehearsal, fee activation, or new 72-hour campaign is established by
these offline checks.
