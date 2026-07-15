# The BTH ↔ wBTH Bridge

Botho's native asset is privacy-preserving and anti-hoarding, but it does not
natively reach the deep liquidity of the Ethereum and Solana DeFi ecosystems.
The **bridge** exists to give holders access to that liquidity: you lock native
BTH into a reserve and receive **wBTH**, a fully-transparent wrapped token that
is redeemable one-for-one for the locked BTH.

> **Canonical sources.** This page is the concepts-level explainer. The
> normative specification is whitepaper **§11 (*The BTH ↔ wBTH Bridge*)**, built
> on the ratified bridge decision records
> [ADR 0002](../decisions/0002-bridge-custody-scp-validator-federation.md)
> (custody), [ADR 0003](../decisions/0003-wbth-peg-factor-1-wrapping-and-demurrage-settlement.md)
> (peg), [ADR 0004](../decisions/0004-bridge-privacy-semantics.md) (privacy),
> [ADR 0005](../decisions/0005-bridge-v1-chain-scope-ethereum-and-solana.md)
> (chain scope), and
> [ADR 0007](../decisions/0007-bridge-import-cluster-tagging.md)
> (import cluster tagging). For operational detail see
> [Bridge Architecture](../bridge/architecture.md) and the
> [bridge threat model](../security/bridge-threat-model.md).

## What wBTH is

wBTH is a wrapped representation of BTH that lives on external chains:

| Chain | Token standard | Transparency |
|-------|----------------|--------------|
| Ethereum | wBTH (ERC-20) | Fully public / linkable |
| Solana | wBTH (SPL) | Fully public / linkable |

wBTH is denominated in **picocredits** (12 decimals) — one wBTH base unit equals
one picocredit equals one unit of native BTH, so the peg carries no scaling
factor. Unlike native BTH, wBTH is **transparent** on its host chain: anyone can
see and link wBTH balances and transfers. This is a deliberate privacy boundary
(see [Privacy at the boundary](#privacy-at-the-boundary)).

## The factor-1 peg

The peg is the promise that outstanding wrapped supply always equals the locked
reserve:

```
Σ(wBTH on Ethereum) + Σ(wBTH on Solana)  ==  locked BTH reserve on Botho
```

The subtlety is **demurrage**. Botho charges a holding cost on wealthy-cluster
coins at spend time (see [Progressive Fees](progressive-fees.md) and
[Monetary Policy](monetary-policy.md)). If reserve BTH demurred while it sat
wrapped, the reserve would shrink below the outstanding supply and the bridge
would silently become fractionally reserved.

Botho avoids this with a single rule (ADR 0003):

> **Only factor-1 coins are wrappable.** A factor-1 (background / commerce) coin
> pays exactly zero demurrage, permanently — so a reserve of only factor-1 coins
> holds its value over time, with no "reserve class" exemption and no rebasing.

Wrap eligibility is checkable at deposit: every output carries its cluster-tag
vector, so the bridge reads the coin's factor directly and mints wBTH **only**
for factor-1 deposits (a non-factor-1 deposit is rejected before any mint).
Reciprocally, releases spend **only** factor-1 reserve outputs. A holder of a
wealthy-cluster coin cannot wrap it directly; a (target-state)
demurrage-settlement operation lets them *pay* a charge to reclassify the coin
down to factor-1 first, with that fee routed to the redistribution lottery pool.

## Custody: the validator federation

Neither Ethereum nor Solana can natively verify Botho's state, and Botho has no
light client for either chain. Every privileged cross-chain action (minting
wBTH, releasing reserve BTH) must therefore be authorized by **signatures from a
trusted signer set** rather than by on-chain proof (ADR 0002). Botho's choice:

> **The bridge federation is the SCP validator set, operating as a `t`-of-`n`
> threshold signer group.**

No mint or release fires without at least `t` distinct federation signatures,
domain-separated and order-bound. The threshold is constrained so that it is
**never easier to move the reserve than to move consensus** (`t ≥ t_SCP`). BTH
reserve release and Solana mint reuse the validators' Ed25519 node keys; Ethereum
minting additionally requires secp256k1 keys held in a Gnosis Safe, with mint /
admin / pause roles split across three separate Safes so no single Safe can both
mint and reconfigure the breaker.

A consequence, accepted deliberately: because the federation *is* the validator
set, bridge safety is coupled to consensus safety — a validator-majority
compromise can also drain the reserve. Growing and hardening the validator set is
a soft prerequisite to holding meaningful bridge value.

## Privacy at the boundary

The bridge is a designed privacy boundary with explicit, documented leakage
(ADR 0004):

1. **Lock reveals the amount.** To mint the correct wBTH, the bridge must learn
   the deposited amount. This is an unavoidable, deliberate privacy exit — it
   leaks only the amount, not the source ring.
2. **The wrapped side is public.** Anyone holding or moving wBTH is linkable on
   the host chain. Wallet UX should warn that *wrapping exits the anonymity set*.
3. **Unwrap re-shields the recipient.** A burn → release pays out to a fresh
   one-time stealth address, breaking the on-Botho link to the destination-chain
   burn.

Because only factor-1 coins are wrappable, the amount revealed at lock is already
of the least sensitive wealth class — and its public-at-the-boundary nature is
exactly what makes import cluster tagging (below) computable with no
zero-knowledge machinery.

## Import cluster tagging: why imports enter "expensive"

Botho's anti-hoarding mechanism prices **coin lineage** — the wealth traceable to
a coin's cluster origin maps onto a demurrage / fee / lottery multiplier
(1×–6×), and it is Sybil-resistant domestically because splitting a coin does not
change the wealth traceable to its origin. The bridge introduces a vector the
lineage mechanism cannot see. If unwrapped BTH simply returned at factor-1, two
leaks would follow (ADR 0007):

- **The entry leak.** External wealth of any size could buy wBTH on a DEX and
  unwrap into Botho at factor-1, having paid no lineage premium. This is
  *unavoidable in general* — you cannot tax external wealth entry without
  breaking the very liquidity the bridge exists to provide.
- **The round-trip laundromat.** A domestic holder who accumulated a high-factor
  lineage could round-trip (BTH → wBTH → unwrap → factor-1 BTH) and reset to
  background for the price of a wrap. If any lineage-reset door is cheap, the
  whole cluster-factor mechanism is trivially bypassable.

The guiding principle: **only money that has circulated within the Botho network
should be cheap to spend.** So unwrapping does *not* mint a plain factor-1
background coin. Instead (ADR 0007):

> **Unwrapping mints BTH into a bridge-import cluster keyed to the block-height
> epoch of the unwrap, at an elevated factor derived from that epoch cluster's
> aggregate unwrap wealth, subject to a floor. Imported wealth normalizes toward
> background only by circulating.**

Concretely, for an unwrap at block height `h`:

- **Epoch key.** `m = ⌊h / K⌋` with **`K = 17,280` blocks (1 day** at the 5s
  reference). Every unwrap in the same epoch joins one shared cluster origin
  `c_import(m) = H("bridge-import" ‖ m)`, and the released output carries a
  100%-weight tag to it — exactly as a minting output is 100% attributed to its
  new mint cluster.
- **Factor from shared epoch wealth.** The import cluster's wealth is the *sum of
  all unwrap amounts in the epoch*, fed through the identical production
  cluster-factor curve domestic clusters use. A quiet epoch → low factor; a
  high-volume flood → high factor.
- **Floor `F`.** The import factor is clamped to **`≥ F = 1.5×`** — the minimum
  "toll" for entering via the bridge rather than earning domestically.
- **Decay only by circulation.** The imported coin's factor falls solely as it
  mixes with background-tagged coins through ordinary spends (a worst-case 6×
  flood import blends to the 1.5× floor in ≈9 domestic-mixing spends). Sitting
  idle never normalizes imported wealth — *using* it does.

### Why the epoch key is load-bearing

A flat per-unwrap factor would be Sybil-proof but would over-tax a small entrant
identically to a whale. A size-based per-unwrap factor recovers size-sensitivity
but is Sybil-able: a whale drip-splits into many dust unwraps, each a separate
low-wealth origin at factor ≈1, then reassembles domestically. The epoch key
defeats the split because all unwraps in a window **share one accumulating
cluster** — intra-epoch splitting piles into the same pool and still hits the
high factor. Diluting requires spreading across *epochs*, which costs wall-clock
time (`K` blocks each): **time-as-cost replaces provenance-as-cost.** A
2M-BTH whale needs 541 separate epochs (541 days) to drip-dilute to the floor.

This means an unwrapper's factor depends partly on strangers who unwrapped in the
same epoch — a shared-fate coupling that is judged a *feature*: a sudden capital
flood (the inflow that would most concentrate the domestic distribution) is
treated as maximally concentrated, while an organic trickle enters benignly.

### What it fixes

- **The round-trip laundromat is closed.** A bridge round-trip now *degrades*
  lineage (out at factor-1, back at import-factor ≥ 1.5×) instead of resetting
  it, so the bridge is removed from the lineage-reset-door list. This
  **collapses the reset-vector map**: the only remaining domestic reset door is
  the spend-to-background leak, tracked separately.
- **The entry leak is materially narrowed** without touching liquidity — imported
  wealth no longer gets the privileged factor-1 status only domestically-
  circulated money earns.
- **Confidential-amounts-clean by construction.** The epoch cluster's wealth is
  the sum of unwrap amounts, which are public at the bridge boundary by necessity
  (ADR 0004). The import factor is therefore computable from already-public data
  with **no zero-knowledge gadget**.

Import cluster tagging is **live as of protocol 5.0.0** (the unwrap path tags the
released output to the epoch import cluster at the elevated factor).

## Security in brief

The bridge's security rests on five load-bearing invariants (whitepaper §11.6):
exactly-once mint, exactly-once release, threshold authorization, peg solvency,
and finality safety. The bridge is **fail-closed** everywhere — peg drift,
backlog and volume caps, the on-chain auto-pause, and the operator kill-switch
all *halt* rather than degrade. A proof-of-reserves reconciler continuously
checks that outstanding supply equals the locked reserve, and because the reserve
holds only zero-demurrage factor-1 coins the invariant is *exact* rather than
approximate.

**Mainnet gate:** no bridge value moves on mainnet until an external security
audit of the Rust service, the Solidity token contract, and the Anchor program
has cleared. See the [bridge threat model](../security/bridge-threat-model.md)
for the full threat → test map.

## Related

- Whitepaper **§11** — *The BTH ↔ wBTH Bridge* (normative specification)
- [ADR 0007](../decisions/0007-bridge-import-cluster-tagging.md) — Bridge-Import Cluster Tagging
- [Bridge Architecture](../bridge/architecture.md) — service design and order state machine
- [Bridge Security](../bridge/security.md) — operational security and incident response
- [Bridge Threat Model](../security/bridge-threat-model.md) — threat → adversarial-test map
- [Progressive Fees](progressive-fees.md) — the cluster-factor mechanism import tagging plugs into
- [Monetary Policy](monetary-policy.md) — demurrage and the factor curve
