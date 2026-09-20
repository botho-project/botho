# Botho's pinned Bulletproofs cleanup patch

Upstream: https://github.com/mobilecoinfoundation/bulletproofs.git

Exact source commit: `9abfdc054d9ba65f1e185ea1e6eff3947ce879dc`.
The complete tracked tree was exported with `git archive` from that commit.
`LICENSE.txt` (MIT, copyright Chain, Inc.) and all upstream attribution are
preserved. The package version remains `3.0.0-pre.1`.

This is a local maintenance patch for #1261, addressing the dependency on
unmaintained `clear_on_drop` (RUSTSEC-2026-0283). It does not change proof
algorithms, transcripts, generators, encodings, or the dalek 4 interop boundary.
It is internal hardening, not an external cryptographic audit (#616).

## Exact local changes

`botho-zeroize.patch` is the complete diff of the four modified upstream files:

- `Cargo.toml`: replace `clear_on_drop` with no_std-compatible `zeroize`, explicitly
  enable dalek's `zeroize` feature, remove `clear_on_drop/nightly`, and isolate
  upstream test builds with an empty workspace. Workspace dependency defaults
  still disable std; upstream R1CS still requires **both** std and yoloproofs.
- `src/util.rs`: change scalar cleanup calls to `Zeroize::zeroize`. One upstream
  **test-only** `Scalar::from_bits` call becomes `from_bytes_mod_order`: its
  constant is canonical, so the value is identical. The removed constructor
  otherwise prevents running upstream unit tests against dalek 4.1.3.
- `src/range_proof/party.rs`: replace every old scalar/value cleanup call with
  `zeroize`, retaining per-element vector wiping.
- `src/r1cs/prover.rs`: replace every old cleanup call, and fix `Secrets.v` and
  `Secrets.v_blinding`: the previous inherent `Vec::clear` only removed their
  length. Each live scalar is now overwritten. Extract that Drop cleanup into
  a private method so a regression test checks storage while still allocated.
  Formatting of these three source files follows rustfmt's edition-2018 defaults.

All remaining upstream files are unchanged. This provenance file and the patch
are additions. The repository patch entry selects this directory; its Cargo.lock
removes `clear_on_drop`. There are no security advisory ignore changes.

To reproduce the vendored source (run from the repository root):

```sh
git clone https://github.com/mobilecoinfoundation/bulletproofs.git /tmp/bulletproofs-upstream
git -C /tmp/bulletproofs-upstream checkout 9abfdc054d9ba65f1e185ea1e6eff3947ce879dc
git -C /tmp/bulletproofs-upstream apply "$PWD/vendor/bulletproofs-og/botho-zeroize.patch"
diff -ru --exclude=.git --exclude=Cargo.lock --exclude=BOTHO-PATCH.md \
  --exclude=botho-zeroize.patch /tmp/bulletproofs-upstream vendor/bulletproofs-og
```

## Cleanup inventory

Every production `Clear` call in the pinned fork is covered:

| Site | Secret storage overwritten |
| --- | --- |
| `util::VecPoly1::drop` | Both coefficient vectors, each live scalar |
| `util::Poly2::drop` | All three scalar coefficients |
| `util::VecPoly3::drop` (yoloproofs) | All four coefficient vectors, each live scalar |
| `util::Poly6::drop` (yoloproofs) | All six scalar coefficients |
| `PartyAwaitingPosition::drop` | `v` (u64), `v_blinding` |
| `PartyAwaitingBitChallenge::drop` | `v`, `v_blinding`, `a_blinding`, `s_blinding`, every `s_L`/`s_R` element |
| `PartyAwaitingPolyChallenge::drop` | Five scalar blindings; polynomial fields use their own Drop implementations |
| `r1cs::Secrets::drop` | Every scalar of `v`, `v_blinding`, `a_L`, `a_R`, `a_O` |
| `r1cs::Prover::prove` | Every scalar of temporary `s_L1`, `s_L2`, `s_R1`, `s_R2` vectors |

Element-wise wiping retains vector lengths and capacities until deallocation;
it does not mistake clearing the header for wiping its elements. This patch
covers the existing cleanup sites; it does not claim to erase historical
allocator copies, stack/register copies, or all prover temporaries.

## Proof compatibility evidence

Before changing either the pinned dependency or its source, the integration
harness `transaction/core/tests/bulletproofs_compat.rs` was run against the
unmodified git fork on base `65687275`. It generated **and verified** the three
committed binary fixtures using `CAPTURE_PINNED_FIXTURES=1 cargo test -p
bth-transaction-core --test bulletproofs_compat`. The golden files were then
checked with the patched dependency, without capture enabled.

Inputs use values cycling through `[0, u64::MAX, 42]`, scalar blindings starting
at 17, token-zero Pedersen generators, the transaction-core transcript, and
`get_seeded_rng()` (Hc128 seed `[7; 32]`). Cases contain 1, 3 (padded to 4), and
8 values. Each file is the proof encoding followed by 32-byte commitments.

| File | SHA-256 |
| --- | --- |
| `range-1.bin` | `5a9bd12d6b58267b91e15baa4b0467da001fae4e34deb88a957b04f77e876537` |
| `range-3.bin` | `855d53e41603761962255a896d4085569154d6a43e3b7a15f3c9405273fce802` |
| `range-8.bin` | `8d173451e9cd80ea51cd8e0692ef28ac08af3a549e3311aeff473eb76b72d2da` |

The patched prover must produce **identical bytes**; the patched verifier checks
both decoded original proofs and freshly generated proofs. Since the original
verifier already accepted those exact bytes, equality establishes both directions
of cross-verification. Corrupted proofs and incorrect commitments are rejected
through transaction-core. Capture mode is only for reproducing the original
baseline; never use it to bless changed output from a replacement implementation.

## Validation

Run from the repository root with its pinned toolchain:

```sh
cargo test --locked -p bth-transaction-core
cargo test --locked -p bth-transaction-core --all-features
cargo test --manifest-path vendor/bulletproofs-og/Cargo.toml --features yoloproofs
cargo check --manifest-path vendor/bulletproofs-og/Cargo.toml --no-default-features --target wasm32-unknown-unknown
cargo clippy -p bth-transaction-core --all-features --tests
cargo clippy --manifest-path vendor/bulletproofs-og/Cargo.toml --features yoloproofs --lib --tests
cargo tree --locked -i clear_on_drop  # expected: package does not exist
cargo deny --all-features check
```

Observed before review: 99 transaction-core tests (120 with all features), plus
the compatibility fixture test; upstream 31 unit tests, 12 R1CS tests, one range
proof integration test, and six doctests passed. no_std compilation for wasm32
passed independently of workspace features (also with `nightly`). The inherited
`yoloproofs` without `std` combination is unsupported: upstream `errors.rs`
references `String` without importing it. R1CS itself remains std-only; this
patch does not broaden that feature contract. Clippy passed with existing upstream
and workspace warnings. The new R1CS storage test also fails when the old
`Vec::clear` behavior is restored in a temporary copy.

The all-features security gate passes with the HTTP/TLS changes from PR #1260
(commit `601f109dec3c9e3c681ccf9d2928bb84dc15d41c`), combined in the final PR
branch; no ignores were added. Linux workspace CI remains the authority for the
full desktop-inclusive workspace. `.github/workflows/bulletproofs-compat.yml`
keeps the isolated upstream tests and no_std check running for future changes.
