# Mobile Native Integration (rust-bridge ↔ React Native)

This document describes how the React Native app (`mobile/app`) links to the
Rust wallet core (`mobile/rust-bridge`, the `botho-mobile` crate). It is the
remaining device/native step that cannot be exercised headlessly.

## What is already wired (verified headlessly)

- **walletStore** (`src/store/walletStore.ts`) is fully un-mocked. Every action
  calls the real bridge surface through `src/native/walletModule.ts`
  (`NativeWallet`): `unlock`, `lock`, `checkSession`, `refreshBalance`,
  `refreshTransactions`, `send`, `requestFaucet`, `setNodeUrl`,
  `refreshNodeStatus`, plus node-URL hydration/persistence.
- **Typed native wrapper** (`src/native/walletModule.ts`) resolves the native
  module via `requireOptionalNativeModule("BothoWallet")` and adapts FFI value
  shapes (notably 64-bit ints marshalled as strings → JS `bigint`).
- **UniFFI bindings generate** from the compiled library:
  - Swift: `mobile/app/ios/Generated/botho_mobile.swift` (+ `.h`, `.modulemap`)
  - Kotlin: `mobile/app/android/generated/uniffi/botho_mobile/botho_mobile.kt`
- **Type safety**: `pnpm -C mobile/app typecheck` is green.

What is NOT done (requires a device/simulator + Xcode/Android toolchains): the
Expo native module that re-exposes the generated `MobileWallet` to JS, and a
full on-device run. UniFFI generates the Rust↔Swift/Kotlin layer but not the
React Native bridge — that thin module is written below.

Also NOT done, and a prerequisite for the wallet-backup feature designed in
[`MOBILE_BACKUP_DESIGN.md`](./MOBILE_BACKUP_DESIGN.md): the `BothoWallet` bridge
will additionally need to forward **backup/restore** FFI methods once the Rust
`export_encrypted_backup` / `import_encrypted_backup` surface exists (see that
doc, §6.2 and §9). An iCloud-Keychain-synchronizable backup on iOS further
requires a native `SecItemAdd(... kSecAttrSynchronizable: true ...)` path, which
`expo-secure-store` cannot produce (see `MOBILE_BACKUP_DESIGN.md` §5) — that too
would live in this bridge.

## Step 1 — Build the Rust static libs for the target platforms

```sh
# from repo root

# iOS device + simulator
cargo build -p botho-mobile --release --target aarch64-apple-ios
cargo build -p botho-mobile --release --target aarch64-apple-ios-sim
cargo build -p botho-mobile --release --target x86_64-apple-ios   # Intel sim

# Android
cargo build -p botho-mobile --release --target aarch64-linux-android
cargo build -p botho-mobile --release --target armv7-linux-androideabi
```

(For iOS, combine the simulator slices into an XCFramework with `xcodebuild
-create-xcframework`.)

## Step 2 — (Re)generate the UniFFI bindings

```sh
# from repo root
cargo build -p botho-mobile                       # host build for the dylib
cargo run -p botho-mobile --bin uniffi-bindgen -- \
  generate --library target/debug/libbotho_mobile.dylib \
  --language swift  --out-dir mobile/app/ios/Generated
cargo run -p botho-mobile --bin uniffi-bindgen -- \
  generate --library target/debug/libbotho_mobile.dylib \
  --language kotlin --out-dir mobile/app/android/generated
```

These outputs are committed so the bindings are reviewable without the Rust
toolchain.

## Step 3 — Run `expo prebuild`

```sh
cd mobile/app
pnpm install
pnpm prebuild          # generates ios/ and android/ native projects
```

## Step 4 — Add the `BothoWallet` Expo native module

UniFFI produces a `MobileWallet` Swift class / Kotlin class but no RN bridge.
Add a small Expo module that holds one `MobileWallet` instance and forwards
calls. The JS side (`walletModule.ts`) already expects a module registered as
`"BothoWallet"` with these async methods (note 64-bit values are passed/returned
as **decimal strings**):

```
setNodeUrl(url: string): Promise<void>
generateWallet(): Promise<string>
unlockWithMnemonic(mnemonic: string): Promise<WalletAddress>
lock(): Promise<boolean>
getSessionStatus(): Promise<SessionStatus>
getBalance(): Promise<WalletBalance>          // picocredits/syncHeight as string
getTransactionHistory(limit, offset): Promise<TransactionEntry[]>  // amount as string
getAddress(): Promise<WalletAddress>
sendTransaction(toAddress: string, amountPicocredits: string): Promise<string>
requestFaucet(): Promise<FaucetResult>        // amount as string
getNodeStatus(): Promise<NodeStatusInfo>      // chainHeight as string
```

### iOS (Swift, `ios/BothoWallet/BothoWalletModule.swift`)

```swift
import ExpoModulesCore

public class BothoWalletModule: Module {
  // One long-lived wallet instance holds in-session keys.
  private let wallet = MobileWallet()   // from the generated botho_mobile.swift

  public func definition() -> ModuleDefinition {
    Name("BothoWallet")

    AsyncFunction("setNodeUrl") { (url: String) in try await wallet.setNodeUrl(url: url) }
    AsyncFunction("generateWallet") { () -> String in try await wallet.generateWallet() }
    AsyncFunction("unlockWithMnemonic") { (m: String) -> [String: Any] in
      let a = try await wallet.unlockWithMnemonic(mnemonic: m)
      return ["viewPublicKey": a.viewPublicKey, "spendPublicKey": a.spendPublicKey, "display": a.display]
    }
    AsyncFunction("lock") { () -> Bool in await wallet.lock() }
    // ... remaining methods, converting UInt64/Int64 fields to String ...
    AsyncFunction("sendTransaction") { (to: String, amount: String) -> String in
      try await wallet.sendTransaction(toAddress: to, amountPicocredits: UInt64(amount) ?? 0)
    }
  }
}
```

Add the generated `botho_mobile.swift` to the target, link
`libbotho_mobile.a` (the XCFramework), and expose `botho_mobileFFI.h` via the
bridging header / modulemap in `ios/Generated`.

### Android (Kotlin, `android/.../BothoWalletModule.kt`)

Mirror the same definition using the generated
`uniffi/botho_mobile/botho_mobile.kt` and bundle the `.so` libs built in Step 1
under `android/app/src/main/jniLibs/<abi>/`.

## Step 5 — Run

```sh
cd mobile/app
pnpm ios       # or: pnpm android
```

In Expo Go (no native module), `walletModule.ts` reports a clear
`NativeModuleUnavailableError`; the UI still renders for layout work. A custom
dev client / prebuild as above is required for live wallet operations.

## Demo flow (once linked)

1. Pick a node (Seed 1 / Seed 2 / Faucet) — node picker shows each node's
   height/sync/peers via `get_node_status`.
2. Create or import a wallet (setup / unlock).
3. Switch to the Faucet node and request coins; the faucet screen polls the
   balance until it rises.
4. Send to another user's `tbotho://1/...` address; the send screen shows the
   resulting tx hash.
5. Receive: share your own address from the receive screen.
