# ADR 0008: Universal PQ Address Format (v2) + Hybrid Stealth Preimage

**Status**: Accepted
**Date**: 2026-07-15
**Decision Makers**: Core Team

## Context

[ADR 0006](0006-pq-architecture-ratification.md) ratified the post-quantum &
privacy direction: universal ML-KEM on every output, an ML-DSA-65 key published
per account, retirement of the unreachable `QuantumPrivateTransaction` (QP)
class, and the whitepaper §4/§4.2 seed-derived ML-KEM construction as the
normative spec. That ADR set *direction*; it did not fix the concrete on-chain
**address format** or the exact **stealth-output key-derivation preimage**.

Two facts on `main` forced those format commitments:

1. **A parallel address type.** The canonical `PublicAddress`
   (`account-keys/src/account_keys.rs`) carried only two 32-byte Ristretto keys
   and had no PQ dependency. A separate `QuantumSafePublicAddress`
   (`account-keys/src/quantum_safe.rs`) bundled the ML-KEM-768 and ML-DSA-65
   public keys and serialized itself under a distinct `botho-pq://1/` address
   string. The universal-PQ path does not use the parallel type — so the PQ
   keys had to move into the canonical address, and the two types had to be
   unified to avoid a permanent fork in the address spine.

2. **Hybrid, not replacement, stealth.** The #957 foundation
   (`crypto/ring-signature/src/pq_onetime_keys.rs`, `transaction/clsag`) derives
   each one-time output key from BOTH the classical Diffie-Hellman shared secret
   AND the ML-KEM shared secret, so an attacker must break classical *and*
   post-quantum cryptography. This is a departure from the "replace ECDH with
   ML-KEM" phrasing in the whitepaper §4.2 prose and needed to be recorded as a
   deliberate format commitment.

Because the entire chain resets to protocol **6.0.0** (fresh genesis, riding
the demurrage/settlement reset batch #925/#831), there is no in-place UTXO or
address migration to straddle. One format break can cover all post-quantum keys.

## Problem Statement

Fix, as durable format commitments for the 6.0.0 reset:

- what the canonical address carries (which keys, in what representation);
- whether the PQ keys are part of address *identity* (digest / hash);
- how addresses are versioned;
- the disposition of the parallel `QuantumSafePublicAddress` type;
- the stealth-output key-derivation preimage (hybrid vs. replacement).

## Decisions

### D1 — PQ keys stored as raw fixed-length bytes (validate-on-parse)

`PublicAddress` gains two fields stored as **raw byte payloads**, not typed
crypto objects:

- `kem_public_key`: ML-KEM-768 public key, `ML_KEM_768_PUBLIC_KEY_LEN` = **1184
  bytes**
- `dsa_public_key`: ML-DSA-65 public key, `ML_DSA_65_PUBLIC_KEY_LEN` = **1952
  bytes**

Rationale: keeping the base `account-keys` type free of a hard `bth-crypto-pq`
dependency preserves the crate's `no_std`/`--no-default-features` builds and
avoids pulling the full lattice stack into every consumer of an address. The
exact byte length is enforced when an address is **parsed** (from its string /
wire form), not by the storage type. A classical-only ("v1") address leaves
both fields empty.

### D2 — Address versioning: bump to `botho://2/` (deferred to the string sub-issue)

The v2 address is `view(32) ‖ spend(32) ‖ ML-KEM-768(1184) ‖ ML-DSA-65(1952)`
≈ **3.2 KB**. The compact base58 address *string* moves from `botho://1/` to
a new **`botho://2/`** prefix so that old 64-byte addresses fail loudly rather
than silently truncating. The retired `botho-pq://1/` prefix is rejected.

Scope note: this ADR records the versioning decision; the base58 string
encoder/decoder change (and the shared codec of D5) land in a later rollout
sub-issue. This sub-issue changes only the struct + prost/serde/digest
representation.

### D3 — Both PQ keys are part of address identity (digest / `ShortAddressHash`)

The ML-KEM and ML-DSA public keys enter the `PublicAddress` `Digestible`
transcript and therefore its `ShortAddressHash`. An address's PQ keys must not
be swappable: two addresses that share Ristretto keys but differ in either PQ
key are distinct identities and hash differently. (Implemented by deriving
`Digestible` over the new fields; covered by a swap-detection test.)

### D4 — Bundle BOTH PQ keys now; unify/retire `QuantumSafePublicAddress`

Address v2 publishes the ML-KEM **and** the ML-DSA key together. One format
break covers all post-quantum keys — there is no third break later when
lattice signature verification consumers arrive. ML-KEM is wired into stealth
outputs by the universal rollout; the ML-DSA key is published and
hierarchy-derived and available for signature-verification consumers as they
arise (post-quantum *spend authorization* via lattice ring signatures is
explicitly out of scope for now).

`QuantumSafePublicAddress` and its `botho-pq://` string path are **retired**:
the KEM+DSA keys are folded into the canonical `PublicAddress`. The secret-side
`QuantumSafeAccountKey` (the mnemonic-seeded keypair holder) is **kept** as a
thin shim that now yields the unified `PublicAddress`; its own unification into
`AccountKey` is tracked by the key-hierarchy sub-issue.

### D5 — Single shared base58 address codec (deferred to the string sub-issue)

At ~3.2 KB, the four independent base58 encoders (node / wasm / mobile /
wallet) risk byte-level drift. A single shared address codec is to be extracted
so they cannot diverge. Recorded here; implemented in the string sub-issue.

### Hybrid stealth preimage (normative)

Each output's one-time key is derived from a hybrid preimage combining the
classical DH shared secret and the ML-KEM shared secret (per #957 /
`pq_onetime_keys`), i.e. `s = Hs(dh ‖ K ‖ index)`-style domain-separated
hashing over BOTH secrets — **hybrid**, not a replacement of ECDH by ML-KEM.
Breaking either cryptosystem alone is insufficient to recover or link an
output. The whitepaper §4.2 prose is updated from "replace ECDH" to this
hybrid construction in the enforcement sub-issue.

## Consequences

### Positive

1. One canonical address type on the spine; no parallel PQ address to keep in
   sync.
2. Defense-in-depth stealth: classical *and* post-quantum security must both
   fall for an output to be compromised.
3. A single format break (6.0.0) absorbs all PQ keys — no future forced break
   for ML-DSA.
4. The base `account-keys` type stays PQ-dependency-free and `no_std`-friendly.

### Negative

1. Addresses grow from 64 bytes to ~3.2 KB; QR/URI capacity grows ~50×
   (addressed by the v2 string + shared codec sub-issue).
2. Changing the address digest changes `ShortAddressHash` for every address —
   acceptable only because 6.0.0 is a fresh-genesis reset with no migration.
3. Storing PQ keys as raw bytes defers structural validity to parse time; a
   malformed in-memory address is possible until validated.

### Neutral

1. Old 64-byte `botho://1/` addresses cannot receive on the reset chain;
   faucet, web wallet, mobile, and any published testnet addresses must
   regenerate after the format bump.

## Alternatives Considered

### 1. Typed `MlKem768PublicKey` / `MlDsa65PublicKey` fields in `PublicAddress`

- Pro: validity enforced by construction.
- Con: forces a hard `bth-crypto-pq` dependency into the base address type and
  every crate that touches an address; breaks `--no-default-features`. Rejected
  in favor of raw bytes + validate-on-parse (D1).

### 2. KEM-only address now, ML-DSA in a later break

- Pro: smaller address initially.
- Con: a second consensus-breaking format change later. Rejected — one break
  covers both (D4).

### 3. Keep `QuantumSafePublicAddress` as a separate type

- Pro: no downstream churn.
- Con: a permanent parallel address spine the universal path never uses.
  Rejected — unify into the canonical address (D4).

### 4. Replace ECDH with ML-KEM (drop classical DH from the stealth key)

- Pro: simpler preimage.
- Con: single point of cryptographic failure; loses classical security during
  the PQ transition. Rejected in favor of the hybrid preimage.

## Implementation

Rolls out across the #958 sub-issues (rides the 6.0.0 reset batch; protocol
version is **not** bumped further):

1. **This sub-issue (#959):** `PublicAddress` carries both PQ keys as raw
   bytes; prost tags 3/4; `Digestible`/`ShortAddressHash` include them (D3);
   `new_with_pq`/`with_pq_keys` constructors; classical callers unchanged;
   `QuantumSafePublicAddress` retired (D4); ADR 0008.
2. Key hierarchy publishes the derived KEM/DSA keys (§4.2 seed-derived).
3. v2 base58 address string (`botho://2/`) + shared codec (D2, D5).
4-6. Send path attaches ciphertexts; minting/lottery outputs; scanner
   decapsulates on the hybrid path.
7. Consensus enforcement + whitepaper §4.2 prose update to the hybrid preimage.

## References

- [ADR 0006: Post-Quantum & Privacy Architecture Ratification](0006-pq-architecture-ratification.md)
- Whitepaper §4 / §4.2 (seed-derived ML-KEM; normative spec)
- Issue #958 (universal-PQ rollout plan + ratified decisions D1–D5)
- Issue #959 (this sub-issue), #954, #957 (hybrid-stealth foundation)
