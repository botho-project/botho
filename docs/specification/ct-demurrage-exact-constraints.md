# Inactive candidate: exact confidential demurrage constraints

Status: research/reference construction for #1281, following #902 and #1264.
This document neither accepts ADR-0009 nor activates confidential amounts. The
[test-only Rust artifact](../../scripts/research/ct-demurrage/src/lib.rs) checks
integer witnesses against the **actual**
[demurrage kernel](../../cluster-tax/src/demurrage.rs), and allocates constraints
through the existing Bulletproofs R1CS API. This integer reference portion does
**not** generate or verify a proof, implement a CT transaction format, or
establish protocol security. The subsequent [inactive proof experiment](ct-demurrage-proof-experiment.md)
adds real proof round trips and binding controls in a separate test-only module;
the derivation and census below describe the original reference portion.

Recommendation: carry the quotient/remainder construction below into a separately
reviewed experimental R1CS gadget. Its nonlinear cap/subtract/max constraints
cannot be replaced by the current specialized 64-bit range-proof API alone.
Keep the protocol inactive until that gadget, its commitment/transcript binding,
and the economic public-input rules have adversarial review. Do not replace the
integer kernel with a single rational multiplier or an amount quantization rule.

## Exact function and public premises

The source baseline is `65687275`. The input amount `V` is a u64; factors and
elapsed/year/horizon blocks are u64; the annual rate `r` is the **full u32** API
domain. A caller's typical 200-bps rate is not a soundness bound.

For one charge, define public constants:

- `M = 2^64 - 1`, `U = 2^128 - 1`, `S = 1,000,000`, `K = 50,000,000`;
- `p = clamp(F, 1000, 6000) - 1000`, `k = r*p`;
- `T = floor(elapsed*S/year)`, with `T = 0` when `year = 0`.

The kernel's annual amount is exactly `A = floor(V*k/K)`. This uses the integer
identity `floor(floor(x/a)/b) = floor(x/(a*b))` for positive integers `a,b`,
with `a = 10000`, `b = 5000`. It only combines the **two adjacent annual floors**;
it does not combine the annual floor with the later time multiplication/floor.
All multiplication before the annual division fits u128: `V*r*p < 2^109`.
The public elapsed numerator is below `2^84`, so it also fits u128.

The source then returns `min(M, floor(min(U, A*T)/S))`. Since `U >= M*S`, this
is exactly `min(M, floor(A*T/S))`, even when `A*T` exceeds u128. Proof: below
`U` the two expressions agree directly; above `U` both division results are at
least `M`, so both final caps return `M`. This removes an intermediate
saturation branch **within each charge**. It never removes the individual u64
charge caps before a subtraction. Zero rate, zero elapsed, zero year, and a
clamped factor of 1000 all give zero with these definitions.

The candidate treats factors, rate, elapsed, year, and horizon as public,
authenticated statement inputs. Proving hidden factor derivation, clamping a
private factor, or hiding age is outside this construction. D1 amount/fee
quantization, D2 factor rules, and D3 tag-derived upper bounds remain separate
protocol decisions; none is resolved by these equations.

## Witnesses and equations

Every named integer below is nonnegative and bit-ranged. Remainder complements
make the upper bounds exact rather than just powers of two. Public coefficients
multiply field variables linearly.

For each distinct input/output annual factor:

```
V*k = K*A + Ra
Ra + Ra_bar = K - 1
```

`A` has 83 bits; `Ra` and `Ra_bar` each have 26 bits. Thus `0 <= Ra < K`,
forcing the annual quotient exactly. Both annual calculations use the **same
original V variable**. The input annual quotient is shared by the accrued and
capitalized-input charge; the output annual quotient is separate.

For each of three temporal charges (accrued, capitalized input, capitalized
output):

```
A*T = S*(C + E) + Rt
Rt + Rt_bar = S - 1
C*E = M*E                     # equivalently (M-C)*E = 0
```

`C` has 64 bits, `E` has 147 bits, and `Rt`, `Rt_bar` each have 20 bits. The first
two equations force `C+E = floor(A*T/S)`. If `E=0`, `C` is that quotient. If
`E>0`, the last equation forces `C=M`. Therefore `C` is exactly the capped
charge and `E` is the excess above the cap. The branch stays private; no public
saturation selector is needed. This argument requires all the stated ranges.

For the capitalized difference, with **already independently capped** `Ci, Co`:

```
Ci + J = Co + D
D*J = 0
```

Both `D,J` have 64 bits. These imply `D=max(Ci-Co,0)`. The source first returns
zero when raw `Fout >= Fin`. The clamp, annual floor, time multiplication,
time floor, and cap are monotone in the factor, so that branch has `Co>=Ci`
and these equations also force zero. A raw-factor branch is therefore
unnecessary, including factors outside the clamp interval.

For the final spend charge:

```
Z = Caccrued + X
Z = D + Y
X*Y = 0
```

`Z,X,Y` have 64 bits, giving `Z=max(Caccrued,D)`. Spend uses the kernel's fixed
`SETTLEMENT_HORIZON_BLOCKS = 31,536,000`; the reference constructor also accepts
a public arbitrary u64 horizon to compare the generic capitalized API.

These equations are complete for the given public inputs: construct the annual
quotients and remainders, exact temporal quotients and remainders, caps/excess,
positive and negative difference parts, and max gaps. They are sound as integer
equations by quotient uniqueness and the zero-product arguments. The following
bounds are essential to transferring that statement to field equations.

## Bounds and absence of scalar-field wrap

The pinned Bulletproofs fork uses the curve25519-dalek scalar field, of order
`l = 2^252 + 27742317777372353535851937790883648493`. The reference uses canonical
scalar conversion, rejecting reduction aliases, and checks integer equalities
as well as their scalar counterparts. Its bound audit substitutes **all allowed
bit-range maxima**, not just the honest witness values, into both sides of every
equality and into each product/output. This makes a malicious but in-range
witness subject to the same no-wrap argument.

| Quantity / equation side | Bound used |
| --- | --- |
| `Amax = floor(M*(2^32-1)/10000)` | 83 bits |
| `Tmax = M*S` | 84 bits |
| `Amax*Tmax` | 167 bits |
| `floor(Amax*Tmax/S)` and excess | 147 bits |
| `V*k` | below `2^109` |
| `K*A + Ra` with all allowed ranges | below `2^110` |
| `A*T` with all allowed ranges | below `2^167` |
| `S*(C+E)+Rt` with all allowed ranges | below `2^168` |
| `C*E` and `M*E` | below `2^211` |
| remainder/complement sum | below `2^27` (annual), `2^21` (time) |
| subtraction/max linear sides | below `2^65` |
| `D*J`, `X*Y` | below `2^128` |

Each entire nonnegative side is strictly below `l`. Thus field equality implies
integer equality; range-checked zero products cannot be nonzero multiples of
`l`. In particular, checking only that honest `A*T` fits is insufficient: the
147-bit excess and all alternative witnesses must also be bounded.

The 83- and 147-bit witnesses cannot use the specialized `RangeProof` u64
interface as single values. The candidate R1CS allocator instead decomposes
every wire directly into boolean multiplier variables. A possible limb layout
is `A = A0 + 2^64*A1` with widths 64/19 and
`E = E0 + 2^64*E1 + 2^128*E2` with widths 64/64/19; each limb must be ranged and
the recomposition equality enforced. Merely proving ranges of unrelated limb
commitments is unsound. This artifact implements bit decomposition, not the
alternative limb commitment layout.

## Commitment binding and allocation census

The R1CS census uses actual `Prover`, `Verifier`, `allocate_multiplier`,
`multiply`, `constrain`, and `metrics` calls from the existing pinned
`bulletproofs-og` dependency with its experimental `yoloproofs` feature. Each bit
has a multiplier with inputs `b,1-b`, constrained output zero and input sum one.
The weighted sum of those bit variables is used directly in all equations;
there are no additional unrestricted aggregate witness variables.

Two external commitments are allocated, to `V` and `Z`, and constrained to the
same bit sums used by the circuit. Future integration must supply the **actual
original input amount commitment**, with its corresponding opening, and bind
`Z` to the charge/balance relation. It must not create an unrelated fresh `V`
commitment or replace `V` with a post-demurrage amount. Multiple inputs, output
amount conservation, transaction context, public factors/ages/rate/year, replay
protection and domain separation must be bound by a reviewed transaction proof
statement/transcript. The census uses fixed dummy blinds and a fixed transcript
label only to allocate and count; these are not a secure proof protocol.

The measured allocation is the same for prover and verifier:

| Allocation | Count |
| --- | ---: |
| amount range bits | 64 |
| two annual witnesses: `2*(83+26+26)` | 270 |
| three temporal witnesses: `3*(64+147+20+20)` | 753 |
| difference pair | 128 |
| max triple | 192 |
| total boolean multipliers | 1,407 |
| cap/difference/max multipliers | 5 |
| **actual total multipliers** | **1,412** |
| **actual linear constraints** | **2,844** |
| external commitments | 2 |

Linear count: two constraints per bit, two external bindings, thirteen integer
linear equalities, and three linear constraints per nonlinear `multiply` call.
There are no randomized second-phase constraints. These are allocation counts,
not proof-size, proof-generation, or verification-cost estimates. No proof is
produced and no cryptographic verification or zero-knowledge property is tested.
The specialized linear/range approach still needs a reviewed way to establish
the hidden cap and max alternatives: independent ranges and linear equations
alone do not force the necessary zero products. Existing R1CS is the concrete
candidate for these five nonlinear relations, not an approved protocol choice.

## Reproducible evidence and remaining review

```
CARGO_TARGET_DIR=/tmp/botho-1281-target RUSTC_WRAPPER= \
  cargo test --locked -p bth-ct-demurrage-reference -- --nocapture
```

The five tests pass against the actual Rust functions:

- 3,456 exhaustive combinations over the documented small loops, including
  zero parameters and both sides of the lower factor clamp;
- 2,048 deterministic wide-domain combinations over u64 values/times,
  full-u32 rates, independently varied factors, and zero/small/large year values;
- 1,320 boundary combinations plus explicit nested-floor counterexamples,
  generic horizon boundaries, and u128 multiplication overflow;
- mutation of every witness, incorrect original amount binding, scalar-order
  aliases, deliberately unsafe widened bounds, and coordinated changes that
  preserve the linear equations but violate cap, difference, or max products;
- exact witness bounds and actual prover/verifier allocation counts.

One key vector is `V=M, Fin=6000, Fout=5999, elapsed=0, rate=u32::MAX, year=1`:
both horizon charges cap to `M`, their difference is zero, and spend is zero.
Another uses `V=100, Fin=6000, Fout=3500, horizon=M-1, rate=200, year=1`:
charges are `M` and `M-1`, and their difference is one. These forbid replacing
the difference of independently capped charges by a cap of the difference.

Initial local cold compilation took 4.80 seconds and tests 0.62 seconds on the
available host; these timings describe the reference checker only. Workspace
Build explicitly executes this package's tests. The package is unpublished,
all dependencies are dev-only, and its library is entirely `cfg(test)`.
Big-integer arithmetic and witness generation are not constant-time and must
not be moved into wallet proof generation without separate engineering review.

Next review must independently validate the integer derivation and all domain
bounds; implement and adversarially test an actual shared prover/verifier gadget
with proof round trips and invalid-proof rejection; bind the real transaction
commitments and public context; assess witness privacy and side channels; and
measure proof costs under realistic aggregation. D1/D2/D3, proof system security,
wallet representation, activation, and external cryptographic audit remain
unresolved. The reference checker supplies a concrete testable construction;
it is not a substitute for any of those decisions.
