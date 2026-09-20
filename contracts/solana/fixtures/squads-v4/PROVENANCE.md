# Pinned Squads v4 test artifact

This is an unmodified production Squads v4 program loaded **only into a fresh
local validator** for ADR 0012 Tier-2 tests. It is not a Botho deployment or a
claim that production bridge integration is complete.

- Official repository: https://github.com/Squads-Protocol/v4
- Audited source revision: `dcac867070a3073929e2240a053780c324f4c29f`
- Source and IDL: https://github.com/Squads-Protocol/v4/tree/dcac867070a3073929e2240a053780c324f4c29f
- IDL path upstream: `sdk/multisig/idl/squads_multisig_program.json`
- Program ID: `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`
- Program: Squads v4, crate version 2.1.0; SDK pinned in npm lockfile: 2.1.2.
- Reviewed-source/deployed-hash attestation: upstream
  [`audits/neodyme_squads_v4_report_2024_final.pdf`](https://github.com/Squads-Protocol/v4/blob/af94153ff77a28b6effe46b9c94baaa93742b48c/audits/neodyme_squads_v4_report_2024_final.pdf),
  section “Scope and methodology”, page 5. It identifies the source revision
  and executable hash below. This uses an existing published audit; no audit
  engagement or purchase was made for this work.

Retrieved 2026-09-20 using a read-only RPC operation:

```sh
solana program dump --url https://api.mainnet-beta.solana.com \
  SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf squads_multisig_program.so
```

| Object | SHA-256 |
| --- | --- |
| Full `.so` file (1,470,416 bytes, including deployment padding) | `dec8d3e0fae58c7c8f2416e5f67c25e673f047afd6dd2bba4a47e0b29a01d34c` |
| Executable after trimming trailing zero padding, matching the published audit | `d48660833989ecea3145ff726164fe640bd90696f03ce00dfd0cda258cbf2fac` |
| Exact upstream IDL bytes | `cb9a0a29040ec3853a5105c547ffc608bb0e3175d78dacf30d463aff939f9b9b` |

`localnet/verify-artifact.cjs` checks all three hashes before starting a node.
No live network fetch, program clone, faucet, wallet, or authority change is
performed by the test. `LICENSE` is copied verbatim from the pinned source;
the program is redistributed here for non-production testing under its
Business Source License. The source reference and hash match are provenance,
not an independent reproducible build claim.

The genesis ProgramConfig fixture is generated from the pinned SDK layout:
its discriminator, owner, zero creation fee, deterministic treasury and
config authority are asserted. This replaces the **state** normally created
by Squads' privileged one-time initializer. That initializer is not executed
and its private key is neither required nor fabricated. The multisig itself
is created by the real `multisig_create_v2` instruction with an immutable
2-of-3 threshold; all proposal/vote/execute/CPI processing is real program
execution, including order-marker account creation and SPL Token minting.
