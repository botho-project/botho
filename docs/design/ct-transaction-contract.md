# Confidential transaction contract — candidate CT1

**Status: proposed, inactive; not an activation or ADR acceptance.** Issue #1301
implements a reviewable specification checkpoint for #902/#904. The words MUST,
REJECT and EXACT below describe this candidate, not current network validation.
Current production still exposes input/output amounts. D2 EpochOrigin is ratified;
its numerical calibration is not evidence that the candidate's propagation rules
are equivalent to the simulated rules.

## 1. Decisions, authority and remaining sign-off

| Item | Authority / disposition |
|---|---|
| Hidden amounts, public fees, universal ML-KEM-768 outputs | ADR0006; preserve |
| D2 EpochOrigin, origin pool floor F=1500 and epoch K=17280 blocks | [explicit #902 ratification](https://github.com/botho-project/botho/issues/902#issuecomment-4986600535); preserve, do not reopen |
| Path C uniform circulating outputs, endogenous cap, reserve carry-forward | ADR0009 / #955/#980; preserve; do not change draw policy here |
| D1 buckets | Recommend **s=2 significant bits**, mandatory exact rounded aggregate charge, no arbitrary tips. Proposed parameter; maintainer sign-off required |
| D3 / circulation enforcement | Recommend **priced public deflation plus maximum ring factor**, defined below. This retains EpochOrigin but does **not** reproduce the calibration's free value-weighted circulation blending. Explicit policy sign-off required |
| CT1 format / transcript / limits / proof family | Concrete candidate below; implementation and independent internal cryptographic review required before acceptance |
| Activation | No height or genesis selected. Separate compatibility/migration checkpoint, including #1286 legacy lottery claims. No reset authorized |

Reviewers can accept/reject D1 and D3 independently of the already ratified D2.
Alternative D3 is a full hidden-value blend proof with authenticated selected-input
lineage; that preserves exact simulated mixing but adds a substantial circuit and
reveals/links more structure unless carefully designed. A wallet-only instruction
to mix correctly is not a consensus alternative.

### Why the existing circulation claim is insufficient

A 6x input tagged 100% to origin A may declare all-background outputs under a
max-weight upper bound: zero is below every inherited weight. If fee derivation
then reads only output tags, a self-spend becomes 1x without acquiring background
value. A ring mean is also vulnerable to dilution: one 6000 member plus nineteen
1000 members has mean 1250. Taking the maximum yields 6000 regardless of those
nineteen decoys. For V=10^12, rate=200 bps and five-year horizon, the exact reset
from 6000 to1000 costs 10^11 picocredits; age zero does not remove it.

This proves only that a valid ring containing the real input cannot understate
its **public factor** via extra low-factor decoys, and that the specified reset is
paid. It does not prove common ownership, real commerce, anonymity or an economic
optimum. Conversely, one 6000 decoy forces that same floor onto a genuine 1000
input. Maximum age similarly charges young inputs for old decoys. Wallets seeking
cheaper rings may select factor/age cohorts, shrinking anonymity and making spend
selection observable. If the resulting fee exceeds input funds, that ring cannot
be spent; no fee exemption is allowed. Candidate activation requires adversarial
and honest-wallet simulations of decoy availability, overcharge, censorship and
Path C net returns. Existing EpochOrigin Gini results must not be quoted as results
of this different rule. Path C tickets remain uniform; altered public fee inflow
can change payout volume, and the cap's economic splitting assumptions require
rechecking for batching/output-count fees.

### Bounded decoy-cost evidence (not economic acceptance)

The executable model assumes19 independent decoys, each either1000 or6000, a real
background input of1BTH, age0 and background output. Conditional on at least one
high-factor decoy, the candidate charges0.1BTH reset before D1 rounding/base.
The exact probability is `1-(1-p)^19`:

| High-factor decoy share p | Chance of forced high floor | Expected reset (BTH) |
|---:|---:|---:|
| 0% | 0% | 0 |
| 1% | 17.3831% | 0.0173831 |
| 10% | 86.4915% | 0.0864915 |
| 50% | 99.9998% | 0.0999998 |
| 100% | 100% | 0.1 |

These reproducible toy-model results expose a serious honest-user cost; they are
not sampled ledger data or a final recommendation to activate maximum floors.
Factor-cohort selection could avoid this cost but leaks selection and can deplete
available decoys. Full production-sampler and Path C simulation, including scarce
cohorts, common-owner churn, batching and high-factor-decoy poisoning, is an open
**ratification gate**. The recommended implementation candidate is conservative
against undercharging; acceptance of its costs remains unresolved.

## 2. Public state and provenance (candidate D3)

All heights are u64; references resolve against the accepted parent at height h-1.
For a spend at h, each ring has exactly20 distinct, canonically ordered outpoints;
all referenced outputs exist, are mature and have authenticated creation height,
point/commitment and provenance. Duplicate references inside a ring REJECT.
A ring may share members with another input; duplicate key images REJECT.
Creation heights greater than h-1 REJECT, rather than saturating an invalid height.
Age for input i is `a_i = max(h - creation_height(member))`; an empty ring REJECTS.
This preserves the value-free maximum order statistic already wired by #577.

An origin ID is `kind:u8 || boundary_domain:32 || epoch:u64LE` (41 bytes).
Kind0 is domestic issuance; its boundary domain is chain genesis hash. Kind1 is
bridge mint/import; domain is the approved bridge deployment ID committed by chain
configuration. Unknown kinds/domains REJECT. Epoch is floor(creation_height/17280),
assigned by consensus, never supplied freely by a transaction. A pool's public
wealth W at h is the checked-u128 sum of **gross publicly authorized issuance**
from that origin in its epoch through h-1. Domestic coinbase contributes newly
issued emission only, not redistributed fees. Bridge contribution is publicly
validated imported amount, never a private transfer or duplicated message. Reorgs
roll back pools atomically. Open epoch totals may grow, so factors can grow until
the epoch closes; this is deliberate and must be included in wallet fee estimates.
No future coinbase projection, private unspent wealth or current holder wealth is
read. An output cannot refer to an unknown/not-yet-issued origin.

`B(c,h) = max(1500, ClusterFactorCurve::default_params().factor(W(c,h)))`.
The pinned integer curve is the existing log2/Q16/LUT implementation, not floating
point or a prose logistic approximation. The executable inventory pins the exact
source and its internal unit tests remain required. Values are in picocredits;
there is no whole-BTH truncation as in the calibration's simplified interface.

Each public tag list has at most32 entries, strictly ascending encoded origin ID,
unique IDs, weights u32 in1..1000000 and total weight <=1000000. Empty means
background. Zero entries, duplicates, unsorted lists and overflowing sums REJECT;
no implicit prune/normalize/truncate rule. Factor is exactly
`f(tags,h)=1000 + floor(sum_c weight[c]*(B(c,h)-1000)/1000000)`.
The total numerator is at most5*10^9, factor is1000..6000. Thus F=1500 floors
the **origin base**, not every mixed output's factor. Coinbase/import outputs
receive their own origin with weight1000000. A public lottery payout inherits the
winner's authenticated tag list; it is fee redistribution, not new origin wealth.
Its new key/context must satisfy #1286/#1293; tags do not authenticate that context.

Ordinary output tags obey `weight_out[c] <= max(weight_member[c])` over the union
of all input rings (missing weight zero). At most32 tags PER output; a union with
more entries is permitted, but no output may exceed32. No amount weighting or
holder-supplied age is involved. Arbitrary deflation is legal but priced. For each
input set `f_i = max(f(member,h))`. Let `g=min(f(output,h))` over all outputs.
All inputs pay the reset toward g, even if some outputs keep higher factors or
are change. This deliberately conservative allocation prevents a tiny low-factor
output/hidden amount allocation from avoiding the reset; it can overcharge change.
At least one output is required. Settlement requires every output background and
uses the same reset; no extra additive settlement fee. No automatic per-hop decay,
per-block tag decay, wallet balance-based mix or hidden ring value lookup exists.

## 3. Exact integer charge and bounds

Unsigned operations below are mathematical integers unless explicitly capped.
Define U=2^64-1, M=2^128-1, S=1000000, Y=6307200, H=31536000.
Annual rate r is the public `MonetaryPolicy::demurrage_rate_bps(h)` (u32), bound
in the statement by the accepted policy configuration; callers cannot substitute
it. Heights, policy version and parent state are bound as described below.

For `D(v,f,t,r,y)` (the production `demurrage_charge` contract):

1. v,t,y are u64; r is u32. If r=0 or t=0 or y=0, return0.
2. `p=clamp(f,1000,6000)-1000`; `T=floor(t*S/y)`.
3. `A=floor(floor(v*r*p/10000)/5000)`.
4. `D=min(U, floor(min(M,A*T)/S))`.

Do not collapse steps2–4 into a real-valued k*v ceiling or omit either cap.
`A<2^91`, `T<2^84`, `A*T<2^175`; an implementation computes the saturation
branch with a bounded wide integer or proven comparisons, never a scalar-field
product masquerading as an unrestricted integer. D is u64. With fixed positive Y
production inputs are tighter; retaining these generic bounds tests the kernel.

For input i with original amount v_i (including change later returned):
`reset_i = 0 if g>=f_i else max(0,D(v_i,f_i,H,r,Y)-D(v_i,g,H,r,Y))`.
Each D saturates independently **before subtraction**.
`d_i=max(D(v_i,f_i,a_i,r,Y),reset_i)`.
`D_total=sum_i d_i` in checked u128, at most16*U (<2^68).
This per-input amount/accounting rule is a CT1 change from any existing
transaction-level transfer-value approximation; do not call it production parity.
No additional maximum-charge clamp may hide an insufficient balance: each d_i
can reach U; a transaction whose public fee cannot be paid simply REJECTS.

### Mandatory D1 quantization

Base B is public and **separate** from charge. Candidate base is
`250000000000 * max(number_inputs,number_outputs)` picocredits (checked u128).
This preserves0.25BTH per created output as a conservative Path C split cost and
also prices additional rings; it is a proposed CT1 fee rule, not an assertion of
today's size fee. Wire-size/proof limits remain hard bounds below. The existing
variable-size progressive base formula must not be accidentally applied as a
second amount-dependent charge. A different base policy requires explicit review
of Path C's per-ticket cost argument and verifier resource pricing.

For s in{2,3,4}, define Q_s(0)=0. For d>0 let e=floor(log2(d)),
`step=2^max(0,e-s+1)`; `Q_s(d)=step*ceil(d/step)`.
Compute Q in at least u128; NEVER clamp to U or wrap. Recommend s=2.
Fee MUST equal `B+Q_2(D_total)`, fit u64, and satisfy conservation. Extra tips,
multiple valid bucket choices and arbitrary overpayment REJECT. At upper endpoint
2^64 the transaction is unrepresentable and REJECTS, even though d<=U. Zero charge
has Q=0, not an invented minimum positive charge. Round **after** summing d_i;
rounding inputs separately can differ and leaks more structure.

| s | Levels per complete large octave | Worst positive relative overpayment | Recommendation |
|---|---:|---:|---|
| 2 | 2 | <50% (and zero where step1) | strongest of these coarse policies; recommend |
| 3 | 4 | <25% | lower cost, finer leakage |
| 4 | 8 | <12.5% | lowest cost, finest leakage |

These bounds concern charge, not total fee or coin value. Exact bucket preimage is
`{d: Q_s(d)=q}`; derive endpoints by monotone integer search, including powers of
two where the predecessor step differs. The proof MUST show BOTH endpoints, or
prove Q's deterministic circuit equality; an inequality `D_total<=q` is insufficient.
E.g. s2, q8 has charge preimage[7,8], q12 has[9,12]; q0 is exactly{0}.
Small buckets are singletons. Nonzero large buckets disclose a narrow charge
range (including scale); there is no constant anonymity set, fixed number of
hidden amount bits, or guaranteed nontrivial value interval. Known public
coefficients plus auxiliary inputs can still identify amounts. Multi-input public
fee bounds their charge sum, not each amount independently. No guarantee that a
spend with zero charge hides its value from all other metadata is implied.

## 4. Statement, witness and complete transaction linkage

Token0 only for CT1; unknown token IDs REJECT. Use exactly Botho's token0
Pedersen generators from `crypto/ring-signature/src/ring_signature/mod.rs::generators(0)` and
`crypto/ring-signature/src/amount/commitment.rs::Commitment::new`: amount generator H and blinding
base G, `C=vH+rG`. Do not swap these or instantiate independent generators per
proof. The amount generator is the existing Blake2b512 hash-to-Ristretto of
`HASH_TO_POINT_DOMAIN_TAG` followed by compressed Ristretto basepoint bytes (token0
XOR is a no-op); blinding G is RISTRETTO_BASEPOINT_POINT. Freeze compressed
generator bytes in cross-runtime implementation vectors before format acceptance. Ristretto encodings and scalar encodings MUST be canonical;
identity spend/ephemeral points REJECT. A zero-value commitment may be identity
if its blinding is zero; it is not a spend key.

Candidate policyhash is SHA256 of ASCII `BOTHO_CT1_POLICY\0` followed by
u32LE revision1, u16LE origin_floor1500, u64LE epoch17280, u8 significant_bits2,
u64LE base_unit250000000000, u64LE year6307200, u64LE horizon31536000,
u8 max_inputs16, u8 max_outputs16, u8 ring_size20 and u8 max_tags32.
Revision1 fixes all other codecs/curve/proof/resource rules in this draft; any
semantic change requires a new revision and hash, never reuse revision1.
The chain genesis/configuration authenticates the monetary schedule and curve;
validators derive rate from that schedule, not solely from the compact hash.

Public statement contains chain genesis/network, CT1 tag, policy ID/hash, parent
hash, spend height, expiry height, ordered input outpoints/keys/commitments/tags,
key images, pseudo-output commitments, ordered outputs including encrypted amount
records and lottery contexts, fee and all public charge parameters. Witnesses:
real ring positions, spend secrets, original amount/blinding openings, pseudo
blinds, output openings, exact charge intermediate integers and remainders,
selectors and aggregate bucket/conservation witnesses. Ordinary transaction wire
MUST NOT include original amounts, pseudo-output amounts, plaintext blindings or
plaintext exact d_i. Public bridge exceptions are separately typed below.

For EVERY input, CLSAG proves possession of one selected ring secret and links
that same selected member's amount commitment to its pseudo-output commitment
`P_i=v_i H+z_i G` through the blinded difference. A key-only ring proof plus a
separate arbitrary amount commitment is invalid. The charge circuit commits to
and opens EXACTLY P_i, with shared v_i,z_i; detached v_i is invalid. Each v_i and
output amount is constrained to[0,U] by range constraints. Every per-input charge
circuit uses that v_i in every accrued/reset branch, not independently supplied
amounts. All D intermediates/quotients/remainders/selectors are constrained,
including cap branches and max ties. Circuit or additional commitment C_di must
bind the same d_i into the aggregate circuit; the latter proves sum, bucket
membership and exact fee equality. No unlinked 'proof passed' boolean is enough.

Conservation is exact integer `sum v_i = sum v_out + fee`. Group relation
`sum P_i - sum C_out - fee*H = 0` requires the wallet to balance
`sum z_i=sum r_out` modulo group order AND explicit u64 ranges and bounded
aggregate integers. For at most16 inputs/outputs, sums <2^68; no field-wrap alias
is possible for these sums. Actual CT CLSAG commitment-difference ownership and
canonical key images prevent changing input amounts; verification consumes
unspent key images atomically with ledger mutation. Aggregate arithmetic
bound checks precede conversion to a scalar. Ristretto scalar order l>2^252;
all unconstrained products above are <2^175, but this numeric observation is
NOT a substitute for range constraints on each witness. Boolean selectors are
constrained to{0,1}; remainder is in[0,divisor), quotients bounded, cap comparison
branches exhaustive and mutually exclusive. Check all expression ranges again
for the final combined circuit, especially signed inequalities and selector
products; rejection tests include l-offset witness aliases.

Transcript version is ASCII `BOTHO_CT1_TX_PROOF\0`. Hash SHA256 over this tag
followed by network:u32LE, genesis32, policyhash32, parent32, spendheight:u64LE,
expiry:u64LE, resolved_hash32 and the unsigned canonical body (no signatures/proof
bytes). `resolved_hash=SHA256(BOTHO_CT1_RESOLVED\0 || resolved)` where resolved is
input_count:u8 followed in input/ring order by each of20 records:
outpoint36, creation_height:u64LE, spendpoint32, amountcommitment32,
tag_count:u8 then canonical(origin41,weight:u32LE) entries; then each input's
f_i:u64LE and a_i:u64LE, followed once by g:u64LE, rate:u32LE, Y:u64LE, H:u64LE.
Counts/order are fixed by the body. Validators compute this from authenticated
parent state; it is not a sender-supplied alternative state. Merlin
appends `statement` with that hash, then canonical ordered pseudo/output/charge
commitments under labels `pseudo`, `output`, `charge`; each vector includes its
u32LE count. No private witness serializes into public transcript. CLSAG signs
that same statement hash plus SHA256(canonical proof section), preventing a proof
substitution from preserving signatures. Proofs never contain/sign their own hash.
Spend height is exact (candidate expires after that block); wallet refresh/reprove
on height change is deliberate and expensive. This prevents public age/rate/state
from changing underneath a proof, but reduces liveness at roughly5-second blocks: generation+propagation+admission
must fit the available block interval. A bounded native/WASM/mobile benchmark
with multi-input proofs and height turnover is an **unmet go/no-go prerequisite**,
not a deployment promise. If it fails, CT1 must be revised to an explicitly
reviewed validity-window construction; never bypass age/state binding. Any future validity-window
optimization needs a worst-case state proof and separate review.

Existing #1284/#1288 evidence covers a **single-input inactive R1CS experiment**,
not these linkage/conservation/tag/bucket constraints. The experiment measured
1412 multipliers/2844 linear constraints and1121 serialized proof bytes per case;
its observed timings are one-host data, not a CT1 budget or multi-input benchmark.
Classic Bulletproof range API supports u64/8,16,32,64-bit widths. It cannot simply
prove the76-bit slack found in #1266. R1CS adds a distinct experimental code path;
no '~0.7KB total transaction proof' or 'no new crypto review' claim is justified.

## 5. Canonical wire, opening encryption and resource envelope

CT1 candidate wire uses fixed LE integers and explicit bounded vectors; no bincode,
JSON number conversion, platform usize or optional-field ambiguity. Prefix:
magic ASCII `BCT1`, network:u32, genesis32, policyhash32, parent32, spendheight:u64,
expiry:u64 (equal spendheight), token:u64(0), fee:u64, input_count:u8(1..16),
output_count:u8(1..16). Each input has exactly20 ordered outpoints(hash32,indexu32),
key_image32, pseudo_commitment32. Output record: spendpoint32, ephemeralpoint32,
amountcommitment32, ML-KEM ciphertext1088, amount_box72, tag_count:u8(0..32),
then ordered(origin41,weightu32) entries, and context_length:u16 plus context bytes
(max128). Normal context has fixed discriminator0 and original output indexu32;
lottery context is discriminator2, base_index:u32 and canonical cumulative
tweak:32 (37 bytes total), corresponding to the **small Context**, not #1293's
full payout Record. That Record includes original winner/domain/amount/key/ML-KEM
data and lives in the block's bounded payout section, outside this output context.
Ledger outpoint resolves the unique authenticated payout Record; full nodes recompute
its expected derivation, and light clients require the #1286 accepted-header body
membership proof (candidate max4 payout records, each<=2048 bytes; proof<=65536
bytes and<=64 sibling hashes). Record KEM/key fields must equal this output's
fields byte-for-byte, never select a second competing ciphertext for decryption.
The proof is a separate RPC/light-client artifact, not included or trusted inside
the37-byte context. Ordinary newly created outputs use normal context; spending a
lottery output does not cause its descendants to inherit its key lineage. Unknown
discriminators reject.
BCT1 ordinary transaction outputs MUST use normal context; discriminator2 occurs
only on separately typed ledger lottery outputs referenced by input outpoints.
Production activation cannot proceed until those context wire versions are frozen.

Proof section: length:u32 (1..32768), then input_count:u8 (must match prefix),
one compressed C_di:32 per input in order, followed by the single combined R1CS
proof consuming exactly the remaining declared bytes, then one CLSAG
signature per input with length:u16 (1..2048). Reject unknown versions, trailing
bytes, nonminimal point/scalar encodings, invalid points, mismatched counts,
oversized lengths before allocation, duplicate key images and duplicate output
spend keys. Future proof versions require explicit version dispatch, not a parser
fallback. Proof point validation/identity policy follows the frozen selected
proof format; acceptance requires format-level malleability vectors. The32KiB
proof cap is a proposed envelope, **not yet demonstrated feasible** for the full
circuit; exceeding it blocks this candidate rather than silently relaxing a bound.

Each output has universal ML-KEM-768 ciphertext. The sender derives the existing
hybrid stealth shared secret using ECDH and ML-KEM; amount encryption MUST use
that KDF's full hybrid secret, never ECDH alone or only a public view tag. Candidate
amount-box subkey is HKDF-SHA256(ikm=hybrid_secret, salt=genesis32,
info=`BOTHO_CT1_AMOUNT_KEY\0`||ephemeral32||spendpoint32||index:u32LE).
Nonce is first12bytes of SHA256(`BOTHO_CT1_AMOUNT_NONCE\0`||ephemeral32||
spendpoint32||index:u32LE); unique ephemeral/key material per output mandatory.
ChaCha20-Poly1305 encrypts 56-byte plaintext: token:u64(0), amount:u64,
blinding:canonical_scalar32, original_index:u32, context_version:u32(CT1=1).
Result is72bytes (includes16-byte authentication tag). Associated data is the
canonical output excluding amount_box, prefixed with network:u32LE and genesis32.
Commitment is computed before encryption and included in AD; no ciphertext hash
feeds its own key/AD. Index is body position and must match recovery plaintext.
Recipient decapsulates/decrypts, checks canonical scalar, token/index/context and
recomputes C; mismatch means no spendable balance. An authenticated ledger
commitment and proved range/conservation protect consensus; recipient opening
availability cannot be proved merely by a range proof. Garbage ciphertext can
burn sender funds, never mint funds. Sender wallet must self-decrypt/check before
broadcast; AEAD failure must not trigger a classical fallback. Existing hybrid
KDF must expose a separately reviewed domain-separated derivation interface; do
not repurpose a scalar or log the shared secret. This exact new box is proposed,
not a claim that today's clients implement it.

Limits: transaction<=102400 bytes, inputs/outputs<=16, rings exactly20,
proof<=32768 bytes, tags<=32/output, context<=128/output; max combined R1CS
65536 multipliers and262144 linear constraints. Verifier uses fixed generators
for that circuit capacity, bounded allocations and no witness-dependent layout.
Reject before expensive verification if structure exceeds limits. Block CT work
budget is at most1048576 multipliers,4194304 linear constraints and5120 ring
members in addition to existing20MiB byte bound; count actual public circuit sizes,
never trust a proposer-declared count. These are proposed admission ceilings,
not measured safe wall-clock limits. Implementation must measure worst-case
Linux/native and mobile/WASM verification/memory before parameter sign-off; inability
to meet the envelope is a design failure requiring a revised draft. Public base
charges counts; no circular fee based on variable proof byte size is introduced.

## 6. Boundaries and rollout gates

CT1 is a new transaction and output version under a coordinated protocol version;
old nodes reject it. No ambiguous reinterpretation of old `pseudo_output_amount`
or old serialized ledger records. V1/V6 records retain frozen decoders/hash rules.
A compatibility matrix must cover node/mempool/miner/validator, snapshots, ledger
migration, RPC protobuf/JSON, CLI/native/WASM/web/mobile, bridge, explorers and
light-client header/body membership proofs. All outputs' commitments, encrypted
openings, provenance and #1286 lineage must be authenticated by accepted body roots;
RPC delivery alone is not authenticity. Snapshot commitments include schema and
provenance pool state, with root validation and atomic rollback/recovery. Unknown
versions fail closed without skipping balances or resetting ledger state.

Public mint/bridge/lottery boundary outputs remain explicitly typed exceptions: amount
and commitment opening are publicly verified against authorized issuance/burn/
settlement proofs, nullifier/message replay protection and reserve accounting.
Lottery payout is a separate public-output record, not a BCT1 encrypted output:
its public awarded amount v has commitment `vH` (zero blinding), its inherited
ML-KEM ciphertext/R and tweaked spend key/context follow #1286, and it has no
amount_box. A miner cannot encrypt to an inherited secret it does not know; the
recipient already knows the public amount and zero opening and recovers only the
spend key. BCT1 input proofs accept that authenticated commitment like other ring
members. Coinbase/new bridge issuers use their separately specified boundary
record, with public amount/opening verified by consensus. Reject any ambiguous
parser that interprets a boundary output as an ordinary encrypted output.
CT transfers into/out of a bridge reveal the authorized boundary amount per ADR0004,
not unrelated input values. Boundary conservation and charge still apply. Fresh
unwrap origins derive from public imported amounts; lottery payouts are redistribution
and inherit lineage/tags without increasing issuance pools. #1286 legacy aliased
payout/key-image claims need an explicit coordinated disposition; merely marking
legacy funds unspendable is not mainnet readiness. Clean CT1 genesis may be a future
option, but this document authorizes no reset, import or fund disposal.

Activation gates, in order: (1) ratify remaining D1/D3/base/resource/wire choices;
(2) implement and independently review pure rules + combined proof; (3) bind actual
CLSAG/conservation and reject all adversarial mutations; (4) complete authenticated
storage/client/bridge/lottery migration; (5) cross-platform local and Linux CI E2E
with bounded verification; (6) approve a concrete network activation/migration plan.
No gate is discharged by this draft or the inactive experiments. External audit
engagement is outside this task.

## 7. Evidence, executable inventory and implementation decomposition

Run `python3 scripts/research/ct_contract_vectors.py`. It checks committed golden
boundary vectors, bucket preimages/monotonicity/overpayment, dilution examples,
tag bounds, aggregation, candidate integer relations and SHA-bound source inventory.
It is a dependency-free **integer reference**, not an independent cryptographic
implementation, Rust differential runner, full wire codec or activation test.
The inventory's symbol/hash checks detect missing/drifted references; they do not
prove semantic equivalence. Full final circuit/codec/client tests remain tasks.

| Evidence | Scope / limit |
|---|---|
| #1266, `e944f140b407f0945188f574c6fd38dda67c264b` | Production-kernel transcription, staged-floor/cap counterexamples and76-bit slack; no proof |
| #1284, `765445cd4543dc71347f6da060ba4f84d694b186` | Actual Rust differential reference and constrained integer allocation, single input |
| #1288, `a00e05015d924e8f21f7c40fb03add0ce032867f` | Actual R1CS prove/verify plus binding controls; not full CT transaction |
| #1293 / #1302 | Inactive lottery key/context/commitment primitives; no CT/body-root activation |

Source references and exact bytes are recorded in
`../../scripts/research/fixtures/ct-contract-inventory.json`; pinned experimental
references may live in unmerged branches and are distinguished from working-tree
production symbols. Follow-up implementation issues must each preserve this
candidate's proposed status until explicit sign-off and include acceptance tests:

1. [**Pure economics — #1306**](https://github.com/botho-project/botho/issues/1306): origin pools/reorgs, tag codec/factor/ring max, exact charges
and D1 inverse bucket vectors; differential actual Rust kernels; attack and honest
wallet/Path C simulation. Depends on D1/D3/base decision.
2. [**Combined proof — #1307**](https://github.com/botho-project/botho/issues/1307): shared pseudo commitments, exact charges + aggregate buckets,
conservation, bounded arithmetic and transcript/wire; real proofs and deliberately
unbound negative-control variants. Depends on1 and proof-family internal review.
3. [**Hybrid opening and clients — #1308**](https://github.com/botho-project/botho/issues/1308): reviewed KDF/AEAD codec, malformed corpus, fixed
native/WASM/mobile interoperability vectors and recovered commitment checks. Can
prototype inactive alongside2; no rollout before frozen wire.
4. [**Authenticated integration/migration — #1309**](https://github.com/botho-project/botho/issues/1309): actual CT CLSAG ownership linkage,
ledger/mempool atomic nullifiers, header/body roots, snapshots/RPC/bridge and lottery
contexts, V1 compatibility/disposition. Depends on1–3 and #1286.
5. [**Rehearsal and resource acceptance — #1310**](https://github.com/botho-project/botho/issues/1310): Linux CI end-to-end transfers, amounts hidden
on actual wire, invalid proof/balance/ring/context mutations, constrained hardware
worst-case verifier budgets, fail-safe migration rehearsal. Depends on4; no live
activation and no declaration that #902/#904 are closed merely because tests exist.
