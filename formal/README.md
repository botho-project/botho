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
```

The scripts take an optional `LAKE=/path/to/lake` environment variable if the
`lake` on your `PATH` is not the elan proxy.

The toolchain is pinned to an exact stable release in `lean-toolchain`. Bump it
deliberately in its own PR. Never use `stable` or `nightly`. The project has no
dependencies (no Mathlib). Whether to add Mathlib is decided in #1708, because
it adds substantial CI time.

## Gates

CI (`.github/workflows/formal.yml`) runs `lake build` and then three scripts.
The required check is `formal-gate`. It passes when the Lean job succeeded, or
when the Lean job was skipped because nothing under `formal/`, the workflow, or
a mirrored Rust file changed.

- **Sorry gate** (`scripts/check-sorry.sh`) fails if `lake build` warns that a
  declaration uses `sorry`. It also fails if the source scan finds `sorry`,
  `sorryAx`, `admit` or `native_decide` outside comments and strings.
  `native_decide` trusts the compiler, so it is allowed only on a line marked
  `-- formal-gate: allow native_decide`, and the compiler-trust axioms it
  introduces (such as `Lean.ofReduceBool`) must also be on the allow-list.
- **Axiom gate** (`scripts/check-axioms.sh`) compiles `Theorems.lean` and
  parses every `#print axioms` result. It fails on `sorryAx`, on any axiom not
  in `axioms-allowlist.txt`, on an empty manifest, or when the number of
  results does not match the number of `#print axioms` lines.
- **Self-test** (`scripts/selftest-gates.sh`) copies the tree, injects a
  `sorry`, an `admit`, a `native_decide`, a bogus axiom, a `sorryAx` and an
  empty manifest, and asserts that the matching gate fails each time. It also
  checks that the clean tree passes and that the tokens are ignored inside
  comments and strings.

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

Only opaque functions belong on the allow-list. Never add an axiom that states
a property (for example "sha256 is injective") without a design discussion on
the relevant issue. Such an axiom can make the model inconsistent.
