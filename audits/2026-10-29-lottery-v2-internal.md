# Internal Security Audit Report: LotteryV2 payouts (P12)

**Date**: 2026-10-29
**Auditor**: coordinator session (internal; issue #1647, part of #1286)
**Scope**: The merged V2 lottery payout path that the x1 endurance re-run (#1576) depends on: the related-key payout construction, proposer/validator determinism, the body-root commitment, the RPC context trust model, the signing-hash change (#1638) and LMDB growth.
**Commit**: `349211d7` (main, after P1–P8, the bridge, the CLI, and #1667 parts 1–2). P9 (#1664, snapshot V2) and P10 (#1672, detached witness) were still in review. Their own judges reviewed them adversarially, so they are covered here by reference only.

This is an internal review. It does **not** replace the external review required before mainnet (#616).

---

## Executive Summary

No issue in the V2 payout construction, its validation or its client recovery blocks the x1 re-run or the 7.0.0 testnet reset.

- **Payout keys:** the related-key construction is sound. A payout's one-time key is `P' = P + T·G`, where `T` is publicly derivable. Its spend key is `x + T`. Only the owner of `x` can produce it, and its key image `(x+T)·Hp(P')` is unlinkable to the source's `x·Hp(P)`.
- **Determinism:** the producer and validator share one rule module and one pinned read, and the validator re-derives every record instead of trusting the wire.
- **Body commitment:** the header root binds every part of the block body without ambiguity.
- **Client recovery:** it fails closed, so a forged RPC context can only cause a missed payout or a refused spend, never a wrong key.

The review found one significant issue that is **not specific to V2**. The lottery draw seed for block N is the SHA header hash of block N−1, which covers `tx_root` and `timestamp`. The RandomX PoW does not cover either field. The producer of N−1 can therefore regrind the next block's draw for free once it has a valid PoW solution. That invalidates the premise behind the H4 acceptance in #715. The finding is filed as **#1676**. It is pre-mainnet High, not x1-blocking.

**Overall Status**: Issues Found (one High that predates V2, two Low, several Info). Nothing blocks x1.

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High | 1 (pre-existing, #1676) |
| Medium | 1 (#1670, filed during P3 review) |
| Low | 2 |
| Info | 5 |

---

## Sections Reviewed

- [x] 1.2 CLSAG / related-key payout spending: clean
- [x] 2. Key derivation (`P' = P_base + T·G`, ML-KEM reuse): clean
- [x] 3. Consensus: producer/validator determinism and the body root are clean. The draw seed is grindable (H-1, pre-existing).
- [x] 4. Transaction validation: the signing hash (#1638) is clean.
- [x] 5. Privacy: payout-to-winner linkage is inherent and public (I-1). Decoy analysis is tracked in #1663.
- [x] 7. Wallet: the RPC context trust model and fail-closed recovery (#1667) are clean.
- [x] LMDB growth: acceptable (I-4).

---

## Findings

### [HIGH] H-1: Lottery draw seed is grindable for free (pre-existing; reopens the #715 premise)

**Location**:
- `cluster-tax/src/lottery.rs:328` (`generate_seed(block_hash, height)`);
- `botho/src/block.rs:71` (`BlockHeader::hash` covers `tx_root` and `timestamp`);
- `botho/src/block.rs:96` (`pow_hash` is RandomX over `nonce ‖ prev ‖ minter keys` only).

**Status**: Open, #1676.

**Description**:
- Block N's draw, including the #573 candidate-window offset and the winners, is a pure function of `hash(N−1)`.
- `hash(N−1)` depends on the transaction set and order (via `tx_root`) and on the timestamp, neither of which the PoW covers.
- Once the producer of N−1 has one valid PoW solution, it can enumerate many distinct `hash(N−1)` values. Each costs a SHA-256 plus a draw simulation.
- From those it can pick the outcome where its own eligible outputs win block N's payouts.

**Impact**: block producers can capture lottery payouts, up to the one-block-reward cap on every block they produce. That undermines the lottery's role in the Gini mechanism. The #715 EV argument assumed each regrind costs a full PoW attempt and competes with coinbase priority. Neither is true. The x1 run is unaffected because its permissioned fleet has no grinding actor.

**Recommendation**: seed block N's draw from the RandomX PoW output of N−1, hashed together with height and parent, or bind `tx_root` and `timestamp` into the PoW preimage. Then re-run the #715 EV argument and update the threat-model note (PR #718) and the #616 audit scope. The fix is consensus-changing, so it is cheapest folded into a planned reset.

### [MEDIUM] M-1: The V2 validator runs the full candidate scan before the cheap header checks (#1670)

**Location**: `botho/src/ledger/lottery_v2.rs` `validate_block`, which calls `drawing()` before `validate_ordinary_block`.

**Status**: Open, #1670 (filed by the P3 judge).

**Description**: `validate_block` draws, and so scans up to 10k strictly read candidates, before the height, parent, difficulty and PoW checks run. A peer can send a block at tip+1 with a wrong parent and no PoW, and force one full scan for free.

**Impact**: a CPU-amplification DoS on open networks. It is harmless on the permissioned x1 fleet. Measured at x1 scale, the extra validation is about 0.3–0.8 s per block against a roughly 20 s slot.

**Recommendation**: run the header, parent, difficulty and PoW checks before `drawing()`, and memoize the creating-block lookup per scan (both in #1670).

### [LOW] L-1: Classical `belongs_to` returns label 1 for change (#1674)

**Location**: `transaction/clsag/src/lib.rs:553`; `botho-wallet/src/transaction.rs` (~802).

**Status**: Open, #1674.

**Description**: the classical detectors return `Some(1)` for the change subaddress, which `recover_spend_key` treats as subaddress index 1. The real change index is `u64::MAX − 1`.

**Impact**: latent only. No production sender puts outputs on the change subaddress, and 6.0.0+ pq builds reject classical outputs.

### [LOW] L-2: Ledger block validation does not itself enforce the coinbase ML-KEM ciphertext shape

**Location**: `botho/src/ledger/store/validation.rs` `validate_ordinary_block`. Enforcement lives only in `TransactionValidator::validate_minting_tx_intrinsic`, called from the SCP path (`commands/run.rs:1414`).

**Status**: Open, #1677.

**Description**:
- `MintingTx::hash` folds the ciphertext in only when it is `Some`, so `None` and `Some(vec![])` hash identically.
- SCP validators reject anything that is not exactly 1088 bytes. The ledger's own `add_block`, which is used for sync and replay, does not check it.
- A block reaching `add_block` without passing SCP intrinsic validation could therefore carry an aliased coinbase ciphertext under the same block hash.
- P10's detached witness closes this on its own path (1088-or-None).

**Impact**: low, because externalized blocks have passed intrinsic validation. It is defense in depth for sync from untrusted peers.

**Recommendation**: call `check_kem_ciphertext` on `block.minting_tx.kem_ciphertext` inside `validate_ordinary_block`, or make `MintingTx::hash` length-prefix the ciphertext. Either way it is a consensus change.

### [INFO] I-1: Payouts are publicly linked to their winning outputs

Each payout reuses the winner's `public_key` and `kem_ciphertext`. `T` is publicly derivable from the domain, and `winnerOutpoint` is published over RPC. Anyone can therefore link a payout to its source. This is inherent to a public lottery and matches V1, which copied the target outright. Decoy-selection consequences are tracked in #1663.

### [INFO] I-2: Related-key spending is sound

The derivation is `T = Σ δ`, with `δ = H512(domain-separated preimage)` reduced mod ℓ. It requires `δ ≠ 0`, cumulative `T ≠ 0`, and a non-identity target. The preimage binds:
- genesis, parent, height, the ordinary root and the award manifest;
- the ordinal, the source outpoint, target and context, and the amount.

Recovery checks the point encodings, subtracts `T·G`, recovers the base key at the original index (the ML-KEM index binding holds for hybrid outputs), adds `T`, and requires `x·G = P'`.

- **Nested payouts:** they carry `T₁ + δ₂` over the same base index, and they recover the same way.
- **Key images:** `x·Hp(P)` and `(x+T)·Hp(P')` are distinct, and they are not linkable without `x`.
- **Forgery:** an attacker without `x` cannot produce `x + T`.

### [INFO] I-3: Determinism and the body root are sound

- **One rule set:** the producer (`derive`, via `seal_block`) and the validator (`validate_block`) use the same `drawing()` and `records_for()` over one pinned read.
- **No node-local inputs:** `canonical_config()` ignores environment overrides, the draw is integer-only, and payout contexts are never taken from the wire.
- **Candidate filtering:** the candidate scan applies exactly the eligibility rule the draw applies, and the default `Uniform` weight is 1. So with a positive pool, a non-empty candidate set always yields winners, and the `inconsistent` guard in `drawing()` cannot halt the chain under current parameters. A future change to selection mode, or a zero-weight rule, must keep that invariant.
- **Body root:** `body_root(minting, ordinary, payouts, summary)` hashes four fixed-width roots. `compute_tx_root` concatenates fixed-width transaction hashes, and since #1638 those commit every output field. `payout_root` commits a count plus canonical fixed-layout records. The encoding has no ambiguity.

### [INFO] I-4: LMDB growth is acceptable

Each output gains a 45 B (Direct) or 49 B (Lottery) context row. At x1 scale (about 13k blocks, at most 4 payouts per block, around 20 tx/min), that adds a few MB. The per-block candidate scan cost is the real scaling concern; see M-1.

### [INFO] I-5: Checks outside this report's direct reading

- #1638 (signing hash commits every output field) was reviewed and merged, and #1649 had its own judge.
- P9 (snapshot V2, bounded decompression, `BelowSnapshotBase`) and P10 (the detached witness and its trust anchor) were each reviewed adversarially by their judges.
- These are not re-derived here.

---

## Disposition for the x1 re-run

There are **no blockers**. H-1 and L-2 are consensus changes. Folding them into the 7.0.0 reset before x1, or into a later bump, is an owner decision recorded on #1676. M-1 and L-1 are tracked.
