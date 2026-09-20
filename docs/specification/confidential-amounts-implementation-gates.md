# Confidential amounts: integer specification and implementation gates

**Status: review draft; not a consensus specification or ADR acceptance.**
Source snapshot: `65687275` (2026-09-19 review). Tracks #1264, #902 and #904.
ADR 0006's confidential-amounts target remains the required end state.
ADR 0009 remains Proposed; its high-level construction needs the exact
integer rules below before a mainnet implementation can be accepted.

## Current implementation versus target

| Concern | Current source | Required CT evidence |
| --- | --- | --- |
| Outputs | `transaction/clsag/src/lib.rs`, `TxOutput.amount` is public | Committed amounts, range proofs, authenticated recipient recovery; retain universal hybrid stealth |
| Inputs | `ClsagInput.pseudo_output_amount`; `verify` builds a zero-blinding commitment | Hidden pseudo-output commitments bound by CLSAG to real ring members; no public real-input amount |
| Balance | Public input/output arithmetic | Commitment balance with fee and every mint/burn/settlement term bound to the signed transaction |
| Age | `botho/src/ledger/store.rs::consensus_fee_floor` calls `ring_elapsed_quantile` | Already value-independent; preserve the chosen quantile and canonical ring/height inputs |
| Factors | `effective_cluster_wealth_from_outputs`, `ring_centroid_floored_factor`, import floor still consume values | Fully specified public EpochOrigin/tag/factor derivation, including input-class floor and multi-output rules |
| Charge | `cluster-tax/src/demurrage.rs`: staged integer floors, cap, and difference of charges | Exact verifier/prover arithmetic and proof of equivalent economics or an explicitly accepted pre-mainnet change |

The existence of `transaction/core` range-proof primitives does not mean the
live CLSAG transaction path hides amounts. Likewise, a public epoch total
does not by itself remove the value weights from the factor's callers.

## 1. Rounding is consensus, not an implementation detail

For non-overflowing inputs, the current kernel computes:

```text
s = clamp(factor, 1000, 6000) - 1000
T = floor(elapsed * 1_000_000 / blocks_per_year)
A = floor(floor(V * rate_bps * s / 10_000) / 5000)
d = floor(A * T / 1_000_000)
```

The source also saturates the multiplication to u128 and clamps the result
to u64. A specification must cover those branches, even if valid-domain
bounds later prove them unreachable. The capitalized charge subtracts two
independently computed charges; spend-time charge is the maximum of accrued
and capitalized charges. Settlement shares the capitalization rule.

Replacing this with `ceil(p*V/q)` is not bit-exact. With rate 200 bps,
year 6,307,200 blocks and horizon 31,536,000 blocks:

| Case (amounts in picocredits) | Current result | Collapsed rational ceiling |
| --- | --- | --- |
| V=49, factor=6000, five-year charge | 0 | ceil(49/10) = 5 |
| V=51, downgrade 6000 to 3500 | 5 - 0 = 5 | ceil(51/20) = 3 |

These are counterexamples to exact equivalence, not a claim that the tiny
amounts are economically significant. Nodes must nevertheless agree on
validity at every boundary. The design review must select either a proof
encoding of the specified intermediate rounding operations, or an explicit
replacement rounding rule with an economic/compatibility review. It must
not silently call the two expressions equivalent.

## 2. The inequality witness is not necessarily a u64

For `C_V = V*H + r_V*G` and `C_d = d*H + r_d*G`, the proposed statement
uses `q*C_d - p*C_V`, whose value opening is `delta = q*d - p*V`.
Both V and d fitting u64 does **not** imply delta fits u64.

Using the same time-fraction precision, factor=5745, rate=200 bps and
elapsed=1,234,567 gives reduced `p=185756311`, `q=50000000000`.
For V=1,000,000,000,000,000,000 pico, rounding the charge upward to an
illustrative one-BTH bucket gives d=3,716,000,000,000,000 pico and:

```text
delta = 43,689,000,000,000,000,000,000  (76 bits)
```

The one-BTH quantum is a counterexample input, **not** a selected D1 policy.
The pinned Bulletproofs `RangeProof::verify_multiple_with_rng` accepts
8/16/32/64-bit ranges and the prover takes u64 witnesses. A two-scalar,
64-bit implementation would reject this valid nonnegative slack.

Before implementing, specify and prove:

- Bounds on V, summed values, d, fee, p, q and every intermediate quantity.
- Integer-to-scalar mapping and a no-modular-wrap argument: a field residue
  accepted by the range proof must imply the intended integer inequality.
- Either sufficient bounds for a single supported witness, or a reviewed
  limb/carry construction that binds every limb to the original commitment.
  Merely range-proving unrelated limbs does not establish that binding.
- Charge nonnegativity and fee coverage, including `fee < base`, zero
  coefficient, maximum balances and sums, malformed commitments and proofs.
- Transcript domain separation binding the network/protocol, signed
  transaction, commitments, public coefficients, fee and proof version.

Until that design is checked, the ~0.7-KB/two-scalar estimate in ADR 0009 is
provisional. Do not use it as an implemented size limit or performance claim.

## 3. Decisions still needed for a normative specification

| Decision | Concrete review artifact required |
| --- | --- |
| D1 fee quantization | Exact bucket function, minimum quantum/relative precision, overflow behavior, fee split, wallet behavior, and information-leakage analysis |
| D2 EpochOrigin realization | Canonical mint/import epoch totals, epoch completion semantics, tags, circulation/deflation pricing, factor aggregation and ring floor without hidden-value reads |
| D3 tag upper bound | Canonical input/output/tag ordering, maximum-weight rule, rounding, missing tags, and enforcement of anti-deflation costs |
| Integer charge | Resolution of §1 and §2, shared rules for ordinary spending and settlement, test vectors at each rounding/cap boundary |
| Network transition | Protocol/version activation, genesis/reset plan, rejection of mixed formats, wallet recovery and rollback procedure for disposable testnet |

D2's high-level choice has ratification evidence on #902; this table does
not reopen that choice. It identifies the missing implementable rules.
Fee quantization limits precision under stated assumptions; it does not
guarantee a fixed anonymity/entropy budget against arbitrary prior knowledge
or repeated observations. State the observer model and evaluate those cases.

## 4. Implementation sequence and acceptance evidence

1. **Freeze the normative rules.** Review the arithmetic, public-factor
   rules and adversarial cases above; record explicit ADR acceptance and
   exact parameters. Cross-reference every rule to code or mark it unbuilt.
2. **Build proof primitives behind an inactive version.** Retain existing
   Pedersen generators and hybrid stealth. Verify output ranges, hidden
   pseudo-output equality, balance, demurrage and settlement statements.
   Test wrong transcript/network, malformed points, negative/wrapped slack,
   missing limbs, replay and tampered fees independently of happy paths.
3. **Integrate all consensus consumers.** Ledger, mempool, block acceptance,
   fees, lottery, supply/reserve accounting and snapshots must agree without
   plaintext value access. Search/audit each existing `.amount` consumer;
   do not substitute zero for unavailable values. Keep public coinbase and
   bridge-boundary disclosures explicit rather than hiding required checks.
4. **Migrate every transaction producer and reader.** Node/CLI, web/mobile/
   Snap signers, scanning, recovery, RPC, explorer and bridge tooling must
   support commitment/blinding/amount encryption and authenticated recovery.
   Cross-implementation fixtures must spend outputs created by each wallet.
5. **Execute the pre-mainnet reset and adversarial validation.** Demonstrate
   multi-node acceptance/rejection parity, supply conservation, demurrage
   economics, malformed-proof resistance, reorg/sync recovery, real wallet
   sends and factor-1 bridge settlement. Record exact build/configuration
   and artifacts; green unit tests alone do not satisfy this gate.
6. **Audit the release candidate.** External crypto/consensus review must
   cover the implemented CT surface and remediation commit, followed by
   reproducible release verification. An earlier public-amount audit cannot
   stand in for that review.

## Reproduce the arithmetic review

```sh
python3 scripts/research/ct_demurrage_integer_audit.py
```

This standard-library script checks the counterexample calculations. Its
reference is explicitly transcribed from the named source snapshot; it is
not an automated Rust parity test or proof-system implementation. Reviewers
must compare it with `demurrage_charge` and `capitalized_reset_charge`, and
replace these examples with cross-implementation consensus vectors once the
normative rules are selected.
