# Dev-only web advisories (glob, minimatch, deepmerge): resolution and exposure

**Date**: 2026-10-31
**Auditor**: Loom Builder (#1745)
**Scope**: The `glob`, `minimatch` and `deepmerge` findings from a full
`pnpm audit` of the `web/` pnpm workspace (pnpm 9.15.9, the major CI uses).
This covers the lock graph, the web-wallet build (vite-plugin-pwa / workbox),
the Snap build (`mm-snap build`), the Snap test harness (jest 30 +
`@metamask/snaps-jest`) and the shipped bundles. Out of scope: the elliptic
finding (#1562 / PR #1746), the other JS lockfiles (see "Other workspaces"),
and any deployed artifact.
**Commit**: origin/main @ `ceba3aee`, rebased onto `890a6791` before merge.
**Status**: **glob RESOLVED. minimatch RESOLVED for every line that contains
the vulnerable code. deepmerge UNRESOLVED (no upstream fix), not reachable
by untrusted input.** No audit exception or ignore entry was added.

---

## Summary

| | Before | After |
|---|---|---|
| `pnpm audit` (web/) | 9 findings (2 low, 7 moderate) | 3 findings (2 low, 1 moderate) |
| glob (GHSA-3g3m-2fjr-6pw4) | 10.5.0, 11.1.0, 13.0.6 | none |
| minimatch (GHSA-g3hv-75wg-m249) | 3.1.5, 5.1.9, 9.0.9, 10.2.6 | 3.1.5 only (see below) |
| deepmerge (GHSA-gcqx-w48m-w2h4) | 4.3.1 | 4.3.1 (no patched release) |
| elliptic (GHSA-848j-6mx2-7j84) | 6.6.1 | 6.6.1 (tracked in #1562) |
| `pnpm audit --prod` | clean | clean |

## Findings

| Advisory | Package / range | Defect | Patched |
|---|---|---|---|
| [GHSA-3g3m-2fjr-6pw4](https://github.com/advisories/GHSA-3g3m-2fjr-6pw4) (moderate, published 2026-10-30) | `glob >=9.0.0 <=14.0.1` | When an `AbortSignal` aborts an async walk, the walk rejects but keeps reading directories in the background (`cb()` without `return` in `walker.ts`) | 14.0.2 |
| [GHSA-g3hv-75wg-m249](https://github.com/advisories/GHSA-g3hv-75wg-m249) (moderate, published 2026-10-30) | `minimatch <11.0.1` | An attacker-supplied pattern grows `globParts` in `firstPhasePreProcess()` (the `**/../` fixpoint loop at `optimizationLevel >= 2`), followed by quadratic `secondPhasePreProcess()` | 11.0.1 |
| [GHSA-gcqx-w48m-w2h4](https://github.com/advisories/GHSA-gcqx-w48m-w2h4) (low) | `deepmerge >=4.2.0 <=4.3.1` | `mergeObject()` can write attacker-controlled keys into the returned object's prototype when the *source* object is attacker-controlled | none |

Both glob and minimatch fixes exist only as new majors. No 10.x/11.x/13.x
glob or 3.x/5.x/9.x/10.x minimatch backport was released.

## Upstream fix check (2026-10-31)

| Parent | Latest on npm | Still pins a vulnerable version? |
|---|---|---|
| `@jest/reporters`, `jest-config`, `jest-runtime` | 30.5.2 (in use) | yes, `glob ^13.0.6` |
| `workbox-build` | 7.4.1 (in use); `vite-plugin-pwa` 2.0.0 still uses `workbox-build ^7.4.1` | yes, `glob ^11.0.1` |
| `test-exclude` | 8.0.0; `babel-plugin-istanbul` 8.0.0 (latest) still asks for `^7.0.1` | yes, `glob ^13` / `minimatch ^10` |
| `fork-ts-checker-webpack-plugin` | 9.1.0 (in use, latest; `@metamask/snaps-cli` 8.4.1 asks for `^9.0.2`) | yes, `minimatch ^3.0.4`, `deepmerge ^4.2.2` |
| `@metamask/snaps-jest` | 10.2.2; still on jest 29 (`@jest/expect ^29.5.0`) → `babel-plugin-istanbul@6` → `test-exclude@6` | yes, `minimatch ^3` |
| `filelist` (via `@trickfilm400/rollup-plugin-off-main-thread@3.0.0-pre1` → `ejs@3` → `jake@10`) | 2.0.2 | yes, `minimatch ^10.2.1` |
| `@rollup/plugin-node-resolve`, `jest-config` | 16.0.3 / 30.5.2 (in use) | yes, `deepmerge ^4` |
| `deepmerge` | 4.3.1 (2023-03) | n/a, no release since |

The advisories are a day old, so no parent bump moves any consumer to a patched
version. That is why the fix uses scoped `pnpm.overrides`.

## Resolution: scoped overrides (web/package.json)

Each override is scoped to one parent, in the existing `parent>child` style,
and pins the newest patched release:

| Override | Effect |
|---|---|
| `@jest/reporters>glob`, `jest-config>glob`, `jest-runtime>glob` → `14.0.3` | jest 30 (Snap tests) |
| `workbox-build>glob` → `14.0.3` | web-wallet service-worker precache globbing |
| `test-exclude@7>glob` → `14.0.3`, `test-exclude@7>minimatch` → `11.0.3` | jest 30 coverage (`babel-plugin-istanbul@8`) |
| `filelist@1>minimatch` → `11.0.3` | `jake` (pulled in by `ejs@3`, never invoked) |

glob 14 brings its own `minimatch ^11.0.2`, so the 9.0.9 and 10.2.6 lines
disappear with it.

**API compatibility check** (glob 14 and minimatch 11 are majors):

- glob 14's changelog changes `Pattern`/`GlobWalker` internals, drops the
  `debug` option and changes how `ignore` patterns starting with `/` are
  rooted. Every consumer above uses only the public API:
  `glob.sync(p, {windowsPathsNoEscape})` (jest), `globSync(p, {cwd, follow,
  ignore})` (workbox-build), `glob`/`glob.sync(p, {cwd, nodir, dot, posix,
  ignore})` (test-exclude 7). All of these still exist with the same
  signatures. None passes `debug`, and none of our ignore patterns start
  with `/`.
- minimatch 11 removes `makeRe`, `Minimatch#regexp` and `AST`. test-exclude 7
  only calls `minimatch(path, pattern, {dot})`. filelist 1 only calls
  `minimatch.match(list, pattern, opts)`, which minimatch 11 still exports.
- Behaviour checks: the web-wallet `dist/sw.js` (workbox `generateSW`, which
  globs the precache list) is **byte-identical** before and after
  (sha256 `9aab62e6…393304f3`, 20 precache entries). A direct smoke test of
  `test-exclude@7` (`globSync` and async `glob` on `packages/core`) returns the
  same 11 files as `find`. `filelist@1` `include()` returns the same files as a
  shell glob. Both Snap suites pass under jest 30 with glob 14.

### Residual minimatch 3.1.5: why it stays

`pnpm audit` still reports minimatch 3.1.5 (11 paths) through
`fork-ts-checker-webpack-plugin@9.1.0` and through `snaps-jest`'s jest 29 chain
(`test-exclude@6.0.0`, `glob@7.2.3`).

- **No patched version can be resolved.** No 3.x fix was released. Forcing
  11.x on these consumers would break them: `test-exclude@6` calls the CJS
  module itself as a function (`require('minimatch')(…)`), which minimatch
  ≥ 9 no longer supports, and `glob@7` reads `Minimatch#set` as `RegExp`
  objects, which minimatch 11 replaced with `Matcher` objects.
  Both parents are already on their latest releases (see the table above).
- **The vulnerable code is not present.** `firstPhasePreProcess` and
  `optimizationLevel` do not exist in minimatch 3.1.5 (or 5.1.9). Grepping the
  installed packages finds them only in the 9.x/10.x lines, and those are gone
  after this change. The advisory's `<11.0.1` range is wider than the code it
  describes.
- **No untrusted patterns.** fork-ts-checker is only instantiated when the
  Snap config sets `typescript.enabled`, which defaults to `false` and is not set
  in either `snap.config.ts`. test-exclude 6 runs only under jest 29 coverage,
  which the Snap suites do not enable. All patterns come from committed config.

## deepmerge: where it sits in the graph

`pnpm why -r deepmerge` (pnpm 9.15.9). Lock: `deepmerge@4.3.1`, integrity
`sha512-3sUqbMEc…knFn+A==`.

1. `@botho/web-wallet` (dev) → `vite-plugin-pwa@1.3.0` → `workbox-build@7.4.1`
   → `@rollup/plugin-node-resolve@16.0.3` → `deepmerge`
2. `@botho/snap`, `@botho/snap-spike` (dev) → `@metamask/snaps-cli@8.4.1` →
   `fork-ts-checker-webpack-plugin@9.1.0` → `deepmerge`
3. `@botho/snap`, `@botho/snap-spike` (dev) → `jest@30` → `@jest/core` →
   `jest-config@30.5.2` → `deepmerge`

All three arrive only through `devDependencies`. `pnpm why -r --prod deepmerge`
returns no importer.

## deepmerge: can untrusted input reach it?

The advisory needs an attacker-controlled *source* object in a merge (for
example `JSON.parse` output with a `__proto__` key). Each call site was
checked:

| Call site | Inputs | Reached in our builds? |
|---|---|---|
| `@rollup/plugin-node-resolve`: `DEFAULTS = deepFreeze(deepMerge({}, defaults))` | a module-level constant | Yes, once at module load, with constant input only |
| `fork-ts-checker-webpack-plugin` constructor: `deepmerge(externalOptions, options)` | plugin options from snaps-cli: `{typescript: {build, configFile}}` from committed `snap.config.ts` | No. snaps-cli only constructs the plugin when `typescript.enabled` is set. The default is `false`, and neither config sets it |
| `jest-config` `mergeGlobalsWithPreset`: `deepmerge(preset.globals, options.globals)` | the `@metamask/snaps-jest` preset and our committed `jest.config.js` | No. It only runs when both define `globals`, and neither does |
| `jest-config` `mergeConfig` (exported helper) | caller-supplied configs | Not called by our jest invocations |

- **Shipped bundles: deepmerge is absent.** deepmerge's unique error string
  (`first argument should be an array`, from `deepmerge.all`) is present in the
  installed package but missing from every `web/packages/*/dist` output:
  the web-wallet `dist/` including `sw.js`, the Snap
  `dist/bundle.js` (sha256 `4b2e7933…4fbbd98`) and the spike
  `dist/bundle.js` (sha256 `1b14c734…8ae8029b`). In all three, deepmerge runs
  only inside the build tool. It is never bundled into the output.
- **Tooling runtime:** every input that reaches deepmerge comes from a
  committed JS/TS config file or a module constant. Anyone who can change those
  files can already run arbitrary code in the build, so prototype poisoning
  gives them nothing extra. No network, user or RPC data flows into any of
  these merges.

**Disposition:** accepted dev-only residual. It stays visible in `pnpm audit`
(no exception added). Re-check when deepmerge publishes a fix, or when any of the
three parents drops it.

## Other workspaces

The same glob/minimatch advisories (and others) also appear in the other JS
lockfiles. They are outside this issue's scope (`web/`) and were left as
they are:

- `contracts/ethereum` (npm): 19 findings (1 low, 5 moderate, 13 high),
  including glob/minimatch through mocha and typechain.
- `contracts/solana` (npm): 4 findings (1 low, 3 moderate), including
  glob/minimatch through mocha.
- `mobile/app` (pnpm): 41 findings (6 low, 13 moderate, 21 high,
  1 critical), including glob, minimatch and deepmerge.

Tracked in #1765.

## Re-check procedure

```sh
cd web
pnpm install --frozen-lockfile          # pnpm 9.x
pnpm audit                              # expect only elliptic, deepmerge, minimatch@3.1.5
pnpm audit --prod                       # expect clean
pnpm why -r minimatch glob deepmerge    # confirm the graph above
npm view fork-ts-checker-webpack-plugin@latest dependencies.minimatch
npm view @metamask/snaps-jest@latest dependencies
npm view deepmerge version
```

When a parent ships a patched dependency range, drop the matching override.
Note for #1567: the seven new overrides must move to `pnpm-workspace.yaml`
together with the existing ones before CI moves to pnpm 10+. pnpm ≥ 10
ignores `package.json#pnpm.overrides`.
