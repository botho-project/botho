# Overnight testnet payments — September 21–22, 2026

Tracker: [#1383](https://github.com/botho-project/botho/issues/1383). The owner requested payments every six hours overnight after the [node redeployment](https://github.com/botho-project/botho/pull/1382). This bounded native-faucet experiment is complete: **all three 1-BTH payments confirmed on all five nodes without an overnight node restart**. Morning evidence was collected at **09:22 PDT / 16:22 UTC September 22**.

## Overnight results

| Scheduled time (PDT) | Payment block | All five reported confirmed | All five confirmed and synced |
|---|---|---|---|
| September 21, 7 PM | 2637 | 2m 02s | 2m 02s |
| September 22, 1 AM | 2639 | 1m 46s | 1m 46s |
| September 22, 7 AM | 2641 | 2m 02s | 3m 09s |

Times run from the HTTPS grant request to the first qualifying fleet observation, including polling granularity. Each transfer delivered 1 BTH with a 0.0001-BTH transaction fee. Three distinct receipts account for 3 BTH delivered and 0.0003 BTH in fees. Morning RPC checks on every host still reported all three transactions confirmed, with 6, 4 and 2 confirmations respectively.

All five nodes currently agree at **height 2642**, tip `57c3e0f9b6b475c8757340d3c4f59290dedfe9c28de67997274d64c5a53f0430`, and retain the deployed clean source and original process IDs. Public HTTPS responds successfully on every host. Their processes have been up for roughly 17 hours since rollout.

### Stability and remaining findings

- **815 local health samples** (163 per host) cover approximately 13.5 hours through 07:55 PDT. Every sample succeeded; PIDs/start times stayed fixed, automatic restart counts stayed zero, and recorded process/service swap stayed zero. The maximum interval between samples was approximately 302 seconds.
- **The 7 AM transition had a brief sync problem.** Seed logged a sync-response timeout at 14:01:56 UTC, and four payment polling samples from 14:02:07 through 14:02:58 reported it unsynced. The transaction was already confirmed across all five by the first of those samples. All nodes were synced by 14:03:14 without intervention. The five-minute health series missed this short interval; the more frequent payment polling captured it.
- **Idle memory increased modestly after payments.** Faucet RSS went from 326.8 to 342.5 MiB (+15.7 MiB), with flat readings between payments; its service high-water mark reached 2.60 GiB during mining. AP RSS rose about 12.7 MiB. There was no observed recurrence of the previous multi-GiB swap condition. These three payments do not establish long-term leak freedom.
- **Warmup/stall diagnostics still need work.** Journals contain fifteen SCP stall warnings (one per host per payment), four faucet miner-stall warnings during the first/third transitions, and three warning lines for the same seed sync failure. The reported SCP ages roughly match the preceding idle intervals, suggesting the warning clock includes idle time. No panic, process-exit or full-gossip-queue event was found in the captured warning/error scan. [#1381](https://github.com/botho-project/botho/issues/1381) remains open for responsiveness, cold initialization cost and the related diagnostics.

The result supports stability for **three low-rate payments separated by idle periods**. It does not establish load capacity, a full web/Snap round trip, or CT/LotteryV2 demonstration readiness. Payment and observation timers have exhausted their dated schedules: all report `elapsed`, with no next firing. No extra payments, node restarts or configuration changes were made during the morning review.

The [results evidence](testnet-overnight-evidence/2026-09-22-results/) contains the three receipts, payment polling, all five complete local health series, morning RPC/timer/journal checks, machine-readable summaries and SHA-256 digests. Recipient keys and configuration contents are excluded. The original setup evidence below remains a historical record of activation.

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

**Activation verified at setup:** all five observation timers and seed's payment timer were enabled, active and waiting. All installed runner checksums matched the source; all five real local samples succeeded, and no payment attempt existed at setup time. The [installation evidence](testnet-overnight-evidence/2026-09-21/) records the checks, activation timestamps, next timer events and source digest. The first payment was scheduled for 02:00 UTC.

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
