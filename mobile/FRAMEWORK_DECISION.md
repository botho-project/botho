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
