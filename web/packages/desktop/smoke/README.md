# Isolated native runtime smoke (draft, not executed)

Issue #1367, part of #1327. This fixture application exercises current-host
WKWebView, real Tauri IPC, the shared theme, NetworkGraph and SendModal. It does
not establish a minimum supported OS, sign/build transactions, contact a node,
or broadcast. Native launch remains held for source/isolation review; build-only collection is authorized.

The ordinary app entry is unchanged. Cargo's default binary remains
`botho-desktop`; `botho-runtime-smoke` requires the nondefault `runtime-smoke`
feature and an explicit runtime opt-in. Even an all-features ordinary app does
not register the automation plugin or fixture commands. No normal App,
ConnectionProvider or WalletProvider is mounted.

The separate identifier is `com.botho.desktop.runtime-smoke`, with a single
programmatically created incognito window and packaged frontend. Navigation
accepts only `tauri://localhost`; CSP connections allow Tauri IPC only. There
is no discovery/send/sync/faucet or filesystem command registration. A fresh
TempDir contains a public BIP-39 test-vector encrypted wallet. The wrapper
accepts only its exact canonical path, supplies the known test password and
calls the existing native unlock implementation. Session and lock commands are
also the real implementations. No default-wallet path is accepted. The pure
recipient/u64/nonzero preflight is shared with production; session lookup and
validation ordering remain unchanged. Preflight does not claim transaction
construction, signing, fee validation or network execution.

## Driver and dependency review

The exact optional `tauri-plugin-wdio-webdriver = 1.4.0` has no declared default
feature set. Its `src/server/mod.rs:76` uses hardcoded `127.0.0.1` for desktop
(Android differs and is outside this harness). `init_with_port` ignores the
plugin's environment/default port. The runner chooses an ephemeral loopback
port and makes no proxy requests. The port reservation is released before the
app binds; another local process could race it. Native fixture IPC assertions
must succeed, not merely the readiness endpoint. This is unauthenticated local
automation over a disposable known fixture, never a production route.

[Tauri's current guidance](https://v2.tauri.app/develop/tests/webdriver/)
documents this embedded server for macOS. The bounded Python standard-library
client directly uses its W3C endpoints, avoiding an additional npm driver tree.
It does not use Chrome browser mode, IPC mocking, or the separate backend-code
execution plugin. The embedded driver's DOM interaction is native-webview
coverage, not hardware keyboard/mouse or accessibility validation.

Cargo resolution adds the driver and 11 transitive packages, plus the required
Objective-C framework feature edges. Existing package versions are retained;
unrelated broad-version lock retargeting was restored and `cargo metadata
--locked` accepted the graph. No pnpm dependencies/lock changes are needed.

## Review-stage checks

- `pnpm install --frozen-lockfile --ignore-scripts`
- `pnpm --filter @botho/desktop exec tsc --project tsconfig.smoke.json --noEmit`
- `python3 -m unittest discover -s web/packages/desktop/smoke -p test_isolation.py`
- Scoped Rust formatting, Python syntax and `git diff --check`.

These are static checks, not native build/runtime evidence. The launch runner
is unexecuted and must be reviewed before use. After authorization: separately
build the packaged frontend with `vite build --config vite.smoke.config.ts`,
then the exact Cargo binary/feature with JSON artifact output and a bounded
compile deadline. The collector sets `TAURI_CONFIG` to the smoke config for
both `tauri-build` and the generated context. Pass that Cargo JSON and exact executable to
`smoke/native_smoke.py --build-manifest BUILD_DIRECTORY/build.json --output NEW_DIRECTORY`.
The runner launches once, bounds execution to 90 seconds, preserves raw logs
and results, and records current source/assets/binary/OS/WebKit identity.
The build-only collector `build_smoke.py --output NEW_BUILD_DIRECTORY
--target-dir ISOLATED_CACHE` records all Git-visible file hashes before and after
its one frontend invocation (180s) and one native compile (900s), plus asset
hashes before/after native compilation, environment, exact Cargo artifact/profile
and executable digest. Timeouts terminate the owned process group. The launcher
requires and validates this manifest before starting anything; source/asset or
executable drift fails closed. No native process starts in the build collector.

The runner closes the only window for normal fixture destruction. On failure it
terminates/kills only its owned process, checks the logged fixture directory,
and marks surviving files as failed cleanup rather than recursively deleting
an unchecked log-derived path. No automatic reruns. Actual source/build/launch
results and any limitations must replace this draft status before publication.
The oldest-host/support-floor requirement remains open in #1327.
