# Stress campaign launch — September 22, 2026

**Running stage: bounded wallet setup. The 72-hour workload clock has not started.**

Run `stress-1404-20260922T190609Z` is active on `loom-worker-1` as the lingering user service `botho-stress-1404.service`. Setup began **19:06:09 UTC / 12:06:09 PDT**. Its fixed deadline is **September 23, 03:06:09 UTC / September 22, 20:06:09 PDT**. Setup automatically selects T0 and the immutable 72-hour end only after all wallet inventory, signer, restore and accounting gates pass. It otherwise stops incomplete.

## First live result

- One normal 1-BTH faucet grant, requested once; transaction `3cee361a4f9bac6875222423c3e92241754eb35db226be517cf38d39a96b2e7e`.
- Confirmed across all five ingresses in block **2643**, **167.883 seconds** after submission. Recipient ownership/value and complete wallet accounting reconciled after **180.566 seconds**.
- Opening balance 0; external funding **1,000,000,000,000 picocredits**; ending experiment balance the same; harness-signed fees 0; accounting difference **0**. Faucet fee **100,000,000 picocredits** is recorded separately.
- Node PIDs unchanged; no automatic node restarts. The first cold-start resource observation reached approximately 2.6 GiB producer RSS with about 800 MiB available host memory, above the configured pause threshold. High mining CPU is expected.
- The next ordinary funding offer is **19:21:09 UTC / 12:21:09 PDT**, then at the fixed 15-minute cadence within the funding cap. Requests are never retried after an uncertain reply.

The [sanitized launch snapshot](testnet-stress-evidence/2026-09-22-launch.json) records exact timestamps, receipts, service state, resources, software hashes and accounting. This first public result proves funding receipt. Public native spending and the offered load stages remain pending; the runner will qualify native spending during setup. Web, Snap and confidential-amount coverage remain unexecuted.

## Software and gates

- Nodes remain on `6dbcb92463214694f3122a05c9c48986d4f4f1b7`, unchanged genesis/protocol and monetary configuration.
- Native adapter: `57d647597b5d438ac281ae66cc3593ca7ebcc259`; Linux binary SHA-256 `23ba0ef227697d40970f3d7b55669635b617d8317a66f2a39e8ad1fe627f6c93`.
- [Linux release acceptance and artifact build](https://github.com/botho-project/botho/actions/runs/35770098357) passed production loopback RPC/mempool/ledger receive → spend → restore → spend and protected-file checks. The first unoptimized Linux fixture exceeded its short request timeout; the release artifact gate passed.
- **19 Python tests passed**, including all 692 offers under a fake clock, durable crash markers, reservations, limits, phase holds, exact accounting and resource/identity failures. Live read-only preflight passed for all five identities/observers and all eight wallets' zero opening balances.
- Controller code was refreshed during setup to `c67ad539` after the first grant reconciled, to publish current accounting promptly. One controlled controller-service restart preserved the original setup timestamp, journal, transaction, budgets and deadlines. No node was restarted. The snapshot pins all actual controller file hashes.
- Initial system-service installation was rejected by the worker's sudo policy before any funding. Deployment uses its already-enabled lingering user manager. Actual cgroup limits verified: one CPU and 1 GiB RAM. The requested filesystem namespaces were not applied by that manager; owner-only files under the Ubuntu account are the effective key boundary. The worker has no operator private SSH key, and only the dedicated forced-command observer key was transferred. See the [runbook](testnet-stress-controller.md).

## Continuing observation

```sh
ssh loom-worker-1 'systemctl --user status botho-stress-1404.service --no-pager'
ssh loom-worker-1 'cat /home/ubuntu/.local/share/botho-stress-1404/report.json'
```

Current reports update after reconciliation, phase changes and stops; six-hour archives are written separately. `activated.json` appears only when setup passes. All five independent resource collectors expire automatically. A health or accounting hold requires investigation; restarting must not be used to erase a hold or replay an offer.

Track implementation/review in [#1405](https://github.com/botho-project/botho/pull/1405) and [#1407](https://github.com/botho-project/botho/pull/1407), and campaign outcomes in [#1404](https://github.com/botho-project/botho/issues/1404). The real-node fixture also found the separate hybrid coinbase RPC-index defect [#1406](https://github.com/botho-project/botho/issues/1406); the live campaign uses ordinary faucet-funded outputs.
