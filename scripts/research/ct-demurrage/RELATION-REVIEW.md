# Conditional relation review for the inactive CT experiment

This is the internal review artifact for #1355, a child of the still-open #1307. It derives the integer relation implemented by the complete allocator and states the additional cryptographic obligations needed to interpret an accepted ownership envelope. It is not a security reduction, an audit, an accepted protocol specification, or production integration. All relevant harness code remains in an unpublished, test-only crate.

## Source boundary and equation map

The companion `relation-review-sources.json` pins the reviewed sources to #1350 head `4dd5452fa6eedf2f80b0bea369657bc182bd504c`. The manifest also records each file’s hash at documentation base `baa193b1a705dfbf1ecf28690375639cbc18df3c`: ten are identical; the ownership module predates measurement-wrapper/fixture factoring, and the resource module is absent there. No resource implementation is copied into this documentation change. Names such as `verify_measured` below refer to the pinned #1350 revision. Paths below are relative to this crate unless otherwise stated. Existing tests and resource observations are evidence of executions, not premises proving the universal claims below.

| Source and symbol | Relation reviewed |
| --- | --- |
| `src/proof_experiment.rs`, `CircuitLayout::{annual,temporal,new,allocate}` | Annual floors, temporal floor and cap, positive subtraction, maximum, bit ranges, committed input/charge equality |
| `src/proof_experiment/combined.rs`, `audit_input`, `Statement::audit` | Both sides of each field equation bounded independently of desired conservation |
| Same file, `bucket`, `preimage`, `Statement::endpoints`, `allocate` | Exact rounded aggregate charge, fee, output ranges and conservation |
| Same file, `Statement::group_balance`, complete proof/verification entry points | Same amount/output commitments and blinding balance |
| `src/proof_experiment/combined/ownership.rs`, `Public::statement_digest`, `arithmetic_transcript`, `proof_digest`, `message`, `pseudo`, `verify_measured` | Ordered public binding, actual arithmetic proof association, exact shared pseudo-output commitment and all signatures |
| Repository `crypto/ring-signature/src/ring_signature/clsag.rs`, `sign`, `verify`, aggregation/challenge helpers | Actual ordinary library ownership relation requiring a code-specific argument |
| Repository `cluster-tax/src/demurrage.rs`, `demurrage_charge`, `capitalized_reset_charge`, `spend_demurrage_charge` | Integer reference semantics, including separate caps and saturating implementation |

Only COMPLETE allocation and the ordinary ownership verifier are covered. Historical omission controls are not an alternate accepted relation. Public parameters must pass the implemented shape/audit checks: one through sixteen inputs and outputs, supported version and token, equal height/expiry, supported significant bits, and a representable fee with a nonempty bucket inverse. The ordinary composition fixes ring size twenty. These checks do not authenticate those public parameters.

## Integer semantics and field lifting

Write M = 2^64−1, K = 50,000,000, S = 1,000,000, and l = 2^252 + 27742317777372353535851937790883648493. G is the blinding generator and H the token-zero amount generator. Amounts are integers in [0,M]; rate is u32. For a public factor f, set p = clamp(f,1000,6000)−1000 and k = rate·p. For elapsed t and year y, set T = 0 when y = 0, otherwise floor(t·S/y). All these are public integer computations, not secret unconstrained field values.

Each bit gadget enforces a·b = 0 and a+b = 1 over the prime field. Consequently a is exactly zero or one. A b-bit wire is its weighted bit sum and denotes an integer in [0,2^b−1]; all declared widths are below 252. COMPLETE allocation equates these ranged wires with the committed amount/charge variables; it does not independently range an unrelated copy.

The following strict upper bounds hold for *every* assignment satisfying the declared bit ranges, before assuming any desired integer equality. They bound both sides of the field equations, including product outputs. Therefore equality modulo l lifts to integer equality: two integers in [0,l) with equal residues are equal. No conservation assumption is used to obtain these bounds.

| Equation expression | Strict upper bound |
| --- | --- |
| V·k | 2^109 |
| K·A+r with A83, r26 | 2^110 |
| Annual r+rbar | 2^27 |
| A·T with A83, T < 2^84 | 2^167 |
| S·(c+e)+r with c64, e147, r20 | 2^168 |
| Temporal r+rbar | 2^21 |
| c·e and M·e | 2^211 |
| ci+j, co+d, accrued+x, d+y, settled | 2^65 |
| d·j and x·y | 2^128 |
| Sum of at most sixteen inputs or charges | 2^68 |
| Sum of at most sixteen outputs plus a u64 fee | 2^69 |
| L+lower-gap and sum(charges)+upper-gap, each gap68 | 2^69 |

Every bound is below l. Public inverse endpoints lie in [0,16M]. Constant sides K−1 and S−1 also lie below l. `audit_input` computes bounds from the actual expressions; the aggregate audit independently includes outputs **plus** fee. Loose bounds here deliberately cover invalid-but-ranged assignments as well as honest witnesses.

### Annual floor: both directions

The equations are V·k = K·A+r and r+rbar = K−1. Nonnegativity implies 0 ≤ r < K, hence A = floor(V·k/K), uniquely. Conversely Euclidean division supplies A and r; set rbar = K−1−r. Both remainders fit 26 bits. A < 2^96/10000 < 2^83 because p ≤ 5000 and K = 5000·10000. The reference's nested positive integer divisions agree: floor(floor(N/10000)/5000) = floor(N/K), by the quotient/remainder decomposition of N. Zero rate or clamped progress yields zero in both paths.

### Temporal floor and independent cap: both directions

For each of accrued, capital-in and capital-out, the equations are A·T = S·(c+e)+r, r+rbar = S−1, and c·e = M·e. Ranges give c ≤ M and nonnegative e. The first two equations uniquely fix q = c+e = floor(A·T/S). The third says (M−c)·e = 0 over integers. If e = 0, q = c ≤ M. If e > 0, c = M and q > M. Thus c = min(q,M), including q = M, where e = 0.

Conversely choose q by division, c = min(q,M), e = q−c, and the Euclidean remainder with its complement. For canonical witness bounds use the exact constants, not the loose no-wrap table: T ≤ t·S, A < 2^96/10000, so q ≤ A·t < 2^160/10000 < 2^147. Thus e fits 147 bits. Remainders fit 20 bits, c fits 64. Year zero makes T and the charge zero.

The native kernel's intermediate u128 saturation has the same final u64 result. When A·T exceeds u128::MAX, floor((2^128−1)/S) > M, so saturating before division and then capping to M equals mathematical division followed by the u64 cap. When it does not overflow, division is exact. The earlier V·rate·p product is below 2^109 and does not overflow u128.

Each capital term is capped separately. In particular, ci = co = M produces a zero difference; replacing these terms by an uncapped difference would change the relation.

### Positive difference and maximum: both directions

The difference equations ci+j = co+d and d·j = 0 with nonnegative d,j imply d = max(ci−co,0), j = max(co−ci,0). If d is positive then j is zero and the equality fixes d; if d is zero it fixes j. Conversely these choices satisfy the equations and fit 64 bits. The reference early return for raw output_factor ≥ input_factor agrees: clamp, annual division, temporal division and cap are monotone, so ci ≤ co. No uncapped subtraction is substituted.

The maximum equations z = accrued+x, z = d+y, x·y = 0 imply z ≥ both arguments and equality with at least one, hence z = max(accrued,d). Conversely take that maximum and the two nonnegative gaps. All three fit 64 bits. This proves the exact per-input spend charge, with the experiment's supplied horizon; it does not authenticate that horizon against a chain policy.

## Aggregate rounding and conservation

For D ≥ 1 let e = floor(log2 D), step = 2^max(e+1−s,0), and B_s(D) = step·ceil(D/step); define B_s(0) = 0. Within each power-of-two interval the step is constant and this function is nondecreasing. At a boundary 2^k, the preceding value 2^k−1 rounds to 2^k when the step exceeds one; with unit step it remains 2^k−1. The new interval maps 2^k exactly to itself even when the step doubles. Thus there is no downward jump. This also covers the zero/one boundary.

On the finite domain [0,U], U = 16M, `preimage` binary-searches the first integer whose bucket exceeds q−1 and q, using U+1 as the exclusive sentinel. For q = 0 the lower endpoint is zero. For a nonempty fiber these give exactly L = min{D:B_s(D)=q} and H = max{D:B_s(D)=q}; absent fibers are rejected, including rounded outputs beyond the supported fee representation. The search predicate is monotone by the preceding argument.

The public q is fee−base_unit·max(input_count,output_count), with checked subtraction in u128. COMPLETE allocation uses the *same committed per-input charges* to form D and enforces D = L+lower-gap and H = D+upper-gap. Field lifting and nonnegative gaps give L ≤ D ≤ H, exactly B_s(D) = q, not merely a rounded upper bound. Conversely any D in that fiber supplies both gaps; they are at most U and fit 68 bits.

Output bits are equated to the committed output variables. The additional equation sum(V_i) = sum(O_j)+fee lifts to exact integer conservation using the independent 69-bit right-side bound. Completeness is conditional on this equality being affordable and representable, not every possible public fee/input combination.

Finally the separate group check is sum(P_i)−sum(Cout_j)−fee·H = identity. For the same openings P_i = V_i·H+b_i·G and Cout_j = O_j·H+b'_j·G, integer conservation cancels the H term, leaving sum(b_i) = sum(b'_j) modulo l. This is an additional blinding-balance requirement; it is not a replacement for the ranged integer equation. Conversely balanced blinds and conserved values satisfy the group check. Blinds are field scalars, not bounded monetary integers.

Together the constructive choices above prove existence of a ranged arithmetic witness for every supported, representable, affordable integer instance whose supplied commitments have those openings and balanced blinds. Conversely every complete satisfying witness has the specified integer semantics. These statements concern the constraint relation; obtaining a witness from an accepted noninteractive proof requires the assumptions below.

## Conditional ownership linkage

For input i and ring member j use distinct symbols T_ij for its target public key and C_ij for its amount commitment. The required same-member relation is knowledge of j,x,z such that T_ij = x·G, P_i−C_ij = z·G, I_i = x·Hp(T_ij), and D_i = z·Hp(T_ij). The arithmetic pseudo-output P_i is taken directly from `amount_commitment`; there is no second public P field to reconcile.

The library verifier computes Z_j = P_i−C_ij, aggregation scalars muP,muC, W_j = muP·T_ij+muC·Z_j, and challenge-loop points L_j = s_j·G+c_j·W_j and R_j = s_j·Hp(T_ij)+c_j·(muP·I_i+muC·D_i). An ordinary honest `sign` invocation checks P_i−C_real against the supplied blinding difference. This checks honest construction; verification does not simply rerun that secret check.

Knowledge of only an aggregate scalar for W_j is insufficient to state knowledge of both x and z. A code-specific argument must justify extracting the two relations for the **same** j under adaptive/adversarial public keys, actual aggregation domains and challenge inputs. Ordinary signature unforgeability alone is not that argument. This artifact does not supply the missing reduction.

Conditionally assume the required same-member knowledge relation and an arithmetic witness opening P_i = V_i·H+b_i·G. Also assume C_ij is an admitted, ranged commitment to v_ij with a consistent opening r_ij. Then P_i−C_ij = z·G gives (V_i−v_ij)·H = (z−b_i+r_ij)·G. Under the applicable commitment-binding/discrete-log assumption with unknown generator relation, inconsistent values would violate binding; the values therefore agree except with negligible probability. Since both are in [0,M] below l, equality of their field representatives is integer equality. Availability/consistency of the admitted commitment's opening must be justified by the surrounding range-proof/ledger system; the synthetic ring fixture does not provide that system.

Canonical scalar conversion and consistent compressed Ristretto decoding must refer to the same order, points, and token-zero generators on both Dalek versions. Current code uses canonical scalar-byte conversion and actual project commitment generators. Tested agreement is supporting evidence, not a general proof of backend correctness. The argument also requires proper generator independence and the appropriate point/identity and zero-secret treatment; the paper's domains must not be silently widened to match the implementation.

## Acyclic composition and assumptions still open

The ordered public research statement is hashed to S using a dedicated Merlin domain. Exhaustive field destructuring binds all current arithmetic metadata, input amount/charge commitments, output commitments, and ordered full rings, with counts and indices. The arithmetic transcript includes S before producing randomized proof A. A separate domain hashes the exact proof bytes to hA. Each ordinary CLSAG signs another domain-separated digest of S, hA and its input index. Verification reconstructs these values from public inputs, verifies the complete arithmetic proof and group equation, and checks every signature. There is no signature-to-arithmetic-proof dependency or hash cycle.

Under transcript framing and collision resistance, this binds the accepted envelope's chosen public statement, exact arithmetic proof and signature positions. Signing adaptively *after* A is intentional. It does not imply that separately available knowledge extractors can be run jointly: an extractor may rewind/program an oracle, change A, or change the very message used by the second component. A reduction must preserve a common final public statement and proof-associated messages, or establish an appropriate joint extraction property with an explicit loss and oracle model.

| Premise or obligation | What it would support | Status of this artifact |
| --- | --- | --- |
| Complete ranged equations, both-side bounds and canonical witnesses above | Integer relation equivalence and supported-instance completeness | Derived from pinned allocator; independent source review requested |
| Knowledge soundness/extractability for the exact vendored noninteractive R1CS implementation, parameters and transcript | A consistent opening/witness for the accepted arithmetic proof | Assumption; paper-to-code and Fiat–Shamir applicability not proved here |
| Same-member dual-relation CLSAG knowledge under actual code/domains/key model | x and z for one common ring member and its image relations | Unresolved code-specific obligation, not inferred from unforgeability |
| Binding commitments, independent generators, canonical backend interpretation, admitted ranged ring values | Same integer amount in arithmetic and owned ring member | Conditional; authenticated ledger admission is outside the harness |
| Joint adaptive composition/extraction for S → A → hA → indexed signatures | Simultaneously usable arithmetic and ownership witnesses | Unresolved; acyclicity and association tests do not prove it |
| Distinct/unspent images, chain membership, age/factor/policy/lineage and settlement rules | A valid spend in an authenticated chain state | Not established by this verifier |

The next substantive internal step is a reviewed mapping of the exact CLSAG verifier to the required same-member relation, followed by an explicit joint noninteractive knowledge argument or a precisely scoped theorem that supplies it. If neither can be justified, that remains an open cryptographic review gate. No additional successful sample, resource measurement or omission-control test closes it. D1/D3 and pure-rule review remain issue-level prerequisites.

## Primary references and applicability limits

[Bünz et al., *Bulletproofs*, IACR 2017/1066](https://eprint.iacr.org/2017/1066), §5.1 Theorem 4, gives computational witness-extended emulation for its arithmetic protocol; §5.2 describes the logarithmic form. §4.4 discusses the random-oracle Fiat–Shamir conversion and binding the statement. These results motivate the needed R1CS premise; this document has not established that the precise fork, circuit API, transcript customization and multi-proof composition meet all theorem hypotheses.

[Goodell, Noether and Blue, *Concise Linkable Ring Signatures and Forgery Against Adversarial Keys*, IACR 2019/654](https://eprint.iacr.org/2019/654), Definition 10, describes aggregated linking and auxiliary keys; Theorem 2 derives unforgeability from hardness of discrete logarithms of linear combinations; Theorem 3 concerns linkability. Its games, key domains and random-oracle assumptions must be mapped to this implementation. Those results are not asserted here to be an off-the-shelf same-member dual-witness knowledge theorem or a composition theorem for this harness.

The companion manifest records the retrieved primary PDFs' hashes as reviewed snapshots. Neither paper substitutes for code review or proves production integration. Existing `OWNERSHIP.md` and #1350 `RESOURCES.md` record ordinary one/four/sixteen-input executions and resource limits; those measurements do not establish full envelope feasibility, mobile/WASM performance, anonymity, consensus safety or readiness. This artifact closes only #1355's review-document scope; #1307 remains open.
