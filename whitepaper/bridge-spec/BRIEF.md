---
project: botho-bridge-spec
audience:
  - Protocol implementers, auditors, and exchange/bridge integrators reading the bridge design as the source of truth
max_iterations: 4
documents:
  - slug: botho-bridge-spec
    artifact_type: spec
    # NOTE: code_ref is scalar-only (anvil#718 — a list silently disables the
    # tier). Point it at the primary bridge Rust service; the auditor also
    # consults contracts/ethereum/contracts/*.sol + contracts/solana manually.
    code_ref: /Users/rwalters/GitHub/botho/bridge/**/*.rs
---

# Botho wBTH Bridge — normative spec section (destined for whitepaper §11)

A normative treatment of the BTH↔wBTH cross-chain bridge, authored as an
`anvil:spec` thread so the `code_ref` consistency audit verifies every claim
against the bridge implementation. The AUDITED LaTeX body is integrated into
the whitepaper as a new section.

## Source of truth (in `refs/`)

The five ratified bridge ADRs are the design source:
- **ADR 0002** — custody / trust model (SCP-validator threshold-multisig federation)
- **ADR 0003** — the peg (factor-1-only wrapping + the demurrage-settlement on-ramp)
- **ADR 0004** — privacy semantics at the boundary (amount revelation on lock, re-shield on unwrap)
- **ADR 0005** — v1 chain scope (Ethereum + Solana)
- **ADR 0007** — bridge-import cluster tagging (epoch-keyed import factor, K=1 day, F=1.5×)

The `code_ref` implementation (`bridge/core`, `bridge/service`,
`contracts/ethereum`, `contracts/solana`) is the consistency oracle — the
audit must confirm the spec matches it, or mark a divergence with a three-way
disposition.

## Implementation-status discipline (load-bearing)

The bridge is partially shipped. The spec MUST carry an `## Implementation
status` register distinguishing live from target-state, because a claim that
reads as "is" when the code says "will be" is exactly the drift ADR 0006's
audit machinery exists to catch. Known live/target split at authoring time:
- **Live**: the Ethereum wrap/unwrap path (Phase 0–3, PRs #832–#864);
  exactly-once semantics; factor-1 peg.
- **Target-state (tracked)**: bridge-import cluster tagging (ADR 0007
  ratified; implementation in flight as #938) — mark target-state until #938
  merges; Solana transports (stubbed, #856/#857/#858/#853); the
  demurrage-settlement operation (#831, blocked on the shared reset-horizon
  ratification); external security audit (#616/#830, the mainnet gate).

Do not describe target-state mechanisms in the present tense without a
register row.
