# Inactive CT1 funded payments and public ticket lifecycles

This is a bounded follow-up to [the fixed-capital controls](ct1-fixed-capital.md),
part of #1306 and implemented by #1323. It connects honest payment fees to output
creation and private spending, then includes lottery payout outputs in future
eligibility. It changes no production rule, wallet policy, protocol or price.
It is **synthetic fixed-stock workload evidence**, not observed calibration,
stationary profitability, a full wallet transaction rehearsal or D3/base approval.

## Exact workload and implementation boundaries

There are sixteen histories: seeds 1306/902, an honest payment attempt every
100/500 blocks, and attacker strategies idle, hold one output, hold two outputs,
or refresh the same two-output position every 720 blocks. Every history starts
with 100 honest owners and one attacker, each with 32 BTH. Honest outputs start
1,000 blocks old; the attacker's original funding output is outside the lottery
window. Strategies have identical initial wealth and paired payment attempts.

Each history covers heights 20,000 through 31,231 inclusive (11,232 blocks).
Honest senders rotate through the 100 owners, paying the next owner in a fixed
cycle of 0.1, 1 and 10 BTH. The model selects that sender's largest single
spendable output, requires ten confirmations, and creates payment plus change.
This is an explicit **single-input workload rule**, not the production wallet's
complete coin-selection/rebuild algorithm. Requests cannot borrow future fees,
create dust change, or mutate inventory on failure. Refreshes spend the current
one/two-output attacker position into the configured count; lottery payouts
remain locked and cannot fund payments or refreshes pending #1286.

Ring membership invokes the actual node
`GammaDecoySelector::select_decoys_for_input` with 19 decoys, mature public
outputs in ledger key order and all selected real input keys excluded. Synthetic
point bytes identify outputs; no signature, WASM, RPC wallet orchestration or
accepted transaction is claimed. The pool retains historical outputs after
private spending, matching the source path. Lottery candidate keyspace rotation
is a source-parity adapter checked against the real ledger API in exact order.
The largest eligible population stays below the production 10,000-candidate cap;
the harness fails rather than approximate the cap if its bound is exceeded.

All factors are background 1000. The candidate rate is 200 bps and bucket setting
s2, using the existing reference wrapper around production charge kernels. At
factor 1000 the demurrage component is zero; a two-output payment costs the
proposed 0.5-BTH base. This isolates population/fee coupling and **does not extend
the earlier high-factor honest-overcharge study**. Candidate pricing remains
proposed. No observed amount, age, factor or ownership distribution was supplied.

The experiment excludes new miner outputs and asserts that the actual lottery
emission-share helper returns zero at every chosen height. It therefore models
a fixed-stock payment economy, not the entire evolving chain. Lottery fee split,
cap, drawing, carryover and integer payout remainder use production functions.
The block-hash schedule is the same disclosed synthetic `seed XOR height` input
used in the prior checkpoint; it is not a chain-randomness security experiment.

## Two distinct inventories

Private spendable principal sums unspent ordinary outputs. Payout claims are
accounted separately and locked from funding pending the ownership/discovery
limitation tracked in #1286; inactive V2 integration is not assumed. Spending
consumes ordinary balances and pays the gross fee once. Public records remain available as
candidates until the public age/value rules exclude them. Key images do not
identify the real spent ring member in the candidate scan. A historical record
therefore never adds its old principal to private wealth a second time.

Every lottery payout creates a new public output and a locked claim attributed
to the winning record's modeled owner. Its distinct output ID inherits the
winner's target and public keys, matching the source. No independent discovery
or spendability is claimed. Selected membership must satisfy size, uniqueness
and input-exclusion invariants before any monetary mutation; invalid attempts
remain in the reported denominator. Its creation height is the payout height; it cannot
be lottery-eligible until age 720 and expires after age 10,000. Payout-generated
tickets can themselves win later. The model checks conservation each block:

`honest spendable + attacker spendable + all locked payout claims + reserve + burn = initial stock`

and independently:

`gross fees = cumulative lottery awards + reserve + burn`.

The actual ledger parity test loads controlled snapshots through `UtxoSnapshot`
and `Ledger::load_from_snapshot`, including a recorded key image. It checks
stored outputs and exact candidate order at ages 719/720/10000/10001 under two
hashes. A payout-shaped output is inserted as fixture state; the test does not
claim to execute a signed spend or an accepted payout block. Production ledger,
codec and existing source-inventory files are unchanged.

## Finite observations

The 100-block cadence funds 113 honest payments (56.5 BTH gross fees); the
500-block cadence funds 23 (11.5 BTH). All scheduled main-grid payments select
and fund successfully. Separate tests deliberately reject unaffordable payments
and insufficient decoy pools with unchanged inventory. Refresh strategies
complete 16/14/10/9 of their 16 scheduled attempts (cadence 100 seed 1306/902,
then cadence 500 seed 1306/902); the other attempts fail membership validation
and pay no modeled fee. These are unsigned construction attempts, not submitted
transaction rejections. Full denominators are preserved.

The final **accounted-value** differences below include unavailable payout
claims and are relative to the same 32-BTH idle holding. They are not realizable
profit or spendable gains:

| Honest cadence | Seed | Hold one | Hold two | Refresh two |
|---|---:|---:|---:|---:|
| 100 blocks | 1306 | +0.15 BTH | +0.30 BTH | −6.00 BTH |
| 100 blocks | 902 | +0.15 BTH | −0.10 BTH | −5.20 BTH |
| 500 blocks | 1306 | −0.05 BTH | −0.10 BTH | −4.10 BTH |
| 500 blocks | 902 | +0.05 BTH | −0.30 BTH | −4.10 BTH |

These are paired sampled observations, not confidence intervals. The compact
artifact separately stores a conditional uniform-ticket capture benchmark,
summing each realized block payout times the attacker's realized eligible share.
It is floating-point reporting over the observed evolving path, not an exact
counterfactual expectation or consensus arithmetic.

Fresh payout tickets and retained spent public records materially populate these
histories; neither is treated as an external fee faucet. The refresh strategy
pays a half-BTH fee only on a successful attempt. Final spendable attacker
principal is 31.75 BTH for hold one, 31.5 BTH for hold two, and
24/25/27/27.5 BTH for refresh in the order above; all awarded value is locked.
Any successful final scheduled refresh has not matured by the finite endpoint;
unearned future capture is not credited. Private principal remains included at face value. No stationary claim
or universal splitting-resistance conclusion follows from this terminal window.

The earlier #1320 funded-fee example used 2.5 BTH of external gross fees **each
block**. These payment workloads have much smaller fee flow. Their results are
not a controlled estimate of the effect of population coupling alone, and do
not invalidate or replace the earlier conditional observations.

## Reproduction and remaining work

```sh
CT_ECONOMICS_WRITE_REPORT=1 cargo test --locked -p botho --test ct_economics_workload -- --nocapture
python3 scripts/research/ct-economics/workload_summary.py
```

The suite runs all sixteen distinct histories and exactly replays one complete
history. `workload-config.json` records every workload axis;
`workload-summary.json` preserves compact rows, failure denominators, spendable/locked value and
fee accounting, ticket snapshots, per-block metric transcript digests (height, fees, payout,
eligible count and attacker accounted value), exact input/source hashes
and runtime provenance. The larger raw file is generated locally and uploaded
by CI. The summarizer independently checks totals and paired comparisons.
Compilation has a separate CI allowance; execution and summarization have a
three-minute limit. Local execution is not substituted for Linux CI.

#1306 stays open for observed workload calibration, richer funding/ownership and
factor distributions, full wallet payment construction, terminal-horizon and
longer reinvestment controls, welfare/Gini evaluation and an accepted
fee/affordability objective. None of those policy choices is inferred from these
sixteen background-only synthetic histories.
