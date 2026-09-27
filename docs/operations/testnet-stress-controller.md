# Bounded testnet stress controller

Implementation tracker: #1404. Design: #1401/#1403. Offline native signer: #1402/#1405.

## Operation

The controller runs as a dedicated unprivileged systemd user service (lingering enabled) on `loom-worker-1`, separately from all five testnet nodes. Its state is `/home/ubuntu/.local/share/botho-stress-1404`. Code and the signer are installed under a versioned `/home/ubuntu/.local/lib/botho-stress-1404-*` directory with source hashes checked at startup. Local recovery material is protected outside the repository under `~/.local/state/botho-stress-1404/`.

The launch configuration pins the node and signer hashes, controller source file digests, endpoint identities, wallet allowlist and setup start. The durable SQLite journal pins its configuration digest and every submitted intent. Signed artifacts, wallet-linked inventory and private recovery material are not public evidence.

Setup has eight hours, at most 24 ordinary one-BTH grants spaced 15 minutes apart, and at most 64 bootstrap self-transfers. The first grant is not a proof of wallet spending. Bootstrap transactions split wallet outputs as eligible inputs become available. Setup must establish four mature outputs per wallet, production decoy availability, successful native four-input preparation probes, full rescan agreement and exact accounting. It then records immutable T0/end timestamps and automatically starts the 692-offer, 72-hour schedule. If these gates cannot be met, the run stops as incomplete.

No missed-slot replay or automatic transaction rebroadcast occurs. A crash after the submission marker retains the hash, inputs and fee reservation and reconciles by hash. A crash before the marker can make its first submission only after querying all five nodes and only within its original slot. Orphaned/partial prepared files cause a hold. Uncertain grant responses consume their funding slot.

The public recovery stage restores a clean wallet profile and exercises controller exits after durable preparation and after recorded acceptance. The latter keeps acceptance evidence; an actually lost HTTP response is tested in the journal recovery tests and remains a hold during public operation. No public node outage, partition, miner change, CT activation, wallet application deployment or chain reset is performed.

## Observation and stop behavior

Each node runs `botho-stress-observer-1404.service`, independently of RPC. It samples resources every five seconds, exports the last 30 records, and retains bounded rotated local logs. This is deliberately more frequent than the design's idle minute: resource collection makes no RPC calls, and its measured overhead is recorded in the launch evidence. Collection expires after 82 hours.

The controller holds only a dedicated SSH key restricted to the fixed observer export command. The unit requests a private user/mount namespace and home masking, but the worker did not apply that isolation (the live process retained the host mount namespace). Files are therefore protected by owner-only permissions under the existing Ubuntu account; this is not a separate OS security principal. No operator private SSH key is present on the worker or copied by deployment; outbound observer SSH uses `IdentitiesOnly=yes`. Shared per-endpoint RPC quotas include all controller reads/writes and persist across restarts. Every 15 seconds it checks node identity, independent resources, sync and common-height block hashes. Receipt polls run every 30 seconds. All money arithmetic uses integers.

A health/accounting/identity failure pauses admission and requires explicit investigation. A signed payment unresolved for 15 minutes, the eight-hour setup deadline, or the 72-hour end stops admission. Read-only reconciliation is bounded to another 30 minutes. A stopped run cannot silently move its end or refill missed events. The service itself has an 82-hour runtime ceiling and bounded CPU/memory; the controller also checks its own disk headroom.

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
