# Mobile Wallet Backup & Recovery — Design & Decision Record

**Status**: Design / decision record (no feature implementation)
**Issue**: [#788](https://github.com/botho-project/botho/issues/788) (from [#719](https://github.com/botho-project/botho/issues/719) Finding 2)
**Date**: 2026-07-12
**Scope**: This document is the design deliverable for a passkey/biometric-gated
encrypted wallet backup. It does **not** ship the feature — the feature has three
real prerequisites that do not exist in the codebase today (see
[Prerequisites](#prerequisites-what-must-land-before-implementation)). This doc
proposes the UX and technology, records the go/no-go decisions the maintainer
must make, restates the non-custodial regulatory constraint, and enumerates the
follow-up work so an implementation issue can be curated once the prerequisites
land.

Related: [`FRAMEWORK_DECISION.md`](./FRAMEWORK_DECISION.md) (the "mnemonic never
leaves Rust" security model this design extends, not violates),
[`NATIVE_INTEGRATION.md`](./NATIVE_INTEGRATION.md) (the `BothoWallet` native
bridge, still "NOT done"),
[`app/src/native/keychain.ts`](./app/src/native/keychain.ts) (current secure
storage layer).

---

## 1. Problem statement

The mobile wallet's non-custodial key storage has no backup/recovery path. The
encrypted wallet blob is written to secure storage with a `THIS_DEVICE_ONLY`
posture:

- iOS: `keychainAccessible: WHEN_UNLOCKED_THIS_DEVICE_ONLY` →
  `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, which excludes the item from
  iCloud Keychain backup.
- Android: Keystore-backed data is excluded from Android Auto Backup by default.

(See `SECURE_OPTIONS` and its doc comment in
[`app/src/native/keychain.ts`](./app/src/native/keychain.ts), and the
platform-specific security sections in
[`FRAMEWORK_DECISION.md`](./FRAMEWORK_DECISION.md).)

Consequence: **if a device is lost, wiped, or replaced, the wallet — and the
funds it controls — are unrecoverable** unless the user transcribed the 24-word
mnemonic at creation time. Raw-seed-phrase transcription is the #1 consumer-UX
failure mode identified in [#719](https://github.com/botho-project/botho/issues/719)
Finding 2 and an open gap in the
[#441](https://github.com/botho-project/botho/issues/441) product architecture
epic ("backup/recovery UX ... and loss/recovery flow for non-technical 'village'
users").

The goal is a recovery path that, for the common case, does **not** require
seed-phrase transcription — recovery should be gated by the same
biometric/device-bound credential that already gates wallet unlock.

---

## 2. What honestly exists today (verified against `origin/main`)

Verified in this design pass against the sources named:

| Capability | Status | Evidence |
|---|---|---|
| Encrypted-blob secure storage (iOS Keychain / Android Keystore) | Present | `saveEncryptedWallet` / `loadEncryptedWallet` in `app/src/native/keychain.ts` |
| Biometric / device-credential gating primitives | Present | `isBiometricAvailable`, `authenticateWithBiometrics`, `isSecureStorageAvailable` (`expo-local-authentication ~14.0.0`) |
| Typed no-enrollment failure mode | Present | `SecureStoreUnavailableError` in `keychain.ts` (from #791/#800) |
| Headless jest test precedent under pnpm | Present | `app/src/native/keychain.test.ts`, `app/app/unlock.test.ts` (run via `pnpm -C mobile/app test`, mocked `expo-secure-store`/`expo-local-authentication`) |
| iCloud-synchronizable Keychain item via `expo-secure-store` | **Absent — architecturally** | `expo-secure-store@13.0.x` `SecureStoreOptions` has no `kSecAttrSynchronizable` field; the Swift module never sets it (see §5) |
| Passkey / WebAuthn library | **Absent** | `mobile/app/package.json` has no `expo-passkeys` / `react-native-passkey` / WebAuthn dependency |
| Seed/mnemonic export FFI (post-creation) | **Absent — by design** | `mobile/rust-bridge/src/lib.rs` exports only `generate_wallet` (returns mnemonic once at creation) and `unlock_with_mnemonic` (consumes one); no `export`/`reveal` method exists |
| `BothoWallet` native bridge module | **Absent** | `NATIVE_INTEGRATION.md` "What is NOT done"; every `walletModule.ts` path throws `NativeModuleUnavailableError` in Expo Go on both platforms |

The three "Absent" prerequisite rows are the reason this issue is a design doc,
not the feature.

---

## 3. Threat model

The backup feature deliberately changes the current posture ("key material never
leaves the device") to "an **encrypted** copy of the key material may leave the
device, but only the legitimate user's device-bound credential can decrypt it."
The design must not weaken protection against any of the following:

**Assets protected**
- The 24-word mnemonic / seed (root secret; controls all funds).
- The encrypted wallet blob at rest.

**Adversaries in scope**
1. **Thief / finder of the device.** Cannot decrypt local storage without the
   biometric/device credential — unchanged from today; backup must not regress
   this.
2. **Compromise of the durable backup store** (iCloud account, Google account,
   or a server-side blob store, if used). The attacker obtains the encrypted
   blob. **Requirement:** the blob must be useless without the passkey/biometric
   secret, which never leaves the user's device(s). This is what keeps the
   design non-custodial (§7) — the cloud provider holds ciphertext only.
3. **Network / provider observer.** Sees ciphertext in transit/at rest only.
4. **Malicious app code / supply-chain.** Out of scope for the storage design,
   but note: adding a seed-export FFI (§6) widens the in-process attack surface,
   which is exactly why that FFI needs its own security review.

**Adversaries out of scope (documented, not solved here)**
- A compromised OS / jailbroken-rooted device with the wallet unlocked (the
  attacker already has session keys regardless of backup).
- Coercion of the legitimate user to authenticate.

**Non-negotiable invariants**
- No wallet data becomes cloud-synced as a side effect. Backup is **opt-in**,
  explicit, and gated by an authenticated user action.
- The durable store never receives plaintext key material — only a blob
  encrypted under a key the user's credential releases/derives.
- The current no-backup users' posture is unchanged unless they opt in.

---

## 4. UX flow

Two flows: enroll (create a backup) and recover (restore on a new/wiped device).

### 4.1 Backup enrollment (opt-in)

1. User already has an unlocked wallet (existing unlock flow, `unlock.tsx`).
2. User opts into backup from settings. The screen states plainly: "An encrypted
   copy of your wallet will be stored in [iCloud / your chosen location]. Only
   this phone's Face ID / passcode (or a passkey you register) can unlock it. The
   provider cannot read it."
3. App obtains the seed-export encrypted blob from Rust (§6 — **prerequisite**),
   never the plaintext seed in JS.
4. App wraps that blob under a key released/derived from the passkey/biometric
   credential (§6.2), then writes it to the durable store (§8).
5. Confirmation screen; backup timestamp recorded.

### 4.2 Recovery on a new / wiped device

1. Fresh install, no local wallet (`hasStoredWallet()` false).
2. User chooses "Restore from backup."
3. App locates the encrypted blob in the durable store (§8); user authenticates
   with the same biometric/passkey credential; the credential
   releases/derives the unwrapping key.
4. App hands the decrypted blob to Rust to re-establish the wallet session
   (import path; §6).
5. Wallet is usable; a fresh local `saveEncryptedWallet` re-establishes the
   `THIS_DEVICE_ONLY` local copy.

### 4.3 No-enrollment path (device has no screen lock)

Backup **enrollment** must reuse the existing
`SecureStoreUnavailableError` contract from `keychain.ts` (introduced by
#791/#800): if `isSecureStorageAvailable()` is false, refuse to enroll and show
the actionable "set up a screen lock" message rather than storing an
unauthenticated or weakly-wrapped blob. **Recovery** on such a device is also
refused with the same typed error — you cannot restore a credential-gated backup
onto a device that cannot satisfy the credential contract. A new error type is
**not** needed; reusing `SecureStoreUnavailableError` keeps callers on one
catchable failure mode (its stated design intent).

---

## 5. Decision record — iCloud Keychain sync is NOT reachable via `expo-secure-store`

**This replaces the original issue's "Decision on iCloud Keychain sync opt-in"
acceptance criterion, which is moot.** There is no such opt-in to make.

Verified directly against the installed package (`expo-secure-store@13.0.x`):

- `SecureStoreOptions` exposes exactly four fields — `keychainService`,
  `requireAuthentication`, `authenticationPrompt`, and `keychainAccessible`
  (the iOS-only `kSecAttrAccessible*` family, default `WHEN_UNLOCKED`). **There
  is no `kSecAttrSynchronizable` field.**
- The native iOS Swift module (`SecureStoreModule.swift`) **never sets
  `kSecAttrSynchronizable`** in any `SecItemAdd` / `SecItemUpdate` /
  `SecItemCopyMatching` query (zero occurrences).
- Apple's Keychain treats an item as **non-synchronizable when the attribute is
  absent**. Therefore every item `expo-secure-store` writes is permanently
  excluded from iCloud Keychain sync, independent of `keychainAccessible`. This
  is architectural, not a version gap — the module was never wired for it.

**Consequence / decision:** any iCloud-Keychain-backed recovery requires a
**custom native module** that calls `SecItemAdd` with
`kSecAttrSynchronizable: true` — i.e., part of the not-yet-built `BothoWallet`
native bridge plus a dev-client/prebuild. It is **not** reachable from Expo Go
or from `expo-secure-store`'s JS API. This is a maintainer **go/no-go** decision,
not an implementation detail, and it is gated on the prerequisites in §9.

There is **no Android analog** to opt into either: Google's Android Auto Backup
is deliberately and correctly excluded for Keystore-backed data by design (see
`keychain.ts` doc comment and the "Android-Specific Security" section of
`FRAMEWORK_DECISION.md`). Per the
[#791](https://github.com/botho-project/botho/issues/791) precedent, sync/backup
concerns in this codebase are **platform-conditional, iOS-first** — this design
honors that and does not introduce a cross-platform sync assumption.

---

## 6. What gets encrypted, and the "mnemonic never leaves Rust" constraint

### 6.1 The constraint

`FRAMEWORK_DECISION.md`'s security model states **"Mnemonic Never Leaves Rust."**
Verified in `mobile/rust-bridge/src/lib.rs`: the session's mnemonic is held in a
`Zeroizing<String>` and is zeroized on `lock()`/drop; the FFI exports
`generate_wallet` (which returns the phrase exactly **once** at creation, for the
user to write down) and `unlock_with_mnemonic` (which **consumes** a phrase).
There is **no** `export_mnemonic` / `reveal_seed` / equivalent method. So the
thing a backup would need to encrypt — the seed — is, by design, **not
retrievable from JS after creation.**

### 6.2 What a seed-export FFI must look like (to preserve the constraint)

A backup flow therefore requires **new, deliberate Rust FFI surface**, designed
so plaintext seed material still never crosses the FFI boundary into JS:

- **Export an encrypted blob, not the seed.** The FFI method must perform the
  encryption inside Rust and return **ciphertext only**. A hypothetical shape:

  ```
  // Rust-side only; JS never sees plaintext seed.
  export_encrypted_backup(wrapping_key: [u8; 32]) -> MobileResult<String>  // base64 ciphertext
  import_encrypted_backup(blob: String, wrapping_key: [u8; 32]) -> MobileResult<WalletAddress>
  ```

  The `wrapping_key` is the 32 bytes released/derived from the passkey/biometric
  credential on the JS/native side; Rust uses it as an AEAD key (e.g.,
  XChaCha20-Poly1305) over the already-at-rest encrypted seed representation.
  JS/native holds only: (a) the credential, (b) the ciphertext. Neither is the
  plaintext seed.

- **No "reveal the words" method** is introduced by the backup feature. If a
  future "show me my recovery phrase" screen is ever wanted, that is a
  **separate** security decision with its own review — it is explicitly *not*
  part of this backup design.

- This new FFI is **security-sensitive** and must get its own reviewed issue
  (§9). This design does **not** implement it.

### 6.3 Fallback shape if the full FFI is deferred

If the seed-export FFI lands later than desired, a documented interim MVP is:
biometric-gated encryption of the **existing** `botho_encrypted_wallet` blob
(which is already ciphertext from Rust) under a device-credential-derived key,
exported as a file the user manually places into their own cloud storage app.
This avoids passkey and iCloud-entitlement work but is weaker than the
"passkey-gated" framing from #719 Finding 2 (it relies on the OS file-share
sheet and user diligence, not a registered credential). **The trade-off is
recorded here so the maintainer chooses it explicitly**; the design does not
prescribe it.

---

## 7. Regulatory note (carried forward from #719 Finding 1)

Recovery **must stay non-custodial**: the user/device holds the credential and
the decryption key; **no provider ever holds a share** of the key material. The
durable store (iCloud/Google/server) holds **ciphertext only** and cannot
decrypt it. This keeps the design outside custody / money-services-business
(MSB) framing per [#719](https://github.com/botho-project/botho/issues/719)
Finding 1.

Explicitly **out of scope** (would change the custody/regulatory posture and/or
require a programmable-contract layer Botho does not have): MPC/TSS with a
provider-held share, server-side key escrow, ERC-4337 account abstraction, and
smart-contract social recovery. Shamir social-recovery among user-chosen
guardians (no provider share) is a viable **later** phase for higher-assurance
users but is out of scope for this MVP.

---

## 8. Where the encrypted blob is persisted (platform-conditional)

Per the #791 precedent, this is decided **per platform, not as a shared toggle**:

- **iOS (primary target):** iCloud Keychain (requires the custom synchronizable
  native module — §5) **or** iCloud Drive / a ubiquity container for the
  ciphertext file. Either requires the native bridge + prebuild + the relevant
  entitlement (Keychain Sharing / iCloud). The credential-release/derivation uses
  `ASAuthorizationPlatformPublicKeyCredentialProvider` (passkeys) *or*, in the
  fallback shape (§6.3), `expo-local-authentication`.
- **Android:** **No first-class MVP backup path is proposed at this time.** The
  honest position (not a placeholder): Auto Backup deliberately excludes
  Keystore-backed data, and a Google-Drive/Credential-Manager path is its own
  native integration with no code in the app today. The Android MVP is therefore
  the **file-export fallback (§6.3)**: produce a credential-gated ciphertext file
  and let the user place it in their own Drive/Files app. A true Android passkey
  (Credential Manager) + Drive backup is deferred to the follow-up feature.
- **Server-side blob store:** possible but **discouraged** — it adds an
  availability dependency and a (ciphertext-only, but still) data-handling
  surface without improving the non-custodial property. Not proposed for MVP.

---

## 9. Prerequisites — what must land before implementation

The design-doc deliverable (this file) has **no** prerequisites. The **full
feature** it describes depends on all three of the following, none of which
exist today. Each should become its own curated issue that references this
document; this issue files none of them (per the curator's instruction that this
doc should drive that decomposition once written):

1. **`BothoWallet` native bridge module** (iOS Swift + Android Kotlin). Tracked
   in `NATIVE_INTEGRATION.md` "What is NOT done." Every wallet FFI path is
   unreachable in Expo Go until this exists. All backup UI is gated on it. This
   design adds the backup-specific bridge methods to that doc's tracked list.
2. **Passkey / WebAuthn RN library selection + Expo prebuild.** No passkey
   dependency is installed; iOS
   (`ASAuthorizationPlatformPublicKeyCredentialProvider`) and Android
   (Credential Manager) passkeys are native-only surfaces with no first-party
   Expo module at the installed SDK. Requires a third-party library (or custom
   native module) plus a dev-client/prebuild — the same "eject from Expo Go"
   step as (1). The library choice is a decision for the implementation issue,
   informed by §6.3's fallback trade-off.
3. **A new Rust FFI seed-export method** (`export_encrypted_backup` /
   `import_encrypted_backup` per §6.2) in `mobile/rust-bridge/src/lib.rs`,
   preserving "mnemonic never leaves Rust" by returning ciphertext only.
   Security-sensitive; requires its own reviewed issue.

An implementation issue for the full feature is **blocked** on (1)–(3). This
design-doc issue is **not** blocked.

---

## 10. Groundwork included under this issue

Per the revised acceptance criteria, only safe, **prerequisite-free** groundwork
is in scope. The design concludes the minimal safe groundwork is a **reserved
storage-key naming convention** for the future encrypted-backup blob, documented
in `keychain.ts` so a future implementer does not collide with the existing
`botho_*` keys or accidentally place backup material under a `THIS_DEVICE_ONLY`
key (which would defeat its purpose). This is a documentation/constant reservation
only — **no behavioral change** to any existing export, no new runtime code path,
no dependency on prerequisites (1)–(3). See the reserved-key comment added to
`app/src/native/keychain.ts`.

No changes are made to `mobile/rust-bridge` or the native bridge under this
issue.

---

## 11. Acceptance-criteria mapping (for reviewers)

| Revised AC (issue #788) | Where satisfied |
|---|---|
| Design doc: opt-in backup + recovery UX | §4 |
| Design doc: what is encrypted and with what | §6 |
| Design doc: where the blob is persisted (platform-conditional) | §8 |
| Design doc: prerequisite list (bridge, passkey lib, Rust FFI) | §2, §9 |
| Decision record replacing "iCloud sync opt-in" AC (cite `SecureStore.d.ts` / `SecureStoreModule.swift`) | §5 |
| Regulatory note (non-custodial) restated in the doc | §7 |
| Doc lists prerequisite work; this issue implements none of it | §9 |
| No changes to `rust-bridge`/native bridge; optional safe `keychain.ts` groundwork only | §10 |
| iOS **and** Android addressed separately | §5, §8 |
| No-enrollment path behavior stated | §4.3 |
| No security regression (no silent cloud sync) | §3 invariants, §5 |
