# Desktop runtime floor: investigation, decision pending

Part of #1327. Source baseline: `1fb1c567` (2026-09-23).

**Decision for this increment:** retain the existing targets and bundle metadata;
keep the runtime support gate open. Neither successful bundling nor a modern-host
smoke test establishes a minimum supported host. No platform minimum is ratified
here. The recommended next decision is a tested modern-WebKit/WebView2 floor,
rather than emulating exact integers on engines without BigInt.

## Required surfaces

Paths below are repository-relative, reviewed at the baseline above.

| Surface | Actual desktop route | Required evidence |
|---|---|---|
| Amounts | `web/packages/core/src/format.ts` parses decimal input with BigInt; desktop `src/contexts/wallet.tsx` parses balances and sends amount/custom fee as decimal strings | Values above 2^53 and fractional picocredits survive parse, arithmetic and native IPC exactly. Display formatting currently uses Number; do not treat rounded display text as a serialization round-trip. |
| Wallet loading/signing | Desktop `src/contexts/wallet.tsx` invokes native unlock/session/send commands; `src-tauri/src/wallet.rs` retains native key material | Owned public fixture unlock, session, lock and native amount preflight on the actual webview. Existing smoke does not sign or broadcast. |
| Browser APIs | Desktop contexts use localStorage; adapters `src/local.ts` use fetch, WebSocket/EventSource; dashboard uses Intl.NumberFormat; shared UI uses clipboard | Secure-origin/IPC behavior, persistence, locale display, clipboard and controlled local transport tests on each proposed platform. |
| Cryptography | Shared core address/claim-link code contains BigInt arithmetic; web-wallet vault has WebCrypto, and web-wallet signer has WASM | Do not import web-wallet vault/WASM requirements wholesale into desktop. Reviewed desktop signing route is native, and generated desktop JS has no `WebAssembly` or `wasm` match. This is an inventory observation, not proof of absence from all future dependency paths. |
| Styling | Desktop Vite imports Tailwind 4 and shared UI/theme/graph | Visual/layout smoke is required, including send modal and graph; JavaScript syntax compatibility alone is insufficient. |

## Platform constraints, not support promises

- Current Vite target is Safari 13 for macOS **and Linux**, Chrome 105 for
  Windows (`web/packages/desktop/vite.config.ts`). These are transform targets,
  not measured native runtime versions.
- No macOS minimum override exists in `src-tauri/tauri.conf.json`. Installed
  Tauri CLI 2.11.4 schema defaults to 10.13, matching the
  [official configuration reference](https://v2.tauri.app/reference/config/#macconfig).
  Inspect the actual built Info.plist before asserting a release bundle floor.
- [WebKit introduced JS BigInt in Safari 14](https://webkit.org/blog/11340/new-webkit-features-in-safari-14/).
  That establishes a missing capability in Safari 13, not a sufficient macOS floor.
- [Tailwind 4's documented baseline](https://tailwindcss.com/docs/compatibility)
  includes Safari 16.4 and Chrome 111. Thus even Safari 14 and the current Chrome
  105 target are insufficient as dependency-support claims. Start platform
  qualification at engines meeting these requirements; ratify exact OS/runtime
  combinations only after native testing.
- [Tauri uses platform webviews](https://v2.tauri.app/reference/webview-versions/):
  WKWebView on macOS, WebView2 on Windows and WebKitGTK on Linux. Record the
  actual engine build, architecture and OS/package versions. A Safari target
  cannot establish WebKitGTK compatibility; an evergreen installer default
  cannot prove an offline/stale Windows runtime meets the floor.

## Reproduction and measured scope

In `web`, with Node 22.23.2 / pnpm 9.15.9 (exact captured install output in
`evidence.json`), run:

```sh
CI=true pnpm install --frozen-lockfile
TAURI_PLATFORM=macos pnpm build:desktop
TAURI_PLATFORM=windows pnpm build:desktop
pnpm exec vitest run packages/core/src/format.test.ts
```

From repository root:

```sh
python3 -m unittest discover -s web/packages/desktop/smoke -p test_isolation.py
```

Both frontend builds passed on macOS 27.0 (26A428). macOS emitted 34
`TOLERATED_TRANSFORM` diagnostics; Windows emitted none. These are **frontend
builds on macOS**, not Windows execution or native installers. Nine amount unit
tests and four existing smoke isolation tests passed. Node execution is not
WKWebView conformance. Source/config/lock and raw build/test log digests are in
`evidence.json`; local raw logs are retained at the listed paths. No native app
was launched, user wallet opened, node contacted, or funds used for this capture.

## Remaining release decision and acceptance

Use the existing isolated harness in
[`web/packages/desktop/smoke/README.md`](../../../web/packages/desktop/smoke/README.md)
(#1367), subject to its source/isolation review and execution authorization.
Do not replace it with browser mocks or duplicate its native fixture.

1. Select exact candidate minimum OS/engine combinations meeting JS **and CSS**
   needs; provision those hosts. The available macOS 27 host cannot substitute
   for the lowest proposed host. Older-host retention requires a separately
   scoped compatibility architecture, never monetary Number substitution.
2. Collect source/assets/binary hashes, native bundle metadata, OS/engine identity,
   raw execution results and cleanup evidence. Exercise startup, wallet load,
   exact amount preflight/IPC, shared send UI and graph. Run the same checks on
   Windows/WebView2 and Linux/WebKitGTK. Record untested APIs explicitly.
3. Once the platform decision is reviewed, update explicit bundle minimum,
   Vite JS/CSS targets, user support documentation and CI matrix together.
   Add release failure on unsupported-transform diagnostics; include an early
   compatibility notice that runs before loading unsupported application syntax.
4. Gate release on minimum-host native receipts, separately from build/unit CI.
   Existing smoke is preflight-only; it cannot establish full transaction
   construction or network correctness. Keep those acceptance claims separate.

Issue #1327 remains open until the chosen minimum is actually exercised.
