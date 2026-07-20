# Botho MetaMask Snap — Key-Handling Security Audit

**Date**: 2026-07-20
**Auditor**: Loom Builder (#1096)
**Scope**: `web/packages/snap` — key-handling surface only (SIP-6 SRP → Botho keys,
ephemeral claim-link bearer secrets, wasm signing under SES, and the Snap's
`snap_manageState` persistence). The audited Rust `bth-wasm-signer` core and the
org-wide external-audit engagement (#616) are explicitly out of scope.
**Commit**: origin/main @ `b13184f9` (Phase-2 surface: #1091 state, #1092 history,
#1093 contacts, #1094 claim-link, #1095 i18n).

---

## Executive Summary

The Snap introduced a new key-handling surface (MetaMask SRP → Botho private keys,
ephemeral claim-link secrets, wasm signing, and the Snap's first `snap_manageState`
persistence). This audit enumerated that surface end-to-end and confirms the design
is **sound**: the two at-rest finding classes from the closed web-wallet audit
structurally **cannot recur** here.

- **#474** (claim-link bearer secrets in plaintext `localStorage`, drainable):
  cannot recur — claim-link secrets are transient in-memory only, re-derived per
  RPC call, and never persisted (`claim.ts`).
- **#475 / #476** (encrypt seed / address book at rest): the Snap **never writes
  the seed or private keys to state at all** — MetaMask holds the SRP and the Snap
  re-derives on demand via `snap_getEntropy`; the only persisted blob (`scan` +
  `contacts`) holds public/derived data and is stored `encrypted: true`.

The real gaps were (a) the invariants were *prose promises*, not *enforced tests*,
and (b) minor defense-in-depth around transient key buffers. Both are addressed in
this pass: a regression suite (`test/keyhandling.snap.ts`) now asserts the
invariants, and `src/derivation.ts` was hardened (F1). No secret-leak vector was
found in the RPC results, dialogs, error messages, or persisted state.

**Overall Status**: Clean (design sound; invariants now test-enforced; F1 hardened)

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High | 0 |
| Medium | 0 |
| Low | 1 (F1 — fixed) |
| Info | 5 (F2–F6 — F2–F4 test-locked, F5/F6 accepted) |

---

## Sections Reviewed

- [x] 2. Key Derivation & Management — SIP-6 → Botho RootIdentity (`derivation.ts`)
- [x] 7. Wallet Security — RPC results, dialogs, persisted state, claim-link ingress
- [x] Logging — `git grep` confirms zero `console.*` in `src/`

---

## Key-handling surface (audited enumeration)

Verified by `git grep` over `web/packages/snap/src/*.ts`:

1. **SRP entropy → Botho keys** — `src/derivation.ts`.
   `deriveMnemonic()`: `snap_getEntropy({version:1, salt:'botho-root'})` →
   `entropyHex` (string) → `entropy` (Uint8Array, 32 B) → `entropyToMnemonic` →
   24-word mnemonic. `walletFromMnemonic()`: `mnemonicToSeedSync` → `seed`
   (Uint8Array) → `SignerKeys { spendPrivateKey, viewPrivateKey, seed }` (hex),
   cached as `cachedWallet`.
2. **Claim-link ephemeral bearer secret** — `src/claim.ts`. `parseClaimLink()` →
   `{ mnemonic, amountHint }`; `ephemeralKeys()` re-derives `SignerKeys` on each
   call (correctly **not** cached). Consumed by `scanClaimLink` / `buildSweep`.
3. **wasm / SES boundary** — `src/signer.ts`. `SignerKeys` hex strings cross into
   the inlined wasm (`buildAndSign`, `scanOwnedOutputs`,
   `computeOwnedOutputKeyImages`) via `setSigner`. Rust-side zeroization inside
   `bth-wasm-signer` is out of scope (that is the audited Rust core), but the trust
   boundary is noted here.
4. **Persisted state** — `src/state.ts` (`snap_manageState`, `encrypted: true`).
   `SnapState = { version, scan?, contacts? }`. `PersistedOwnedOutput` stores target
   keys / public keys / amounts / kem ciphertext — public/derived data.
   `keys: SignerKeys` is an *input* to `incrementalScan` and is **never written**:
   the write spreads the prior blob and sets only `version` / `scan`.
5. **Contacts** — `src/contacts.ts` — labels + validated public addresses only; no
   secret material; `writeContacts` spreads the prior blob so it never clobbers
   `scan`.
6. **RPC results & dialogs** — `src/index.ts`, `src/ui.ts`. Results return only
   address / balances / txHash / entries / contacts / `{ revealed: true }`. The
   mnemonic is surfaced **only** through `mnemonicBackupContent` as
   `Copyable({ value: mnemonic, sensitive: true })`, gated behind a `confirm()`;
   the pre-confirm dialog shows a masked placeholder. Error paths interpolate host /
   method / node-error — not keys.
7. **Logging** — zero `console.*` in `src/` (asserted by a guard test).

---

## Findings

### [LOW] F1 — Transient key buffers not zeroized; mnemonic cached module-long

**Location**: `src/derivation.ts` (`deriveMnemonic`, `walletFromMnemonic`)
**Status**: Fixed

**Description**: The transient `entropy` (raw SIP-6 secret) and `seed` (64-byte
BIP39 seed) `Uint8Array`s were left on the heap after being consumed, and a
module-level `cachedMnemonic` pinned the raw 24-word recovery phrase (full spending
authority) for the entire Snap execution context even though it is only re-read on
the rare `revealMnemonic` path.

**Impact**: Defense-in-depth only. There is no known in-SES adversary that can read
another execution context's heap, and MetaMask already holds the SRP; this reduces
the residency window / copy-count of secret material.

**Recommendation**: `.fill(0)` the transient byte arrays after use; stop caching the
raw mnemonic (re-derive on demand — deterministic in `(SRP, snap id, salt)`, so no
functional change), keeping only `cachedWallet` (the derived keys the signer needs).

**Resolution**: Applied in this PR:
- `walletFromMnemonic` computes `seedHex` then `seed.fill(0)` before deriving keys.
- `deriveMnemonic` no longer caches; wipes `entropy` in a `finally`; re-derives on
  demand. `deriveWallet` still caches the derived `cachedWallet` (the keys are
  required in-memory to sign, so this residual copy is inherent and documented).

**Residual (accepted, documented)**: the intermediate `entropyHex` string and the
`SignerKeys` hex strings are **immutable JS strings** and cannot be zeroized in
place; `cachedWallet.keys` necessarily lives for the execution context so the signer
can build/scan without a re-derivation per call. The `@scure/bip39` /
`@botho/core` internals also allocate their own transient buffers we do not control.
These are inherent to a JS/SES wallet and are the accepted lower bound.

### [INFO] F2 — Claim-link bearer-secret hygiene was untested (the #474 class)

**Location**: `src/claim.ts`, `src/index.ts`
**Status**: Fixed (test-locked)

**Description**: The ephemeral claim-link mnemonic is correctly never persisted /
dialogged / returned / errored, but this was asserted only in prose.

**Resolution**: `test/keyhandling.snap.ts` drives `botho_previewClaimLink` and
`botho_claimLink` with a known bearer mnemonic and asserts the secret appears in
**no** RPC result, **no** `snap_dialog` content, and **no** thrown error (verbatim
or as standalone BIP39 tokens, excluding template vocabulary).

### [INFO] F3 — Mnemonic-reveal exposure was correctly gated but untested

**Location**: `src/index.ts` (`botho_showMnemonic`), `src/ui.ts`
**Status**: Fixed (test-locked)

**Resolution**: Tests assert (a) the pre-confirm dialog shows only the masked
placeholder (none of the real 24 words), (b) after confirm the phrase appears
**only** inside a `sensitive: true` Copyable, (c) the RPC result is `{revealed:true}`
and never contains the phrase, and (d) rejecting throws the fixed decline string
with no secret.

### [INFO] F4 — No proof that persisted state omits key material (the #475/#476 class)

**Location**: `src/state.ts`, `src/contacts.ts`
**Status**: Fixed (test-locked)

**Description**: The Snap never writes seed/keys to `snap_manageState`, but nothing
enforced it. The SES simulation harness (`@metamask/snaps-simulation` 4.x) exposes
**no state getter**, so the blob cannot be read back through `installSnap`.

**Resolution**: The invariant is enforced at the **write boundary** instead: a
capturing `snap_manageState` stub records what `incrementalScan` (driven with a
KNOWN `SignerKeys` and a discovered owned output) and `writeContacts` actually write.
The tests assert the persisted blob **contains** the public receive facts / contact
(non-vacuous) but **none** of the private spend/view-key or seed hex passed in. A
complementary SES-harness test reveals the real derived phrase, derives its keys,
and asserts none appear in any silent-read result after a persisted round-trip.

### [INFO] F5 — Manifest permission least-privilege

**Location**: `snap.manifest.json`
**Status**: Acknowledged (no change — all permissions are used)

**Description**: `initialPermissions` = `endowment:rpc` (dapps),
`endowment:webassembly`, `endowment:network-access`, `snap_getEntropy`,
`snap_dialog`, `snap_manageState`, `snap_getPreferences`. Each maps to a live
consumer:

| Permission | Used by |
|---|---|
| `endowment:rpc` (`dapps:true`) | `onRpcRequest` (the whole dApp API) |
| `endowment:webassembly` | inlined `bth-wasm-signer` (`signer.ts`) |
| `endowment:network-access` | node JSON-RPC `fetch` (`node.ts`) |
| `snap_getEntropy` | SIP-6 SRP derivation (`derivation.ts`) |
| `snap_dialog` | receive / balance / send / claim / mnemonic dialogs |
| `snap_manageState` | persisted scan + contacts (`state.ts`) |
| `snap_getPreferences` | i18n locale selection (#1095, `i18n.ts`) |

No unused endowment found. In particular, `snap_getPreferences` (added by #1095) is
actively read by `resolveLocale()` and MUST be retained.

### [INFO] F6 — SIP-6 entropy is bound to the Snap's npm id

**Location**: `src/derivation.ts` (`ENTROPY_SALT`, header trade-off note)
**Status**: Accepted risk (documented)

**Description**: `snap_getEntropy` is deterministic in `(SRP, snap id, salt)`, so
republishing under a different npm id derives a **different** wallet. `revealMnemonic`
(export the 24 words for an off-MetaMask backup) is the documented mitigation.
Production snap-id pinning is operator-gated and tracked under umbrella #1089 — out
of scope for this in-code pass.

---

## Comparison to the closed web-wallet at-rest audit (#474 / #475 / #476)

| Web-wallet finding | Recurs in Snap? | Why |
|---|---|---|
| #474 — claim-link bearer secret plaintext in `localStorage`, drainable | **No (structural)** | Claim secrets are transient in-memory, re-derived per call, never persisted (`claim.ts`). Locked by F2 tests. |
| #475 — encrypt seed at rest + KDF/password | **No (structural)** | The Snap never writes seed/keys to state; MetaMask holds the SRP and the Snap re-derives via `snap_getEntropy`. Persisted `scan` is `encrypted:true`. Locked by F4 tests. |
| #476 — address-book encryption | **No (structural)** | Contacts live in the same `encrypted:true` blob and hold only public data (`contacts.ts`). Covered by the global "no secret in state" F4 test. |

---

## Fixes Applied During Audit

| Finding | Severity | File | Fix |
|---------|----------|------|-----|
| F1 | LOW | `src/derivation.ts` | Zeroize transient `entropy`/`seed` buffers; drop `cachedMnemonic` (re-derive on demand) |
| F2 | INFO | `test/keyhandling.snap.ts` | Assert bearer secret in no result/dialog/error |
| F3 | INFO | `test/keyhandling.snap.ts` | Assert mnemonic-reveal gating (masked/sensitive/`{revealed}`) |
| F4 | INFO | `test/keyhandling.snap.ts` | Assert no key material in the `snap_manageState` write |

---

## Verification

| Check | Result |
|-------|--------|
| `pnpm --filter @botho/snap typecheck` | PASS |
| `pnpm --filter @botho/snap build:snap` | PASS |
| `pnpm --filter @botho/snap test:snap` | PASS (incl. new `keyhandling.snap.ts`) |
| `git grep 'console\.' web/packages/snap/src` | 0 matches (guarded by test) |
| `test/derivation.snap.ts` regression | PASS (F1 did not change the derived wallet) |

---

## Recommendations for Next Audit

1. Re-run once the SES simulation harness exposes a state getter, to read the
   `snap_manageState` blob back directly (currently proven at the write boundary).
2. Live-send / live-claim validation (blocked on betanet resume #1051) — exercises
   the wasm signing path with real owned-output fixtures.
3. Production snap-id pinning + npm/allowlist publishing (operator-gated, #1089).
4. Rust-side (`bth-wasm-signer`) key zeroization — the wasm/SES trust boundary
   (item 3) is deferred to the audited Rust core.

---

## Time Spent

| Activity | Hours |
|----------|-------|
| Code review | 1.5 |
| Testing | 1.5 |
| Documentation | 1.0 |
| **Total** | **4.0** |
