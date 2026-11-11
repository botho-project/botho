# ADR 0015: The Ring Floor — Amount-Matched Max per Input Now, Max over the Ring under Confidential Amounts

**Status**: Accepted for protocol 10.0.0 (operator ruling 2026-11-10, recorded on #1686). The CT-era successor is the agreed plan; it has two open questions (below) and needs its own spec before it can ship.
**Date**: 2026-11-10
**Decision Makers**: Operator
**Related**: issues [#1686](https://github.com/botho-project/botho/issues/1686) (this decision), [#1687](https://github.com/botho-project/botho/issues/1687) (shared spend-candidate selection, PR #2000), [#2001](https://github.com/botho-project/botho/issues/2001) (wallets compute the consensus floor), [#902](https://github.com/botho-project/botho/issues/902) (confidential amounts), [#925](https://github.com/botho-project/botho/issues/925) (downgrade charge), [#1757](https://github.com/botho-project/botho/issues/1757) (production decoy routes), epic [#1683](https://github.com/botho-project/botho/issues/1683); [ADR 0006](0006-pq-architecture-ratification.md) (CT is the target), [ADR 0009](0009-confidential-amounts-economics.md) (CT economics); `docs/research/ring-floor-dilution-sweep.md`, `docs/research/ct-economics-gadgets.md` §4 and §5.3

## Context

A spend's demurrage factor is `max(claimed, ring floor, import floor)`. The
claimed factor comes from the spender's own output tags, so the **ring floor**
is what stops a wealthy spender from declaring background outputs and paying
nothing: it is the floor under the #925 downgrade charge.

Through protocol 9.x the ring floor was `ring_centroid_implied_factor`: the
value-weighted mean of cluster wealth over every member of every ring of the
transaction, pooled. The spender chooses the decoys, so padding a ring with
high-value background decoys diluted it. For a coin from a 10M-BTH cluster in
a ring of 20, the floor fell from 5.745× to 4162 (1000 BTH coin, 2000-BTH
decoys), 1441 (10 BTH coin) or 1000, which is 1×, (1 BTH coin, 100k-BTH decoys).
The attacker pays only for the split.

The dilution sweep (`docs/research/ring-floor-dilution-sweep.md`, PR #1995)
compared the mean, the amount-matched mean and max, the max over the ring, and
several quantiles on dilution, honest over-charge, privacy and compatibility
with confidential amounts (CT). Only the two max-type rules passed the #1686
bar of collecting at least 95% of the undiluted charge in every cell. Of
those, only the amount-matched max had no honest over-charge.

## Decision

### Now (public amounts, protocol 10.0.0)

1. **Per input, the max over amount-matched members.** With public amounts the
   spent member is one of the ring members whose amount equals the input's
   `pseudo_output_amount`: C3 (`verify_ring_members`) rejects an input whose
   pseudo-output amount matches no member, and the balance proof ties it to the
   real member. The input's floor wealth is the **largest** effective cluster
   wealth among those members. A member with a different amount is provably a
   decoy and is ignored. A same-amount decoy can only raise the floor.
2. **Across inputs, value-weighted by `pseudo_output_amount`.** The per-input
   floor wealths are averaged with the pseudo-output amounts as weights, and
   the curve is applied once. This is the same way the claimed side is
   computed (output wealth averaged by output value, then the curve), so an
   honest multi-input blend is compared like for like.
3. **One shared selection.** The candidate set is
   `bth_cluster_tax::amount_matched`, the same selection the #1687 rules (the
   import floor read from the inputs, and the retained-weight floor) use, so
   every rule agrees on which members can be the spent coin.
4. **One shared kernel.** `bth_cluster_tax::amount_matched_ring_floor_factor`
   (`cluster-tax/src/ring_floor.rs`) is the only implementation. The node calls
   it through `ValidationReads::ring_floored_factor_for_tx`, so block
   application, SCP value validation, the block builder and mempool admission
   run the same code. The simulations call it through
   `fee_floor_inputs::resolve_fee_floor_at`. Wallets call it to estimate the
   consensus floor (#2001). Its inputs are each ring member's amount and
   effective wealth, both public.

Measured (sweep re-run against the production kernel): 100% of the undiluted
charge in every dilution cell, including decoys of the coin's own amount; 0%
honest false positives on all three production decoy routes (1.1% if 25% of
amounts are round); 0% on honest two-input class blends. It reads only what
public amounts already reveal: the real input is the only member with its
amount in 99–100% of rings.

### Under confidential amounts (successor, not yet buildable)

Under CT neither member amounts nor `pseudo_output_amount` are public, so rule
(1) and the weights in rule (2) cannot be computed. The successor is:

1. **Per input, the max implied factor over the whole ring.** It is value-free
   and dilution-proof: the real member is in the ring, so the floor is at least
   its factor whatever the decoys are. The sweep measured 100% collection in
   every cell. This is the order-statistic move `ct-economics-gadgets.md` §5.3
   already proposed, with the max chosen because every lower quantile collects
   0% against a spender who picks 19 of 20 members.
2. **A production wallet filter that drops wealthier decoys.** Without it, the
   max over-charges every honest spend whose ring holds a decoy wealthier than
   the real input. That is 54.5% of honest spends at 5% wealthy outputs, about
   the same as the pre-10.0.0 mean. The wallet redraws any decoy whose implied
   factor exceeds the real input's. The sweep measured the cost: about 5% of a
   background spender's draws and 0.8% of a whale's. The max-factor
   adversary's identification rate stays at the uniform 5% for background
   spenders but rises from 80% to 93% for whales, whose ring max is usually
   already the real input. `select_decoys_cluster_aware`
   (`botho/src/decoy_selection.rs`) implements a two-sided version of this
   band and has no production caller; the successor needs the one-sided form
   wired into every production route (#1757: node, CLI, web).

## Open questions (must be resolved before the CT successor ships)

1. **Combining inputs without amounts.** Rule (2) weights each input's floor by
   its pseudo-output amount, which becomes a commitment under CT. A pooled max
   over all inputs over-charges every honest class blend (100% of the sweep's
   wealthy-plus-background two-input spends), and per-input max with equal
   weights over-charges any blend of unequal values. Candidates: a committed
   weighted floor proved in zero knowledge against the input commitments (the
   §2 gadget's machinery), weights from a public value bucket, or a rule that
   requires multi-input spends to declare a per-input class split and proves it.
   None is designed.
2. **Cluster wealth under CT (`ct-economics-gadgets.md` §4).** Every member's
   implied factor still needs that member's cluster wealth, and cluster wealth
   is a sum of hidden amounts. This is the same irreducible core as the claimed
   factor: Option A (a bucketed curve evaluation over a committed wealth) or
   Option B (a value-free provenance factor). The ring floor inherits whichever
   is chosen; it adds no new primitive, but it cannot ship before it.

## Consequences

**Positive**
- The #925 downgrade charge can no longer be diluted by choosing decoys.
- Wealthy decoys no longer over-charge honest spends: the sweep's honest
  false-positive rate drops from 54.3% (old mean, 5% wealthy outputs) to 0%.
  That 0% is measured against the spent coin's own undecayed class. It counts
  a charge for lowering the class below that as correct, so it does not mean
  honest spends pay no downgrade charge (see Negative).
- Relay, consensus, simulations and wallets share one kernel and one candidate
  selection.

**Negative**
- The floor is now the spent coin's own undecayed class. Every hop that lowers
  a wealthy coin's class therefore pays the #925 downgrade charge, including
  the wallet's normal 5% per-hop tag decay. At the current settlement horizon
  and rate the charge is 2.0% of value per 1× of class drop. Measured in
  `docs/research/bridge-import-calibration.md` §0 for a 5.448× flood coin
  (before → after #1686):
  - honest domestic spending, cumulative over 20 spends: 0.19% → 2.29%;
  - stock-wallet self-churn until the tag is pruned: 0.108% → 9.0%;
  - shedding through same-amount decoys (max-shed to 1×): 0.154% → 8.9%.

  This is intrinsic to an undiluted floor. Per-hop decay and deliberate
  shedding both lower the class one hop at a time, and the floor cannot tell
  them apart. Exempting the wallet's decay step would reopen the self-churn and
  shed paths, so any exemption would be a separate rule with its own analysis.
  The operator accepted this cost on 2026-11-10 (#1686): exempting per-hop decay would reopen the churn and shed paths, and deferring would leave decoy dilution open in 10.0.0.
- Consensus now depends on public amounts in one more place. ADR 0006 makes
  CT the target, so the rule has a known end of life and its successor still
  has two open questions.
- A spender can raise their own floor by adding a same-amount decoy from a
  wealthier cluster. That costs only them, so it is not an attack.
- An honest spend whose ring by chance holds a same-amount decoy from a
  wealthier cluster is over-charged. The sweep measured 1.1% of spends when a
  quarter of amounts are round, and 0% otherwise.

**Neutral**
- `ring_centroid_implied_factor` stays in `bth-cluster-tax` for the
  simulations that study the old rule and for its Lean model; the node no
  longer calls it.
- Protocol version: part of the 10.0.0 batch, no separate bump.

## Alternatives Considered

- **Keep the value-weighted mean.** Fails the dilution bar (0% in the worst
  cell) and has the same honest false-positive rate as the max.
- **Amount-matched mean.** Loses 23.8% of the charge to decoys of the coin's
  own amount.
- **Max over the ring now.** Dilution-proof, but without the wallet filter its
  honest over-charge is the same as today's, and its cross-input composition
  is an open question. It is the CT-era plan, not the public-amount rule.
- **A median or high quantile.** Collects 0% in effectively every cell: 19
  attacker-chosen members of 20 defeat any order statistic below the max.
- **Wallet-side band only.** Not enforceable by consensus.
