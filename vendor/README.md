# vendor/

In-tree copies of third-party crates that Botho patches. Each one is wired in
through `[patch.crates-io]` in the workspace `Cargo.toml`, so every dependent
crate in the build uses the patched copy instead of the crates.io release.

| Crate | Version | Why it is vendored | Provenance |
|---|---|---|---|
| `bulletproofs-og` | 3.0.0-pre.1 | Removes the dependency on unmaintained `clear_on_drop` (RUSTSEC-2026-0283, #1261). Proof algorithms, transcripts, generators and encodings are unchanged. | [`BOTHO-PATCH.md`](bulletproofs-og/BOTHO-PATCH.md), `botho-zeroize.patch` |
| `glib` | 0.18.5 | Backports the upstream iterator fix for RUSTSEC-2024-0429 while keeping desktop compatibility (#1541). | `Cargo.toml.orig` |
| `lru` | 0.16.4 | Backports the upstream `pop` panic-safety fix without changing Alloy 1.x requirements (#1539). | [`SECURITY-PROVENANCE.json`](lru/SECURITY-PROVENANCE.json) (archive URL and SHA-256) |
| `randomx-rs` | 1.4.1 | Upstream's `build.rs` hardcodes a host-specific CPU target. This copy defaults to the portable `DARCH=default` and honours a `RANDOMX_ARCH` override, for reproducible, hermetic builds (#640). | [`README.md`](randomx-rs/README.md), `Cargo.toml.orig` |

`bulletproofs-og` is also a workspace member, so its tests run with the
workspace.

## Changing a vendored crate

- Keep the upstream license and attribution files.
- Record the exact upstream source (crate archive or commit) and what was
  changed, next to the crate.
- Update the reason in the workspace `Cargo.toml` comment and in this table.
- Drop the copy and its `[patch.crates-io]` entry once upstream ships the fix in
  a release we can use.
