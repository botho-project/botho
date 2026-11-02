# formal-aeneas/ - Rust to Lean extraction (Charon + Aeneas)

Generated Lean for the cluster-tax kernels that Charon and Aeneas accept
unchanged, with no axioms in the output (#1712, epic #1704). The spike memo
with per-kernel results and the go/no-go recommendation is
[`formal/aeneas/SPIKE.md`](../formal/aeneas/SPIKE.md).

- `toolchain.env`: pinned Aeneas release (with asset sha256 hashes) and the
  Rust nightly that charon-driver needs.
- `regen.sh`: regenerates `generated/`. Use `regen.sh --check` to compare
  instead of overwrite. Its header lists the toolchain prerequisites.
- `generated/`: output. Do not edit it by hand.

This directory is deliberately outside `formal/`. The generated Lean imports
the Aeneas library, which depends on Mathlib, and `formal/`'s gates allow
neither. It is not part of the `formal-gate` verification. CI
(`.github/workflows/aeneas-extract.yml`) checks only that `generated/` is up to
date with the Rust sources.

If you edit a mirrored Rust kernel (see `start_from` in `regen.sh`), run
`formal-aeneas/regen.sh` and commit the regenerated file in the same PR.
