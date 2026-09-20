# Bounded bigint codecs for Solana tooling

This private package replaces the four-function `bigint-buffer` API used by
`@solana/buffer-layout-utils@0.2.0` and `0.3.0`. It uses JavaScript integer operations and has
no native addon, native loader, lifecycle script, or WASM implementation. The
Node entry explicitly imports Node's Buffer. The browser entry explicitly imports
the declared `buffer@6.0.3` polyfill; neither assumes a global Buffer exists.

`toBigIntLE`/`toBigIntBE` accept a Buffer of 0–1024 bytes and return an unsigned
bigint. `toBufferLE`/`toBufferBE` require an unsigned bigint, an integer width of
0–1024 bytes, and a value that fits exactly. Encoding zero in zero bytes returns
an empty Buffer. Invalid types throw TypeError; invalid bounds throw RangeError.
Decoding never mutates input. SPL's audited codecs use 8, 16, 24, and 32 bytes;
the 1024-byte ceiling bounds allocation and conversion work for general callers.

These intentional stricter rules reject upstream coercions, negative values,
and overflow/truncation. They are not a general promise of compatibility with
invalid upstream inputs. Anchor's signed i128 codec remains the separate BN
implementation with two's-complement encoding.

## Provenance and validation

The API and compatibility behavior were inspected against
[no2chem/bigint-buffer 1.1.5](https://github.com/no2chem/bigint-buffer), an
Apache-2.0 package by Michael Wei. The implementation here is a small local
rewrite; the upstream Apache license is retained in LICENSE. No native code,
bindings loader, or upstream install script is included. This is not an upstream
patched release; [GHSA-3gc7-fjrx-p6mg](https://github.com/advisories/GHSA-3gc7-fjrx-p6mg)
reports no patched upstream release.

`../../tests-tooling/fixtures/bigint-buffer-1.1.5.json` contains valid-input
outputs captured before replacement and compared between the installed native
addon and upstream browser fallback. It records SHA-256 hashes of both source
entry files. The checked-in tests additionally compare deterministic boundary
and generated vectors to independent BN encoding; they exercise both entries,
invalid inputs, SPL layouts/mint/account data, and Anchor i128 limits. Known
invalid-input native crash paths were not executed to capture fixtures.

The runtime dependency is aliased as `bigint-buffer` and selected through a
parent-scoped npm override referencing that direct dependency. This avoids
npm10's broken frozen-install resolution of parent-only relative file overrides.
`npm ci --omit=dev` must work and retain the adapter. Audit results alone do not
validate this local code: changing its package identity is not a security fix
without the implementation review and tests.

Run `npm run test:tooling` from `contracts/solana`. These Node/polyfill tests do
not execute browser UI, Rust/WASM operators, or on-chain transactions. Existing
Anchor/local-validator tests remain the on-chain regression gate.
