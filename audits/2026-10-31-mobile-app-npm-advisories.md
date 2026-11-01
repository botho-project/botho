# mobile/app npm advisories (incl. critical shell-quote): resolution and exposure

**Date**: 2026-10-31
**Auditor**: Loom Builder (#1769, split from #1765)
**Scope**: Every finding from a full `pnpm audit` of the standalone
`mobile/app` pnpm project (pnpm 11.28, the major Mobile CI uses). This covers
the lock graph, the Expo CLI/config/prebuild/fingerprint tooling, the jest-expo
test harness, and the shipped JS bundle (`expo export`). Out of scope: the
other JS lockfiles (`web/` #1745/#1766, `contracts/*` #1767/#1768) and any
Expo SDK or React Native major upgrade.
**Commit**: origin/main @ `f11488ec`.
**Status**: **critical RESOLVED (not reachable at runtime before the fix
either). 18 of 20 high RESOLVED.** Residuals: decode-uri-component (in the
bundle, but not reachable from incoming-URL handling), zod in `@expo/cli`
(dev server only, and the vulnerable v4 code is never loaded), deepmerge (no
upstream fix) and minimatch 3.1.5 (the vulnerable code is not in it). No audit
exception or ignore entry was added.

---

## Summary

| | Before | After |
|---|---|---|
| `pnpm audit` (mobile/app, pnpm 11) | 39 (1 critical, 20 high, 12 moderate, 6 low) | 5 (0 critical, 2 high, 2 moderate, 1 low) |
| `pnpm audit --prod` | 38 (1 critical, 19 high, 12 moderate, 6 low) | 5 (same as above) |
| Packages in the release JS bundle with an open finding | decode-uri-component | decode-uri-component (unchanged, see below) |

The #1765 baseline of 41 (6 low, 13 moderate, 21 high, 1 critical) was taken
with pnpm 9. pnpm 9 cannot audit this project any more (`packages field
missing or empty`, because `pnpm-workspace.yaml` only carries settings), and
pnpm 11 groups two advisories differently. The finding set is the same.

`--prod` is not a useful "shipped" filter here. `expo` lists `@expo/cli`,
`@expo/config`, `@expo/fingerprint` and friends as regular dependencies, so
almost all build tooling counts as "prod". Bundle reachability below was
decided from the release bundle's source map instead.

## How bundle reachability was decided

`CI=1 npx expo export --platform android --source-maps --no-bytecode` (release
mode), before and after, then every `node_modules/<pkg>` path in the source
map's `sources` was listed. Both bundles contain the same 64 packages.
The only package with an advisory in either bundle is **decode-uri-component**
(through `query-string`, a dependency of `expo-router`). None of shell-quote,
ip, joi, fast-xml-parser, node-forge, zod, glob, minimatch, brace-expansion,
js-yaml, browserslist, baseline-browser-mapping, source-map-js, uuid, deepmerge
or react-devtools-core is in the release bundle.

## Findings and disposition

### Critical: shell-quote 1.10.0 (GHSA-pqg4-j6r4-53mv), RESOLVED

`quote()` command injection when a `{ comment }` token is followed by a string
with a line terminator. Two paths:

1. `react-native` → `@react-native/community-cli-plugin` → (optional peer)
   **`@react-native-community/cli@12.1.1`** → `cli-tools` → shell-quote.
2. `react-native` → `react-devtools-core@6.1.5` → shell-quote.

**Runtime reachability: none, before or after.** shell-quote is absent from the
release bundle. In path 2 it is used only by the Node-side React DevTools
standalone editor launcher, and only as `parse(process.env.REACT_EDITOR)`,
never `quote()`. Path 1 is a CLI that this app never runs.

**Fix.**
- Path 1: `@react-native-community/cli@12.1.1` is an RN 0.73-era CLI. The
  lockfile had kept it as the resolution of
  `@react-native/community-cli-plugin`'s *optional* `'*'` peer since before SDK
  57. Expo apps start, build and prebuild through `@expo/cli`, and nothing in
  `package.json` or the scripts runs `react-native <cmd>`. A scoped override
  `'@react-native/community-cli-plugin>@react-native-community/cli': '-'`
  removes it. That one change also clears every **ip**, **joi** and
  **fast-xml-parser** finding and two of the four deepmerge paths. These
  packages had no parent bump. The newest `@react-native-community/cli` (20.x)
  is unrelated to RN 0.86's plugin contract and is not needed.
- Path 2: re-resolved within `react-devtools-core`'s `^1.6.1` range to
  **shell-quote 1.12.0**.

### High and moderate findings resolved by in-range re-resolution

`pnpm up --depth Infinity <pkg>`. These are lockfile-only changes, and every
new version is inside the parent's declared range:

| Package | Before → After | Advisories | Parent(s) |
|---|---|---|---|
| browserslist | 4.28.2 → 4.29.5 | GHSA-c83g-rgw3-j3cx, GHSA-73wf-gq98-2v4g | `@babel/helper-compilation-targets` (build) |
| baseline-browser-mapping | 2.10.38 → 2.11.34 | GHSA-w5vr-8v7q-w6rv | browserslist (build) |
| brace-expansion | 1.1.18 / 5.0.9 → 1.1.21 / 5.0.12 | GHSA-6j4f-fj2g-mc7p, GHSA-qhr7-859c-m2p7, GHSA-q2hr-2g5m-vwhr | minimatch (tooling) |
| js-yaml | 3.15.1 / 4.3.1 → 3.15.2 / 4.3.2 | GHSA-2883-xcg3-v3hh | cosmiconfig, istanbul, eslintrc (tooling) |
| node-forge | 1.4.0 → 1.4.2 | 12 advisories (TLS/PKCS#12/X.509/ASN.1) | `@expo/cli`, `@expo/code-signing-certificates` (dev server HTTPS and EAS code signing) |
| source-map-js | 1.2.1 → 1.2.2 | GHSA-68fv-2mgg-jv7q | postcss (build) |
| zod (4.x line) | 4.6.2 → 4.6.7 | GHSA-mrhx-c7xc-m3w5 | `eslint-plugin-react-hooks` (lint) |

### Moderate findings resolved by scoped overrides (in `pnpm-workspace.yaml`)

pnpm 11 reads overrides from `pnpm-workspace.yaml` (where `react-dom` was
already pinned), so the new entries go there.

| Override | Advisory | Why an override |
|---|---|---|
| `@expo/{cli,config,config-plugins,fingerprint,metro-config}>glob` → 14.0.3 | GHSA-3g3m-2fjr-6pw4 (glob ≥ 9 ≤ 14.0.1) | The fix exists only in glob 14. The latest `@expo/*` (including SDK 58) still asks for `glob ^13` |
| `@expo/fingerprint>minimatch`, `@typescript-eslint/typescript-estree>minimatch` → 11.0.3 | GHSA-g3hv-75wg-m249 (minimatch < 11.0.1) | The fix exists only in minimatch 11. `@expo/fingerprint@latest` still asks for `^10.2.2` |
| `xcode>uuid` → 11.1.1 | GHSA-w5hq-g745-h8pq (uuid < 11.1.1) | `xcode@3.0.1` (latest) pins `uuid ^7`. uuid 11 still ships CJS (`dist/cjs`) |

**API compatibility (glob 14 / minimatch 11 / uuid 11 are majors):**

- glob 14 drops the `debug` option, changes `Pattern`/`GlobWalker` internals and
  roots `ignore` patterns that start with `/` at the root instead of the cwd.
  Every Expo call site uses only `glob`, `globSync`, `sync` or `globStream` with
  `{cwd, absolute, ignore, nodir, dot}`. None passes `debug`. The only `ignore`
  values are `'**/@(Carthage|Pods|vendor|node_modules)/**'`,
  `'**/@(Carthage|Pods|node_modules)/**'` and `['node_modules']`, and none starts
  with `/`.
- minimatch 11 removes `makeRe`, `Minimatch#regexp`, `AST` and `debug`.
  `@expo/fingerprint` only builds `new Minimatch(p, {dot: true})` and reads
  `.match()`, `.negate` and `.pattern`. typescript-estree only calls
  `minimatch(path, pattern, {dot: true})`.
- xcode only calls `uuid.v4()` (`pbxProject.generateUuid`). It does not use the
  v3/v5/v6 APIs that the advisory affects.

**Behaviour checks:**

- `npx expo prebuild --no-install` (both platforms) on main and on this branch,
  run in scratch copies: the generated `android/` and `ios/` trees are
  **byte-identical** (`diff -r` is empty). Prebuild exercises config-plugins
  globbing, `@expo/config` and xcode project writing.
- With uuid 11, `xcode.project(...).parseSync()`, then `generateUuid()` (24-char
  id), then `addSourceFile()` and `writeSync()` all work on the generated
  `BothoWallet.xcodeproj`.
- `@expo/fingerprint` (glob 14 + minimatch 11): both trees hash the same 46
  sources. The only differing hashes are the four autolinking configs. After
  the pnpm virtual-store directory names are normalized out, their contents
  are identical. The store names changed because the removed optional peer
  was part of every `react-native@0.86.3(...)` peer suffix.
- `npx expo config --type public` resolves (name, slug, scheme `botho`, SDK 57).

### Also bumped: expo 57.0.31 / expo-router 57.0.29 (SDK 57 patches)

`expo-doctor` failed on main ("2 packages out of date": `expo ~57.0.31`,
`expo-router ~57.0.29`). That failure is Expo's own SDK-57 version set moving
on, not something this PR caused. Both are same-SDK patch releases (published
2026-10-21) and do not change the audit result. They are bumped so the Mobile
CI `expo-doctor` step passes (21/21 checks).

## Residual findings (no exception added; they stay visible in `pnpm audit`)

### decode-uri-component 0.2.2: high GHSA-qr28-fpjm-wq7f, moderate GHSA-vcc3-ghjq-m6fr. **In the bundle**

- **Where:** `expo-router@57.0.29` → `query-string@7.1.3` (`^7.1.3`) →
  `decode-uri-component@^0.2.2`. This is the only finding in the release
  bundle.
- **Defect:** CPU DoS (exponential, or quadratic in 0.5.0) in the fallback
  decoder, which runs only when native `decodeURIComponent` throws on
  malformed `%`-sequences.
- **Why no fix:** the patched releases (0.5.0, 0.5.1) are ESM-only
  (`"type": "module"`, default export). `query-string@7` does
  `const decodeComponent = require('decode-uri-component'); decodeComponent(x)`.
  Under Metro's interop that `require` returns a namespace object, so an
  override would turn every `queryString.parse` into a TypeError. The fixed
  query-string majors (8+) are ESM too, still use the vulnerable 0.4 range,
  and are outside expo-router's `^7.1.3`. No expo-router release (SDK 57 or 58)
  changes this.
- **Reachability in this app:** decode-uri-component runs only from
  query-string's `parse`/`parseUrl`/`stringifyUrl`/`pick`/`exclude`. In
  expo-router:
  - Incoming deep links (`botho://…`) go through
    `getLinkingConfig().getStateFromPath` → `fork/getStateFromPath`, which
    parses queries with the platform `URL`/`URLSearchParams`
    (`parseQueryParams` → `parseUrlUsingCustomBase(path).searchParams`). The
    original `queryString.parse` call there is commented out in the fork.
  - `queryString.stringify` (outgoing path building in `getPathFromState`)
    never decodes.
  - The one remaining `queryString.parse` caller is the vendored
    `react-navigation/core/getStateFromPath`. It is used only as a default
    when a linking config does not supply `getStateFromPath`. expo-router
    always supplies one, and the app's own code (`app/`, `src/`) imports
    neither query-string nor any linking/`getStateFromPath` API.

  So the module is bundled but no attacker-controlled URL reaches it. Even if
  one did, the impact would be a frozen JS thread in the user's own app, an
  availability problem that a restart clears. Keys, seed and funds are not
  exposed.
- **Disposition:** accepted residual. Re-check when expo-router drops
  query-string 7 or decode-uri-component ships a CJS fix. If future app code
  starts calling `queryString.parse` on external input, cap the input length
  first.

### zod 3.25.76 in `@expo/cli`: high GHSA-mrhx-c7xc-m3w5. Dev server only

- **Where:** `expo` → `@expo/cli@57.0.32` (`zod ^3.25.76`, unchanged in the
  latest SDK 58 CLI). Not in the bundle.
- **Defect:** memory exhaustion in `$ZodArray`'s `handleArrayResult`. That code
  is in the **zod v4 core**. zod 3.25.x ships it only under the `zod/v4`
  subpath.
- **Not loaded:** `@expo/cli` does `require("zod")`, which resolves to
  `index.cjs` → `./v3/external.cjs`. Loading it pulls in **zero** `/v4/`
  modules (checked through `require.cache`). `handleArrayResult` exists only in
  `v4/core/schemas.*`.
- **Inputs:** devtools-plugin manifests from installed packages
  (`DevToolsPlugin.js`) and JSON output from devtools-plugin CLI extensions
  (`DevToolsPluginCliExtensionResults.js`). Both come from code the developer
  already chose to install and run.
- **Why no override:** forcing zod 4 on a v3-API consumer is a major API break
  in the dev server.
- **Disposition:** accepted dev-only residual.

### deepmerge 4.3.1: low GHSA-gcqx-w48m-w2h4. No upstream fix

- **Where:** `@expo/config@57.0.10` (`mergeConfigModifications`, used by
  `modifyConfigAsync` when `expo install`/config plugins write `app.json`) and
  `jest-config@29.7.0` (`globals` preset merge). Not in the bundle. The two
  `@react-native-community/cli*` paths went away with that package.
- **Inputs:** the committed `app.json`, CLI-generated modifications, and the
  committed jest preset/config. All are developer-controlled. Anyone who can
  change them can already run code in the build.
- **Disposition:** accepted dev-only residual (same reasoning as #1766's web
  analysis). deepmerge has had no release since 2023-03.

### minimatch 3.1.5: moderate GHSA-g3hv-75wg-m249. Vulnerable code not present

- **Where:** `glob@7.2.3` and `test-exclude@6.0.0` (jest 29 coverage), plus
  the eslint 9 stack. Not in the bundle.
- **Why it stays:** the advisory describes `firstPhasePreProcess` /
  `optimizationLevel`. Grepping the installed tree finds them only in
  minimatch 11.0.3 (where they are fixed), not in 3.1.5. No 3.x fix exists.
  `glob@7` and `test-exclude@6` call the CJS module as a function and read
  `Minimatch#set` as RegExps, so minimatch 11 would break them. Patterns come
  only from committed config.
- **Disposition:** accepted dev-only residual. It goes away with jest 30
  (`jest-expo` moves there with a future SDK).

## Verification

Run from `mobile/app` with pnpm 11.28.10 and Node 26 (CI: Node 22):

| Step | main (`f11488ec`) | this branch |
|---|---|---|
| `pnpm install --frozen-lockfile` (clean `node_modules`) | ok | ok |
| `pnpm typecheck` | ok | ok |
| `pnpm test` | 5 suites, 65 tests pass | 5 suites, 65 tests pass |
| `pnpm lint` | 0 errors, 4 warnings | 0 errors, 4 warnings (same) |
| `pnpm peers check` | 2 pre-existing (worklets, metro-config) | same 2 |
| `npx expo-doctor` | **fails** (expo/expo-router out of date) | 21/21 pass |
| `pnpm audit` | 39 | 5 |

## Re-check procedure

```sh
cd mobile/app
pnpm install --frozen-lockfile                 # pnpm 11.x
pnpm audit                                     # expect decode-uri-component x2, zod (@expo/cli), deepmerge, minimatch@3.1.5
pnpm why decode-uri-component zod deepmerge minimatch glob uuid
npm view expo-router@latest dependencies.query-string
npm view @expo/cli@latest dependencies.zod dependencies.glob
npm view @expo/fingerprint@latest dependencies.minimatch
npm view xcode@latest dependencies.uuid
```

When a parent ships a patched range, drop the matching override from
`pnpm-workspace.yaml`.
