# ADR 0013: libcrux as the ML-KEM-768 Implementation (ML-DSA Deferred)

**Status**: Proposed
**Date**: 2026-11-02
**Decision Makers**: Core Team
**Related**: issue [#1715](https://github.com/botho-project/botho/issues/1715) (this evaluation), [#1825](https://github.com/botho-project/botho/issues/1825) (migration), epic [#1704](https://github.com/botho-project/botho/issues/1704) (formal methods), [ADR 0006](0006-pq-architecture-ratification.md) (PQ architecture), [ADR 0008](0008-universal-pq-address-format-and-hybrid-stealth.md) (v2 address format); harness: [`scripts/research/libcrux-eval/`](../../scripts/research/libcrux-eval/README.md)

## Context

All of Botho's post-quantum cryptography goes through one facade crate,
`bth-crypto-pq` (`crypto/pq/`). The facade wraps two RustCrypto crates. The
root `Cargo.lock` resolves them to:

| Crate | Locked | Latest on crates.io (2026-11-02) | Machine-checked proofs |
|-------|--------|----------------------------------|------------------------|
| `ml-kem` | **0.2.3** | 0.3.4 | none known |
| `ml-dsa` | **0.1.1** | 0.1.2 | none known |

The two primitives play different roles today:

- **ML-KEM-768 is live and hot.** Every output carries a 1088-byte ciphertext
  (`check_kem_ciphertext`, `botho/src/consensus/validation.rs`). Every
  wallet decapsulates once per output it scans: the node wallet, the web
  wallet and `wasm-signer`, the mobile `rust-bridge`, `botho-exchange-scanner`,
  and the bridge's `bth_scan`. Recipient privacy depends on this code being
  correct and secret-independent.
- **ML-DSA-65 is derivation-only today.** [ADR 0006](0006-pq-architecture-ratification.md)
  retired ML-DSA minting signatures and the quantum-private tier. ML-DSA public
  keys are still derived from the wallet seed and published in v2 addresses
  ([ADR 0008](0008-universal-pq-address-format-and-hybrid-stealth.md)), and
  ML-DSA-65 is the designated future signature family. No consensus path
  calls ML-DSA verify today.

Cryspen's [libcrux](https://github.com/celabshq/libcrux) (the repository has
moved from `cryspen/libcrux`, which redirects to it) publishes ML-KEM and
ML-DSA in Rust and verifies parts of them with hax and F\*. Epic #1704 asked
whether adopting libcrux is a cheaper formal-methods gain than writing our own
proofs. This ADR records the evaluation (issue #1715) and a recommendation.
It does not swap any crate.

The candidate evaluated is `libcrux-ml-kem` **0.0.11** and `libcrux-ml-dsa`
**0.0.11**, both released 2026-10-07, which are the latest versions.

## 1. Verification scope: proven, tested, or claimed

The sources are libcrux's own documents. Each was read at the version or
commit noted:

- the top-level [`Readme.md`](https://github.com/celabshq/libcrux/blob/main/Readme.md);
- the per-crate READMEs shipped in the 0.0.11 crates;
- [`libcrux-ml-kem/proofs/verification_status.md`](https://github.com/celabshq/libcrux/blob/main/libcrux-ml-kem/proofs/verification_status.md),
  last regenerated 2026-03-10;
- the hax methodology paper, [ePrint 2025/142](https://eprint.iacr.org/2025/142).

### What libcrux says about itself

- The project is **pre-release**. All crates are versioned below 0.1. The
  maintainers ask production users to contact them first: "If you wish to use
  any of these crates in production, get in touch with the maintainers".
- The *verified* badge means "most (or all)" of the default-feature Rust code
  is verified with hax. "Where indicated", this covers panic freedom and
  functional correctness against a mathematical spec.
- Side channels: "executables compiled from the code in this repository are
  *not* verified to be side-channel resistant, although we try to enforce
  that the source code is secret-independent."
- **ML-KEM README:** "The portable and AVX2 code for field arithmetic, NTT
  polynomial arithmetic, serialization, and the generic code for high-level
  algorithms is formally verified." NEON is not mentioned.
- **ML-DSA README:** "The portable and AVX2 code for field arithmetic, NTT
  polynomial arithmetic, and serialization is formally verified." The
  high-level algorithms are **not** claimed, and neither is NEON.
- Secret independence is enforced by the type system and is opt-in. The
  `check-secret-independence` feature switches the portable ML-KEM code to
  `libcrux-secrets` integer types, which turns secret-dependent operations into
  compile errors. Constant-time behaviour of the compiled code is "best-effort
  … validate[d] … via inspection of the generated assembly". That is review,
  not proof.

### ML-KEM per-module status (libcrux's own table, "a rough guide")

| Layer | Panic-free | Correct | Notes |
|-------|-----------|---------|-------|
| Generic: `ind_cca` (KEM top level) | 27/27 | 26/27 | Math + spec |
| Generic: `mlkem*` (parameter-set APIs) | 134/134 | 128/134 | |
| Generic: `hash_functions`, `constant_time_ops`, `polynomial` | all | all or all-but-1 | |
| Generic: **`ind_cpa`** (the PKE inside the KEM) | **0/21** | **0/21** | not verified |
| Generic: **`sampling`, `matrix`** | 0/5, 1/5 | 0/5, 0/5 | not verified |
| Generic: `ntt` / `invert_ntt` / `serialize` | 4/8, 4/6, 16/20 | 4/8, 4/6, 2/20 | partial |
| Portable backend (arith, ntt, serialize, compress, sampling) | all | all | **fully verified** |
| AVX2 backend | 42/47 | 41/47 | mostly verified |
| **NEON backend** | **1/40** | **1/40** | **effectively unverified** |

### Summary

| Property | ML-KEM portable | ML-KEM AVX2 | ML-KEM NEON | ML-DSA (any backend) |
|----------|----------------|-------------|-------------|----------------------|
| Panic freedom | **Proven** for the backend and most generic code. `ind_cpa`, `sampling`, and `matrix` are open | Mostly proven | **Not proven** | Proven for arithmetic, NTT, and serialization only |
| Functional correctness vs. FIPS 203/204 | **Proven** for the backend and the `ind_cca` and `mlkem*` layers. `ind_cpa` is open, so the end-to-end proof has gaps | Mostly proven | **Not proven** | **Not proven** for keygen, sign, or verify |
| Secret independence | Source-level, type-checked (opt-in feature) | Claimed, best-effort | Claimed, best-effort | Claimed, best-effort |
| Binary constant-time | **Not verified** (explicit disclaimer) | Not verified | Not verified | Not verified |
| Tested | KATs, ACVP, Wycheproof upstream. ACVP and Botho-specific cross-tests here | Same upstream; **not exercised on this host** (see §3) | Same upstream; exercised here | Same upstream; exercised here |

Two practical consequences:

1. **On aarch64, the default API runs the unverified backend.** This covers
   Apple Silicon, every modern phone, and AWS Graviton. libcrux's `build.rs`
   enables `simd128` on aarch64, and the multiplexed functions pick NEON at
   runtime. A migration that wants proven code must call the `portable`
   instantiation explicitly on non-x86_64 targets.
2. **The advisory history fits this picture.** Five RustSec advisories have
   touched libcrux ML-KEM or ML-DSA behaviour. None was in the ML-KEM
   high-level or portable code that the proofs cover:
   - RUSTSEC-2025-0133: an *unverified* aarch64 intrinsics fallback produced
     wrong ML-KEM secrets and wrong ML-DSA signatures. Fixed in intrinsics
     0.0.4.
   - RUSTSEC-2026-0076: a panic in ML-DSA hint decoding.
   - RUSTSEC-2026-0077: ML-DSA verify **accepted signatures with an
     out-of-bound z norm**.
   - RUSTSEC-2026-0125: the AVX2 ML-DSA `use_hint` edge case accepted crafted
     invalid signatures.
   - RUSTSEC-2026-0126: AVX2 ML-DSA intermediates were not fully reduced.

   The ML-DSA verify bugs are forgery-class bugs in the unproven high-level
   algorithm. All five are fixed at or before 0.0.9. Separately,
   libcrux-sha3 and libcrux-secrets have had advisories (RUSTSEC-2026-0074,
   -0207, -0208, -0212). The sha3 ones state they do not affect ML-KEM or
   ML-DSA use.

The incumbents carry no proofs at all. RustCrypto `ml-dsa` had a
timing side channel in decomposition (RUSTSEC-2025-0144, fixed in
0.1.0-rc.3; 0.1.1 is not affected).

## 2. Wire compatibility (results)

The harness is `scripts/research/libcrux-eval/`. It lives in its own Cargo
workspace and is not a member of the node workspace, so libcrux appears in
neither the node dependency graph nor the root `Cargo.lock`. It drives
libcrux on every compiled backend (`Auto`, `Portable`, `Neon`) side by side
with RustCrypto and with `bth-crypto-pq` itself.

### NIST ACVP known-answer tests

The vectors are the `usnistgov/ACVP-Server` `internalProjection.json` files at
commit `975de31e`, fetched and SHA-256-pinned by `fetch-kats.sh`. Only
ML-KEM-768 and ML-DSA-65 were run, because those are the only parameter sets
Botho uses.

| Vector set | Cases | libcrux (Auto/Portable/Neon) | RustCrypto |
|------------|-------|------------------------------|------------|
| ML-KEM-768 keyGen (d, z → ek, dk) | 25 | 25/25 | 25/25 |
| ML-KEM-768 encapsulation (ek, m → c, K) | 25 | 25/25 | 25/25 |
| ML-KEM-768 decapsulation, incl. implicit-rejection cases | 10 | 10/10 | 10/10 |
| ML-KEM-768 encapsulationKeyCheck (FIPS 203 §7.2) | 10 (5 invalid) | 10/10 | **no API** |
| ML-KEM-768 decapsulationKeyCheck (FIPS 203 §7.3) | 10 (5 invalid) | 10/10 | **no API** |
| ML-DSA-65 keyGen (ξ → pk, sk) | 25 | 25/25 | 25/25 |
| ML-DSA-65 sigGen, external/pure, deterministic + hedged | 30 | 30/30 | 30/30 |
| ML-DSA-65 sigGen, internal, deterministic + hedged | 30 | 30/30 | 30/30 |
| ML-DSA-65 sigVer, external/pure + internal (24 negative) | 30 | 30/30 | 30/30 |

PreHash (HashML-DSA) and external-μ groups were skipped. Botho uses neither,
and libcrux supports only SHAKE128 pre-hashing.

### Botho-specific cross-tests

These ran with `LIBCRUX_EVAL_ITERS=10000` and `overflow-checks = true`,
matching Botho's release profile, and once more as a debug build. Every test
passed.

| Property | Result |
|----------|--------|
| `MlKem768KeyPair::from_seed(seed)` keys reproduced by libcrux | **10000/10000 byte-identical** (pk 1184 B, sk 2400 B) on every backend. This requires drawing `d‖z` from `ChaCha20Rng::from_seed(seed)` exactly as ml-kem 0.2.3's `generate` does (`botho::kem_seed_to_dz`). |
| Existing ciphertexts: `bth` encapsulate (OsRng) → libcrux decapsulate with the stored 2400-byte sk | **10000/10000** same shared secret |
| libcrux encapsulate → `bth` decapsulate | 10000/10000 |
| Same (ek, m): byte-identical 1088-byte ciphertext and 32-byte K; cross-decaps both ways | 10000/10000 |
| Implicit rejection, 5000 random ciphertexts + 5000 single-bit flips: identical rejection key J(z‖c) | 10000/10000 |
| `derive_pq_keys_from_seed` (BIP39 path) keys interoperate | 2500/2500 |
| `MlDsa65KeyPair::from_seed(ξ)` and `derive_onetime_sig_keypair`: pk (1952 B) and expanded sk (4032 B) byte-identical | 10000/10000 |
| `bth` sign (deterministic, empty context) vs. libcrux `sign(ctx="", rnd=0)` | **byte-identical 3309-byte signatures**, 10000/10000 |
| Hedged libcrux signatures verify under `bth`; `bth` signatures verify under libcrux | 10000/10000 both ways |
| Single-bit-flipped signatures: identical reject verdicts | 30000/30000 rejected by both |
| Random 3309-byte "signatures" | 10000/10000 rejected by both, no panics |

**One behavioural difference.** A non-canonical ML-KEM public key has a
coefficient ≥ q. Today `MlKem768PublicKey::from_bytes` accepts such a key
(length check only), and RustCrypto encapsulates to it without complaint.
libcrux's `validate_public_key` rejects it. If libcrux's *unchecked*
`encapsulate` is used anyway, it produces a **different** ciphertext from
RustCrypto. Honestly generated keys are always canonical, so no real wallet
is affected. The only case is a hand-crafted v2 address, and the correct
response to one is to reject it. A migration should call
`validate_public_key` in `MlKem768PublicKey::from_bytes` (FIPS 203 §7.2
requires this check anyway). This touches address parsing, not consensus:
`check_kem_ciphertext` remains a length-only rule.

**Conclusion:** the swap is wire-compatible for every key, ciphertext, and
signature Botho has produced or will produce. Existing on-chain ciphertexts
decapsulate unchanged, and existing seeds derive the same v2 addresses.

## 3. Targets

| Target | Result |
|--------|--------|
| Native aarch64-apple-darwin | Builds, and every test above passes on Auto, Portable, and NEON |
| Native x86_64 (AVX2 backend) | Compiles (`--target x86_64-apple-darwin`). **Not executed**: this host is Apple Silicon without Rosetta, so the AVX2 code path was **not exercised here**. Follow-up item: run the harness on an x86_64 Linux CI runner. |
| `wasm32-unknown-unknown` | Builds `no_std` with `default-features = false`, no `getrandom` needed (randomness is caller-supplied), and selects the portable backend. Run under Node 26 (V8), the ACVP subset passes: ML-KEM-768 encaps 25/25, decaps 10/10, ML-DSA-65 sigVer 15/15. The RustCrypto baseline also passes. |
| Mobile (`rust-bridge`) | The `staticlib` builds for `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `aarch64-linux-android`, and `x86_64-linux-android` with no NDK or C toolchain. libcrux is pure Rust plus `libc` for CPU-feature detection. **Not run on a device or simulator.** |

wasm32 size of a probe exposing decaps, encaps, and ML-DSA verify, at
`opt-level = "s"` with LTO and wasm-opt `-Oz`:

| Build | Raw | -Oz | -Oz gzip |
|-------|-----|-----|----------|
| libcrux | 286 KB | 216 KB | **43 KB** |
| RustCrypto | 118 KB | 44 KB | **20 KB** |

libcrux adds about 23 KB gzipped to the wallet bundle.

## 4. Supply chain

- **`cargo deny check`**, run with the repository's `deny.toml` against a
  crate depending only on libcrux (`wasm-probe`) and against the full harness:
  `advisories ok, bans ok, licenses ok, sources ok`. All five libcrux-ML
  advisories (§1) are patched at or before the evaluated 0.0.11.
- **License:** Apache-2.0 for every libcrux crate. The incumbents are
  Apache-2.0 OR MIT. Both are compatible with the existing allow-list.
- **Dependency tree:** the normal-target tree has 15 packages. It contains
  `libcrux-{ml-kem, ml-dsa, sha3, intrinsics, platform, secrets, traits}` and
  `libcrux-macros`, which is a proc-macro pulling in `syn` **3.x**, a new major
  for Botho, build-time only. It also contains `rand`/`rand_core` 0.10, which
  Botho already uses, and `libc`. For comparison, the RustCrypto pair pulls
  about 30 packages, counting duplicate majors such as two `hybrid-array`,
  `digest`, and `keccak` versions, plus `rand_core` 0.6. The `hax-lib` and
  `core-models` dependencies apply only under `cfg(hax)` and are not built
  normally.
- **Maintenance:** releases are frequent. libcrux-ml-kem shipped 0.0.8 on
  2026-03-19, 0.0.9 on 05-13, 0.0.10 on 07-15, and 0.0.11 on 10-07, each
  alongside a coordinated release of the whole repo. The last push was
  2026-11-01. The repo has 131 open issues, and advisories are filed promptly
  with RustSec. Downloads: libcrux-ml-kem 4.4 M, libcrux-ml-dsa 0.28 M,
  RustCrypto ml-kem 13.6 M, ml-dsa 4.9 M. Releases are **0.0.x with breaking
  changes**; for example, 0.0.11 changed the `rand::` API signatures. Pin
  with `=` and expect migration work on each bump.
- **MSRV:** the published crates declare no `rust-version`. The libcrux
  workspace declares 1.89.0. In practice the floor is 1.85, set by `rand`
  0.10. The probe builds with **1.85.0**, 1.89.0, 1.96.0, and the pinned
  nightly-2026-06-09. This is the same floor as the current `ml-dsa` 0.1.1,
  which also needs 1.85. Botho's declared workspace `rust-version = "1.83.0"`
  is already unenforceable for `bth-crypto-pq`.

## 5. Performance

The harness is `examples/bench.rs`, a timed loop with a 3 s budget per case,
single-threaded, running in `--release`. The machine is an Apple M3 Ultra
(28 cores, 96 GB), macOS 27.0.1, with the pinned nightly-2026-06-09. The host
was shared, with a load average of about 12 from unrelated jobs, and
repeated runs moved individual figures by up to about 20 %. The table below
is the last of three runs. It uses `overflow-checks = true`, which matches
Botho's release profile. Across all runs, the speedup of libcrux portable
over RustCrypto for ML-KEM decaps ranged from 1.18× to 1.39×, and NEON from
2.6× to 4.5×. The run without overflow checks gave 1.27× and 2.6×.

| Operation | RustCrypto (via `bth-crypto-pq`) | libcrux portable (verified) | libcrux NEON (default on aarch64) |
|-----------|----------------------------------|-----------------------------|-----------------------------------|
| **ML-KEM-768 decaps** (wallet scanning) | 41.8 µs | 35.5 µs (**1.2×**) | 9.9 µs (**4.2×**) |
| ML-KEM-768 encaps | 34.1 µs | 27.9 µs | 9.0 µs |
| ML-KEM-768 keygen | 34.0 µs | 22.0 µs | 8.3 µs |
| **ML-DSA-65 verify** (256 B msg) | 118.2 µs | 53.9 µs (**2.2×**) | 45.8 µs (**2.6×**) |
| ML-DSA-65 sign (deterministic) | 609 µs | 173 µs | 153 µs |
| ML-DSA-65 keygen from seed | 284 µs | 52 µs | 40 µs |

wasm32 under Node 26, single thread:

| Build | ML-KEM-768 decaps | ML-DSA-65 verify |
|-------|-------------------|------------------|
| RustCrypto, `opt-level=3` | 46.5 µs | 340 µs |
| **libcrux, `opt-level=3`** | **33.0 µs (1.4×)** | **64.2 µs (5.3×)** |
| RustCrypto, `opt-level="s"` | 58.5 µs | 692 µs |
| libcrux, `opt-level="s"` | **257.9 µs (4.4× slower)** | 1017 µs |

libcrux's wasm performance is **very sensitive to optimisation level**. The
web wallet's `wasm-pack` build inherits the root `[profile.release]
opt-level = 3`, where libcrux wins. A future switch of the wallet bundle to
`opt-level = "s"`/`"z"` would turn a 1.4× gain into a 4.4× regression.

## 6. Fuzzing

`fuzz_mlkem_decapsulation` was ported to libcrux as
`scripts/research/libcrux-eval/fuzz/fuzz_targets/fuzz_mlkem_decapsulation_libcrux.rs`.
The port keeps the same `FuzzMode` grammar and the same no-panic,
determinism, and round-trip assertions. It adds a mode for a garbage
2400-byte decapsulation key. Every operation is also checked
**differentially** against `bth-crypto-pq`, so any disagreement between the
two implementations crashes the run.

The run used this command:

```
cargo fuzz run -O -a fuzz_mlkem_decapsulation_libcrux -- -max_total_time=900 -fork=4
```

That is cargo-fuzz with libFuzzer and AddressSanitizer, release build with
debug assertions, on the pinned nightly-2026-06-09 on the M3 Ultra above. The
backends exercised on every input were Auto, Portable, and NEON.

| Metric | Value |
|--------|-------|
| Wall time | 906 s (15 min), 4 fork workers |
| Executions | **1,420,900** (about 370–450 exec/s aggregate, slowed by running every input on three backends plus RustCrypto) |
| Coverage | 3135 edges, 5273 features, 322-input corpus |
| Crashes / timeouts / OOMs | **0 / 0 / 0** |
| Differential mismatches vs. `bth-crypto-pq` | **0** (any mismatch would assert) |

Garbage 2400-byte decapsulation keys did not panic either library. Both
returned the same 32 bytes. FIPS 203 still expects keys to be validated
first, and the migration plan adds that validation.

Side note: `fuzz/README.md` (#920) says libFuzzer+ASan hangs before `main` on
macOS 26.5. On this host (macOS 27.0.1, nightly-2026-06-09) it ran normally,
so that caveat may be stale.

## Decision

**Recommendation: GO for ML-KEM-768. DEFER ML-DSA-65.**

1. **Replace RustCrypto `ml-kem` with `libcrux-ml-kem` inside
   `bth-crypto-pq`**, pinned `=0.0.x`, with `default-features = false` and
   `features = ["mlkem768"]`. The public API of `bth-crypto-pq` does not
   change. The reasons:
   - Byte-for-byte compatibility is demonstrated for every Botho key,
     ciphertext, and derivation (§2).
   - The proven envelope (portable code plus generic `ind_cca`) covers the
     live, privacy-critical decaps path. This is a real step up from zero
     proofs.
   - Even the verified portable backend is faster than the incumbent:
     1.2–1.4× native and 1.4× on wasm at `opt-level = 3`.
2. **Stay inside the proven envelope by construction.**
   - On every non-x86_64 target, `bth-crypto-pq` calls
     `mlkem768::portable::*` explicitly. This covers aarch64 servers, Apple
     Silicon, phones, and wasm.
   - On x86_64 it uses the multiplexed API, which selects AVX2 (mostly
     verified) or portable.
   - The NEON backend's 2.6–4.5× speedup is deliberately left on the table until
     libcrux's verification table shows NEON proofs. Revisit when it does. A
     one-line dispatch change will pick it up.
3. **Add FIPS 203 input validation.** Call `validate_public_key` in
   `MlKem768PublicKey::from_bytes`, and `validate_private_key_only` when a
   secret key is loaded from bytes. RustCrypto 0.2.3 offers neither check.
4. **Defer ML-DSA.**
   - libcrux's ML-DSA high-level algorithm is unverified.
   - It has had two 2026 verify-path bugs that accepted invalid signatures.
   - ML-DSA has no consensus role today (ADR 0006).

   Keep RustCrypto `ml-dsa` for key derivation. The cross-tests show that
   libcrux reproduces RustCrypto's keys and signatures exactly, so a later
   swap remains cheap. Re-evaluate when ML-DSA gains a consensus role (the
   theft-protection migration) or when libcrux publishes ML-DSA high-level
   proofs, whichever comes first.

The migration plan is in follow-up issue [#1825](https://github.com/botho-project/botho/issues/1825).

## Consequences

### Positive

1. The privacy-critical ML-KEM decaps path moves from unverified code to code
   whose panic freedom and functional correctness are machine-checked. The
   portable backend is fully verified, and the generic KEM layer is mostly
   verified.
2. Wallet scanning gets faster on every target measured: 1.2–1.4× native on
   the verified portable backend, and 1.4× on wasm at `opt-level = 3`.
3. FIPS 203 §7.2 and §7.3 input checks become available.
4. The `rand_core` 0.6 boundary goes away for ML-KEM. Note that
   `from_seed` still needs `rand_chacha` 0.3's exact ChaCha20 stream, so the
   follow-up must either keep `rand_chacha` 0.3 or prove that a newer ChaCha20
   produces the same stream.

### Negative

1. Botho takes a dependency on a self-declared pre-release (0.0.x) library
   with breaking changes between minor versions. The maintainers ask
   production users to get in touch first.
2. The proofs do not cover `ind_cpa`, `sampling`, or `matrix`, and nothing
   covers binary-level constant time. "Verified" here means "substantially
   more assurance than RustCrypto", not "end-to-end proven".
3. Two ML-KEM stacks coexist during the transition, and ML-DSA stays on
   RustCrypto, so the tree carries two SHA-3 implementations.
4. The wasm bundle grows by about 23 KB gzipped, and wasm performance depends
   on keeping `opt-level = 3` for the wallet build.
5. A new `syn` major (3.x) enters the build graph through `libcrux-macros`.

### Neutral

1. AVX2 was not exercised on this host. The follow-up gates the swap on an
   x86_64 CI run of the harness.
2. Mobile builds compile but were not run on a device.

## Alternatives Considered

### 1. Swap both ML-KEM and ML-DSA now

- Pro: one library and one SHA-3 stack. ML-DSA is 2–5× faster.
- Con: ML-DSA's high-level algorithm is unverified and had forgery-class
  verify bugs in 2026. The proof gain for ML-DSA is small (arithmetic only),
  and ML-DSA has no live consensus role that would benefit.

### 2. Use libcrux's default (multiplexed) API everywhere

- Pro: the 2.6–4.5× NEON speedup on Apple Silicon, phones, and Graviton.
- Con: on those targets this runs the backend that libcrux reports as 1/40
  verified, which throws away the reason for switching. A NEON-adjacent
  unverified fallback was the source of RUSTSEC-2025-0133.

### 3. Stay on RustCrypto and upgrade to `ml-kem` 0.3.x

- Pro: a mature, widely used crate (13.6 M downloads) with no pre-release
  caveat.
- Con: no proofs, and the ML-KEM half of #1704's formal-methods goal stays
  unaddressed. The upgrade itself is independent of this decision.

### 4. Write Botho's own Lean or Aeneas proofs of ML-KEM

- Pro: assurance tailored to Botho's exact usage.
- Con: a large, specialised effort that duplicates what libcrux already
  publishes. #1704's effort is better spent on Botho-specific logic such as
  the lottery and fees.

## References

- libcrux: https://github.com/celabshq/libcrux. Read: `Readme.md`,
  `libcrux-ml-kem/README.md`, `libcrux-ml-kem/proofs/verification_status.md`,
  and `libcrux-ml-dsa/README.md` (0.0.11).
- hax: Bhargavan et al., *hax: Verifying Security-Critical Rust Software using
  Multiple Provers*, IACR ePrint 2025/142.
- FIPS 203 (ML-KEM) and FIPS 204 (ML-DSA).
- NIST ACVP-Server vectors: https://github.com/usnistgov/ACVP-Server
  (commit `975de31e`).
- RustSec advisories: RUSTSEC-2025-0133, -2025-0144, -2026-0074, -0076,
  -0077, -0125, -0126, -0207, -0208, -0212.
- Harness: [`scripts/research/libcrux-eval/`](../../scripts/research/libcrux-eval/README.md).
