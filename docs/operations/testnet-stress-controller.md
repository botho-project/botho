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

### Controller behavior during a degrade step

With an x-series plan (`degrade_steps`, see "Degrade phase plan" below), the
controller keeps the campaign running on the healthy validators while a step's
named hosts are faulted. Everywhere else it still fails closed. One helper
decides this: `fault_view(now)` returns the step active for
`fault_start <= now < window_end` (absolute times from the journal `start`),
with its `expected_down` hosts (`target_hosts`) and `expect`.
`live_hosts(now)` is `HOSTS` minus `expected_down`. With no active step, or
with no `degrade_steps` at all, `live_hosts()` is exactly `HOSTS` and every
check below behaves as before. A step relaxes nothing for a host it does not
name, and nothing outside its window.

- **Monitor.** It still reads every host. A failed `node_getStatus` or observer
  read from a named host is recorded as `expected_down_unreachable` and the
  host is marked `{expected_down: true}`. It is not a monitor error. Identity
  checks run on every reachable host. The unsynced timer is suspended for
  named hosts until `window_end`, and for **all** hosts during a `safe_halt`
  step. Finalized-hash agreement uses the common minimum height of every
  reachable host, so an isolated or netem'd host that is behind is still
  included.
- **Restarts.** A named host's observer ring goes to `check_resources()` with
  its step window (#1586). The one accepted restart is persisted as
  `baseline:<host>`, with `rebased_step` stored in the same row. That makes it a
  single journal write. A second non-`None` re-baseline in the same step is a
  `Gate`.
- **Rejoin deadline.** At the first monitor tick at or after `window_end`,
  each named host must be reachable, synced, and at or above the minimum
  height of the hosts that neither this step nor the active one names.
  Otherwise the run halts with `<step>: <host> failed to rejoin by window end`.
  The check is recorded once as `rejoin:<step>`.
- **Admission.** `gate()` ignores `synced` for named hosts. During a
  `safe_halt` step it raises `Quota` (a pause) for the whole window. New
  payments are hashed over `live_hosts()` and never routed to a named host. A
  row that is already `prepared` keeps its recorded ingress. Its prepared-row
  status check asks only the live hosts. A submit to an ingress that has gone
  down fails into `unknown` and halts as before.
- **Reads.** `sync()` output and spent-image reads, and `observe_fees()` block
  reads, rotate deterministically over `live_hosts()`. Scan and fee tips are
  the minimum over live hosts. `observe_fees()` skips named hosts and records
  `fee_quote_skipped` for each.
- **Confirmation (decision 6).** During a step a payment is `confirmed` when
  every live host returns `confirmed` with agreeing receipts. Load is not
  paused. Its named hosts are recorded in `info.deferred_hosts` and
  `deferred_step`. A spent-image check run during a step does the same. After
  the step's `window_end`, and while no later step names those hosts, the
  reconciler fetches each deferred host's `getTransaction` and
  `chain_areKeyImagesSpent`. These must match the saved receipts and spent
  state, including on rows already `reconciled`, or the run stops with
  `fleet transaction receipt disagreement` or `spent-input state disagreement`.
  Verified rows record `deferred_verified` and drop `deferred_hosts`.
- **Halt timers.** The 300 s unconfirmed and 900 s unresolved ages exclude
  time inside `safe_halt` windows, and nothing else.
- **Phase boundary.** A payment still carrying `deferred_hosts` from a step
  whose window ended by the boundary is a `Gate`, not a drain wait. A step that
  opens with its phase (D1 at 48 h) is not yet due at that boundary.
- **Memory.** A named host's samples are never post-idle RSS evidence during
  its step. A stopped node's error row is not stored as `latest:<host>`.

## Results

- `report.json`: current sanitized status, phase counts, fees, accounting, latency, fleet and resource snapshots.
- `report.json` `discovery.offer_deferrals` (v2, #1615): for `rehearsal` and
  `campaign`, the distinct due offers held back because their sender still had
  a pending spend (`sender_busy`), `max_inflight` was full (`inflight_cap`), or
  the admission pass ended before reaching them (`admission_backlog`, #1618:
  controller throughput, i.e. the per-tick wall-clock budget or attempt bound),
  `offers_deferred`, `skipped_after_deferral`, and `binding_limit` (the reason
  that held back the most offers; `null` when none). The rehearsal summary
  carries its own copy. Each deferral is journaled once per offer and reason as
  an `offer_deferred` event with its lag at first deferral.
- `reports/`: six-hour snapshots; the current report is also updated on hold, phase transition and completion.
- `activated.json`: exists only once setup actually passes; immutable workload start/end.
- `journal.sqlite`: durable intent/input/request/transition records (protected).
- `artifacts/`, `probes/`, `wallets/`, `inventory.json`, `blocks.json`: protected operational state.

The controller reports actual coverage. Native headless signing does not establish web wallet, MetaMask Snap or confidential-amount acceptance. The full showcase gates remain open.

## Verification before activation

`python3 -m unittest discover -s scripts/stress -p 'test_*.py'` includes offline plan validation, a fake-clock traversal of all 692 offers, input/fee/submission/RPC bounds, durable crash markers, missed-slot/phase holds, and resource/network breakers. The signer has separate production loopback RPC/mempool/ledger receipt → spend → restore → spend acceptance and protected artifact tests. A live read-only preflight verifies five identities/checkpoints, restricted observer access, full-chain scans and zero opening wallet balances.

The deployment script has explicit `observers` and `controller` operations. It uses operator credentials only on the local machine, refuses to overwrite an existing experiment journal, stages owner-only private files, and installs versioned units. Invoking its controller operation activates setup; it must follow the acceptance gates above.

### Running the degrade phase

`scripts/stress/faults.py` executes the `degrade_steps` schedule (D1–D5, see
[Degrade phase plan](#degrade-phase-plan-x-series)) against the **isolated
qualification fleet only**. For x1 it runs on the operator laptop over the
existing SSH access (`ssh -F <operator-ssh-config> <role> -- …`). The inventory
role name (`controller`, `n1`…`n5`) is the SSH alias.

```
python3 scripts/stress/faults.py dry-run --plan <staged x-series plan> \
    --inventory inventory.json --unit <node unit> [--start <T0>] \
    [--controller-stop controller:<controller state dir>]
python3 scripts/stress/faults.py run --plan <staged x-series plan> \
    --inventory inventory.json --ssh-config operator-ssh-config --unit <node unit> \
    --start <T0 from activated.json> \
    --controller-stop controller:<controller state dir> --journal faults.jsonl
```

`--start` is the campaign start in UNIX seconds, copied from the controller's
`activated.json`; the laptop cannot read the controller's SQLite journal. Review
the `dry-run` output, which lists every command with its absolute UTC time
(`T0+<s>` without `--start`), before launching `run`.

**Refusals.** The executor refuses a plan without `degrade_steps`, including a
`rehearsal_only` plan whose `degrade_steps` is `[]` (an empty schedule is never
run); a target missing from the inventory or equal to the plan's
`producer_role`; an inventory hostname or alias that is a public
`runtime.HOSTS` name; a private IP outside RFC1918; and a malformed unit. At
launch, and again 30 s before each fault (so the check never eats into the
30 s late window), it reads `node_getStatus` on each target over the same SSH,
from `http://127.0.0.1:19090/rpc`: `fleet_provision.RPC_PORT`, the staging
renderer's `rpc_port` (override with `--status-url`). It **refuses to fault a
host that reports `mintingActive`**, or whose status cannot be read, whatever
the plan names as the producer. A refusal applies nothing and exits 2.

**Faults.** D1 is `systemctl stop` and records the unit's `Result` (`timeout`
means systemd had to SIGKILL). D2 is `systemctl kill -s SIGKILL`. D3 adds
iptables chain `botho-x-<step>` dropping tcp and udp port 17110 to and from the
**other n-nodes'** private IPs only; SSH, 443 and the controller stay
reachable. D4 adds a 4-band `prio` root qdisc on `--dev` (default `ens5`) with
netem on band 4 and `u32` filters for `sport` and `dport` 17110. D5 is D1 on
both targets together. Each step's start is `T0 + fault_start_offset`. A step
the runner reaches more than 30 s late is recorded as `skipped_late` and never
run late. An apply is never retried; a failed apply is restored at once.

**Stop confirmation (D1, D2, D5).** After the stop or kill, the runner polls
`systemctl is-active <unit>` every 5 s until it reports `inactive` or `failed`
(the units are `Restart=no`, so a SIGKILL leaves `failed`). It journals one
`stopped` line per target with `active_state` and `polls`. The stop must be
confirmed at least 60 s before `restore_at`, so the controller's restart gate
never sees the old process outlive the planned restore. If a target is not
confirmed stopped by then, the runner restores it at once and marks the run
incomplete (exit 1).

**Dead-man guarantee.** Before each fault, the runner arms an on-host transient
timer, `botho-x-restore-<step>-<role>` (one per target for D5), that runs the
same idempotent restore 120 s after the planned restore time. The node is
therefore restored even if the laptop, its SSH session or the runner dies. The
normal path runs **restore, then verify, then disarm**, per target. The verify
checks that the unit is `active`, that `iptables -S` shows no `botho-x`, and
that `tc qdisc show` shows no `prio` or `netem`. The runner disarms a target's
timer only after that target's restore and verify both pass. A failed restore
or verify leaves the timer **armed**, so the dead-man still runs its restore.
A late firing is harmless because the restore is idempotent. A failed
restore or verify also writes `STOP` in the `--controller-stop` directory
through the same SSH (`sudo -n touch <dir>/STOP`). The runner then prints the
manual restore commands, names the dead-man timers that are still armed and
when they fire, and exits 3.

**Manual restore**, on the target (replace `<step>`, `<unit>`, `<dev>`):

```
sudo systemctl start <unit>                               # D1, D2, D5
while sudo iptables -w -D INPUT -j botho-x-<step>; do :; done   # D3
while sudo iptables -w -D OUTPUT -j botho-x-<step>; do :; done
sudo iptables -w -F botho-x-<step>; sudo iptables -w -X botho-x-<step>
sudo tc qdisc del dev <dev> root                          # D4
sudo systemctl stop botho-x-restore-<step>-<role>.timer   # only after the verify passes
```

**Journal.** Every action appends one line to `faults.jsonl`:
`{step, role, action, phase, planned_at, at, ok, stderr_tail}` plus
`unit_result` on `result` lines, `active_state` and `polls` on `stopped` lines,
and `minting_active` on `precheck` lines. `phase` is `precheck`, `apply`,
`restore`, `verify`, `disarm` or `warn`.

**Laptop conditions warn, never stop** (the w9 lesson): low local disk, clock
skew against the controller and slow SSH are journalled as `warn` and the
schedule continues.

- **Keep the laptop awake** for the whole degrade phase: run under
  `caffeinate -i` on macOS. A sleeping laptop turns steps into `skipped_late`;
  the dead-man still restores any fault already applied.
- **SSH ControlPath** sockets must stay under 104 bytes (the w10 lesson). Use a
  short `ControlPath`, such as `~/.ssh/cm-%C`, in the operator SSH config.

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
staging command. For x runs, validator configs, node units, node-stop timers,
nginx sites and this profile's `targets`/`observers[].node` are rendered by
`scripts/stress/fleet_nodes.py` (see "Validator staging for x runs" below). Verify the staged source/binary/profile hashes, forced export,
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

### Validator staging for x runs

x-series validators are staged from the tracked, stdlib-only renderer
`scripts/stress/fleet_nodes.py`, which replaces the config half of the untracked
w-series `stage-nodes.py`. It makes no SSH calls:

```sh
python3 scripts/stress/fleet_nodes.py render --profile profile.json \
  --inventory inventory.json --identities identity/ \
  --producer-mnemonic /private/producer.mnemonic --output /private/payloads
```

The output directory must not exist. It receives one `payload-<role>` directory
per validator (`config.toml`, `node.service`, `node-stop.service`,
`node-stop.timer`, `nginx.conf`, `paths.json`, all mode 0600) and
`staging-fragment.json`. The operator's untracked installer (rsync plus
`install.sh`, accounts, journald) consumes the payloads; TLS identities are
generated separately.

- **Profile:** exactly `run_id` (`qYYYYMMDD-xN-<label>`, validated by
  `fleet_provision.validate_run_id()`), `producer_role`, `node_end` and
  `rpc_requests_per_minute`. Hostnames (`<role>.<run_id>.internal`), paths
  (`/opt/botho-<run_id>`, `/var/lib/botho-<run_id>`) and unit names
  (`botho-<run_id>.service`) are derived from `run_id`, so no token rename is
  needed.
- **Inventory:** all six roles with distinct RFC1918 `private_ip` values.
  Identities (`nodes.json`) give a distinct `peer_id` for each of `n1`..`n5`.
- **Ports:** `gossip_port`, `rpc_port` and `metrics_port` come from
  `fleet_provision.GOSSIP_PORT`, `RPC_PORT` and `METRICS_PORT` (17110, 19090,
  19100). The security group uses the same constant. RPC and metrics ports must
  differ, because the testnet default `metrics_port` collides with `rpc_port`.
- **Quorum:** explicit, `threshold = 4`, with the four other validators as
  `members` and never self. The node counts self in the quorum size, so this is
  4-of-5 with f = 1. The threshold is a module constant, not a profile knob.
- **Isolation:** `fallback_seeds = false`, `dns_seeds.enabled = false`, and
  `bootstrap_peers` are the four other validators' private
  `/ip4/<ip>/tcp/17110/p2p/<id>` addresses. No public seed hostname is ever
  rendered.
- **Producer:** only `producer_role` gets `[wallet] mnemonic`,
  `[minting] enabled = true` and `--mint --mint-threads 1
  --testnet-mint-until <node_end>`. The mnemonic is read from an owner-only
  regular file and appears only in that role's payload. Other roles get no
  `[wallet]` and `[minting] enabled = false`. No config has `ledger_rules`.
- **Exact keys:** every rendered config is parsed back with `tomllib` and must
  have exactly the reviewed key set at each table level. The node ignores
  unknown keys silently, so a typo is refused at render time.
- **Unit:** `Restart=no` (any restart not made by the fault executor is a
  failure), `TimeoutStopSec=30`, `RuntimeMaxSec` of 81.5 h, `IPAddressDeny=any`
  with `IPAddressAllow` for localhost and each inventory IP only. A separate
  node-stop timer stops the unit at `OnCalendar = node_end`.
- **nginx:** `location = /rpc` proxies to `127.0.0.1:<rpc_port>/rpc` and
  `location = /metrics` to `127.0.0.1:<metrics_port>/metrics`, the standalone
  metrics server. Everything else returns 404. The RPC listener's own
  `/metrics` lacks the node-loop histogram that the degrade recovery
  measurements read.
- **Staging fragment:** `targets` in `HOSTS` order (`n1`..`n5`, rpc URL
  `https://<hostname>/rpc`, observer SSH as `botho@<hostname>`) and
  `observers[].node` blocks. These are validated with `campaign_targets()` and
  `observer.node_config()`; copy them into the deployment profile above, and
  list the same URLs as the plan's `endpoints`.

The renderer refuses a public inventory address, a malformed `run_id`, self in
`members`, equal RPC and metrics ports, and a producer mnemonic file that is
not owner-only.

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

### Degrade phase plan (x-series)

`x-series-plan.json` is the v2 plan with one scoped exception: a scripted
`degrade` phase that faults validators on the **isolated qualification fleet**
and checks recovery. It is **never** for the public testnet. Schema v1 (the
public-testnet format) rejects `faults.validator_degrade_phase` and any
`degrade` block, and v2 still rejects every `public_*` fault key, `isolated_only`
other than `true`, and public ingresses.

- **Opt-in.** `faults.validator_degrade_phase: true`, accepted only by the
  isolated v2 planner. An absent key means `false`. The opt-in and a top-level
  `degrade` block require each other.
- **Block.** `degrade.phase` names a plan phase, `producer_role` names the
  minting producer (`n3` on the qualification fleet), and `steps` lists the
  faults. Each step has exactly `id`, `action`, `targets`, `start_seconds`
  (from the phase start), `fault_seconds` (60–3600), `recovery_seconds`
  (300–3600), `offers` (`continue` or `none`) and `expect` (`progress` or
  `safe_halt`). Actions: `stop_restart` (`systemctl stop`, SIGTERM),
  `kill_restart` (SIGKILL), `isolate` (no restart), `netem` (also requires
  integer `loss_percent` 0–50 and `delay_ms` 0–2000) and `stop_restart_pair`.
- **Rules.** Steps are ordered and lie wholly inside the named phase, recovery
  included, so only one is active at a time. Targets are distinct `n1`–`n5`
  roles and never the producer. Single-target actions take one target;
  `stop_restart_pair` takes two and, being below the 4-of-5 quorum, must be
  `expect: safe_halt` with `offers: none`. For an `offers: none` step no
  campaign offer may fall in `[fault_start − 300 s, window_end)`, so in-flight
  payments confirm before quorum is lost.
- **Expanded schedule.** With the opt-in, the summary carries `degrade_steps`:
  `id`, `action`, `targets`, `target_hosts` (the `runtime.HOSTS` strings,
  `n1`–`n5` in order), `offers`, `expect`, the netem parameters on `netem`
  steps, and three integer offsets in seconds from campaign start:
  `fault_start_offset` (fault applied), `restore_offset` (= fault start +
  `fault_seconds`; the executor restarts, un-isolates or removes netem) and
  `window_end_offset` (= restore + `recovery_seconds`; the node must have
  rejoined and the step verdict is taken). This is the one schedule the
  restart gate, controller tolerance, fault executor and verdicts read.
  Without the opt-in the key is absent and existing summaries are unchanged.
  In `rehearsal_only` mode the block is still validated but `degrade_steps` is
  empty: no fault is scheduled.

The x-series plan keeps every v2 budget and the seed, and splits the old
48–72 `repeat` into `degrade` 48–54 and `repeat` 54–72. Since #1620 its peak is
**20/minute**, not v2's 30/minute: the rehearsal offers every 3 s (360 offers)
and each five-minute burst in hours 30–42 is 100 offers at 3 s instead of 150
at 2 s. x1 measured a controller ceiling of about 26/minute (median prepare gap
1.57 s), so a 30/minute rehearsal backlogs and holds; 20/minute leaves about 25 %
headroom, and 64 senders then recur every 192 s, above the ~170 s reconcile
maximum. That gives 21,880 campaign offers and a 22,240-write ceiling with
rehearsal. The degrade phase offers at 4/minute from 48 h (880 offers, last at
+13,185 s) and again from 52.5 h (360 offers), leaving D5 and its lead quiet:

| Step | Action | Target | Fault start → restore → window end (s after 48 h) | Offers | Expect |
| --- | --- | --- | --- | --- | --- |
| D1 | `stop_restart` | n1 | 0 → 1800 → 3600 | continue | progress |
| D2 | `kill_restart` | n2 | 3600 → 4200 → 6000 | continue | progress |
| D3 | `isolate` | n4 | 6000 → 7800 → 9600 | continue | progress |
| D4 | `netem` 20 % loss, 250 ms | n5 | 9600 → 11400 → 13200 | continue | progress |
| D5 | `stop_restart_pair` | n1, n2 | 13500 → 14100 → 15900 | none | safe_halt |

15,900–21,600 s is an in-phase soak, followed by the 54–72 h post-fault soak.
The producer `n3` is never faulted. `node_commit` and `genesis` are per-run
pins set in the staged copy, as are any netem adjustments.

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
records `generator_limited` and cannot activate a 72-hour clock, unless the v2
plan sets `rehearsal.generator_limited_acceptable: true` (#1576 decision 1,
#1613; schema v1 rejects the key). The opt-in accepts only cadence failures
(`submission_offer_lag`, `overall_cadence`, `rolling_cadence`,
`non_monotonic_submissions`). It requires a clean ledger: all offers
reconciled, nothing signed-unsubmitted or unsigned, and a checked artifact
directory with no orphaned or ambiguous signed artifacts. The controller
journals a `rehearsal_generator_limited_accepted` event and still runs every
post-rehearsal gate. Reports keep `status: generator_limited` and add
`accepted_as`; rehearsal-only qualification reports
`accepted_generator_limited`, never `passed`. The x-series plan opts in and caps
normal submissions at 40/min (wire budget 24 MiB) so that a backlog can drain
during its 20/min peaks (#1620). Calibration
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

### Prefunding an isolated fleet

A v2 plan cannot fund itself. `expand_discovery()` requires
`funding.mode == "prefunded_isolated"` and zero grants and bootstrap transfers.
The controller also pins exact `opening_balances` and needs at least
`min_mature_utxos_per_wallet` eligible outputs per wallet before it rehearses.
`scripts/stress/prefund.py` builds that inventory with main's
`botho-stress-wallet`. It uses no faucet. Run it on the **controller host**, the
only host with a private route to the fleet RPC. The operator's untracked copy
step installs it; `staging.py` does not.

The order for an x run is:

1. **Prefund.** Generate the campaign wallets with the signer's `generate`.
   Then run `prefund.py check --manifest M` (local checks only), followed by
   `prefund.py run --manifest M`.
2. **Stage.** Copy `opening_balances.json` into the deployment profile's
   `opening_balances`, then stage with `deploy.py stage`. Staging pins the
   balances into `launch.json`.
3. **Activate.** The controller's first setup tick full-scans the fleet. It
   holds unless every wallet's unspent balance equals its pinned value.

Stage and activate promptly after prefunding. A lottery award to a campaign
wallet between prefund completion and the first setup tick changes that
wallet's balance, and the exact pin then holds the controller.

The manifest is an owner-only JSON file with exactly these keys:

- `schema_version` (1);
- `plan_file` and `plan_sha256`, pinning an x-series v2 plan;
- `targets`, `tls_ca_file` and `tls_ca_sha256`, with the same shape as the
  staging profile;
- `signer` and `signer_sha256`;
- `producer_role`, the logical role of `n3`, the sole producer;
- `funding_wallet` `{address, key}`, the producer's `[wallet]` mnemonic. It is
  not one of the campaign wallets;
- `wallets`, the `wallets.count` fresh campaign `{address, key}` entries;
- `grants_per_wallet` and `grant_picocredits`;
- `state_dir`, `deadline` and `infrastructure_end`. `infrastructure_end` must
  be the same value as the deployment profile's `end`, which staging writes into
  `launch.json` as `infrastructure_end`. The prefunder cannot read the profile
  and does not cross-check it, so the deadline rule below is only as good as
  this value.

Every key file must be owner-only. The prefunder refuses a manifest if:

- `grants_per_wallet` is below `min_mature_utxos_per_wallet`. There are no
  bootstrap splits, so each grant output becomes exactly one future input;
- `grant_picocredits` is below the largest transfer plus
  `max_single_signed_fee_picocredits`;
- `count × grants_per_wallet × grant_picocredits` exceeds
  `max_principal_picocredits`;
- the worst-case fees (`count × grants_per_wallet × max_single_signed_fee`)
  exceed `max_signed_fee_picocredits`.

For x1, 32 grants of 10^11 per wallet gives a principal of 1.024×10^14.
Since #1615 the x-series plan has 64 wallets, and 64 × 32 × 10^11 would exceed
the 2×10^14 principal cap. Use 32 grants of 5×10^10 per wallet: the same
1.024×10^14 principal and per-wallet input depth as x1, 2048 grants, and
2.048×10^12 of worst-case grant fees.

**Deadline rule.** No grant is prepared or submitted at or after `deadline`.
The prefunder refuses any `deadline` later than:

```text
infrastructure_end − 72 h − 1800 s − rehearsal.duration_seconds
                   − rehearsal.drain_seconds − 3600 s
```

This leaves an hour for output maturity and staging, plus the rehearsal, before
the 72-hour campaign must start (`discovery.py`). Plans with
`execution_mode: rehearsal_only` drop the 72-hour term.

**Opening snapshot.** Before the first grant, the prefunder checks:

- all five nodes are synced, report `gitCommit == node_commit` and the plan
  genesis;
- `mintingActive` is true only on the producer;
- no campaign wallet has any on-chain output. With fresh wallets on a fresh
  chain, this detects reuse directly;
- the funding wallet's verified spendable balance covers the principal plus
  worst-case fees.

The snapshot is saved once, to `opening.json`.

**Grants and the never-resubmit rule.** Grant *N* goes to wallet `N % count`.
It uses `selection.choose()` over the funding wallet. The inputs of every
unconfirmed grant are passed to `prepare` as `reserved`. For each grant, the
prefunder:

1. inspects the signed artifact;
2. creates `grants/grant-NNNN.json` exclusively;
3. creates the exclusive `grants/grant-NNNN.submit` marker;
4. calls `tx_submit` exactly once, on the producer.

A grant with a marker is **never prepared or submitted again**. An uncertain
reply marks the grant `unknown` and holds the run. A grant left unconfirmed for
15 minutes after its submission also holds. Pacing follows the plan's
`max_inflight` and `max_normal_submissions_per_minute`. Nothing new is signed
while any grant is uncertain or unsent.

The producer's request budget (the same per-endpoint count and `Retry-After`
backoff that `Rpc` enforces) is checked before signing, with room for
`fee_getRate` and `tx_submit`. It is checked again just before the marker is
written. If that second check defers the grant, its record stays `prepared` with
no marker. Nothing was sent, so every later tick retries it, as
`Controller.admission_tick()` does: once all five nodes report its hash
`unknown`, it is submitted once. A deferred, unsent grant is not subject to the
15-minute hold. Only the write deadline bounds it.

A grant is `confirmed` only when:

- all five nodes report it confirmed, with agreeing
  `(blockHeight, fee, outputCount, totalOutput)`;
- all five report its key images spent;
- a scan finds the recipient and change outputs.

The funding wallet must account exactly:
`opening + coinbase/lottery inflow − remaining − principal − fees == 0`.
Any other transfer into the funding or campaign wallets holds the run.

**Resume.** Restarting the prefunder is safe:

- it reloads every grant record and verifies each artifact's SHA-256;
- it holds on any orphan artifact or marker that has no record;
- it holds on a leftover `grant-NNNN.json.tmp`, which an interrupted record
  update leaves behind. The record still holds its previous state. Review the
  `.tmp` against it, then remove the `.tmp`;
- it requires `state_dir`, `grants/` and `artifacts/` to be owner-only
  directories, including directories that already exist;
- it restore-scans the chain instead of trusting persisted inventory;
- it re-queries every unconfirmed grant before offering anything new.

A record without a marker is a crash or quota deferral between signing and
submission. It is submitted once, but only if all five nodes report its hash
`unknown`; otherwise the run holds. A failure writes `held-*.json` and stops; the operator
reviews it before restarting. `STOP` in `state_dir` stops the prefunder.

**Output.** When every grant is confirmed and the accounting is exact, a final
full scan writes two files:

- `opening_balances.json`: the per-wallet unspent sums, which is exactly the
  profile's `opening_balances` list;
- `funding-health.json`: grants, fees, the accounting difference, per-wallet
  grant-output counts, and per-wallet `choose()` readiness evidence at the
  current height.

The prefunder then stops. Outputs that are still maturing show as
`maturing_or_generator_limited`; the controller rechecks readiness before
rehearsal.

### In-flight accounting and measured ramp ceiling

`max_inflight` counts only work that is not yet confirmed on every host:
`prepared`, `submitting`, `accepted`, and `unknown` (`Journal.inflight()`). A
payment that is `confirmed` or `recipient_verified` but not yet `reconciled` is
waiting on the batched wallet scan. It no longer occupies the cap, but it stays
pending for every other purpose: phase-boundary gates, accounting, drain, its
input reservations, and the per-sender guard. That wallet is therefore not chosen
as a sender again until its change has been scanned. Before #1551, those rows
did count against the cap. Bursts filled it between the 2.5-3 minute reconcile
batches and skipped offers (`generator_slot_expired` / `generator_queue_limit`,
or `queue budget` on the legacy plan).

**Measured ceiling (pending re-measurement).** The w12 qualification run
(`q20261022-w12-endurance`) sustained about **797 confirmed payments/hour**
during the 30–42 h ramp phase. The nominal target is 810/hour: 660 offers at
5-second spacing plus 150 at 2-second spacing in the final five minutes of each
hour. 160 offers were skipped. Measurement conditions: discovery-v2 ramp
profile, `max_inflight` widened to 64 by an operator script, and harness branch
`fix/w7-endurance-performance` @ `b5b76009`. That branch had not landed on
`main` and does not include the #1551 cap change. Part of the shortfall is
node-side: burst blocks were slow to produce and apply on the minter (#1545,
tracked as #1550 / #1565). Judge ramp runs against ~797/hour rather than 810/hour
until the ceiling is re-measured with #1550 / #1565 and this cap change in place
(#1568).

### Degrade-step recovery verdicts and measurements

Every step in `degrade_steps` gets a recovery verdict and a set of
time-to-rejoin measurements. These are the per-step checks of #1576 and the
inputs to the ramp ceiling work in #1568. Evidence is kept in the journal under
`degrade:<step>`. The monitor (inherited by `DiscoveryController`) records a
sample on every 15 s tick from the first tick at or after `fault_start`. With no
`degrade_steps`, or with a rehearsal-only plan (`degrade_steps: []`), nothing
is read or recorded.

**Restore clock.** The fault runner and its `faults.jsonl` live on the operator
laptop, so the controller cannot read them. Every time below is therefore
measured from the **planned** `restore_at`. Milestones are absolute in the
journal and reported as seconds from `restore_at`. They are taken at 15 s
monitor resolution.

**Measurements**, per named host:

- `first_reachable_at`: the first tick at or after `restore_at` when the host
  answers `node_getStatus`.
- `synced_at_tip_at`: the first such tick with `synced` true and
  `chainHeight` at or above the live-fleet height. The live-fleet height is the
  minimum `chainHeight` over reachable hosts the step does not name. That is
  the same convention as `fleet_height()` and the rejoin gate.
- `first_ballot_at`: the first such tick where `lastExternalizedSlot` has
  advanced past its last value before `restore_at` (or, if no value was seen
  before `restore_at`, past the live-fleet height at `restore_at`), or where
  `scpBallotCounter > 0` on an `scpSlotIndex` above the live-fleet height at
  `restore_at`.
- `first_coinbase_at`: always `null` in x1. Only the producer mints, and
  decision 4 never names it.
- `catchup_blocks_per_second`: blocks the host gained from
  `first_reachable_at` to `synced_at_tip_at`, divided by that interval. It is
  `null` when either milestone is missing or they coincide.

Per step, `outage_blocks` is the change in the live-fleet height from the
first tick at or after `fault_start` to the first tick at or after
`restore_at`.

**Loop stalls.** The controller reads `botho_node_loop_iteration_seconds` and
`botho_node_loop_delayed_total` with `Rpc.metrics(host)`, which is a read-only
GET of `<rpc_url minus /rpc>/metrics`. Only the node's standalone metrics
server on `metrics_port` serves those families (the global `REGISTRY`). #1589
proxies `= /metrics` to that server. The RPC listener's own `/metrics` lacks
both families, and a response without them is a `Gate`. The read uses the same
TLS context, redirect refusal, 8 MiB bound and shared per-endpoint quota as
`Rpc.call` (method `metrics`). Samples are taken from every host at the step's
start and again over the last 45 s before `window_end` and at the verdict.
That way a host the next step stops at the same instant still has an end
sample.

- Iterations over 1 s are `Δcount(le="+Inf") − Δcount(le="1")`. The Rust
  encoder prints the 1.0 bound as `"1"`. This count is **recorded and reported
  per host, and never fails a step** (decision 7). There is no plan knob for it.
- The ≥ 5 s counter `botho_node_loop_delayed_total` is a **hard signal**. Any
  increase on any host within the step fails the verdict.
- A node restart resets both counters. When the step re-baselined the host,
  when its start sample is missing because it was already stopped, or when any
  counter went backwards, the delta counts from zero to the post-restart
  value.

**Fault observed.** For restart actions, `fault_observed` is true only if the
step accepted a re-baseline (#1586) for every target. For `isolate`, it is true
only if the target reported `scpPeerCount == 0` or `peerCount == 0` at least
once in `[fault_start, restore_at)`. For `netem` it is `unknown`. A step the
runner skipped (`skipped_late`) or failed to apply therefore records `false`
and fails as `fault not exercised`.

**Verdict.** At the first tick at or after `window_end` (after the rejoin
gate), each of the following must hold. Otherwise the run halts with
`<step>: recovery check failed: <which>`:

- One tip: every reachable host returns the same `getBlockByHeight` hash at the
  common minimum `chainHeight`. Only a host that a later, active step names may
  be absent.
- No `botho_node_loop_delayed_total` increase on any host, and a start and end
  loop sample for every host.
- `fault_observed` is not `false`.
- For `expect: "safe_halt"`: no two hosts reported different `tipHash` at the
  same height during the window, and the live-fleet height advanced again
  after `restore_at`.
- No campaign offer in `[fault_start, window_end)` is `skipped`, `unresolved`,
  `expired`, `rejected` or `aborted`.

Some evidence only exists after `window_end`, so the verdict stays `open` until
all of it is in place:

- every campaign offer in the window is `reconciled`;
- the step's `deferred_hosts` receipts are verified (#1587);
- the first accounting snapshot at or after `window_end` shows difference 0.
  The verdict takes one itself whenever nothing is pending and the wallet
  inventory has been established by a sync, because accounting is only exact
  then. After a controller restart the inventory is untrusted until the first
  sync, so the verdict waits rather than failing on empty balances.

A missed payment or an accounting mismatch found while the verdict is open
fails it. An unverified deferred receipt never fails here, but it also never
passes: the phase-boundary gate (#1587) halts on it. If the controller did not
see a step start within 60 s of `fault_start` (for example because it was
down), the step is recorded as `unobserved`. That is not a halt, and it is
never a pass.

**Completion and report.** `workload_complete()` requires a stored `pass`
verdict for every planned step. A controller restart that missed a verdict
therefore cannot report `complete`. Open verdicts are settled once more after
the final drain accounting. Every report, including the 6-hourly snapshots,
has a `degrade` section with one row per step: verdict (`pass`, `fail`, `open`,
`in_progress`, `not_started` or `unobserved`), failures, the evidence still
awaited, `fault_observed`, the milestones in seconds from `restore_at`,
`outage_blocks`, the catch-up rate, and per-host `over_1s` / `delayed` loop
counts. `coverage.validator_degrade` is true only when there is at least one
step and every step passed. An empty `degrade_steps` is never vacuous coverage.

**Post-run review.** Metrics cannot show whether a stall coincided with a block
apply. Correlating stalls with applies from node logs, and confirming a netem
step (`fault_observed: unknown`) against `faults.jsonl`, remain operator
post-run review.

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

### Retaining evidence without retaining whole EC2 disks

Stopped EC2 instances still incur EBS storage charges. For disposable
qualification fleets, use `scripts/stress/cloud_retention.py` after archival.
Its default mode writes a plan; `--apply` terminates eligible stopped instances
and deletes their released disks. It requires an explicit AWS account, profile,
region, local state directories, backup directory, and a new output directory.
Repeat `--state-root` for each place run directories are kept. Each may be a
run directory or a directory of runs; any child holding
`final-evidence-manifest.json` or `retirement.json` is checked, whatever its
name:

```sh
python3 scripts/stress/cloud_retention.py \
  --profile "$AWS_PROFILE" --region "$AWS_REGION" --account-id "$AWS_ACCOUNT_ID" \
  --state-root "$BOTHO_RUN_STATES" --state-root "$BOTHO_RUN_ARCHIVES" \
  --backup-root "$BOTHO_ARCHIVE_BACKUP" \
  --output "$BOTHO_RETENTION_RESULT" --min-age-hours 24
```

Only qualification fleets are eligible: those tagged `project=botho` with
`Purpose=qualification`, or the legacy `Project=botho-qualification`. Testnet
nodes tagged `project=botho` without that purpose are never selected. Every instance
must be stopped and match a six-role local inventory, terminal evidence, and
six verified archives. A second archive copy is verified before termination.
Cloud state, tags, roles, disk attachments, and archive identity are rechecked
before each fleet is removed. The newest cloud fleet is always retained for
diagnosis, even if it failed. Public nodes, other projects, snapshots, and
fleets with missing evidence are excluded. The default archive retention
window is 24 hours; reducing it to zero is for reviewed one-time cleanup.

For periodic operation, run the same command with `--apply` from the operator
host, using a unique output directory each time. A lock in each state root
prevents overlapping cleanup processes. The operator must be awake and have working AWS
credentials. Archive checks or unavailable credentials stop deletion; review
`plan.json`, `execution.json`, and `result.json` in each output directory. If
execution stops partway through, inspect the recorded instances and disks;
do not assume that the entire plan completed. Do not delete local archives
when releasing AWS disks.

A fleet aborted before deployment may instead provide `retirement.json` with
remote verification of all six hosts: matching run IDs, completed bootstrap,
no node service, and no controller data. Local deployment markers must also
be absent. That proof is copied and verified before deletion, and a later EC2
launch invalidates it. This exception cannot substitute for archives once a
workload has been deployed.

### x-series tags and host bootstrap

x-series qualification fleets are provisioned from tracked renderers in
`scripts/stress/fleet_provision.py`. It is pure and makes no AWS calls; the
operator's untracked `provision.py` imports it and keeps only the `aws`
invocations and the files it persists between retries.

Run ids have the form `qYYYYMMDD-xN-<label>`, for example
`q20261101-x1-endurance`. `run_id()` and `validate_run_id()` require lowercase,
a real calendar date, a positive `N`, and at most 48 characters, and they refuse
any `-w<digits>` token so w-series names are never inherited.

Every instance and its root volume carry the same tags, set at creation through
the `RunInstances` `TagSpecifications` (`tag_specifications()`):

| Key | Value |
|---|---|
| `Name` | `botho-<run>-<role>` |
| `project` | `botho` |
| `Purpose` | `qualification` |
| `SideProject` | `botho` |
| `Run` | the run id |
| `Role` | `controller` or `n1`..`n5` |
| `ExpiresAt` | `host_end`, epoch seconds |

The security group carries `project`, `Purpose`, `SideProject` and `Run`.
`project=botho` with `Purpose=qualification` is the scheme that
`cloud_retention.py` selects; a regression test builds a fleet with
`fleet_provision.tags()` and checks that retention selects it.

- **Never emit `Project=` (capital P).** IAM tag keys match case-insensitively,
  so `Project` collides with `project`, and `Project=botho-qualification` is
  denied to the botho-deploy role. The renderer refuses any key that equals
  `project` case-insensitively unless it is exactly `project`.
- **`SideProject=botho` is set at creation**, not added later by another
  profile. botho-deploy passed a dry-run of `ec2:RunInstances` and
  `ec2:CreateSecurityGroup` with all three tags on 2026-10-24.

`security_group_rules()` returns the only allowed ingress: tcp 22 from the
operator addresses, and tcp 22, 443 and 17110 plus udp 17110 from the group
itself. Port 17110 is the gossip port the staging renderer uses. Operator
addresses must be explicit IPv4 `/32` hosts; `0.0.0.0/0` and wider ranges are
refused.

`bootstrap()` renders the EC2 user data in a fixed order: an absolute-stop
systemd timer whose `OnCalendar` is `host_end`, then the
`host_maintenance.bootstrap()` block that masks unattended APT maintenance,
then package installs, users and SSH hardening, then the role and run markers
under `/etc/botho-qualification/`. `infrastructure_plan()` returns the plan to
freeze before any launch: `host_end = created_at + 82h` (inside the 83 h
maintenance cap), `node_end = created_at + 81.5h`, and the roles with their
instance types (`m6i.2xlarge` controller, `m6i.large` producer `n3`,
`c6i.large` validators). `launch_request()` renders one role's `RunInstances`
input from that plan. SSH host-key pinning and the SSH ControlPath, which must
stay under macOS's 104-byte socket path limit, remain operator-side.
