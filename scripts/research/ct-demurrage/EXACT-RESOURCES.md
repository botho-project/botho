# Exact CT demurrage resource observations

Measured 2026-11-06 for [#1954](https://github.com/botho-project/botho/issues/1954),
part of [#1914](https://github.com/botho-project/botho/issues/1914). The exact
research circuit passes fresh local arithmetic, combined and ownership runs.
The complete retained macOS and Linux ownership matrices have identical shape,
allocation counts and proof-field sizes: eight processes and thirteen composed
proofs on each platform. These are bounded observations, not CT1 acceptance,
cryptographic soundness, mobile/WASM feasibility, liveness or activation approval.

## Source and execution identities

| Delivery | Immutable identity and evidence |
| --- | --- |
| Exact implementation and production parity, #1952 | [PR #1955](https://github.com/botho-project/botho/pull/1955), commit `6bc0b08623e973a79ea5be084fe5c6c094faa669`, tree `0fde6491d16e16674f4d27532c59ee79d74752cf` |
| Independent source derivation, #1953 | [PR #1959](https://github.com/botho-project/botho/pull/1959), commit `e070cedfc8e433ffe6aa3e7e29ae12ed83a8839c`; [review](EXACT-RELATION-REVIEW.md) and unchanged [52-path manifest](exact-relation-review-sources.json) pin the implementation above |
| Fresh macOS arithmetic and ownership capture | Clean checkout `e070cedfc8e433ffe6aa3e7e29ae12ed83a8839c`, tree `9c8535916b57c8928319b359274d3d8a822a4129`; [arithmetic evidence](evidence/exact-9-arithmetic/retention.json), [ownership evidence](evidence/exact-9-macos/retention.json) |
| Retained Linux ownership capture | Successful [workflow run 42215470766, attempt 1](https://github.com/botho-project/botho/actions/runs/42215470766); actual PR merge checkout `e7b24e9c43e1a9f6bbc6dd820af1c529ad655255`, tree `0fde6491d16e16674f4d27532c59ee79d74752cf`; PR head was `701afc3e16a2e4e14808bca48c45f5c9f34404d4`; [retention](evidence/exact-9-linux/retention.json) |
| This publication | Commit containing [exact-measurements.json](exact-measurements.json); resolve with `git log -1 --format=%H -- scripts/research/ct-demurrage/exact-measurements.json`. It is distinct from the measured checkout and implementation identities. |

The macOS run used Apple M3 Ultra, arm64, Darwin 27.0.0 and Python 3.15.2.
Linux used Intel Xeon Platinum 8573C, x86_64, Linux 7.0.0-1019-azure and Python
3.14.4. Both report `rustc 1.98.0-nightly (cb46fbb8c 2026-06-08)` from pinned
`nightly-2026-06-09`. Cargo selected one test executable in the optimized release
profile, with overflow checks enabled. The source retains its existing scoped
Dalek overflow-check exceptions. Local compilation used two jobs and the managed
worktree's private `target/`, took 26.41 seconds, and completed before measurement.

Actual executable SHA256 values are:

- macOS: `27f05fc85c30ec1eaaabb43cf14e2c97e357ddfefad74090047144b75a78f034`.
- Linux: `6ce397db86939f5f5f3ed228b26b160d83a82df9c54649ed0d24d71fb8e277a9`.

These identify the observed executable bytes. Rebuilds may differ; neither a
hash nor Cargo stdout proves a hermetic build. The binaries are not retained;
Cargo JSON, actual paths, hashes, profiles, commands and raw outcomes are.
The local capture was clean. Linux recorded only untracked
`ownership-build.jsonl`; both tracked-diff hashes equal SHA256 of empty bytes.
No measurement attempt failed or timed out, and no successful subset was selected.

## Fresh arithmetic and allocation observations

All three exact test filters ran separately and each reported one passing test.
The [raw logs and parsed measurements](evidence/exact-9-arithmetic/measurements.json)
retain every result:

- Eight single-input fixtures, including zero year, full domain and both cap
  cases, produced 1,121-byte proofs. Prove times were 224.79–226.92 ms and verify
  times 16.33–17.32 ms. The pinned test checks both actual allocator counts and
  prover/verifier equality for every fixture; those counts are not printed in
  each timing line.
- The separate same-factor short-hold proof printed exact charge 58,494,241,735,
  legacy zero and capital difference zero, with 1,121 bytes, 1,562 multipliers
  and 3,140 constraints. Its passing assertions also check actual verifier
  equality. This discriminator is not selected by either ordinary measurement
  filter.
- Combined measurements include three samples at each of significant bits 2, 3
  and 4, for both 1/1 and 16/16 inputs/outputs: eighteen timed round trips.
  Six additional boundary round trips passed, including unequal 1/16 and 16/1
  shapes. Combined generator setup was 365.25 ms for capacity 32,768, measured
  separately from the per-proof operations.

| Relation / shape | Actual multipliers | Actual constraints | Arithmetic proof bytes | Proposed combined-section bytes |
| --- | ---: | ---: | ---: | ---: |
| Single input | 1,562 | 3,140 | 1,121 | — |
| Combined 1/1, s = 2/3/4 | 1,762 | 3,544 | 1,121 | 1,158 |
| Ownership arithmetic 4/4, s = 2 | 6,640 | 13,351 | 1,249 | — |
| Combined 16/16, s = 2/3/4 | 26,152 | 52,579 | 1,377 | 1,894 |

The delivered source has `64 + 3*(64+147+90+90) + 5*64 = 1557` range bits,
nine linear equations and five products per input. This yields `1557+5 = 1562`
multipliers and `2*1557+9+3*5+2 = 3140` constraints including external bindings.
Each output adds 64/129; the two aggregate gap ranges and three equations add
136/275. Thus the measured counts match `1562*n+64*m+136` and
`3140*n+129*m+275`. Relative to the **historical** 1412/2844 input census, the
exact relation adds 150 multipliers and 296 constraints per input (2,400 and
4,736 at sixteen inputs). Historical observations are not fresh measurements.
The 65,536-multiplier, 262,144-constraint and 32,768-section-byte ceilings are
unchanged. The proposed section is `4+1+32*n+proof_bytes`, not a full transaction
or a production proof-section admission test.

## Complete ownership matrices

Every proof uses ring size 20, significant bits 2, complete arithmetic constraints,
external commitment binding, separate group balance and all ordinary indexed
CLSAG verifications. Case premises are asserted by the unchanged resource test.
All cases retain the 120-second process and 300-second matrix deadlines. Collector
elapsed time was **22.8753 s on macOS** and **23.9426 s on Linux**; the local outer
command took 23.2807 s. Compilation is excluded from these times.

Below, operation times are milliseconds, using the median of three ordinary
samples and the sole observation for each boundary case. RSS is the peak for
that **whole case process**, in MiB. No baseline subtraction or per-proof memory
estimate is made. All samples, fixture/setup times and raw RSS units remain in
the [macOS summary](evidence/exact-9-macos/summary.json) and
[Linux summary](evidence/exact-9-linux/summary.json).

| Case | Proofs | macOS prove | macOS verify | macOS peak MiB | Linux prove | Linux verify | Linux peak MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| generators | 0 | — | — | 12.97 | — | — | 13.35 |
| ordinary-1 | 3 | 231.76 | 21.67 | 27.73 | 213.28 | 19.34 | 19.33 |
| ordinary-4 | 3 | 889.38 | 81.23 | 70.22 | 838.07 | 77.75 | 35.05 |
| ordinary-16 | 3 | 3505.90 | 322.93 | 250.28 | 3424.64 | 311.86 | 147.09 |
| zero | 1 | 233.61 | 21.75 | 25.20 | 221.76 | 20.24 | 19.34 |
| both-caps-cancel | 1 | 235.47 | 21.91 | 25.00 | 219.74 | 20.34 | 19.48 |
| one-cap-difference | 1 | 237.56 | 21.21 | 26.05 | 222.10 | 19.03 | 19.31 |
| maximum-sum | 1 | 3517.21 | 325.16 | 165.16 | 3431.45 | 313.33 | 96.38 |

Generator setup in the eight fresh processes ranged from 361.23–370.42 ms on
macOS and 543.81–570.87 ms on Linux. Ownership operation times include the
arithmetic proof and CLSAG composition; fixture generation/setup are separate.
Arithmetic payloads are 1,121/1,249/1,377 bytes at 1/4/16 inputs. CLSAG fields
add `736*n` bytes: 736/2,944/11,776, respectively. These are field lengths, not
an encoded wire envelope. Timing and RSS differences reflect these two machines,
backends and runtime environments; these small samples support no portability
or production throughput claim.

## Retention and reproducible validation

[The index](exact-measurements.json) hashes all **65 retained evidence files**.
Each directory has a complete flat inventory. The Linux directory preserves the
original 18-file workflow ZIP, SHA256
`29b0303e1e5e857852eee1fae7e96a4773eb82c193232a31bb3ef54ae12cf9f8`, matching the
forge artifact digest for artifact 13500827575. The checker compares every ZIP
member to the retained raw bytes, including decompressed Cargo JSON.

Each original collector summary stays byte-for-byte unchanged. Its exact
15-source archive is separate from a **174-path supplemental archive**. The
supplement includes all 52 review/context paths, the production library export,
tracked Rust sources/manifests in the thirteen Cargo-identified local crates,
literal included documentation, Cargo configuration and collector inputs.
Some test/context files are retained conservatively. Every supplemental byte
matches the reviewed implementation; archives are reconstructed from the actual
capture commits, not falsely attributed to the runner upload. Registry packages
are identified by lockfile checksums, not archived here. The source-selection
rule and complete path/hash maps are retained alongside each archive.

From the repository root:

```sh
python3 scripts/research/ct-demurrage/check_exact_resources.py
# Also compare each archived source with its named Git commit (requires history):
python3 scripts/research/ct-demurrage/check_exact_resources.py --verify-git
python3 scripts/research/ct-demurrage/check_retained_resources.py
python3 -m unittest discover -s scripts/research/ct-demurrage -p 'test_*.py'
```

The new entrypoint reuses `check_cases` and `check_linux`, checks complete raw
inventories, source supplements, executable selection/profile, exact-filter
results and deterministic cross-platform fields. Its controls reject missing
production export coverage, an altered artifact and a zero-test discriminator.
`--verify-git` additionally checks the named commit/tree/bytes; offline validation
checks retained identities and internal consistency without pretending to query
Git history. Existing historical validation and captures are unchanged.

For another measurement, first create a helper-managed Loom issue worktree and
verify its `.loom-managed` sentinel. Honor a Loom-assigned target/marker if one
exists. The following recipe shows the fallback used by this run: that worktree's
own `target/`; `capture` holds only small raw evidence, not compiled build artifacts:

```sh
set -eu
WORKTREE_ABS="$(git rev-parse --show-toplevel)"
test -f "$WORKTREE_ABS/.loom-managed" || exit 1
export CARGO_TARGET_DIR="$WORKTREE_ABS/target" CARGO_BUILD_JOBS=2
export RUSTUP_TOOLCHAIN=nightly-2026-06-09 RUSTC_WRAPPER=''
capture="$(mktemp -d /tmp/ct-exact-capture.XXXXXX)"
cargo test --locked --release -p bth-ct-demurrage-reference --no-run \
  --message-format=json > "$capture/cargo.jsonl" 2> "$capture/build.stderr"
EXACT_BIN="$(python3 -c 'import sys; from pathlib import Path; sys.path.insert(0,"scripts/research/ct-demurrage"); from measure_ownership import executable; print(executable(Path(sys.argv[1])))' "$capture/cargo.jsonl")"
"$EXACT_BIN" proof_experiment::proof_round_trips_and_measurements --exact --nocapture --test-threads=1 \
  > "$capture/arithmetic.stdout" 2> "$capture/arithmetic.stderr"
"$EXACT_BIN" proof_experiment::combined::combined_round_trips_and_measurements --exact --nocapture --test-threads=1 \
  > "$capture/combined.stdout" 2> "$capture/combined.stderr"
"$EXACT_BIN" proof_experiment::same_factor_short_hold_exact_proof --exact --nocapture --test-threads=1 \
  > "$capture/short-hold.stdout" 2> "$capture/short-hold.stderr"
python3 scripts/research/ct-demurrage/measure_ownership.py \
  --cargo-messages "$capture/cargo.jsonl" --output-dir "$capture/ownership"
```

Retain command exits, raw stdout/stderr, actual executable hash and source/dirty
identity as this capture does. Use a fresh output directory; collector refusal,
nonzero exits and timeouts are failures, not permission to shrink the matrix.
Use the existing Linux workflow and retain its complete successful artifact.

The [source review's open obligations](EXACT-RELATION-REVIEW.md#ownership-and-cryptographic-obligations-that-remain-open)
remain open: fork-specific R1CS extraction, same-member CLSAG knowledge, joint
adaptive composition, admitted ranged openings and authenticated/unspent state.
The model's Lean claims, finite production parity, source-derived arithmetic and
these observations are separate evidence categories. Historical
[RESOURCES](RESOURCES.md), [COMBINED](COMBINED.md), [OWNERSHIP](OWNERSHIP.md),
manifests and captures retain their original meaning. No production, model,
proof-system, collector, workflow or policy change is part of this publication.
