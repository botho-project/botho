# Testnet redeployment — 2026-09-21

Tracker: [#1380](https://github.com/botho-project/botho/issues/1380). Authorized by the owner to redeploy and assess stability. This is the node-fleet follow-up to D1 in the [showcase review](https://github.com/botho-project/botho/pull/1374).

## Deployed candidate

| Item | Value |
|---|---|
| Source | `6dbcb92463214694f3122a05c9c48986d4f4f1b7` (merged main) |
| Runtime build identity | `6dbcb92`, clean |
| Crate / protocol | `0.6.0` / `6.0.0` |
| Target | `aarch64-unknown-linux-gnu`, release, default features |
| Build | [Release workflow 35666070877](https://github.com/botho-project/botho/actions/runs/35666070877), `dry_run=true`; successful ARM64 job, no release publication |
| Node binary SHA-256 | `88c169423fdc003a5dbc6d688852fdbcda26b78f85cd371de29d9192571c674f` |
| Previous binary SHA-256 | `a265f01d311fc8bc561531aa39451ebbbcf6c077635cff979b7a995f2773bf6d` |
| Preserved genesis | `9ba39a7a724ce1c954d9cbf9b83c019e898a5129ef5f485c0488a3fad6d396df` |
| Preserved pre-deploy tip, height 2634 | `616f30244d10e591144159a77cea8fe68628b05f73bfa3dbfbac31912d37ecd6` |

The artifact checksums were verified locally and on each host before replacement. Local tests at the pinned source passed: **4 consensus-convergence tests and 6 transfer-pattern tests**. These use the local integration harness and supplement the live payment below; they do not model the deployed host resource limits.

## Operation

The five hosts are `seed.botho.io`, `seed2.botho.io`, `faucet.botho.io`, `eu.seed.botho.io` and `ap.seed.botho.io`; each runs `botho.service` with `/usr/local/bin/botho`.

1. Captured local RPC, process/service memory, peer, chain and restart baselines over SSH. The July `8f498a8-dirty` build was present everywhere; all nodes were at height 2634. The faucet was hashing but had an active nomination stalled for over ten days and logged full gossip send queues. Its process used about 3.4 GiB resident memory plus 3.6 GiB swap. These observations do not establish the original stall's root cause.
2. Saved the old binary and unit in the root-only `/var/backups/botho-redeploy-20260921` on each host. Stopped each node for a consistent `testnet.tar.gz` archive containing its existing ledger, configuration, node key and wallet directory where present. Backups containing keys remain on their respective hosts.
3. Upgraded EU as a canary, verified the original height/tip and peering, then upgraded faucet, seed, seed2 and AP. All reported the pinned clean build. The rollout completed around **23:14:15 UTC**; no rollback was needed.
4. After the peer restarts, the faucet remained in initial discovery. One additional operator restart at approximately **23:15:38 UTC**, with the rest of the fleet settled, released its sync gate and started minting. Its existing systemd unit's `--mint-threads 1` took effect; the old process had reported two. No quorum, monetary, difficulty, or slot configuration was changed.
5. Renewed seed2's certificate and changed its saved renewal authenticator from `standalone` to `webroot`, using the existing `/var/www/certbot` nginx challenge location and a successful-renewal nginx reload hook. The old method repeatedly failed to bind port 80. The new certificate expires **2026-12-20**; a subsequent unattended renewal dry run passed. All five public HTTPS certificates validated normally.

There was **no chain reset, key replacement, CT/LotteryV2 activation, web deployment or Snap publication**. Wallet PRs #1378/#1379 were not part of this node candidate.

## Live payment and idle behavior

The faucet held **131,700 BTH**, above the hardcoded **10,000 BTH** high-water mark. The node deliberately pauses mining when its mempool is empty and resumes for pending transactions. An unchanged height during that idle pause is therefore insufficient evidence of a consensus stall.

One normal **1-BTH** grant was requested through public HTTPS to a fresh, isolated test wallet. Recovery material was retained only in a protected local directory and is excluded from this report and evidence.

| Check | Result |
|---|---|
| Request started / returned | 23:17:32 / 23:17:43 UTC |
| Transaction | `8266cff6334770633e127e5964603528992a2b66eda8bb2e36968fad6a4dc8e3` |
| Amount / fee | 1 BTH / 0.0001 BTH (100,000,000 picocredits) |
| Accepted block | 2635, `df440eab2490fa989a64af2a634a008ce1699d7e9f78b521e19bbddc982d33a9` |
| Follow-up tip | 2636, `f268ec1b77def04fef6b19d9004e103c12787bd61389ee87a896cd3b569bed99` |
| Fleet confirmation | All five local RPCs reported the same transaction confirmed with two confirmations by 23:19:36 UTC |
| Old-chain preservation | All five still returned the original genesis and height-2634 block hashes |

The grant propagated to all five mempools, woke mining, and confirmed. The minter then returned to its configured idle pause. This demonstrates a current public testnet payment through the native faucet path. It does not establish web/Snap interoperability, a recipient restore/second spend, bridge readiness, or confidential-transaction support.

## Stability observations and remaining work

Observation continued through **23:24:26 UTC**, approximately ten minutes after the rolling deployment. All five nodes were active, synced, on the same height-2636 tip, and reported zero process swap in the final sample. Every available post-rollout service sample recorded zero automatic restarts; the additional manual faucet restart is recorded above. Seven fleet samples from 23:20:31 onward all reported every node synced. This is a short recovery and idle observation window, not sustained-load acceptance.

The first payment took roughly two minutes from request to observed fleet confirmation, including miner warmup. The faucet's balance-pause path synchronously joins mining workers, which may still be building the RandomX dataset; its periodic balance scan also runs inside the network event loop. During the transition, peers logged sync-response timeouts and recovered, and seed temporarily reported unsynced despite holding the common tip. [#1381](https://github.com/botho-project/botho/issues/1381) tracks these responsiveness issues and the required measurements before treating the deployment as demonstration-ready.

One faucet SSH health sample timed out during the transition; that failed observation is retained in the evidence rather than treated as a successful health check.

Sampled faucet service memory peaked at approximately **2.57 GiB** during mining and returned to approximately **0.3 GiB** after idle; its process swap returned to zero. A few minutes of observations cannot show that the previous long-running memory growth is fixed. Longer loaded and idle runs, repeated payments after idle, and a controlled reconnect test remain necessary.

Sanitized observation samples, chain checks, build identity, certificate checks and the grant receipt accompany this report in the [evidence directory](testnet-redeployment-evidence/2026-09-21/). Seeds, private keys, configuration contents and wallet recovery logs are excluded.

## Rollback reference

The protected host directory retains `botho.previous`, `botho.service.previous` and the consistent `testnet.tar.gz` archive. The deployment wrapper restored the old binary automatically on replacement/readiness failure; that path was not needed. Any later rollback must first stop the service and take a new backup of the now-advanced ledger. Restoring the old archive would discard blocks 2635 onward and is a separate recovery decision, not routine binary rollback.
