# LotteryV2 proof boundaries

The executable model is in `Botho/Lottery/`; its Rust correspondence and
existing differential tests are described in [README.md](README.md#the-lottery-model).
The theorems below are **proven (model)**, exported in `Theorems.lean` and
checked against the statement records. They quantify over the hash functions
in `Prims`; no cryptographic property is assumed and no hash axiom is added.

## Seed-independence

`SeedIndependence.lean` proves:

- `seed_body_independent`: changing the drawn block's fees, reward or emission
  share preserves the entire seed result when height and parent link agree.
- `seed_reads_tip_only`: rows and stored pool do not influence the seed.
- `seed_factors`: a successful seed is exactly
  `lotterySeedV2 P (P.powHash r.tip) r.tip.prevBlockHash b.height`.
  `seed_eq_of_pow_fields` compares successful seeds with equal PoW output,
  parent previous hash and height. It does not remove parent-link validation:
  `headerHash` still reads all parent header fields before a seed succeeds.
- `candidates_of_postState`: every successful drawing uses exactly
  `lotteryCandidates P r.rows b.height s canonicalConfig.draw`, with the
  successful seed `s`, the checked fee sum and its resulting accounting.
  The input rows and tip are the pinned state after block N−1, used at block
  N's height. This is the [#1689](https://github.com/botho-project/botho/issues/1689)
  boundary; a change to that state timing must update this theorem and the
  production correspondence.
- `winners_ids_independent_of_amounts`: successful drawings with the same
  tip, rows, height and parent link select the same ordered winner IDs when
  both payouts are nonzero. Fees, reward, emission share and stored pool may
  differ. Payout values may differ; a zero payout may suppress the draw, and
  changing fees may instead cause a fee-overflow error. The hypotheses retain
  these restrictions. The supporting `drawLotteryWinnersSeeded_ids` theorem
  applies to any fixed draw configuration. `pickWinners` uses payout only to
  populate winner amounts; the canonical winner count is `min(4, eligible.length)`.

**Model-abstraction caveat.** `DrawBlock` omits block N's timestamp, nonce,
transaction root and minter keys. Their inability to reach the seed is true
by construction of this model. The evidence that Rust reads no more than
modeled is the existing `drawing` DRT leg, which calls Rust's drawing through
an in-memory `DerivationReads` double. It is finite test evidence, not a proof
about all Rust executions. Source inspection at `bef3be4b` confirms both
production `DerivationReads::lottery_candidates` implementations pass only
`block.height()` from the current block to the common reader:
[`store/v2.rs:633`](../botho/src/ledger/store/v2.rs#L633) (call at line 658) and
[`experimental/validated.rs:241`](../botho/src/ledger/experimental/validated.rs#L241)
(call at line 247). The strict readers also check stored row context; those
read failures are outside the decoding-success model.

## Determinism

`Determinism.lean` adds `sortById`, a stable merge sort by the existing
`bytesLt` comparator. This is a specification of LMDB's default key ordering
(`memcmp`, with shorter keys first), **trusted** as a correspondence to LMDB;
it is not an additional Rust sort or a change to the existing drawing model.
The existing `lottery_candidates` DRT leg runs the production reader over a
real LMDB ledger and supplies finite evidence for that correspondence.

The byte comparator is proven irreflexive, transitive and trichotomous on
all byte strings, including equal-length 36-byte IDs. `sortById_sorted`
proves strict ordering when IDs are unique; `sortById_of_sorted` proves the
existing strictly ordered model inputs are unchanged. `sortById_perm`
uses core Lean's sorted-permutation uniqueness theorem, with ID uniqueness
to identify entire rows. `drawing_perm_invariant` then proves the complete
drawing result, including errors, is invariant under any permutation of
uniquely keyed rows after canonical sorting. Equal IDs carrying different
row data are excluded, matching LMDB's unique keys.

Canonical ordering affects outcomes. `draw_order_dependent` uses two
eligible, distinct 36-byte IDs and constant-zero hashing, making every roll
zero. A pool of 3 pays `[a:1, b:2]` in one order and `[b:1, a:2]` in the other:
selection visits the rows in order, and `addToLast` assigns the remainder to
the final winner. This is an explicit mathematical witness, not a model of
production SHA-256. `scan_cap_order_dependent` gives a second witness at the
actual 10,000-candidate cap: with 9,999 already collected, reversing the
next two eligible rows changes which one gets the last slot.

These results establish determinism conditional on canonical rows and a
fixed `Prims`. They establish neither hash unpredictability nor a theorem
about LMDB, Rust, corruption handling or the timing issue in #1689. The
executable drawing and DRT dispatch are unchanged by these proofs.
