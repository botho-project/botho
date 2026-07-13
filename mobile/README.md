# Botho Mobile Wallet

Mobile wallet for Botho cryptocurrency with post-quantum cryptography support.

## Architecture

```
mobile/
├── FRAMEWORK_DECISION.md    # Framework selection rationale
├── rust-bridge/             # Rust FFI crate (UniFFI bindings)
│   ├── Cargo.toml
│   ├── build.rs
│   └── src/
│       ├── lib.rs           # Rust implementation
│       └── botho_mobile.udl # UniFFI interface definition
├── app/                     # React Native app (Expo)
│   ├── app/                 # Expo Router screens
│   ├── src/
│   │   ├── components/      # Reusable UI components
│   │   ├── hooks/           # Custom React hooks
│   │   ├── native/          # Native module bridges
│   │   ├── screens/         # Screen components
│   │   ├── store/           # Zustand state management
│   │   └── types/           # TypeScript types
│   └── package.json
└── ios/                     # iOS native code (after prebuild)
```

## Getting Started

### Prerequisites

- Node.js 18+
- pnpm
- Rust 1.75+
- Xcode 15+ (for iOS)
- iOS Simulator or device

### Development Setup

```bash
# Install dependencies
cd mobile/app
pnpm install

# Build Rust bindings (first time)
cd ../rust-bridge
cargo build --target aarch64-apple-ios-sim --release

# Generate UniFFI bindings
cargo run --features=uniffi/cli -- generate \
  --library target/aarch64-apple-ios-sim/release/libbotho_mobile.a \
  --language swift \
  --out-dir ../app/ios/Generated

# Start development server
cd ../app
pnpm start
```

### Running on iOS Simulator

```bash
cd mobile/app
pnpm ios
```

## Security Model

### Key Protection

- **Mnemonic never leaves Rust memory**: All cryptographic operations happen in Rust
- **Zeroization on drop**: Keys are securely wiped when wallet locks
- **Session timeout**: 15-minute auto-lock for inactivity

### iOS Keychain

- `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`: No iCloud backup
- Biometric protection (Face ID / Touch ID)
- Hardware-backed encryption (Secure Enclave where available)

### Android Keystore

The same `expo-secure-store`-backed storage layer (`app/src/native/keychain.ts`)
runs on Android with no per-platform caller branching — parity with the iOS path:

- **Android Keystore + `EncryptedSharedPreferences`**: the encrypted wallet blob
  is stored in `EncryptedSharedPreferences`, encrypted with an Android
  Keystore-held AES key. Analogous to iOS's `WHEN_UNLOCKED_THIS_DEVICE_ONLY`,
  Keystore-backed data is excluded from Android Auto Backup by default, so the
  wallet is not synced to the cloud.
- **Biometric / device-credential protection**: `requireAuthentication: true`
  routes reads/writes through Android's `BiometricPrompt` and creates the
  Keystore key with `setUserAuthenticationRequired(true)`. This requires the
  device to have a biometric or device credential (PIN/pattern/passcode)
  enrolled — on a device with "Swipe"/"None" screen lock, `keychain.ts` throws a
  typed `SecureStoreUnavailableError` prompting the user to set up a screen lock,
  rather than silently downgrading to unauthenticated storage.
- **Hardware backing is device-dependent**: whether the Keystore key lands in a
  hardware-backed **StrongBox** / TEE keymaster vs. a software keystore depends
  on the device's hardware. There is no public `expo-secure-store` API to *force*
  StrongBox, so the app does **not** enforce it — this is a known platform
  variance, not a guarantee. (Note: `keychainAccessible` in `SECURE_OPTIONS` is
  iOS-only and is silently ignored by `expo-secure-store` on Android.)

### Network Security

- Certificate pinning for RPC connections
- TLS 1.3 required

## Framework Decision

See [FRAMEWORK_DECISION.md](./FRAMEWORK_DECISION.md) for detailed rationale on choosing React Native with UniFFI.

**Summary**: React Native provides the best balance of:
- Mature Rust FFI (UniFFI from Mozilla)
- Large ecosystem for wallet features
- Team expertise alignment (existing TypeScript stack)
- Cross-platform support (iOS + Android)

## Related

- [Desktop Wallet](../web/packages/desktop/) - Tauri-based desktop wallet
- [botho-wallet](../botho-wallet/) - Core wallet library
- [account-keys](../account-keys/) - Key derivation and management
