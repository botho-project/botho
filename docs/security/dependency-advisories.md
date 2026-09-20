# September 2026 dependency advisory remediation

Tracking: [issue #1254](https://github.com/botho-project/botho/issues/1254) and
[issue #1261](https://github.com/botho-project/botho/issues/1261).

The all-features Security gate is `cargo deny --all-features check`.
The September 19 baseline reports three additional advisories:

- [RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258):
  h2 accepts unbounded empty DATA frames. The old path was `botho ->
  opentelemetry-otlp 0.14 -> tonic 0.9 -> hyper 0.14 -> h2 0.3.27`.
  OTLP 0.17 moves this path to tonic 0.12 / hyper 1, allowing the shared h2
  dependency to use patched version 0.4.16. This is the node's outbound
  telemetry exporter; the old h2 branch has no other workspace consumers.
- [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285):
  rustls incorrectly accepts TLS handshake messages across encryption-level
  boundaries. Version 0.23.45 fixes this for the node's direct TLS dependency
  and its shared reqwest, WebRTC, and other TLS consumers.
- [RUSTSEC-2026-0283](https://rustsec.org/advisories/RUSTSEC-2026-0283):
  clear_on_drop is unmaintained. The exact pinned Bulletproofs fork is now
  vendored with its secret cleanup migrated to zeroize. No policy exception
  was added, and the all-features Security gate passes with these changes.

## Pinned Bulletproofs cleanup migration

The original `cargo tree --locked --all-features -i clear_on_drop` identified
only this introducing dependency:

```text
clear_on_drop 0.2.5
└── bulletproofs-og 3.0.0-pre.1 (9abfdc054d9ba65f1e185ea1e6eff3947ce879dc)
    └── bth-transaction-core
```

Transaction core is used by the transaction signer and SCP playground, and
is also a dev-dependency of the node. This is cryptographic secret cleanup,
not merely a build-time dependency: the fork imports `clear_on_drop::clear::Clear`
in `src/util.rs`, `src/range_proof/party.rs`, and `src/r1cs/prover.rs`.

As checked on September 19, 2026, the pinned revision is still the tip of
the upstream `sam/fix` branch. The upstream default branch also uses
clear_on_drop and is an older, incompatible package. RustSec lists no
patched clear_on_drop release and recommends zeroize instead.

The local migration preserves the pinned proof algorithms, encodings,
transcripts, and generators. All old cleanup sites, including optional R1CS
witness vectors, now overwrite their scalar/value storage with zeroize.
Deterministic fixtures captured from the unmodified fork prove byte-for-byte
compatibility for individual, aggregated, and padded range proofs through
transaction-core. Original proofs still verify; invalid proofs are rejected.
See the [exact upstream diff, cleanup inventory, licenses, and validation
record](../../vendor/bulletproofs-og/BOTHO-PATCH.md).

`cargo tree --locked --all-features -i clear_on_drop` now reports that the
package is absent. `cargo deny --all-features check` passes advisories, bans,
licenses, and sources on the combined HTTP/TLS and cleanup branch. This does
not replace independent review or external cryptographic audit #616.

After this reviewed baseline merges, rerun or rebase the blocked dependency
PRs (#1239, #1243, #1248, and #1251); their security gates must not be bypassed.
