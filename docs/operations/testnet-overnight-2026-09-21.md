# Overnight testnet payments — September 21–22, 2026

Tracker: [#1383](https://github.com/botho-project/botho/issues/1383). The owner requested payments every six hours overnight after the [node redeployment](https://github.com/botho-project/botho/pull/1382). This is a bounded native-faucet experiment. Its scheduled payments are **not yet evidence of successful overnight operation**.

## Schedule and budget

| Payment | America/Los_Angeles (PDT) | UTC |
|---|---|---|
| 1 | September 21, 7:00 PM | September 22, 02:00 |
| 2 | September 22, 1:00 AM | September 22, 08:00 |
| 3 | September 22, 7:00 AM | September 22, 14:00 |

Each slot requests one ordinary **1-BTH** grant from `https://faucet.botho.io/rpc`: at most **three requests / 3 testnet BTH** total, plus the faucet's normal transaction fees. A fresh native v2 testnet wallet receives the payments. Its address SHA-256 is `7fa9f604c2d993890ed2e10af848787d6bae6d01865fb2b0ff50b828554903bf`. Recovery material stays in the operator's protected local `~/.local/state/botho-testnet-overnight-1383/`; only the public address is installed on the runner. The previous smoke wallet already received a grant, so reusing it would exceed the faucet's three-per-recipient rolling 24-hour limit on the final request.

The runner lives on **seed.botho.io**, independent of the operator laptop. Explicit dated systemd timers have no next-day recurrence or catch-up firing. A slot may start only within two minutes of its scheduled time. A durable marker is written before any submission; a timeout, rejected request, failed preflight or repeated service start never triggers a second request for that slot. A missed slot is reported as missing during review, not replayed.

## Acceptance and observations

Before sending, the runner checks all five HTTPS ingresses for the original genesis, deployed clean source `6dbcb92463214694f3122a05c9c48986d4f4f1b7`, testnet identity, synchronization and matching tips. It checks that the faucet is enabled and still dispenses exactly 1 BTH. A failed check consumes the slot without sending; the evidence records the failure.

After a successful request, it polls all five hosts approximately every 15 seconds for up to 15 minutes. Success requires the returned transaction to be confirmed on every host in the same block, with all nodes synced and sharing a tip. Records contain request/response times, transaction hash, observed confirmation latency, fees and block metadata, and RPC errors. Latency includes HTTPS submission time and polling granularity.

All five hosts also record local service PID/start time/restart count, memory/current peak/swap, process RSS/high-water mark/swap, and local RPC status every five minutes through **07:55 PDT / 14:55 UTC September 22**. These snapshots can miss short disturbances; systemd/process high-water marks and later journal inspection supplement them. The script records collection errors instead of treating missing data as healthy.

The experiment does not restart nodes or change mining/quorum/network configuration. In particular, the existing faucet high-balance idle pause remains in effect so each payment exercises idle-to-mining recovery. [#1381](https://github.com/botho-project/botho/issues/1381) tracks the observed cold-start/pause responsiveness problem. Web/Snap and CT/LotteryV2 acceptance remain separate showcase requirements.

## Installed files and review

- Runner: `/opt/botho-overnight-1383/runner.py`, root-owned; source is [overnight_payments_1383.py](../../scripts/operations/overnight_payments_1383.py).
- Only on seed: public recipient `/opt/botho-overnight-1383/address.txt`; `botho-overnight-1383-pay.service` and `.timer`.
- On all five nodes: `botho-overnight-1383-observe.service` and `.timer`.
- Unit sources: [overnight-1383](../../scripts/operations/overnight-1383/). Services run as `ubuntu` with a read-only system/home sandbox, one writable experiment directory, no automatic restart, and a 128-MiB memory ceiling.
- Evidence on each host: `/var/lib/botho-overnight-1383/`, owned by `ubuntu`, mode 0700. `health.jsonl` is local health history. Seed additionally holds `preflight.json`, `payment-<UTC-slot>.json` and `fleet-<UTC-slot>.jsonl`.

Before activation: eight local tests cover fixed windows, at-most-once submission including ambiguous timeouts, three-request budget, wrong amount/identity, late preflight, and fleet confirmation criteria. `systemd-analyze verify` validates units; `systemd-analyze calendar --iterations=4` shows precisely the three payment events. A live read-only preflight and real service invocations validate connectivity and the service sandbox without submitting an early payment.

**Activation verified:** all five observation timers and seed's payment timer are enabled, active and waiting. All installed runner checksums match the source; all five real local samples succeeded, and no payment attempt existed at setup time. The [installation evidence](testnet-overnight-evidence/2026-09-21/) records the checks, activation timestamps, next timer events and source digest. The first payment is scheduled for 02:00 UTC.

## Morning review and stopping

Retrieve evidence from each host using the existing operator SSH key, for example:

```sh
mkdir -p /tmp/botho-overnight-1383-results/seed
scp -i ~/.ssh/botho-nodes.pem -r \
  ubuntu@seed.botho.io:/var/lib/botho-overnight-1383/ \
  /tmp/botho-overnight-1383-results/seed/
```

Repeat for `seed2.botho.io`, `faucet.botho.io`, `eu.seed.botho.io` and `ap.seed.botho.io` into separate directories. Require three receipts with `confirmed_on_all_five`; inspect errors or missing receipts, confirmation latency, tip agreement, sync changes, PID/start-time changes, restart counters and memory/swap. Compare the three payment windows with the idle intervals, then inspect node journals for each transition. A timeout is a finding, not permission to restart the node or resend automatically.

To cancel future payments, run on seed:

```sh
sudo systemctl disable --now botho-overnight-1383-pay.timer
```

This does not recall an already submitted transaction. To stop observation, run on each host:

```sh
sudo systemctl disable --now botho-overnight-1383-observe.timer
```

The dated timers naturally exhaust their schedules; disabling them afterward is optional cleanup. Preserve the evidence and recipient recovery material for the review. No automatic notification or morning chat response is configured.
