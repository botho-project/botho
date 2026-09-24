# Warm-baseline Snow testnet stress restart — September 24, 2026

Status: all five nodes deployed; bounded mining and the fresh setup controller are running. The 72-hour workload has not started; its automatic readiness gates remain in effect. This is a new run, not a continuation or successful disposition of the previous setup.

## Why the previous setup stopped

`botho-stress-snow-20260924` held at 01:29:48 UTC before workload T0 and finished its bounded drain as incomplete. Three funding grants totaling 3 BTH reconciled with zero accounting discrepancy. The original wallet keys, transaction journal, reservations, deadlines and evidence remain preserved.

The faucet's initial RSS baseline was 55.87 MiB. Post-payment settled RSS was 332.28, 341.30 and 342.82 MiB. It then stayed at exactly 359,477,248 bytes for 1,289 samples over 107.3 minutes, with zero swap and unchanged PID. The process-wide RandomX verification cache intentionally retains approximately 256 MiB after first use. This supports a cold-baseline false positive; it does not prove that all retained memory is harmless or eliminate the need for growth detection.

## Explicit revised experiment policy

The original experiment left the faucet's balance pause unchanged. That policy can prevent mature inputs and age-matched decoys from becoming available on an otherwise idle chain. This replacement explicitly uses the existing producer with one minting thread and a fixed, testnet-only continuous-minting deadline. It introduces no new producer, consensus rule, reward formula, difficulty setting, faucet limit or wallet eligibility exception. Normal block production increases actual issuance and CPU use. At most 82 mining thread-hours are permitted; node/network overhead is additional. An absolute-deadline restoration timer is armed before the temporary service argument is installed.

The new option defaults off, rejects mainnet and deadlines more than 82 hours ahead, and retains the original deadline across restarts. A running process also enforces monotonic expiry. Expiry precedes fallible balance reads, and peer/sync events cannot bypass a balance pause. Successful normal balance checks may resume ordinary pending-transaction mining after expiry.

The controller requires five continuous minutes of observed per-host idle before comparing post-idle RSS against a fixed baseline. Mining, pending transactions, missing observations, new writes and observation gaps reset idle qualification. The continuously active faucet is excluded from this *post-idle* comparison while mining; its available-memory, swap, disk, identity and service-restart guards remain active. This run does not claim repeated faucet idle-memory-cycle coverage while continuous mining is enabled.

All four passive nodes must establish warmed stable RSS, and the producer must show stable active RSS, increasing hashes, resource headroom and new common-chain blocks before launch. Baselines are not adaptively raised during the run.

## History and controller bounds

Current public block measurements forecast approximately 38 MiB of signer input at 40-second blocks, or 57 MiB at 20-second blocks, including the permitted funding/bootstrap/campaign transfers and lottery outputs across setup and workload. These are scenario estimates, not bounds on unrelated external traffic. The stdin-only adapter now accepts at most 128 MiB, retaining a bounded limit-plus-one read and all transaction/fee constraints. The synthetic 64 MiB test passed: eight scans took 55.14 seconds, with a combined Python/native kernel cgroup memory peak of 409.60 MiB under one CPU, one GiB and 64 tasks. No memory-limit/OOM events or chain writes occurred. This validates capacity, not chain consensus.

The new run retains eight fresh wallets, 24 ordinary grants at 15-minute intervals, 64 bootstrap transfers, 692 planned campaign offers, the 800-chain-write cap, 0.5 BTH aggregate signed-fee cap, eight-hour setup deadline and immutable 72-hour workload deadline. T0 is assigned only after real maturity/decoy availability, four spendable outputs per wallet, production four-input signer probes, full rescan agreement and exact accounting pass. New observer/mining deadlines must cover setup, workload and drain.

Implementation reviews: [controller #1430](https://github.com/botho-project/botho/pull/1430), [signer #1434](https://github.com/botho-project/botho/pull/1434), [bounded mining #1435](https://github.com/botho-project/botho/pull/1435). Exact sources and validation results follow; launch evidence is recorded separately below.

## Validated implementation and deployment

- Node: `9e671836940bafd5a14d64ffa99eec03932db735`, executable SHA-256 `815a08968390ee00540098fbdb05fdb595b0be95101833bd6cc64fc91e83e159`, pinned Snow `8192d73c676ecdc93f91c5621caaa95245969ccc`. [Final ARM64 validation](https://github.com/botho-project/botho/actions/runs/35951348149) passed 79 tests: 18 network, 11 minter, 49 run-command and one CLI test; two existing minter tests were ignored.
- Offline signer: `e963fd78f59b34260c3bf5ead4fd9d463ca374ef`, executable SHA-256 `0f5a6cfb2ba52c00ee130672c0690d234f9ef5ea0d35c1c37a1db17f2645eaf6`. [Linux x86_64 validation](https://github.com/botho-project/botho/actions/runs/35951069055) passed five adapter unit tests and one real-node integration test. This is the exact artifact used in the capacity check.
- Controller: `55c09a7998b4eb819ea0035290775d473600558f`; all 44 Python tests passed. Routine scans process appended outputs only after independent initialization. Complete history remains available for signer/decoy preparation; startup, restarts, phase boundaries and final accounting retain independent full scans. Failed scans do not advance progress, and duplicate output identities fail closed.

A read-only real-adapter regression scanned the old run's history before its first grant, appended blocks 2657–2662, and compared all eight wallet inventories against independent full restores. The inventories matched exactly, totaling 3 BTH; an empty delta performed current public spent checks without repeating native scans. Old state remained unchanged and no chain writes occurred.

All five nodes passed sequential upgrade and final fleet checks. The existing faucet's one-thread policy is `mint-20260924T033741Z`, with fixed expiry **2026-09-27 13:36:36 UTC** and a persistent restoration timer. Process arguments and same-PID startup logs confirm one actual mining thread. [#1436](https://github.com/botho-project/botho/issues/1436) records a separate telemetry defect: current RPC thread fields report host CPU count instead of the configured worker count. Warm qualification binds actual process arguments, PID/start, binary hash and deadline; it does not use the misleading thread field as evidence of concurrency.

## Warm qualification

The successful observation window ended at **2026-09-24 03:54:26 UTC**. All four passive nodes showed at least 302 seconds of stable idle RSS; the producer showed 122 seconds of stable active RSS with actual hash-counter growth. The fleet advanced eight common blocks (2679 to 2687). Passive RSS increased by less than 5 MiB within each window; active producer RSS varied by less than 2 MiB. Existing headroom, swap, disk, identity and source guards passed. One earlier observation attempt ended on a TLS handshake timeout; its evidence was preserved and a complete new window was required.

The hash counter advances in batches of 10,000, so qualification requires nondecreasing counters and positive reported hash rate throughout, plus actual counter growth over the active window. It does not require an increment in every 15-second observation. Independent review verified this against the production counter implementation and rejection cases.

## Fresh setup activation

Preflight at **03:56:07 UTC** verified all eight new wallets had zero opening balances, exact integer accounting difference zero, the expected genesis and pinned fleet/controller builds. The actual initial signer request was 7,536,444 bytes, below the 128 MiB limit.

`botho-stress-snow-warm-20260924.service` was activated once with immutable setup start **2026-09-24 03:57:27 UTC** (September 23, 8:57:27 PM PDT) and setup deadline **2026-09-24 11:57:27 UTC**. The running service's kernel cgroup limits were verified: one CPU, 1 GiB and 64 tasks. All five new bounded observers are running. The producer restoration timer remains armed for September 27 at 13:36:36 UTC.

The service runs independently of this interactive session. Funding and bootstrap readiness occur first; the controller assigns workload T0 and its fixed 72-hour end only when readiness succeeds. Activation of the setup service is not evidence that the workload has begun or that the test passed.

The first ordinary 1 BTH grant was accepted at **03:57:34 UTC** and reconciled at **03:58:42 UTC**, transaction `ff51209d1733ea5167c6455229a068c90c737f1ab9e0e371993611cab2ad7b68`. The next accounting snapshot showed 1 BTH granted, 1 BTH ending balance, zero signed fees and zero discrepancy. The controller remained in setup, with no hold or restart and no workload T0/end assigned.

[Sanitized launch evidence](evidence/testnet-snow-warm-2026-09-24.json) records source pins, resource validation, independent signer regression, warmed RSS windows, preflight and live setup status. Private wallets, observer credentials and original run evidence remain in protected storage.
