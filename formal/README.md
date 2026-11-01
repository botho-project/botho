# formal/ - Lean 4 models and proofs

This directory holds Lean 4 models of selected Botho mechanisms, proofs about
those models, and the gates that keep the proofs honest. It is the scaffold for
epic #1704, set up by #1705. It currently contains one toy model
(`Botho/Basic.lean`) that mirrors no Rust code.

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
   The DRT harness lands with the first real model (a child issue of #1704).
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
scripts/check-sorry.sh          # sorry gate
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
with `--sysroot` it needs no `lean` on `PATH` at all. It also needs a
`python3` with `tomllib` (3.11 or later).

The scripts take an optional `LAKE=/path/to/lake` environment variable if the
`lake` on your `PATH` is not the elan proxy.

The toolchain is pinned to an exact stable release in `lean-toolchain`. Bump it
deliberately in its own PR. Never use `stable` or `nightly`. The project has no
dependencies (no Mathlib). Whether to add Mathlib is decided in #1708, because
it adds substantial CI time.

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

The in-tree gates:

- **Sorry gate** (`scripts/check-sorry.sh`) fails if `lake build` warns that a
  declaration uses `sorry`. It also fails if the source scan finds `sorry`,
  `sorryAx`, `admit` or `native_decide` (bare or `_root_.`-qualified) outside
  comments, string literals, raw strings (`r#"..."#`) and char literals.
  `native_decide` trusts the compiler, so it is allowed only on a line marked
  `-- formal-gate: allow native_decide`, and the compiler-trust axioms it
  introduces (such as `Lean.ofReduceBool`) must also be on the allow-list.
- **Axiom gate** (`scripts/check-axioms.sh`) compiles `Theorems.lean` and
  parses every `#print axioms` result. It fails on `sorryAx`, on any axiom not
  in `axioms-allowlist.txt`, on an empty manifest, or when the number of
  results does not match the number of `#print axioms` lines. It anchors on
  Lean's full result-line format, so primed names like `foo'` parse.
- **Self-test** (`scripts/selftest-gates.sh`) copies the tree, injects a
  `sorry`, an `admit`, a `native_decide`, a bogus axiom, a `sorryAx` and an
  empty manifest, and asserts that the matching gate fails each time. It also
  checks that the clean tree passes and that the tokens are ignored inside
  comments, strings, raw strings and char literals. Since #1724 it also covers
  `sorry` hidden after a `'"'` char literal or an `r#"a"b"#` raw string, a
  `_root_.sorryAx` term, and primed theorem names in the axiom gate.
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
  with and without the source scan. The self-test also checks that no
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
   set of libraries and executables is too). Other keys, such as `version`,
   options or a `[[require]]` pin, may change. A `lakefile.lean` on either
   side, or a `lakefile.toml` that does not parse, is rejected. Without this
   a pull request could leave the scanned `Botho/` as a decoy and compile a
   different copy (`srcDir = "src"`) whose theorems state something else;
2. runs the base commit's source scan (`check-sorry.sh --no-build`) over the
   pull request's sources, read as data;
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
`one_eq_two : 1 = 2` with `one_eq_two : True` in the compiled library. The
verify job rejects them only because of other evidence they leave behind
(an `import Lean`, a declared axiom). A build-time attack that also rewrote
those modules would leave a library that passes. Two things stand in the
way of such an attack:

- the base commit's source scan, which the pull request cannot weaken, has to
  miss every way the attack runs code during the build; and
- review: the sources of such a pull request contain both the false proof
  and the code that runs during the build.

Statement substitution through build configuration alone, with no code run
during the build, is closed by step 1: Lake compiles only the sources the
base's layout names, and those are the sources the scan reads and the
intake's module-set check compares (the `lakefile-decoy` fixture; it passes
every other check). That check covers the layout keys of Lake v4.35.0's
TOML configuration. Other build configuration, such as `[[require]]`
dependencies, `leanOptions` or `moreLeanArgs`, can still change and is
reviewed like any other diff; the base's source scan and the artifact checks
still apply to what it produces. Binding each theorem's compiled statement
to a reviewed record is tracked in #1805.

Binding the artifact to the sources needs a build that runs no model code,
which Lean does not offer (macros, `decide +native` and initializers all run
user code during elaboration), or a rebuild under a sandbox that confines
each module's elaboration to its own output.

**Other limits.**

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

1. Prove it in a module under `Botho/` that `Botho.lean` imports.
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
