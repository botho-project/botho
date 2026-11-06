# Source-derived review of the inactive exact CT relation

Reviewed 2026-11-06 for [#1953](https://github.com/botho-project/botho/issues/1953),
part of [#1914](https://github.com/botho-project/botho/issues/1914). This review
derives the complete allocator's integer relation in both directions from the
delivered source. No substantive arithmetic or statement-binding defect was
found in that scope. Acceptance of a cryptographic proof implies that relation
only under the unresolved premises below. This is research documentation, not a
formal Rust/R1CS equivalence theorem, security reduction, or protocol activation.

## Immutable scope and source map

The implementation is merged [PR #1955](https://github.com/botho-project/botho/pull/1955),
commit `6bc0b08623e973a79ea5be084fe5c6c094faa669`, tree
`0fde6491d16e16674f4d27532c59ee79d74752cf`. The documentation was started from
`4606a66f7fbe075921968dac635ab5a6a86fc2c1`, whose only intervening changes are
`scripts/stress/prefund.py` and `scripts/stress/test_prefund.py`.
[The manifest](exact-relation-review-sources.json) records SHA256 of raw
`git show <source_commit>:<path>` bytes, checked separately at the documentation
base; every listed source is identical there. It also pins dependency resolution,
the nightly toolchain declaration, cryptographic boundary files, and context-only
model sources. Registry package checksums identify dependencies; they do not
certify their implementations. The resulting documentation commit is the commit
containing these additions, not the implementation commit; resolve it with
`git log -1 --format=%H -- scripts/research/ct-demurrage/EXACT-RELATION-REVIEW.md scripts/research/ct-demurrage/exact-relation-review-sources.json`.
The PR/Builder receipt records its head without a self-referential file hash.

Paths in this table are relative to this crate unless marked repository paths.

| Source / symbols | Obligation traced |
| --- | --- |
| `src/lib.rs`: `Candidate::{exact_charge,construct,check,allocate}`, `Expr::{eval,lc}`, `order`, `scalar` | Canonical witnesses, integer checks, expression translation, census |
| `src/proof_experiment.rs`: `CircuitLayout::{exact_charge,new,allocate,assert_reference_parity}`, `Bindings::COMPLETE`, `prove`, `verify` | Independent public-only graph, ranges, all external equalities, complete proof path |
| `src/proof_experiment/combined.rs`: `bucket`, `preimage`, `Statement::{endpoints,audit}`, `audit_input`, `range`, `allocate` | Supported public domain, bucket inverse, both aggregate gaps, output ranges and monetary conservation |
| Same file: `Statement::{transcript,append_statement,group_balance}`, `Controls::COMPLETE`, `prove_control_with_transcript`, `verify_control_with_transcript` | Ordered commitments, all constraints and separate group balance |
| `src/proof_experiment/combined/ownership.rs`: `Public::{check_shape,statement_digest,append_statement,arithmetic_transcript}`, `proof_digest`, `message`, `pseudo`, `sign_proof`, `verify_measured`, `verify_ownership` | Exact shared pseudo-output, statement/proof/index association, both verifiers |
| Repository `cluster-tax/src/demurrage.rs`: `demurrage_charge`, `capitalized_reset_charge`, `spend_demurrage_charge` | Production integer decomposition, separate caps, early return, fixed horizon |
| Repository `vendor/bulletproofs-og/src/r1cs/{prover,verifier,constraint_system,proof,linear_combination,metrics}.rs`, plus `transcript.rs`, `generators.rs`, `inner_product_proof.rs`, `util.rs` | Allocation semantics, commitment append, proof parsing and verification dependencies; no security reduction claimed |
| Repository `crypto/ring-signature/src/ring_signature/{clsag,mod,key_image,curve_scalar}.rs`, `domain_separators.rs`, `compat.rs`, `amount/{commitment,compressed_commitment}.rs`; `crypto/keys/src/ristretto.rs` | Actual CLSAG equations/domains, generators, hashes, scalar/point boundaries |
| `PublicStatement::gens`, `project_commit`, ownership `project_scalar`; repository `transaction/core/src/range_proofs/mod.rs::convert_gens` | Canonical Dalek 4/5 byte conversion and project amount generators |

Only COMPLETE paths are covered; omission controls intentionally implement
different relations. The single-input layout accepts typed u64 public factors,
elapsed, year and horizon, a u32 rate, and a private u64 amount. It binds supplied
metadata but does not validate a chain policy. Combined statements require 1–16
inputs and outputs, significant bits 2–4, outer version 1, token zero for every
input, height = expiry, and a u64 fee whose checked base subtraction has a
nonempty bucket preimage. Ownership also requires one ring per input and exactly
20 members per ring. Nested input version/network/context are bound, not checked
for consistency with outer fields. Equal height/expiry is not a freshness check.

## Ranges and strict field lifting

Let M = 2^64−1, R = 2^32−1, K = 50,000,000 and
ℓ = 2^252 + 27742317777372353535851937790883648493. For a raw factor f,
p(f) = clamp(f,1000,6000)−1000 lies in [0,5000]. At year y > 0 put
A = rate·p(f)·t, D = K·y and N = V·A. At y = 0 the public construction
instead sets A = 0 and D = 1; there is no division by zero in either builder.

`allocate_multiplier` imposes a·b = o; the bit gadget adds o = 0 and a+b = 1.
Over the prime field this forces a ∈ {0,1}. The weighted sum of b such bits is
therefore in [0,2^b−1]. All wire widths are below 252. Conversely ordinary binary
digits with assignments (a,1−a,0) satisfy these gadgets. `multiply` adds two
linear equalities tying its factors to the supplied expressions, so products do
not introduce independent free factors. `Expr::lc` translates the same indexed
nonnegative coefficients as `Expr::eval`.

Let E = 2^147−1, T = 2^90−1 and W = 2^68−1. The following inclusive maxima
are calculated from independent ranges, **before** imposing the equations or
cap. They cover invalid-but-ranged assignments, not just canonical witnesses.
The manifest contains their decimal values and scalar-order comparisons.

| Expression | Inclusive maximum / safe bound | Strict power bound |
| --- | --- | --- |
| Public A; D | R·5000·M; K·M (also covers y = 0) | < 2^109; < 2^90 |
| N = V·A | M²·R·5000 | < 2^173 |
| D·(c+excess)+r | K·M·(M+E)+T | < 2^237 |
| r+rbar; D−1 | 2T; K·M−1 | < 2^91; < 2^90 |
| c·excess and M·excess | M·E | < 2^211 |
| ci+j, co+d, accrued+x, d+y; z | 2M; M | < 2^65; < 2^64 |
| d·j, x·y | M² | < 2^128 |
| Sum of n amounts or charges, n ≤ 16 | 16M | < 2^68 |
| Sum of m outputs **plus fee**, m ≤ 16 | 17M | < 2^69 |
| L+lower-gap, sum(charges)+upper-gap | 16M+W | < 2^69 |
| Inverse endpoints L,H | 16M | < 2^68 |

Every strict power bound is at most 2^237 < ℓ. Zero product outputs and all
constant sides are covered too. Thus equality of the two nonnegative sides in
the scalar field lifts to integer equality. Subtracted linear combinations are
read as these two sides, not as unsigned subtraction. The external commitment
variables are initially field scalars: COMPLETE's amount/charge equalities set
them equal to these ranged representatives. The same holds for output bindings;
there is no independently ranged, disconnected copy. Extracting these committed
openings from an accepted proof remains a separate cryptographic premise.

## Exact division and cap, in both directions

Each of `accrued`, `capital.in`, `capital.out` has c64, excess147, r90, rbar90
and equations

```text
N = D·(c+excess)+r
r+rbar = D−1
c·excess = M·excess
```

After lifting, nonnegativity of rbar gives 0 ≤ r < D. Euclidean uniqueness
gives q = c+excess = floor(N/D). Since 0 ≤ c ≤ M, the last equality says
(M−c)·excess = 0. If excess = 0, q = c ≤ M; if excess > 0, c = M and
q > M. Hence c = min(M,floor(N/D)), including the equality boundary q = M.
At year zero, r+rbar = 0 forces both zero, and 0 = c+excess+r forces c
and excess zero. The positive D is public; no secret selector weakens that case.

Conversely divide N by D, choose c = min(q,M), excess = q−c, r = N mod D
and rbar = D−1−r. For positive years,
q ≤ floor(M²·R·5000/K) = 146150163699062055112429004426127322518879050
< 2^147. Therefore excess fits 147 bits. Both remainders are below D < 2^90;
c fits 64 bits. At year zero choose all four zero. These witnesses satisfy all
three equations; zero rate, elapsed or clamped progress simply makes N zero.
This canonical bound does not replace the larger 237-bit arbitrary-RHS bound.

The production implementation computes the same rational without a wide integer
type. Set u = V·rate·p, write u = D·whole+rem, and t = e_hi·B+e_lo with
B = 2^32. Its `part` is exactly

```text
floor(rem·e_hi/D)·B + floor(((rem·e_hi mod D)·B + rem·e_lo)/D)
= floor(rem·t/D).
```

Here u < 2^109, D < 2^90, rem·e_hi < 2^122, and the residual sum is
< 2^123; `part` ≤ t < 2^64. Thus those u128 operations do not overflow.
The only possibly overflowing steps are `whole*elapsed` and its addition to
`part`. Either overflow makes the true nonnegative result exceed u128::MAX and
therefore M; saturating there and finally capping at M preserves the result.
Otherwise the operations compute floor(u·t/D) exactly. The production guards
agree with the zero cases above. This is a source-derived argument, not a
machine-checked proof of Rust evaluation or compiler/backend correctness.

## Independent capital caps, positive difference and maximum

The three divisions use input-factor/elapsed, input-factor/horizon, and
output-factor/horizon respectively. They finish their caps separately.
`ci+j = co+d` and `d*j = 0` with d,j ≥ 0 imply
d = max(ci−co,0), j = max(co−ci,0). Conversely those choices satisfy both
relations and fit 64 bits, since ci,co ∈ [0,M]. In particular ci = co = M
gives zero, and ci = M, co = M−1 gives one. Subtracting uncapped quantities
would be a different relation.

The raw-factor production early return `output_factor >= input_floor_factor`
agrees: clamping, multiplication by nonnegative quantities, division by fixed
positive D, and capping are monotone in the raw factor. Thus co ≥ ci in that
branch; year zero is also monotone because both are zero. When the branch is
not taken, `saturating_sub` is exactly the positive difference, including distinct
raw factors clamped to the same endpoint.

`z = accrued+x`, `z = d+y`, `x*y = 0` with nonnegative gaps implies z is at
least both arguments and equals at least one: z = max(accrued,d). Conversely
choose that maximum, x = z−accrued, y = z−d; all fit 64 bits. This is the
generic per-input relation for the supplied horizon. It equals production
`spend_demurrage_charge` at `SETTLEMENT_HORIZON_BLOCKS = 31,536,000`; the
research verifier does not authenticate that choice or the supplied factors/age.

## Bucket inverse and both conservation checks

For aggregate charge S in [0,U], U = 16M, `bucket` sets B_s(0) = 0 and for
S > 0 uses e = floor(log2 S), step = 2^max(e+1−s,0),
B_s(S) = step·ceil(S/step). It is nondecreasing within each power-of-two
interval. At the next boundary 2^k the preceding value rounds to at most 2^k,
and 2^k maps exactly to itself even when step doubles; there is no downward
jump. Zero maps to zero and every positive S maps to a positive value. For this
domain the largest rounded value is at most 2^68, so the u128 calculation is safe.

`preimage` binary-searches the first S with B_s(S) > q−1 and > q, using U+1
as the exclusive sentinel (never evaluating `bucket` at that sentinel). It
handles q = 0 with lower endpoint zero and rejects empty/noncanonical fibers.
Consequently its [L,H] is exactly {S ∈ [0,U] : B_s(S)=q}. `endpoints` takes
q = fee−base_unit·max(n,m) in u128 with checked subtraction. The base product
is at most 16M and the fee is u64, so neither public arithmetic nor q underflows
or overflows silently; unsupported/unaffordable combinations may be rejected.

COMPLETE allocation enforces S = L+lower-gap and H = S+upper-gap using the
same committed per-input charges, and two 68-bit gaps. By the independent bounds
above these give L ≤ S ≤ H, hence exactly
fee = base_unit·max(n,m)+B_s(S). Conversely, if that equation is representable
and the public shape supported, choose lower-gap = S−L and upper-gap = H−S.
Both are nonnegative and ≤ U < 2^68, so both equations and ranges hold.

Each output has a 64-bit decomposition equated to its committed scalar.
`sum(amounts) = sum(outputs)+fee` then lifts to integer conservation. Its RHS
must be bounded by **17M**, independently of the desired equality; assuming
conservation first would miss invalid ranged assignments. Conversely conserved
u64 amounts/outputs satisfy this equation and their canonical bit decompositions.
There is no additional subtraction of charges: the fee already includes the
rounded aggregate charge.

With token-zero amount generator H and blinding generator G, the separate
`group_balance` check is ΣP_i−ΣCout_j−fee·H = identity. Given the same openings
P_i = V_i·H+b_i·G and Cout_j = O_j·H+b'_j·G, integer conservation cancels
the H term, leaving Σb_i = Σb'_j modulo ℓ. Conversely conserved values and
balanced blinds satisfy that group check. Blinds are field scalars, not monetary
integers; invalid compressed points fail decoding.

Together these constructions establish existence of a complete arithmetic
witness for every supported, representable and affordable integer instance with
the stated commitment openings and balanced blinds. They do not promise an
accepting envelope for arbitrary public commitments or fees. Conversely every
complete satisfying arithmetic witness and group check has exactly these integer
semantics. Moving from accepted proof bytes to such a witness is conditional.

## Independent verifier, counts and statement association

`CircuitLayout` contains names, widths, expressions and indices, with no amount
argument or witness values. `new` builds all three exact divisions and the
difference/max solely from `PublicStatement`; verification passes `None` to
allocation. `Candidate` computes values independently. `assert_reference_parity`
compares every wire name/width, equation, product and amount/charge index;
`fixture` calls it, and `public_only_layout_domain_parity` exercises zero and
extreme domains. Shared expression types are not a formal independence theorem.

Counting the source gives 64 + 3·(64+147+90+90) + 5·64 = 1,557 range bits,
nine linear equations and five products per input. Each bit costs one multiplier
and two linear constraints; each product costs one multiplier and three linear
constraints (two factor bindings plus its output equality). With two external
equalities this is 1,562 multipliers and 2·1557+9+15+2 = 3,140 constraints.
Each output adds 64 multipliers and 129 constraints. Two gap68 decompositions
add 136 multipliers and 272 constraints; the two gap equations and conservation
add three more. Thus totals are `1562*n + 64*m + 136` and
`3140*n + 129*m + 275` (26,152 and 52,579 at 16/16), with `2*n+m`
external commitments and no randomized second-phase constraints. Group balance
and CLSAG add no R1CS gates. This is a source count, not fresh resource measurement.

The fixed identifier `botho/inactive/demurrage-exact-9-v1` is appended as
`arithmetic-relation` in the single-input transcript, combined transcript and
ownership statement digest. It is not selected by a version argument. Single
statements append all fields including amount/charge commitment bytes. Combined
statements append outer metadata, counts, ordered input metadata; amount/charge
commitments in input order and then output commitments in output order are bound
by `Prover::commit` / `Verifier::commit` appending `V`. The proof also binds their
count `m`. Explicit circuit equalities remain necessary despite these appends.

Ownership exhaustively destructures the statement and each input, binding every
field, all commitments, and ordered full rings with counts and indices. It uses
the domains `botho/inactive/ownership-{statement,arithmetic,proof,message}-v1`:
statement digest S → arithmetic transcript containing S → exact proof bytes A →
proof digest hA → CLSAG message digest (S,hA,input-index). `pseudo` takes the
arithmetic amount commitment directly; no detached public pseudo-output exists.
`verify_measured` checks shape/signature count, complete arithmetic with group
balance, and every indexed signature. This acyclic association is conditional on
the transcript/hash assumptions. It does not establish joint knowledge extraction.

## Ownership and cryptographic obligations that remain open

The actual CLSAG verifier uses Z_j = P_i−C_ij, aggregation coefficients μP,μC,
W_j = μP·T_ij+μC·Z_j and loop points
L_j = s_j·G+c_j·W_j,
R_j = s_j·Hp(T_ij)+c_j·(μP·I_i+μC·D_i).
Its aggregation hashes bind ordered target keys/commitments and both images;
round hashes bind the supplied message, images and loop points, under the exact
domain strings in `domain_separators.rs`. The enclosing ownership statement also
binds each member's `public_key`, which ordinary CLSAG does not use as T_ij.
Honest `sign` checks the real commitment difference against the supplied blind
difference. Verification does not rerun that secret check.

The needed ownership relation is knowledge of one common j,x,z such that
T_ij = xG, P_i−C_ij = zG, I_i = xHp(T_ij), D_i = zHp(T_ij).
Knowledge merely of μP·x+μC·z is insufficient to assert separate knowledge of
x,z for the same member. A reduction must address actual hash inputs, adaptive
adversarial keys, accepted point/identity and zero-secret domains; unforgeability
alone does not supply this code-specific same-member knowledge statement.

Conditionally, if an extracted arithmetic opening is P_i = V_iH+b_iG and an
admitted ring commitment has a consistent ranged opening C_ij = v_ijH+r_ijG,
then the same-member relation gives (V_i−v_ij)H = (z−b_i+r_ij)G. Under
computational commitment binding with unknown generator relation this forces
matching values except with negligible probability. Since both values are in
[0,M] below ℓ, scalar agreement gives integer agreement. Availability of those
admitted ranged openings is an additional premise, not supplied by synthetic rings.

`gens` uses actual project token generators, not the Bulletproofs defaults.
`project_commit` and `project_scalar` use canonical scalar bytes across Dalek
versions; points cross by compression/decompression. Consistent backend group
semantics and generator independence remain assumptions. The envelope is an
in-memory research structure, not a production wire-format/canonical-codec proof.

| Premise / obligation | Status after this review |
| --- | --- |
| Ranged complete equations ⇔ exact integer semantics, supported-instance witnesses | Derived above from pinned source; not machine-checked |
| Exact vendored noninteractive R1CS knowledge/extractability, transcript customization, generators and randomized verification | Conditional; no paper-to-fork reduction or Fiat–Shamir proof supplied |
| Same-member dual-relation CLSAG knowledge for the actual implementation/domains | Unresolved |
| Binding, consistent Dalek interpretations and admitted ranged ring openings | Conditional; ledger admission outside harness |
| Joint adaptive composition/extraction for S → A → hA → indexed signatures | Unresolved; rewinding/programming may change A or the signed message; separate extractors need not compose |
| Authenticated/unspent state: membership, distinct/unspent key images, factors, age, lineage, policy and settlement | Not checked by this experiment |

## Finite evidence, model context and reference provenance

The retained #1952 Builder logs and [#1955 Judge verdict](https://github.com/botho-project/botho/pull/1955#issuecomment-6691344691)
were inspected as execution evidence: 21 normal research tests passed; the
same-factor short-hold proof gives 58,494,241,735 versus legacy zero; six
invalid-but-ranged witness proofs were produced and rejected (cap, difference,
max, quotient, remainder, remainder bound); relation-identifier controls rejected
single, combined and ownership paths, including the indexed CLSAG message.
Those tests do not exhaust all assignments or discharge cryptographic premises.
No new Rust/Lean build or resource run was performed for this documentation review.

`Botho.Demurrage.exactCharge` models the single rational floor on Nat with a u64
cap (Nat division by zero gives zero). Its four recorded claims are
`exactCharge_positive_iff`, `exactCharge_split_loss_bound`,
`exactCharge_split_rounding`, and `exactCharge_split_clipping_counterexample`.
They are model context; they prove neither the Rust decomposition, this R1CS
allocator, nor the cryptographic composition. All declarations/records and the
historical `Counterexamples.lean` claims remain unchanged.

Both canonical IACR PDF endpoints returned HTTP 403 on 2026-11-06. An
[author-hosted Bulletproofs copy](https://web.stanford.edu/~buenz/pubs/bulletproofs.pdf)
was retrieved and inspected that day: 617,865 bytes, SHA256
`f7b6c9297bdd5b5705b3a4866f1f0dcd9d236517642265854667e6794619e4bc`.
This is a distinct snapshot, not a relabeling of the historical IACR bytes.
In that copy, §5.1 Theorem 4 states computational witness-extended emulation for
Protocol 3; §5.2 gives the logarithmic arithmetic protocol and Theorem 5 states
computational soundness under the discrete-log assumption. Section 4.4 discusses
the random-oracle Fiat–Shamir conversion. Relation (73) requires full column rank
of the committed-value coefficient matrix. COMPLETE's distinct external binding
rows supply a signed identity submatrix, satisfying that structural requirement.
This observation does not establish the remaining exact-fork, generator,
noninteractive knowledge or joint-composition hypotheses.

[IACR 2019/654](https://eprint.iacr.org/2019/654.pdf) remained unavailable for
primary-text inspection; no theorem statement, number or applicability from that
paper is newly verified here. The manifest separately records failed canonical
retrievals, the inspected author snapshot, and the old manifest's 2026-09-20
hash/date records. Historical records are **not new retrievals**. Neither paper
is used to establish the source-derived arithmetic argument.

[The historical review](RELATION-REVIEW.md), its source manifest, old resources
and captures retain their original scope. Fresh resource measurement/publication
is still [#1954](https://github.com/botho-project/botho/issues/1954). Neither finite
tests nor future measurements close the unresolved obligations in the table.
