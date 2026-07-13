# Mobile Framework Decision

**Decision**: React Native with UniFFI for Rust bindings

**Date**: 2025-12-31

## Evaluation Summary

### Options Evaluated

| Framework | Rust FFI | Crypto Libs | iOS Keychain | Biometrics | Bundle Size | Dev Velocity |
|-----------|----------|-------------|--------------|------------|-------------|--------------|
| **React Native** | uniffi (mature) | Via Rust | expo-secure-store | expo-local-auth | ~25MB | Fast |
| Flutter | ffigen (experimental) | Via Rust | flutter_secure_storage | local_auth | ~12MB | Fast |
| Native Swift | cbindgen (excellent) | Via Rust | Security.framework | LocalAuthentication | ~3MB | Slow |

### Decision Criteria Results

#### 1. Rust FFI Support - React Native Wins

- **UniFFI** (Mozilla): Production-ready, generates Swift/Kotlin bindings from Rust
- Existing `botho-wallet` crate already has correct crate-types (`staticlib`, `cdylib`)
- Pattern proven in crypto wallets (Signal, Firefox)

#### 2. Crypto Library Availability - All Equal

All options use Rust for cryptography:
- ML-KEM (post-quantum key exchange)
- ML-DSA (post-quantum signatures)
- LION (lattice-based ring signatures)
- Ed25519/Ristretto (classical fallback)

Crypto stays in Rust memory, only API surface exposed.

#### 3. Team Expertise - React Native Wins

- Existing desktop wallet uses TypeScript (Tauri + React)
- Web frontend team can contribute to mobile
- Shared component libraries possible

#### 4. Prototype Validation

Cross-compilation tested:
```bash
# iOS simulator (x86_64)
cargo build --target x86_64-apple-ios -p botho-wallet --lib

# iOS device (aarch64)
cargo build --target aarch64-apple-ios -p botho-wallet --lib
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                  React Native App                    │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │   Screens   │  │ Components  │  │   Hooks     │ │
│  └─────────────┘  └─────────────┘  └─────────────┘ │
├─────────────────────────────────────────────────────┤
│              Native Modules (Swift/Kotlin)           │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │  Keychain   │  │ Biometrics  │  │   Wallet    │ │
│  │   Bridge    │  │   Bridge    │  │   Bridge    │ │
│  └─────────────┘  └─────────────┘  └─────────────┘ │
├─────────────────────────────────────────────────────┤
│                 UniFFI Generated Bindings            │
├─────────────────────────────────────────────────────┤
│                    Rust Core Library                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │ WalletKeys  │  │ Transaction │  │   RpcPool   │ │
│  │             │  │   Builder   │  │             │ │
│  └─────────────┘  └─────────────┘  └─────────────┘ │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐ │
│  │  Encrypted  │  │   Crypto    │  │   Network   │ │
│  │   Wallet    │  │  (PQ-safe)  │  │    Sync     │ │
│  └─────────────┘  └─────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────┘
```

## Security Model

### Key Protection (Matching Desktop Wallet)

1. **Mnemonic Never Leaves Rust**: Same pattern as `src-tauri/src/wallet.rs`
2. **Session-Based Access**: Keys decrypted on unlock, zeroized on lock
3. **Auto-Lock Timeout**: 15-minute inactivity timeout
4. **iOS Keychain**: Encrypted wallet stored with biometric protection

### iOS-Specific Security

```swift
// Keychain access control
let accessControl = SecAccessControlCreateWithFlags(
    nil,
    kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    [.biometryCurrentSet, .or, .devicePasscode],
    nil
)
```

- `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`: No iCloud sync
- `.biometryCurrentSet`: Invalidate if biometrics change
- Certificate pinning for RPC connections

### Android-Specific Security

`expo-secure-store` is cross-platform, so the same `keychain.ts` storage layer
backs Android via the **Android Keystore** + `EncryptedSharedPreferences` — no
separate native module or per-platform caller branching is needed.

- **Android Keystore + `EncryptedSharedPreferences`**: the encrypted wallet blob
  lives in `EncryptedSharedPreferences`, encrypted with a Keystore-held AES key.
- **`requireAuthentication: true`** maps to `BiometricPrompt` + a Keystore key
  created with `setUserAuthenticationRequired(true)` — the Android analog of the
  iOS `SecAccessControl` biometry flags. It requires an enrolled biometric or
  device credential (PIN/pattern/passcode); with none enrolled, `keychain.ts`
  surfaces a typed `SecureStoreUnavailableError` (asks the user to set a screen
  lock) instead of silently storing the wallet unauthenticated.
- **No-cloud-backup**: Keystore-backed data is excluded from Android Auto Backup
  by default — the security intent behind iOS's
  `WHEN_UNLOCKED_THIS_DEVICE_ONLY`, achieved via a different platform mechanism.
- **StrongBox / TEE hardware backing is device-dependent and NOT enforced**:
  whether the key lands in a hardware-backed StrongBox / TEE keymaster vs. a
  software keystore is a property of the device hardware. There is no public
  `expo-secure-store` API to require StrongBox, so the app does not attempt to
  condition behavior on it — this is a documented platform variance, not a
  guarantee. The iOS-only `keychainAccessible` option is silently ignored by
  `expo-secure-store` on Android.

## Rejected Alternatives

### Flutter

- **ffigen** is less mature than UniFFI
- Dart ecosystem has fewer crypto wallet libraries
- Team would need to learn new language

### Native Swift/Kotlin

- 2x development effort (separate iOS and Android codebases)
- Harder to maintain consistency across platforms
- Slower iteration speed

## Next Steps

1. Set up React Native project with Expo (managed workflow)
2. Create UniFFI bindings for `botho-wallet` core APIs
3. Implement iOS Keychain native module
4. Build balance display and transaction history screens
