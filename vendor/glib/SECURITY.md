# GLib string iterator safety backport

This is the published MIT-licensed glib 0.18.5 crate, with the two-line
upstream fix from gtk-rs/gtk-rs-core#1343 for RUSTSEC-2024-0429.
`VariantStrIter` now gives `g_variant_get_child` a mutable output pointer.
Writing through the original shared reference was undefined behavior and
reproduces a segmentation fault in an optimized build.

The parent Tauri dependency graph requires this GLib API version. This
path override preserves it and all unrelated package records. The license
and upstream tests are retained; SECURITY-PROVENANCE.json records archive
and original-file hashes. No advisory ignore is added. The version-based
unsoundness warning remains and is accompanied by this source evidence.

Five added regressions cover empty and Unicode strings, forward/backward
iteration, skips, mixed directions, bounds, and borrowed string lifetime.

An unrelated existing upstream test, `test_into_strv`, indexed a four-item
Rust slice at index four with `get_unchecked` to inspect the allocated NULL
sentinel. The sentinel assertion now reads the raw pointer at `s.len()`,
consistent with `IntoStrV::run_with_strv`'s NULL-terminated allocation
contract, without forming an out-of-bounds reference. This is test-only.
The corrected test passes on the original crate while the original
security regression still crashes; the production backport then passes
all 226 library tests and all five security regressions. No test is skipped.

```sh
cargo test --manifest-path vendor/glib/Cargo.toml --release --locked --lib
cargo test --manifest-path vendor/glib/Cargo.toml --release --locked --test security_variant_iter
```

The standalone Cargo.lock makes these checks reproducible. CI runs them
with native GLib development libraries. Source integration does not prove
coverage of installed application binaries; rebuild and deployment remain
separate. Remove this override when the parent can use a maintained fixed
GLib release.
