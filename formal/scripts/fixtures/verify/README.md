# Verify-job fixtures (#1799)

Each directory is one pull request that tries to change the verdict of the
trusted `verify` job (`../../verify-artifacts.sh`). `../../selftest-verify.sh`
applies each fixture to a copy of the clean `formal/` tree, builds it the way
the untrusted `build` job does (running the fixture's code), keeps only the
files the workflow uploads, and runs the verifier with the clean allow-list,
as the base branch would. Each fixture must fail for the reason in
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

Fixture files:

- `Botho/*.lean`: copied into the tree.
- `imports.txt`: lines appended to `Botho.lean`.
- `manifest.txt`: lines appended to `Theorems.lean`.
- `allowlist.txt`: lines appended to the tree's (pull request side)
  `axioms-allowlist.txt`. The verifier never reads that copy.
- `srcdir`: if present, the model moves to `src/` and the lakefile gets
  `srcDir = "src"`.
- `artifact-extra.txt`: paths added to the uploaded artifact (copies of
  `Botho.olean`), as a build job that uploads extra files would.
- `expect.txt`: an extended regex that the verifier's output must match.

These files are test inputs, not part of the model: `lake build` of
`formal/` does not compile them, and the gates do not scan `scripts/`.
