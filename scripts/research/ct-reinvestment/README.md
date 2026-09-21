# Candidate payout recycling: inactive finite model

Part of #1306 via #1366. Eight fixed histories compare two seeds with legacy
locked, synthetic-independent locked, consolidation after age 720, and
consolidation after age 10001. The candidate keys are distinct synthetic model
identities, not V2 derivation or authenticated wallet claims. The local seven
native accepted spends in #1358 support conditional feasibility; that child still
requires its separate review/Linux gate. No CT1, activation or legacy repair is
inferred.

All owners start with 32 BTH. The existing funded payment cycle runs every 100
blocks. Each owner has one consolidation opportunity every 100 blocks, consuming
up to sixteen oldest policy-aged awards into one self-output. A failed attempt
changes no balances, fees or inventory. It cannot borrow ordinary principal to
pay this fee. Successful recycled outputs may later fund ordinary payments.
Fees retain the existing unratified background model: quantized charges plus
0.25 BTH times max(input count, output count). Consolidating N awards therefore
costs N times 0.25 BTH at background factor; batching 0.1-BTH awards cannot make
them affordable. The earlier planning premise that consolidation amortizes the
base was incorrect. Main-grid fees and selection remain unchanged.

A separate positive accounting control spends an existing 32-BTH ordinary input
into sixteen self-outputs, paying the actual helper-derived 4-BTH fee. Actual
pool accounting and the draw fund larger awards, which can pay their own
input-count-dependent consolidation fee. This control changes no main-grid
parameter and injects neither a fee override nor free principal. Duplicate input
IDs are rejected before charges or state mutation.

Ten confirmations, lottery maturity at 720, lottery expiry after age 10000, and
owner policy delays are distinct. Spending does not remove a historical public
ticket, and expiry does not erase private principal. Award principal is reconciled
as unspent plus consumed against cumulative capture. Self-consolidation is not
payment income; fee burn is not charged twice. The independent checker sums all
101 owners and every consolidation receipt, alongside block-level invariants.
The old sixteen-history report/config/raw observations remain historical and
unchanged. Their shared model is extracted without registering old tests in the
new target. The fresh old-workload collector now hashes that shared file; dated
captures retain their original source lists.

The new collector permits exactly one run with a 900-second compile bound and
180-second process-group matrix bound. Release optimization retains debug
assertions and overflow checks. The matrix includes one exact deterministic
replay and separate boundary/accounting controls. No partial/failed result is a
pass and there are no retries or runtime schedule adjustments.

The first capture followed parent source review. See [RESULTS.md](RESULTS.md)
for observed outcomes and captured evidence. Do not overwrite an existing capture.

```sh
python3 scripts/research/ct-reinvestment/collect.py \
  --target-dir /path/to/approved-idle-target --output /new/capture/directory
python3 scripts/research/ct-reinvestment/check.py /new/capture/directory
```

The local capture passed; hosted Linux evidence is pending. A zero-success
main-grid mode is a valid outcome,
not a reason to relax the rules. Oldest-first selection without ordinary-principal
subsidy is this fixed strategy, not an optimal strategy; zero successes do not
prove that all awards are forever unspendable. Before publication report selection/affordability
failures, consumed/unspent/ordinary value, all owner fees/capture, retained public
and recycled ticket counts, exact source/config/raw provenance and hosted Linux
results. Fixed-stock finite histories do not establish steady state, observed
calibration, electricity cost, welfare, universal split resistance or a fee target.
