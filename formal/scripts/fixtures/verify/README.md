# Verify-job fixtures (#1799)

Each directory except `lakefile-benign` (a legitimate edit that must pass)
is one pull request that tries to change the verdict of the trusted `verify`
job (`../../verify-artifacts.sh`). `../../selftest-verify.sh`
applies each fixture to a copy of the clean `formal/` tree, builds it the way
the untrusted `build` job does (running the fixture's code), keeps only the
files the workflow uploads, and runs the verifier with the clean allow-list,
lakefile and statement record (`formal/statements.txt`, plus the fixture's
`record.txt`), as the base branch would. Each fixture must fail for the reason in
`expect.txt`, and none of its code may run inside the verifier.

The first four rows reproduce the bypasses found in the reviews of PR #1783:

| Fixture | Bypass of the in-job gates |
|---------|----------------------------|
| `init-allowlist` | an `@[init]` initializer appends `Botho.cheat` to the allow-list |
| `simproc-swap` | a simproc overwrites `Botho/Cheat.olean` with a harmless one during `lake build` |
| `srcdir-init` | `srcDir = "src"` moves the sources out of the scan; an initializer writes `PWNED_INIT` |
| `native-world-swap` | `decide +native` plus an IO world-token cast runs the same olean swap without `import Lean` |
| `allowlist-edit` | the pull request adds `Botho.cheat` to its own `axioms-allowlist.txt` |
| `init-only` | a module whose only content is an `[init]` initializer |
| `artifact-shadow` | the uploaded artifact carries an `Init/Prelude.olean` to shadow the toolchain's |
| `lakefile-decoy` | `srcDir = "src"` compiles a copy whose `satAdd_le_cap` states `True`, while the scanned `Botho/` stays unchanged; no code runs during the build |
| `lakefile-benign` | (must pass) a lakefile edit that keeps the layout: a version bump and a `leanOptions` table |
| `require-dep` | (scan-only) a `[[require]]` path dependency whose `lakefile.lean` `#eval` rewrites `Botho/Cheat.lean` before Lake compiles it; the committed manifest lists it (#1783 review) |
| `native-split-swap` | (scan-only) `decide +native` in an `example` in a module with no axiom; its IO overwrites `Cheat.olean` and the `Tidy.olean` that holds the world-token axiom (#1810 variant A) |
| `guard-swap` | (scan-only) the same with `#guard`, which evaluates with the compiler and has no `native` token (#1810 variant B) |
| `eval-swap` | (scan-only) a prelude `#eval` that overwrites `Cheat.olean`, with no axiom and no `import Lean` |
| `statement-swap` | (statement-only, #1805) the artifact is compiled from a copy in which `satAdd_le_cap` states `True`, while the scanned sources are unchanged: the swap a detached build-time process could leave, with no other trace |
| `definition-swap` | (statement-only, #1805) the same with the body of `Botho.satAdd` changed (`min (b + a) cap`); every theorem's type is unchanged |
| `statement-missing` | (#1805) an honest new theorem with no block in the base statement record |
| `record-pending` | (must pass, #1805) the base record holds a second, pending block for `satAdd_le_cap` |
| `implicit-type-swap` | (statement-only, #1824) the record binds `implicit_zero : (0 : Nat) + 0 = 0`; the artifact is compiled from `(0 : Fin 2) + 0 = 0`. Only an implicit type argument differs: the pre-#1824 printer showed both as `Eq (instHAdd.hAdd 0 0) 0`. The statement check must report that the readable text differs, and print the compiled block with `Fin` in it (`statement.txt`) |
| `manifest-drop` | (statement-only, #1824) the pull request deletes `#print axioms Botho.satAdd_eq_add` from `Theorems.lean`, which the base manifest lists and the base record binds |
| `manifest-drop-retired` | (must pass, #1824) the same deletion, with `retired Botho.satAdd_eq_add` in the base record |
| `retired-swap` | (statement-only, #1824) `retired Botho.satAdd_le_cap` in the base record, while the theorem stays listed and the artifact states `True`: retiring does not unbind a listed theorem |

A `scan-only` fixture leaves an artifact that is an honest build of a
different source, so no check on the artifact alone can reject it: only
the base branch's source scan or lakefile check (check 2), or, since #1805,
the statement check against the base record (check 6). For these the
verifier must fail with the full checks (matching `expect.txt`), must fail
without the scan for the statement check's reason (matching
`statement.txt`), and must pass with neither the scan nor the statement
check (matching the regex in `scan-only`), which shows the forgery is
real.

A `statement-only` fixture (#1805) passes every check but the statement
check, the scan included: the verifier must fail with the full checks
(matching `expect.txt`) and pass with `--no-statements` (matching the regex
in `statement-only`).

The fixtures that add `one_eq_two` carry a `record.txt` with the block of
the claimed `one_eq_two : 1 = 2`, as if a reviewer had accepted the claim
on the base branch. So the statement check compares the artifact with the
claim, not with an empty record. `simproc-swap` and `native-world-swap` also
carry a `statement.txt`: besides the evidence they leave behind (an
`import Lean`, a declared axiom), the statement check rejects them for the
swapped statement.

Fixture files:

- `Botho/*.lean`: copied into the tree.
- `imports.txt`: lines appended to `Botho.lean`.
- `manifest.txt`: lines appended to `Theorems.lean`.
- `manifest-drop.txt`: theorem names whose `#print axioms` line is deleted
  from the pull request's `Theorems.lean`. The base manifest is always the
  clean `Theorems.lean`.
- `allowlist.txt`: lines appended to the tree's (pull request side)
  `axioms-allowlist.txt`. The verifier never reads that copy.
- `srcdir`: if present, the model moves to `src/` and the lakefile gets
  `srcDir = "src"`.
- `decoy.txt`: lines `FILE NAME`. Copies `Botho.lean` and `Botho/` to `src/`
  and, in that copy only, replaces theorem `NAME` in `FILE` (up to the next
  blank line) with `theorem NAME : True := trivial`.
- `lakefile.toml`: replaces the tree's `lakefile.toml`.
- `lakefile-append.txt`: lines appended to the tree's `lakefile.toml`.
- `lake-manifest.json`: replaces the tree's `lake-manifest.json`.
- `dep/`: copied next to the tree, as `../dep` (a `[[require]]` path
  dependency).
- `scan-only`: a regex. The fixture must pass the checks without the scan
  and match it (see above); the full checks must fail and match
  `expect.txt`.
- `artifact-extra.txt`: paths added to the uploaded artifact (copies of
  `Botho.olean`), as a build job that uploads extra files would.
- `forged.txt`: lines `FILE NAME`. After the verifier's copy of the sources
  is taken, replaces theorem `NAME` in `FILE` (up to the next blank line)
  with `theorem NAME : True := trivial` in the tree that is built only.
- `forged-replace.txt`: lines `FILE<TAB>OLD<TAB>NEW`, the same for a text
  replacement (`OLD` must occur once).
- `record.txt`: blocks (and `retired` lines) appended to the base statement
  record.
- `statement.txt`: a regex the run without the scan must also match: the
  statement check's own reason.
- `statement-only`: a regex. The fixture must pass with `--no-statements`
  and match it (see above).
- `expect.txt`: an extended regex that the verifier's output must match.
- `pass`: if present, the fixture must pass instead (no `expect.txt`).
- `expect-same-layout.txt`: if present, the verifier also runs (without the
  scan) with the fixture's own `lakefile.toml` as the base's, so the layout
  check passes, and its output must match this regex. It shows what the
  fixture gets past when the layout check is not what stops it.

These files are test inputs, not part of the model: `lake build` of
`formal/` does not compile them, and the gates do not scan `scripts/`.
