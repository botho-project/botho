# Protocol and privacy integration coverage

The Linux `workspace-build.yml` PR job executes these eight Botho integration
targets after compiling the workspace tests. This closes #1272's execution gap:
`cargo test --workspace --no-run` checked compilation but never exercised their
assertions. The job reuses its existing build and gives the executing step ten
minutes, including any Cargo feature-resolution rebuild. No ignored tests are
enabled, and no external relay or public STUN service is provisioned.

Run the same explicit target list from the repository root:

```bash
cargo test --locked -p botho \
  --test circuit_handshake_integration \
  --test relay_handler_integration \
  --test onion_broadcast_integration \
  --test ice_stun_integration \
  --test signaling_integration \
  --test transport_negotiation_integration \
  --test traffic_indistinguishability \
  --test privacy_integration
```

## Measured coverage

All **117 tests passed**, with none ignored or filtered, on macOS using the
repository's `nightly-2025-12-03` toolchain in September 2026. Each target was run
separately with a two-minute cap so one failure could not hide the other results.
The table gives test-harness execution times after compilation, not Cargo startup
or clean-build time; `0.00s` results are shown as less than 0.01 seconds.

| Target | Tests | Test time | What executes / environmental requirements |
| --- | ---: | ---: | --- |
| `circuit_handshake_integration` | 12 | 0.08 s | In-process cryptographic circuit construction and handshake failure cases |
| `relay_handler_integration` | 9 | 1.11 s | Local multi-hop forwarding, authentication failures, rate limits and key cleanup; includes a 1.1-second expiry sleep |
| `onion_broadcast_integration` | 11 | <0.01 s | Local circuit pools, onion wrapping and broadcast selection |
| `ice_stun_integration` | 19 | <0.01 s | ICE/STUN configuration, candidate/encoding and NAT-selection checks; no public STUN requests |
| `signaling_integration` | 9 | 0.06 s | Local WebRTC signaling sessions and serialization; includes a 50-ms expiry sleep |
| `transport_negotiation_integration` | 18 | <0.01 s | Negotiation over in-memory duplex streams, capability fallback and transport policy |
| `traffic_indistinguishability` | 17 | 0.08 s | Seeded sampling of padding/timing/cover traffic and statistical-helper checks |
| `privacy_integration` | 22 | 0.07 s | Simulated relay/adversary networks, diversity, load, correlation and timing assertions; also uses unseeded production randomness |

The initial shared compile completed in 20.84 seconds with a populated compatible
node build cache. Individual Cargo invocations took roughly 4.7–5.9 seconds,
including startup/resolution. These observations justify one explicit execution
step after the shared workspace compile, rather than eight duplicate build jobs.
They are not a clean-build performance guarantee for hosted Linux runners.

Both the statistical and simulated-privacy targets were repeated twice after the
initial pass (three successful runs each). This is a bounded repeatability check,
not proof that probabilistic assertions can never fail. No failure required a
production-code or assertion change in this work.

## Limits and remaining coverage

These are existing internal regression assertions, not proof of anonymity,
privacy guarantees, external cryptographic audit approval, or tests of a live
public relay network. The suites exercise in-process behavior; their names must
not be taken as evidence of public-network ICE/STUN/WebRTC interoperability.
Linux CI checks the final workflow head and remains authoritative for that
platform. Timing/randomness failures should be investigated, not hidden by
weakening assertions or silently dropping targets.

This workflow change does not run every Botho integration target. Compact-block
execution and [ledger/RPC execution](ledger-rpc-integration-tests.md) run in
adjacent workspace steps. The remaining consensus/E2E execution policy is
tracked by [#1274](https://github.com/botho-project/botho/issues/1274). Long ignored chaos/load tests retain their separate manual
opt-in; no blanket ignored-test execution is introduced here.
