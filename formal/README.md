# formal/ - Lean 4 models and proofs

This directory holds Lean 4 models of selected Botho mechanisms, proofs about
those models, and the gates that keep the proofs honest. It is the scaffold for
epic #1704, set up by #1705. It contains a toy model (`Botho/Basic.lean`)
that mirrors no Rust code, and the LotteryV2 model (`Botho/Lottery/`, #1707),
which mirrors the node's lottery draw, reward cap and pool accounting and is
differentially tested against the Rust code (see
[The lottery model](#the-lottery-model) and
[Differential random testing](#differential-random-testing-drt)).

The approach follows Cedar's verification-guided development: write a small
executable Lean model of a Rust component, prove properties about the model,
and use differential random testing (DRT) to check that the Rust code and the
model agree.

## The model, prove and DRT loop

1. **Model.** Write an executable Lean definition (`def`, not `axiom`) that
   mirrors a Rust function or state transition, in `Botho/<Area>.lean`. Keep it
   as close to the Rust structure as the proofs allow. Cite the Rust file and
   function it mirrors in the module docstring.
2. **Prove.** State and prove properties of the model (conservation, bounds,
   determinism, monotonicity) as `theorem`s in the same module. Add each
   exported theorem to `Theorems.lean` (see below).
3. **DRT.** Compile the model to an executable and run it against the Rust
   implementation on random and boundary inputs, comparing outputs. A proof
   only says something about Rust if DRT shows the model and the code agree.
   The harness is `formal-drt/` (#1706, #1707); see
   [Differential random testing](#differential-random-testing-drt).
4. **Mirror.** Add the mirrored Rust files to the `pattern` in
   `.github/workflows/formal.yml` so that edits to them re-run the Lean job.

## Labeling rule: proven / tested / trusted

Every claim made about a Rust component carries exactly one of these labels in
its module docstring, and in any document that cites it:

| Label | Meaning |
|-------|---------|
| **proven** | A theorem about the Lean model, listed in `Theorems.lean`, whose axioms are only `propext`, `Classical.choice` and `Quot.sound`. Says nothing about Rust unless DRT also passes. |
| **tested** | The Rust code and the Lean model agree under DRT. This is evidence, not proof, that a proven property holds for the Rust code. |
| **trusted** | Depends on an allow-listed opaque axiom (for example a `sha256` or `randomx` stand-in), or on an assumption about code outside the model. Name the axiom or assumption next to the claim. |

Never write "verified" on its own. Write "proven (model)", "proven + tested",
or "trusted (axiom `Botho.sha256`)".

## Building locally

Install elan, the Lean toolchain manager:

```bash
curl -sSfL https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y --default-toolchain none
# or: brew install elan-init
```

Then build. elan reads `lean-toolchain` and installs the pinned version
automatically:

```bash
cd formal
lake build                      # type-checks every module under Botho/
scripts/check-sorry.sh          # sorry gate (scans, then builds)
scripts/check-axioms.sh         # axiom gate
scripts/selftest-gates.sh       # proves both gates reject injected defects
scripts/selftest-verify.sh      # proves the verify job rejects the bypass fixtures
```

To run the verify job's checks on your own build, stage the `.olean` files
the way CI uploads them and point the verifier at them:

```bash
rsync -am --include='*/' --include='*.olean' --exclude='*' \
  .lake/build/lib/lean/ /tmp/formal-lib/
scripts/verify-artifacts.sh --lib /tmp/formal-lib --sources .
```

`verify-artifacts.sh --sysroot DIR` and `selftest-verify.sh`'s `LEAN_SYSROOT`
name the toolchain directory when `lean --print-prefix` does not find it.
`verify-artifacts.sh` passes that directory to every Lean process as
`LEAN_SYSROOT`, so `Init` always comes from it, never from a `lean` on `PATH`;
with `--sysroot` it needs no `lean` on `PATH` at all. It and the gates need a
`python3` with `tomllib` (3.11 or later).

The scripts take an optional `LAKE=/path/to/lake` environment variable if the
`lake` on your `PATH` is not the elan proxy.

The toolchain is pinned to an exact stable release in `lean-toolchain`. Bump it
deliberately in its own PR. Never use `stable` or `nightly`. The project has no
dependencies (no Mathlib). Whether to add Mathlib is decided in #1708, because
it adds substantial CI time.

## The lottery model

`Botho/Lottery/` (#1707, the pilot of epic #1704) models what the node
computes for each block's LotteryV2 draw. Each definition's docstring names
the Rust function and file it mirrors:

| Lean (`Botho.Lottery.*`) | Rust | Status |
|--------------------------|------|--------|
| `lotterySeedV2` | `lottery_seed_v2`, `botho/src/consensus/lottery.rs:81` | tested |
| `Candidate.isEligible` | `LotteryCandidate::is_eligible`, `cluster-tax/src/lottery.rs:187` | tested |
| `Candidate.weight` | `LotteryCandidate::weight`, `cluster-tax/src/lottery.rs:203` | tested (through the draw) |
| `countEligible` | `count_eligible`, `cluster-tax/src/lottery.rs:248` | tested |
| `sybilRewardCap` | `sybil_reward_cap`, `cluster-tax/src/lottery.rs:272` | tested |
| `drawWinnersWithSeed`, `drawEligible`, `verifiableRandomU128` | `draw_winners_with_seed`, `draw_eligible`, `verifiable_random_u128`, `cluster-tax/src/lottery.rs:341,361,498` | tested |
| `splitFees` | `LotteryFeeConfig::split_fees`, `botho/src/consensus/lottery.rs:167` | tested |
| `Accounting.carryoverAfter` | `LotteryPoolAccounting::carryover_after`, `botho/src/consensus/lottery.rs:283` | tested |
| `rewardCap` | `reward_cap`, `botho/src/consensus/lottery.rs:306` | tested |
| `computePoolAccounting` | `compute_pool_accounting`, `botho/src/consensus/lottery.rs:326` | tested |
| `drawLotteryWinnersSeeded` | `draw_lottery_winners_seeded` and `finish_draw`, `botho/src/consensus/lottery.rs:391,410` | tested |
| `offsetKey`, `lotteryCandidates` | `Ledger::lottery_candidate_offset_key`, `botho/src/ledger/store.rs:2811`; the candidate read `lottery_candidates`, `botho/src/ledger/store/validation.rs:733` (reached through `DerivationReads::lottery_candidates`, `botho/src/ledger/lottery_v2.rs:168`) | tested |
| `headerHash` | `BlockHeader::hash`, `botho/src/block.rs` | tested (through `drawing`) |
| `seed`, `drawing` | `lottery_v2::seed`, `lottery_v2::drawing`, `botho/src/ledger/lottery_v2.rs:220,238` | tested |

Line numbers are those of main at `651d0c45`.

How the model is written:

- **Integers.** The model computes on `Nat` and states each Rust width.
  `Botho/Lottery/Arith.lean` defines one helper per Rust primitive
  (`satSub`, `satAdd`, `satMul`, `checkedAdd`, `truncU64`, `leBytes`,
  `ofLeBytes`), and a plain `+` or `*` is used only where the Rust operation
  cannot overflow, with the reason next to it.
- **Recursion.** Lists are walked by structural recursion, mirroring the Rust
  loops: the cumulative winner walk (`selectAt`), the roll loop
  (`pickWinners`), the capped candidate scan (`scan`).
- **Hashes are parameters, not axioms.** Every hashing function takes a
  `Prims` argument holding `sha256` and RandomX's `powHash`. A theorem about
  the model therefore holds for every hash function, and no axiom is added
  (`axioms-allowlist.txt` is unchanged). A theorem that needs a property of
  the hash takes it as a hypothesis and is labeled **trusted**. The DRT
  executable instantiates `sha256` with a SHA-256 written in Lean
  (`formal-drt/lean/Drt/Sha256.lean`, checked against the FIPS 180-4 vectors
  at start-up) and takes the RandomX output of the parent header from the Rust
  side as data.
- **Inputs, not models.** The per-UTXO cluster factor (the fee curve, #1711),
  the emission share (`MintingTx::lottery_emission_share`, #1710) and
  RandomX are inputs.
- **Out of scope.** `SelectionMode::EntropyWeighted` (research-only, reads a
  floating-point entropy), the `f64` to basis-point conversion of
  `SelectionMode::Hybrid` (the model takes `alpha_bps`; DRT checks the round
  trip for every value it generates), and the strict read policy's failures
  in the candidate read (the model assumes every row decodes).
- **Preconditions.** `winners_per_draw > 0` (Rust divides by it; consensus
  fixes it at 4), and the candidate read's rows sorted strictly by key (as
  LMDB iterates them).

The theorems about this model are #1708.

## Differential random testing (DRT)

`formal-drt/` (#1706, #1707) checks that the Lean model and the Rust code
compute the same function. It has two parts:

- `formal-drt/lean/`: a Lake package with the executable `botho-model`. It
  `require`s the model by path (`../../formal`), reads JSON-lines requests on
  stdin (`{"fn": ..., "args": {...}}`) and writes one JSON-lines response per
  request (`{"ok": ...}` or `{"err": ...}`). Integers are decimal strings and
  byte strings are hex. The codec and dispatch are in `Drt/Codec.lean`.
- `formal-drt/` (Rust): the dev-only crate `botho-formal-drt`. It holds one
  long-lived `botho-model` process, generates inputs with `proptest`
  (biased to 0, 1, `u64::MAX`, the eligibility and cap boundaries, empty and
  single candidate sets, and `MAX_LOTTERY_CANDIDATES` ± 1), sends them in
  batches, runs the Rust function on each, and compares the two JSON values.
  A Rust panic counts as a mismatch. A mismatch is shrunk and appended to
  `formal-drt/regressions/<leg>.jsonl`, and every run replays those files
  first.

**Why it lives outside `formal/`.** The gates (above) accept exactly one
shape of the gated package: no `.lean` file under `formal/` besides
`Botho.lean`, `Botho/**` and `Theorems.lean`; no `[[lean_exe]]` or
`[[require]]` in `formal/lakefile.toml`; and no `IO`, `#eval` or `extern` in
model sources. An executable needs all three, so it is a separate package
next to `formal/`, never scanned by the gates or built by the verify job. It
runs the gated model sources unchanged. The Rust crate is kept beside it so
that DRT changes do not touch the gated tree: the formal gate job does not
re-run for them, and `selftest-gates.sh` (which copies all of `formal/` for
each case) never copies a Cargo build.

The Rust crate is its own Cargo workspace (excluded from the root one, like
`fuzz/`): it enables botho's `test-fixtures` feature, which must never be
unified into a node build. It uses that feature for two things: seeding a
real LMDB ledger (the `lottery_candidates` leg runs the production reader)
and `botho::ledger::formal_drt`, which runs the production
`lottery_v2::drawing` over an in-memory `DerivationReads` double (the
`drawing` leg).

### Legs

| Leg | Rust | Model |
|-----|------|-------|
| `lottery_seed_v2` | `consensus::lottery::lottery_seed_v2` | `lotterySeedV2` |
| `is_eligible` | `LotteryCandidate::is_eligible` | `Candidate.isEligible` |
| `count_eligible` | `count_eligible` | `countEligible` |
| `sybil_reward_cap` | `sybil_reward_cap` | `sybilRewardCap` |
| `split_fees` | `LotteryFeeConfig::split_fees` | `splitFees` |
| `reward_cap` | `consensus::lottery::reward_cap` | `rewardCap` |
| `compute_pool_accounting` | `consensus::lottery::compute_pool_accounting` | `computePoolAccounting` |
| `carryover_after` | `LotteryPoolAccounting::carryover_after` | `Accounting.carryoverAfter` |
| `draw_winners_with_seed` | `draw_winners_with_seed` | `drawWinnersWithSeed` |
| `draw_lottery_winners_seeded` | `consensus::draw_lottery_winners_seeded` | `drawLotteryWinnersSeeded` |
| `lottery_candidates` | `Ledger::get_lottery_validation_candidates` over a real LMDB ledger | `lotteryCandidates` |
| `drawing` | `lottery_v2::drawing` over the in-memory double | `drawing` |

The `drawing` double reimplements the candidate read over an in-memory
table (it cannot use LMDB); the `lottery_candidates` leg checks the
production reader against the same model, so the two legs together tie both
to the model.

### Running it (macOS or Linux)

Prerequisites: elan (see [Building locally](#building-locally)), the pinned
Rust toolchain, and `cmake` (RandomX). From the repository root:

```bash
(cd formal-drt/lean && lake build)          # builds the model and botho-model
cp Cargo.lock formal-drt/Cargo.lock         # build the node's exact dependency versions
cd formal-drt
cargo run --release -- --cases 10000        # the per-PR tier: seed 1707, all legs
cargo run --release -- --cases 1000000 --seed random --legs drawing,reward_cap
cargo run --release -- --list               # leg names
```

Options: `--cases N` (per leg), `--seed S` or `random`, `--legs a,b`,
`--batch B`, `--json PATH` (machine-readable report), `--regressions DIR`
and `--no-write-regressions`. The report prints, per leg, the cases run, the
fixtures replayed, the mismatches and a histogram of outcome classes (winner
counts, error kinds, saturated or exact sums), which shows which branches the
cases reached. On macOS the first `drawing` case builds a RandomX light cache
for each epoch of the parent headers (a few seconds); everything else needs no
special setup. The legs are independent, so a long run can be split across
processes with `--legs`.

CI (`.github/workflows/formal-drt.yml`) runs the per-PR tier: seed 1707,
10,000 cases per leg, about a minute of run time. The nightly random-seed
tier of 1M cases per leg, and the leg that feeds the same inputs to the
`cluster-tax` simulation code (#1709), are follow-ups of #1706.

### Adding a function

One generator, one Lean entry point and one comparison:

1. **Model.** Add the definition under `Botho/`, with a docstring naming the
   Rust function and file, and import it from `Botho.lean`.
2. **Lean entry point.** In `formal-drt/lean/Drt/Codec.lean`, add an arm
   `| "my_fn" => ...` to `dispatch` that decodes the arguments, calls the
   model and encodes the result (the decoders and encoders for the common
   types are in the same file).
3. **Rust leg.** In `formal-drt/src/legs.rs`, add a type implementing `Leg`:
   `NAME` (the same `"my_fn"`), an `Input` type, `strategy()` (the generator;
   reuse the edge-biased strategies in `src/gen.rs`), `args()` (the request
   arguments, in the wire format of `src/wire.rs`) and `rust()` (the Rust
   result in the same wire format). Optionally `classify()` for the
   histogram. List it in `LEGS` in `src/main.rs`.
4. Run `cargo run --release -- --legs my_fn --cases 100000`, then inject a
   bug into the Rust function (an off-by-one, a wrong saturation) and check
   that the leg catches it within the 10,000-case tier. Revert the bug.
5. If the leg needs the Rust files it mirrors to trigger CI, add them to the
   `paths` of `formal-drt.yml` and to the `pattern` in `formal.yml`.

A mismatch is either a model bug or a Rust bug. Fix a model bug in the
model. File a Rust bug as an issue with the shrunk fixture; do not change the
model to match it.

## Gates

CI (`.github/workflows/formal.yml`) runs two jobs on opposite sides of a trust
boundary (#1799; see [Limits](#limits)):

- **build** (untrusted) checks out the pull request, runs the gates below
  (which run `lake build`) and the self-tests, and uploads the compiled
  `.olean` files as the artifact `formal-lib`. Its results are fast feedback
  only: model code runs during the build and could fake them.
- **verify** (trusted) runs `scripts/verify-artifacts.sh` from the **base**
  commit over that artifact. See [The verify job](#the-verify-job).

The required check is `formal-gate`. It passes when both jobs succeeded, or
when both were skipped because nothing under `formal/`, the workflow, or a
mirrored Rust file changed.

The in-tree gates (#1770). The build job runs them in full; the verify job
runs the base commit's `check-sorry.sh --no-build`, which is the layout check
and the source scan below, over the pull request's sources as data:

- **Build configuration.** Before anything else, both gates check that
  there is no `lakefile.lean` (a Lean program that runs on every `lake`
  call) and that `lakefile.toml` passes the allow-list in
  `scripts/gate-common.sh`:
  - *Layout:* no package `srcDir`, one `[[lean_lib]]` named `Botho` with no
    `srcDir`, `roots`, `globs` or `root`, and no `[[lean_exe]]`. These are
    the keys the verify job's layout check compares with the base's
    `lakefile.toml`. So Lake compiles only `Botho.lean` and the modules it
    imports, all in `Botho/`.
  - *Keys:* every other key must be on an allow-list of keys that cannot
    load or run code: package metadata (`name`, `version`, `defaultTargets`,
    `description`, `keywords`, `homepage`, `license`) and `leanOptions`
    with plain values for a short list of options (`autoImplicit`,
    `relaxedAutoImplicit`, `maxHeartbeats`, `maxRecDepth`, the
    `synthInstance` limits, `linter.*`, `pp.*`). A `[[lean_lib]]` may carry
    only its `name`. Everything else is rejected, known or unknown:
    `[[require]]`, `moreLeanArgs`, `weakLeanArgs`, `moreServerOptions`,
    `moreLinkArgs`, `plugins`, `dynlibs`, `precompileModules`,
    `extraDepTargets`, `buildDir`, `testDriver`, and options such as
    `debug.skipKernelTC`.
  - *No dependencies:* `lake-manifest.json`, if present, must list no
    packages.

  `lean-toolchain` is not pinned by the gates, so toolchain bumps pass; see
  [The verify job](#the-verify-job).
- **Source scan** (`scripts/gate_scan.py`). Both gates run it before their
  first `lake` call, and stop if it fails, so the in-tree gates never build a
  tree the scan rejected. It checks:
  - *Layout.* The model is `Botho.lean` and `Botho/**/*.lean`, the manifest
    is `Theorems.lean`, all regular files (no symlinks under `Botho/`), and
    there is no other `*.lean` file outside `.lake/` and `scripts/`.
  - *Import allow-list.* Every scanned file may import only `Botho` modules
    that are scanned files (`import Botho.X.Y` must be `Botho/X/Y.lean`). Not
    `Lean`, `Std`, `Lake`, `Init.*`, Mathlib or anything else. A model that
    needs another import must extend the list in `gate_scan.py` in a reviewed
    diff. `prelude`, `module`, import modifiers (`public`, `meta`,
    `import all`) and an `import` after the header are rejected. Multi-line
    headers, comments between imports and `«»`-quoted names are parsed the
    way Lean parses them.
  - *Tokens*, as listed under the sorry gate below.
- **Sorry gate** (`scripts/check-sorry.sh`) fails if `lake build` warns that a
  declaration uses `sorry`, or if a sweep of the compiled library finds any
  declaration that reaches `sorryAx`. The sweep (`scripts/AxiomReport.lean`)
  walks declaration bodies directly, so it still works when the warning is
  hidden (for example by `#guard_msgs`). The gate also fails if the source
  scan finds `sorry`, `sorryAx`, `admit` or `native_decide` (bare,
  `_root_.`-qualified, or with explicit universes such as `sorryAx.{0}`)
  outside comments, string literals, raw strings (`r#"..."#`) and char
  literals. Code inside `{...}` string interpolation is scanned.
  `native_decide` trusts the compiler, so it is allowed only on a line marked
  `-- formal-gate: allow native_decide`, and the compiler-trust axioms it
  introduces (such as `Lean.ofReduceBool`) must also be on the allow-list.
  The scan also rejects commands that run code or hide messages at compile
  time (`#eval`, `#guard`, `#guard_expr`, `run_cmd`, `run_elab`, `run_meta`,
  `run_tac`, `by_elab`, `#guard_msgs`, `initialize`, and `#print` outside
  `Theorems.lean`), the evaluators that run compiled code while the model is
  elaborated (`native` in any form, such as `decide +native` or
  `native := true`, and `reduceBool`, `reduceNat`, `ofReduceBool`,
  `ofReduceNat`, `trustCompiler`), the prelude IO surface (`IO`, `EIO`,
  `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`, `System`,
  `FilePath` as a name or any dotted component, so `IO.FS`, `IO.Process`,
  `IO.Ref` and `ST.Ref` too; the `unsafe` keyword; and any name starting
  with `unsafe` or ending in `Unsafe`, such as `unsafeCast` or
  `unsafeBaseIO`), metaprogramming commands that can expand to
  them (`macro`, `macro_rules`, `syntax`, `elab`, `elab_rules`,
  `declare_syntax_cat`), simprocs (`simproc`, `dsimproc`), and the option
  `debug.skipKernelTC`. Inside `@[...]` and `attribute [...]` it rejects
  attributes that run code at build or import time, bind native or
  substitute code, or extend the elaborator: `init`, `builtin_*`, `extern`,
  `implemented_by`, `export`, `macro`, `*_elab`, `*_parser`, `tactic`,
  `delab` and similar.
- **Axiom gate** (`scripts/check-axioms.sh`) runs the layout check and the
  source scan, then checks every theorem listed in `Theorems.lean` in four
  layers:
  1. `Theorems.lean` must be a plain manifest: comments, `import` lines, then
     one `#print axioms NAME` per line, no duplicates, nothing else.
  2. `leanchecker` replays every declaration of the `Botho` library in the
     kernel, so a declaration admitted with kernel checking off fails.
  3. `lean --json Theorems.lean` must print only Lean messages and no errors.
     Exactly one result is accepted per manifest command, matched by source
     position and theorem name, so text printed elsewhere (by `#eval` or an
     imported initializer) cannot stand in for a result.
  4. `scripts/AxiomReport.lean` imports the compiled library without
     elaborating its syntax and computes each theorem's axioms itself. These
     must equal Lean's `#print axioms` results, which catches a model that
     redefines `#print axioms`. The independent result is checked against
     `axioms-allowlist.txt` and must not contain `sorryAx`. Like the sorry
     gate, it also fails if any library declaration reaches `sorryAx`.
- Both gates read their inputs (`axioms-allowlist.txt`, `Theorems.lean`,
  `scripts/AxiomReport.lean`) before the first `lake` call and use only those
  copies afterwards, and the axiom gate runs the independent walk before it
  compiles `Theorems.lean`. This keeps local runs honest against cruder
  tricks; it is not what the verdict rests on (see [Limits](#limits)).
- **Self-test** (`scripts/selftest-gates.sh`) copies the tree, injects a
  `sorry`, an `admit`, a `native_decide`, a bogus axiom, a `sorryAx` and an
  empty manifest, and asserts that the matching gate fails each time. It also
  checks that the clean tree passes and that the tokens are ignored inside
  comments, strings, raw strings and char literals. Since #1724 it also covers
  `sorry` hidden after a `'"'` char literal or an `r#"a"b"#` raw string, a
  `_root_.sorryAx` term, and primed theorem names in the axiom gate. Since
  #1770 it covers each known bypass of the in-tree gates, and checks that the
  gate fails for the intended reason: a fake `#print axioms` line printed by
  `#eval` or an imported initializer, `#print axioms` redefined by
  `macro_rules`, a theorem admitted with `debug.skipKernelTC`, a `sorry`
  hidden by `#guard_msgs`, `sorryAx.{0}`, `s!"{sorry}"`, duplicate manifest
  lines, a `lakefile.lean`, an `@[init]` initializer that rewrites
  `axioms-allowlist.txt` during the build, one that erases its own `@[init]`
  line, each forbidden attribute and metaprogramming command, a simproc that
  overwrites a compiled `.olean`, a bare `import Lean`, each import-header
  form, symlinked or stray sources, and layout changes (`srcDir`, an extra
  `lean_lib`, a `lean_exe`, `roots`, `globs`). Since the #1783 review it
  also covers a `[[require]]` dependency whose `lakefile.lean` rewrites a
  model source (its code must never run), a manifest that lists a package,
  each disallowed lakefile key and option, and (#1810) the `decide +native`
  split, `#guard` and prelude `#eval` `.olean` swaps, with `--no-scan`
  controls that show each swap fools the later layers, plus each banned IO
  name and evaluator. Passing controls cover a second module with quoted
  multi-line imports, a commented lakefile, the verify job's benign lakefile
  edit, allow-listed metadata and `leanOptions` in both TOML spellings, a
  tree without `lake-manifest.json`, names that merely contain a banned
  name, and the verify job's `--no-build` call with `LAKE=false`.
- **Verify self-test** (`scripts/selftest-verify.sh`) runs the verify job's
  checks against the fixtures in `scripts/fixtures/verify/`: the bypasses
  found in the reviews of PR #1783 (an `@[init]` that widens the allow-list,
  a simproc that swaps an `.olean`, a `srcDir` move with an initializer, and
  `decide +native` with an IO world-token cast), a pull request that edits
  its own allow-list, a module with an initializer, and an artifact that
  carries an `Init` module, a `srcDir` decoy that compiles a weakened copy
  of the model while the scanned `Botho/` stays unchanged, and (accepted) a
  benign lakefile edit. Each fixture is built the way the build job
  builds it, and the verifier must reject it with the clean allow-list, both
  with and without the source scan. The verifier reads a copy of the
  sources taken before the build, as the verify job reads its own checkout.
  Four `scan-only` fixtures leave an artifact that is an honest build of a
  different source, so only the base's scan or lakefile check can stop
  them: a `[[require]]` dependency whose `lakefile.lean` rewrites
  `Botho/Cheat.lean` before it is compiled, and (#1810) `decide +native`
  split across modules, `#guard`, and a prelude `#eval`, each overwriting
  compiled `.olean` files. For these the self-test shows both halves: the
  full verifier rejects them, and the checks without the scan accept the
  forged `one_eq_two`. The self-test also checks that no
  fixture initializer runs inside the verifier, and, as a control, that the
  same initializer does run when the library is imported through the Lean
  frontend. The clean tree is also verified with no `lean`, `lake` or `elan`
  on `PATH`, as in the verify job.

### The verify job

`scripts/verify-artifacts.sh` is the authority for the verdict. The verify job
checks out `formal/scripts/` (this script, `AxiomReport.lean`,
`check-sorry.sh`), `axioms-allowlist.txt` and `lakefile.toml` from the base
commit of the pull request (on a push, the pushed commit), never from the
pull request head. A pull request that edits these files changes nothing the
verify job reads until it is merged. It then:

1. checks that the pull request's `lakefile.toml` compiles the same sources
   as the base's. Both are parsed with Python's `tomllib`, and the
   package-level `srcDir` and, for every `[[lean_lib]]` and `[[lean_exe]]`,
   `name`, `srcDir`, `roots`, `globs` and `root` must be identical (so the
   set of libraries and executables is too). Which other keys may change is
   decided in step 2 by the base's key allow-list (no `[[require]]`, no
   code-loading keys; see Build configuration above). A `lakefile.lean` on either
   side, or a `lakefile.toml` that does not parse, is rejected. Without this
   a pull request could leave the scanned `Botho/` as a decoy and compile a
   different copy (`srcDir = "src"`) whose theorems state something else;
2. runs the base commit's source scan (`check-sorry.sh --no-build`: the
   build-layout check and `gate_scan.py` described above) over the pull
   request's sources, read as data;
3. takes in the artifact: only regular files named `Botho.olean` or
   `Botho/**/*.olean` (and the `.olean.server`, `.olean.private` and `.ir`
   parts), copied to a private directory that is the only entry on
   `LEAN_PATH`, so the artifact cannot shadow a toolchain module;
4. replays every `Botho` declaration in the kernel with `leanchecker`;
5. walks the compiled library with `AxiomReport.lean`, and fails if a
   theorem listed in `Theorems.lean` (parsed as text, never compiled) is
   missing or depends on an axiom outside the base allow-list or on
   `sorryAx`; if any library declaration reaches `sorryAx`; if a library
   module declares an axiom outside the base allow-list, imports anything
   but `Init` and `Botho` modules, or declares an `[init]` initializer; or if
   the compiled modules differ from the source modules.

It never runs `lake`, never compiles a model source, installs the toolchain
fresh (an official `leanprover/lean4` release only) and uses no cache.

## Limits

**What the trust boundary guarantees.** No model code runs in the verify job,
so model code cannot change the verify job's inputs (the gate scripts, the
allow-list, the report program) or its output. This closes the bypasses
found in the reviews of #1783: each one ran model code during `lake build` to
rewrite something the gate read in the same job.

**Why no model code runs there.** `leanchecker` and `AxiomReport.lean` load the
`.olean` files with `importModules (loadExts := false)`. Lean runs `[init]`
and `initialize` declarations only from `finalizePersistentExtensions`, which
only `loadExts := true` calls (`finalizeImport` in `Lean/Environment.lean`,
v4.35.0). The Lean frontend does load extensions: elaborating any file that
imports the model (`import Botho`, `lean Theorems.lean`) runs the model's
initializers. So the verify job never passes a model file, or a file that
imports the model, to the frontend, and it does not compile `Theorems.lean`
(the build job's axiom gate does; the verify job parses it as text).
`AxiomReport.lean` itself is elaborated, but it imports only `Lean` and loads
the model at run time. `selftest-verify.sh` checks this with a canary
initializer and the frontend control described above.

**What it does not guarantee: the artifact matches the sources.** The build
job is untrusted, so the `.olean` files it uploads are whatever model code
left on disk. The verify job proves that the uploaded library is
kernel-valid and that the listed theorems, as compiled, depend only on
allowed axioms. It does not prove that those theorems state what the
sources say. The `simproc-swap` and `native-world-swap` fixtures replace
`one_eq_two : 1 = 2` with `one_eq_two : True` in the compiled library, and
the later checks reject them only because of other evidence they leave
behind (an `import Lean`, a declared axiom). A build-time attack that also
rewrites those modules leaves a library that passes every artifact check:
the `native-split-swap`, `guard-swap`, `eval-swap` and `require-dep`
fixtures do exactly that. Two things stand in the way of such an attack:

- the base commit's source scan, which the pull request cannot weaken, has to
  miss every way the attack runs code during the build; and
- review: the sources of such a pull request contain both the false proof
  and the code that runs during the build.

Statement substitution through build configuration alone, with no code run
during the build, is closed by step 1: Lake compiles only the sources the
base's layout names, and those are the sources the scan reads and the
intake's module-set check compares (the `lakefile-decoy` fixture; it passes
every other check). That check covers the layout keys of Lake v4.35.0's
TOML configuration. Build configuration that loads or runs code is closed
by the base's key allow-list in step 2. A `[[require]]` dependency's
`lakefile.lean` is a Lean program that Lake elaborates, `#eval` included, on
every `lake` call, and its source is never scanned, so it could rewrite a
model source before Lake compiles it and leave an artifact that is an
honest build of the rewritten source (the `require-dep` fixture; no check
on the artifact can tell). The allow-list rejects `[[require]]`, a manifest
that lists a package, `moreLeanArgs`, `weakLeanArgs`, `plugins`, `dynlibs`,
`precompileModules`, `extraDepTargets`, any unknown key and any
`leanOptions` entry outside a short list. Binding each theorem's compiled statement
to a reviewed record is tracked in #1805.

Binding the artifact to the sources needs a build that runs no model code,
which Lean does not offer (macros, `decide +native` and initializers all run
user code during elaboration), or a rebuild under a sandbox that confines
each module's elaboration to its own output.

**The in-tree gates.** In the build job, model code runs during
`lake build`, and from then on nothing that reads `.lake/` can be trusted:
build-time code can replace a module's `.olean` with a forged one, and then
`leanchecker`, the walk and `#print axioms` in that job all check the forged
output. The in-tree gates' snapshots, kernel replay and walk only defend
against cruder tricks. Their source scan is what carries over to the verify
job: there it runs from the base commit and decides, before any of the
artifact checks, whether the sources may run code at build time at all.
Without `import Lean` a model cannot define a simproc, elaborator, tactic or
`deriving` handler, so what is left is the prelude. The scan covers it in
two layers (#1810). First, it bans the evaluators that run compiled code
while a module is elaborated: `#eval`, `#guard` (which evaluates its term
with the compiler and needs no `native` token), `native` in every spelling
(`native_decide`, `decide +native`, `native := true`), `Lean.reduceBool`
and its relatives, and initializers. Second, it bans the prelude IO surface
(`IO`, `EIO`, `BaseIO`, `ST`, `EST`, `EStateM`, `RealWorld`, `Void`,
`System`, `FilePath`, `unsafe*`), so an evaluator missing from the first
list has no named effect left to run: the world-token trick (`cast` through
`axiom : Void Unit = Void IO.RealWorld`) and every file or process call are
spelled with these names. Both layers are denylists, and a denylist can
miss routes. A pure model needs none of these names, so the lists can stay
broad. Hence the review rule below.

**Other limits.**

- A human must review every change to the model (`Botho.lean`, `Botho/`,
  `Theorems.lean`, `lakefile.toml`, `lean-toolchain`, `lake-manifest.json`,
  `axioms-allowlist.txt`) for IO and metaprogramming, with the same care as
  a change to `formal/scripts/`. A green `formal-gate` does not replace that
  review.

- The `.olean` format is a memory image that Lean maps without validating
  it. A hand-crafted `.olean` that corrupts the memory of `leanchecker` or of
  `AxiomReport.lean` is out of scope. The verify job holds no secrets and a
  read-only token.
- The workflow file comes from the pull request, as it does for every
  `pull_request` workflow. A pull request can edit `formal.yml` itself. Such
  an edit, and any edit under `formal/scripts/`, is visible in review and
  needs a human reviewer. (The repository has no CODEOWNERS file yet; one
  that covers `formal/scripts/`, `formal/axioms-allowlist.txt` and
  `.github/workflows/formal.yml` would enforce this.)
- Until verify-artifacts.sh is on the base branch (the pull request that
  adds it, #1799), the verify job runs the pull request's own copy and
  prints a warning.
- The pull request picks the toolchain version, so a toolchain bump is
  verified with the bumped toolchain. Only official `leanprover/lean4`
  releases are accepted.
- `leanchecker` trusts the toolchain's own library (`Init`). The artifact
  cannot replace it (step 3 above), and it is loaded from `--sysroot` through
  `LEAN_SYSROOT`, never from a `lean` found on `PATH`.
- The artifact download relies on `actions/download-artifact` (v4.1.3 and
  later) to refuse archive entries outside the target directory (zip-slip).
  Intake then accepts only regular files with library names.

## Adding a theorem

1. Prove it in a module under `Botho/` that `Botho.lean` imports. Do not
   declare anything in `Theorems.lean`; the axiom gate rejects it.
2. Add one line to `Theorems.lean`:

   ```lean
   #print axioms Botho.my_theorem
   ```

   Group lines under a comment naming the module and its label (proven /
   tested / trusted).
3. Run `scripts/check-axioms.sh`. If it reports a non-allow-listed axiom,
   either remove the dependency or follow the next section.

## Adding an allow-listed opaque axiom

Some primitives (hashes, RandomX) cannot be modeled usefully. They enter as
opaque `axiom` declarations, for example
`axiom Botho.sha256 : ByteArray → ByteArray`, and theorems may depend on them.
To add one:

1. Declare the axiom in the model module with a docstring that says what it
   stands for and which Rust function it abstracts.
2. Add its fully-qualified name to `axioms-allowlist.txt` with a one-line
   justification comment on the line above it:

   ```text
   # SHA-256 as used by botho/src/block.rs; modeled as an opaque function.
   Botho.sha256
   ```

3. Label every theorem that depends on it **trusted** and name the axiom.

The verify job reads the allow-list from the base commit, so a new axiom
fails the verify job until its allow-list entry is on `main`. Land the
allow-list line (with the axiom's justification) in its own pull request
first, then the model that declares and uses it.

Only opaque functions belong on the allow-list. Never add an axiom that states
a property (for example "sha256 is injective") without a design discussion on
the relevant issue. Such an axiom can make the model inconsistent.
