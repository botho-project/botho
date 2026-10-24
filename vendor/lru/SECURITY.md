# LRU panic-safety backport

This is the published MIT-licensed lru 0.16.4 crate, with the fix from
[jeromefroe/lru-rs#238](https://github.com/jeromefroe/lru-rs/pull/238) applied
to `LruCache::pop`. The node is detached before its key destructor can panic,
so catching that panic cannot leave a freed node linked into the cache.
The original license and all upstream tests are retained.

Botho currently uses Alloy 1.8.3, whose provider requires lru 0.16. A direct
upgrade to the fixed lru 0.18.2 release does not satisfy that requirement.
This path override preserves the dependency API and all other package pins.
`SECURITY-PROVENANCE.json` binds the published archive and every original file.
The implementation diff is the exact upstream fix; the Cargo manifest only
adds an independent test workspace.

The regression covers singleton, head, middle and tail removal, traversal in
both directions, refill/eviction and clear after a caught stored-key panic.
It fails with undefined behavior on the original crate under Miri.

```sh
cargo test --manifest-path vendor/lru/Cargo.toml --locked
cargo test --manifest-path vendor/lru/Cargo.toml --locked --no-default-features
cargo +nightly-2026-10-23 miri test --manifest-path vendor/lru/Cargo.toml --locked --test pop_panic_safety
cargo +nightly-2026-10-23 miri test --manifest-path vendor/lru/Cargo.toml --locked --test pop_panic_safety --no-default-features
```

The version remains 0.16.4. A version-based RUSTSEC-2026-0253 warning may
remain; source provenance, the upstream patch and the regression establish
this mitigation. No advisory ignore is added. Remove the override when the
parent dependency accepts a maintained fixed release. Runtime rebuilding
and deployment must be verified separately.
