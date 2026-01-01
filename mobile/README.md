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
- Hardware-backed encryption

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
