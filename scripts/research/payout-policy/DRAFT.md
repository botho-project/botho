# Source-review draft, no captured results

Part of #1306 via #1372. Based on main df09a41d. PR1370 archived observations
remain the baseline; no original model or captured source has been changed.

`policy.rs` is an unregistered pure proposed adapter, outside production crates.
It consumes production-computed available pool, cap and eligible count; chooses
zero distribution or the configured count; and returns exact carryover. It never
raises the cap or changes fees. `check.py` independently checks raw recurrence and
individual award division/remainder using integer inequalities rather than this
Rust function. The proposed adapter is now wired into the new ct_payout_policy integration target; no compile or execution has run.

Implemented integration seam for review: moved only the non-test history/account/draw
helpers from ct_economics_reinvestment.rs into a shared integration-test module;
keep its three tests in the old target. Parameterize a draw decision callback and
an optional block observer. Original callback retains exact current behavior.
New target uses the proposed adapter, actual draw kernel and actual funded model.
All baseline JSON fields must equal archived candidate_age720 rows exactly; the
new per-block observations live separately and cannot perturb RNG/hash inputs.
If this extraction cannot preserve those bytes, stop before capture and review.
Update fresh source inventories; never rewrite archived measured provenance.

Collector plan: distinct schema3 target, locked release compile with assertions
and overflow checks,900s process-group deadline, uniquely selected Cargo JSON
artifact and executable hash. Record all CARGO_PROFILE_* and both Rust flag
variables, exact source/config/build/log/raw hashes and checkout. One six-history
process plus one baseline replay and controls,180s deadline, no retries. Every
outcome is retained; failed/incomplete is not success. Original target regression
and archived baseline comparison are required separately, not silently counted as
part of the six histories. No compile or execution is authorized at this checkpoint.

Every block observer records pre-reserve, gross fees, split/burn, cap, available,
eligible count, decision/reason, actual winner IDs/values and post-reserve. Fee
inflow lots are tracked FIFO solely as an accounting attribution convention for
waiting time, not claimant entitlement. Track eligible IDs at each deferral and
which age out before release, with bounded public population; unchanged public
records remain after private spends. Report per-owner capture/fees/remaining
principal, fee/value ratios, all failure denominators and concentration; fewer
larger payouts can change variance and cohort access even with uniform draws.
No assumption that a 0.250001-BTH payout is usefully affordable.

Required edge controls: no eligible outputs, zero inflow/reserve, capped positive
reserve, rho1 cap below threshold, exact threshold and ±1 pico, four-slot
threshold and ±1, integer remainder to last winner, prolonged cap-stalled reserve,
release after sufficient real fee inflows, near-u128 accounting saturation parity,
invalid typed/negative/overflow checker inputs, altered recurrence/award totals,
and unchanged baseline fields. Zero-main-grid recycling remains a valid result.
CPU timing is not energy; this draft provides no new resource-price calibration.


Source checkpoint: source_regression.py checks exact original three test bodies,
helper equivalence after reversing only visibility/callback changes and rustfmt
trailing commas, and unchanged archived raw bytes. This does NOT prove regenerated
rows equal without execution. The new target explicitly compares serialized
serde_json Values against archived rows for both baseline seeds; that runtime
acceptance is pending. Four small Python policy-checker tests pass.

Collector, aggregate owner checker composition, inherited config validation and
FIFO attribution are now drafted. The observer records eligible expiry histograms
at fee inflow; independent Python attribution consumes fee-pool lots FIFO and
records delay, expired original-cohort counts per released portion, unreleased
lots and exact owner concentration numerator/denominator. This is attribution,
not a retained entitlement to a future draw. No successful capture is claimed.
Five Python checks cover threshold neighbors/remainders, empty/cap stalls,
malformed recurrence/types, arithmetic saturation and FIFO cohort expiry.

Final source paths: common/reinvestment_model.rs; ct_payout_policy.rs;
ct_economics_reinvestment.rs (tests retained); fresh ct-reinvestment/check.py
source inventory; payout-policy/{policy.rs,check.py,collect.py,config.json,
test_check.py,source_regression.py}. No original archived files are changed.
Collector selects exactly one six-history/replay test; the independent small
Python controls run separately. Original target regression remains required
before acceptance. No Rust compile or numerical run at this checkpoint.
