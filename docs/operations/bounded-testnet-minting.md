# Bounded continuous minting on testnet

A sole producer with a high wallet balance pauses when its mempool is empty.
Ordinary payments wake it again, but wallet outputs and age-matched decoys need
block progression even when no eligible payment can yet be formed. An operator
can explicitly keep the existing testnet producer minting until an absolute
Unix timestamp:

```sh
botho --testnet run --mint --mint-threads 1 --testnet-mint-until <UNIX_SECONDS>
```

Replace `<UNIX_SECONDS>` with the experiment's recorded, fixed deadline. The
CLI rejects mainnet, expired deadlines, and deadlines more than 82 hours ahead.
The option is disabled by default, does not enable minting on its own, and is
not written to configuration. Reuse the same deadline after a restart: computing
a new deadline in `ExecStart` would silently extend the experiment. Once the
original deadline has passed, remove the option before restarting the service;
startup with an expired option fails instead of renewing it.

During the window, only the local high-balance pause is bypassed. A previously
balance-paused producer may resume subject to the existing mint-request,
quorum, and initial-sync guards. Wallet, identity, ledger, transaction limits,
block validation, difficulty, and reward calculations retain their normal
behavior. Blocks generate normal rewards; enabling continuous production
therefore increases actual issuance and resource consumption compared with an
idle producer. This option does not request faucet grants or submit payments.

At expiry the next balance-control tick (normally ten seconds) stops mining
before any fallible balance scan. After a successful scan, the normal policy
applies: high balance plus an empty mempool stays paused; pending transactions
or a low balance can resume mining subject to the existing quorum/sync guards.
A failed scan leaves the expired producer stopped. Expiry does not stop the node or prevent ordinary
payments. A monotonic timer bounds the running process even if the wall clock
moves backward; an observed expiry or clock failure cannot reactivate the
window. Keep the host clock synchronized, including across restarts.

## Experiment deployment

Record an explicit revised experiment plan before use. The existing stress
plan leaves producer changes disabled: enabling this option is a recorded
operator change, not something the controller may do automatically. Choose a
fixed deadline covering at most eight hours of setup, 72 hours of workload,
and two hours of observation/draining. Complete setup before the existing
setup deadline; this option does not relax input age, decoy, accounting, fee,
funding, or health gates.

Use the existing producer, wallet, data directory, peer identity, and network
configuration. Set one minting thread to bound hashing demand; RandomX still
needs approximately 2 GiB for its shared fast-mode dataset plus node overhead.
On the existing four-GiB producer, retain the stress runner's memory/headroom
checks and monitor swap and networking latency. One mining thread over the
maximum 82-hour window budgets at most 82 thread-hours of hashing; node,
networking, and dataset initialization add overhead. This is a resource budget,
not a claim about achieved hashrate or block cadence.

Before activation, verify the pinned binary/source, common finalized history,
node identity, peers, and resource headroom. Back up the service unit outside
public logs and install a durable, absolute-deadline restoration timer before
restarting with the fixed argument. The timer should remove the temporary
argument and restore the original unit while preserving the advanced ledger.
Capture the new PID/service configuration as the campaign baseline so expected
deployment is not mistaken for an unplanned restart. A single-version stress
manifest requires the same reviewed binary on all five nodes; only the
producer receives the option. Keep all other runtime breakers unchanged.
