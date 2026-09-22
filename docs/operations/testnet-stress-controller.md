# Bounded testnet stress controller

Implementation tracker: #1404. Design: #1401/#1403. Offline native signer: #1402/#1405.

## Operation

The controller runs as a dedicated unprivileged systemd user service (lingering enabled) on `loom-worker-1`, separately from all five testnet nodes. Its state is `/home/ubuntu/.local/share/botho-stress-1404`. Code and the signer are installed under a versioned `/home/ubuntu/.local/lib/botho-stress-1404-*` directory mounted read-only inside the service. Local recovery material is protected outside the repository under `~/.local/state/botho-stress-1404/`.

The launch configuration pins the node and signer hashes, controller source file digests, endpoint identities, wallet allowlist and setup start. The durable SQLite journal pins its configuration digest and every submitted intent. Signed artifacts, wallet-linked inventory and private recovery material are not public evidence.

Setup has eight hours, at most 24 ordinary one-BTH grants spaced 15 minutes apart, and at most 64 bootstrap self-transfers. The first grant is not a proof of wallet spending. Bootstrap transactions split wallet outputs as eligible inputs become available. Setup must establish four mature outputs per wallet, production decoy availability, successful native four-input preparation probes, full rescan agreement and exact accounting. It then records immutable T0/end timestamps and automatically starts the 692-offer, 72-hour schedule. If these gates cannot be met, the run stops as incomplete.

No missed-slot replay or automatic transaction rebroadcast occurs. A crash after the submission marker retains the hash, inputs and fee reservation and reconciles by hash. A crash before the marker can make its first submission only after querying all five nodes and only within its original slot. Orphaned/partial prepared files cause a hold. Uncertain grant responses consume their funding slot.

The public recovery stage restores a clean wallet profile and exercises controller exits after durable preparation and after recorded acceptance. The latter keeps acceptance evidence; an actually lost HTTP response is tested in the journal recovery tests and remains a hold during public operation. No public node outage, partition, miner change, CT activation, wallet application deployment or chain reset is performed.

## Observation and stop behavior

Each node runs `botho-stress-observer-1404.service`, independently of RPC. It samples resources every five seconds, exports the last 30 records, and retains bounded rotated local logs. This is deliberately more frequent than the design's idle minute: resource collection makes no RPC calls, and its measured overhead is recorded in the launch evidence. Collection expires after 82 hours.

The controller holds only a dedicated SSH key restricted to the fixed observer export command. Its private user/mount namespace hides the rest of the home directory and all operator keys; only the experiment state and read-only package are bound into it. It has no operator SSH key. Shared per-endpoint RPC quotas include all controller reads/writes and persist across restarts. Every 15 seconds it checks node identity, independent resources, sync and common-height block hashes. Receipt polls run every 30 seconds. All money arithmetic uses integers.

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
