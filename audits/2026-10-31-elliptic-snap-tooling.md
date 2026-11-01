# elliptic ECDSA nonce advisory in MetaMask Snap tooling: exposure and containment

**Date**: 2026-10-31
**Auditor**: Loom Builder (#1562)
**Scope**: The `elliptic` dependency in the `web/` pnpm workspace. This covers the
lock graph, the Snap build (`mm-snap build`), the Snap test harness
(`@metamask/snaps-jest`) and the shipped Snap bundles. Out of scope: the Rust
`crypto-bigint` finding (#1556), the running x1 endurance campaign, and any
deployed artifact.
**Commit**: origin/main @ `8d5ecef2`.
**Status**: **UNRESOLVED. No upstream fix exists.** The advisory is not
suppressed, and `pnpm audit` keeps reporting it. This document records the
exposure and the CI-enforced containment. It does not claim a fix.

---

## Finding

| Field | Value |
|---|---|
| Advisory | [GHSA-848j-6mx2-7j84](https://github.com/advisories/GHSA-848j-6mx2-7j84) / CVE-2025-14505 |
| Package | `elliptic` `<= 6.6.1`. The lock resolves 6.6.1, integrity `sha512-RaddvvMatK2L…TAfe3g==` |
| Defect | ECDSA signing can truncate the generated nonce incorrectly. The result is faulty signatures and possible private-key recovery ([indutny/elliptic#321](https://github.com/indutny/elliptic/issues/321)) |
| Audit severity | low. We do not discount it for that reason, because the impact class is key recovery |
| Patched version | none. `first_patched_version` is `null` |

## Upstream fix check (2026-10-31)

| Package | Latest on npm | Still pulls elliptic? |
|---|---|---|
| `elliptic` | 6.6.1 (2024-11-13) | n/a. No release since then, and upstream issue #321 is still open |
| `@ethersproject/signing-key` | 5.8.0 | yes, it pins `elliptic: 6.6.1` |
| `@ethersproject/abi` | 5.8.0 | yes, through hash → abstract-signer → abstract-provider → transactions → signing-key |
| `@metamask/controller-utils` | 13.0.0 | yes, it still depends on `@ethersproject/abi ^5.7.0` |
| `@metamask/snaps-cli` | 8.4.1 (the version we use) | yes, it still depends on `crypto-browserify ^3.12.0` |
| `@metamask/snaps-jest` | 10.2.2 | yes, through snaps-simulation → controller-utils |
| `crypto-browserify` / `browserify-sign` / `create-ecdh` | 3.12.1 / 4.2.6 / 4.0.4 | yes |

No parent update removes the chain, and there is no upstream patch or backport to
qualify. Upgrading `snaps-jest` from 10.2.1 to 10.2.2 or `controller-utils` to 13
would not change the exposure, so the dependency graph was left as it is. Under
#1562's Out of Scope list, we add no audit exception and no homemade ECDSA or
nonce shim.

## Where elliptic sits in the graph

`pnpm why -r elliptic` (pnpm 9.15.9, the major CI uses):

1. `@metamask/snaps-cli@8.4.1` → `crypto-browserify@3.12.1` →
   `browserify-sign@4.2.6` and `create-ecdh@4.0.4` → `elliptic@6.6.1`
2. `@metamask/snaps-jest@10.2.1` and `snaps-cli` → (`snaps-simulation`,
   `snaps-controllers`, `snaps-utils`, `snaps-rpc-methods`,
   `permission-controller`, `chain-agnostic-permission`) →
   `@metamask/controller-utils@12.3.0` → `@ethersproject/abi@5.8.0` → … →
   `@ethersproject/signing-key@5.8.0` → `elliptic@6.6.1`

Only `@botho/snap` and `@botho/snap-spike` reach it, and both do so only
through `devDependencies`. `pnpm why -r --prod elliptic` returns no importer with
dependencies.

## Can real keys reach the vulnerable signing code?

The lock graph alone does not answer this. Each path was checked directly.

### 1. Shipped Snap bundles: elliptic is absent

- snaps-cli injects `crypto-browserify` only as a webpack `resolve.fallback`
  for `crypto`, and only when the Snap config sets `polyfills: true` or
  `polyfills.crypto: true` (`getFallbacks` in `snaps-cli/dist/webpack/utils.cjs`).
  The default is `false`, which maps every Node builtin to `false`.
  `web/packages/snap/snap.config.ts` and `web/packages/snap-spike/snap.config.ts`
  set no `polyfills`.
- The built bundles contain none of elliptic's ECDSA error strings
  (`Can not sign a negative message`, `The recovery param is more than two
  bits`, `Unable to find sencond key candinate`). They also contain no
  `ethersproject`, `browserify`, `createSign` or `createECDH`, and no spaced-hex
  curve constants.

  | Bundle (local build at `8d5ecef2`) | Size | sha256 |
  |---|---|---|
  | `web/packages/snap/dist/bundle.js` | 607,276 B | `4b2e7933fed05efb32d08f7b819542971a3d02a54571b6e92d04c780c4fbbd98` |
  | `web/packages/snap-spike/dist/bundle.js` | 582,772 B | `1b14c734b401ffa0ba10c01ae8db9b38cd996b98ba31c39f956955b18ae8029b` |

  The bundles were built with rustc `1.98.0-nightly (cb46fbb8c 2026-06-08)`,
  wasm-pack 0.13.1 and node v26.13.0. These local hashes differ from the
  `source.shasum` committed in each `snap.manifest.json`. That difference
  already existed before this work, comes from the toolchain (the inlined wasm),
  and is unrelated to elliptic. No deployed Snap artifact needs replacing
  because of this finding.
- **Positive control**: the bundle check was tested on a real build. In a
  throwaway change to `snap-spike`, we set `polyfills.crypto` (plus the stream
  and process helpers it needs) and imported `crypto.createSign`. With default
  settings, `mm-snap build` refuses that bundle at the SES evaluation step
  (`Cannot read properties of undefined (reading 'browser')`). With
  `evaluate: false` it emitted a 995 KB bundle that contains all three
  fingerprints, and the guard reported both the config and the bundle. The
  throwaway change was reverted, and the rebuilt bundle matched the hash above
  byte for byte.
- The Snap's own signing never uses secp256k1. It derives Botho keys from
  `snap_getEntropy` and signs with the inlined `bth-wasm-signer` (see
  [2026-07-20 Snap key-handling](2026-07-20-snap-keyhandling.md)).

### 2. Build and test tooling: elliptic is never loaded

- The only `@ethersproject/*` import in the MetaMask packages is
  `@metamask/controller-utils/dist/abi.cjs`. It uses `AbiCoder` and
  `getFunction`/`getSighash` to encode function calls, which involves no
  signing. `@ethersproject/signing-key` builds its `EC("secp256k1")` lazily, on
  the first sign, recover or ECDH call.
- **Runtime probe**: we temporarily instrumented the installed
  `elliptic/lib/elliptic/ec/index.js`. We first broke its pnpm hardlink so the
  store copy was untouched, then logged module load, `EC` construction,
  `keyFromPrivate`, `genKeyPair` and `sign`. A direct `node -e` call confirmed
  the probe fires (`LOAD`, `EC_CTOR secp256k1`, `KEY_FROM_PRIVATE`, `SIGN`). We
  then ran the full `pnpm --filter @botho/snap test:snap` (`mm-snap build` +
  jest, 114 passed / 1 skipped) and `pnpm --filter @botho/snap-spike test:snap`
  (2 passed / 1 skipped). **The probe log was never created.** elliptic was not
  even `require`d during either Snap build or either jest suite. The file was
  restored from the saved original.
- The test harness holds no real secrets. snaps-jest runs the Snap against
  its built-in test SRP and an in-process mocked node, and CI passes no
  credentials to these steps.

### Exposure summary

| Surface | Contains elliptic | Executes elliptic signing | Real keys present |
|---|---|---|---|
| Shipped Snap bundle (`dist/bundle.js`) | no | no | n/a |
| `mm-snap build` (webpack) | in `node_modules` only | no (never loaded) | no |
| `snaps-jest` test run | in `node_modules` only | no (never loaded) | no (test SRP) |
| Web wallet, desktop, BaaS worker, core | not in graph | no | n/a |

Residual risk: a developer could write new tooling code that imports elliptic
or ethers-5 signing, and the guard below cannot catch that. It is limited to
developer machines and CI, which hold no production keys.

## Containment (enforced in CI)

`web/scripts/check-elliptic-containment.mjs` runs in `web-ci.yml` right after
the Snap tests (`--require-bundle snap`). It fails if any of these happens:

1. any workspace importer reaches elliptic through a production dependency;
2. any importer other than `@botho/snap` / `@botho/snap-spike` reaches it at all;
3. a Snap config enables the `crypto` polyfill (`polyfills: true` or
   `polyfills.crypto: true`);
4. a built Snap bundle contains elliptic's ECDSA fingerprint strings. Before
   this check, the script confirms the fingerprints are still present in every
   installed elliptic, so the check cannot pass silently after an elliptic
   change. CI builds only `@botho/snap`, so in CI the `snap-spike` bundle check
   is skipped (the script logs this) and only its config check (3) runs. That
   is acceptable for a spike that does not ship. "Every built Snap bundle"
   means every bundle present when the script runs.

Checks 1 and 2 fail closed. `pnpm why --json` changed shape between pnpm 9
(entries per importer) and pnpm 10+ (a reverse `dependents` tree).
`web/scripts/elliptic-why.mjs` parses both and throws on anything else. If
elliptic is installed but the parsed dev graph does not show `@botho/snap`,
the script treats the output as unrecognized and fails, so a pnpm upgrade
cannot quietly disable these checks. The parser has fixture tests for both
shapes and for malformed output (`node --test scripts/elliptic-why.test.mjs`,
also run in CI).

If elliptic leaves the lock graph, the script says so and passes. That is the
signal to re-run `pnpm audit` and close #1562.

## Re-check procedure (to close #1562)

1. `npm view elliptic version`, and check the advisory's `first_patched_version`.
2. `npm view @metamask/snaps-cli dependencies` (look for `crypto-browserify`)
   and `npm view @metamask/controller-utils dependencies` (look for
   `@ethersproject/abi`).
3. If a supported release removes the chain or patches elliptic, bump it in
   `web/packages/snap{,-spike}/package.json`. Then run
   `pnpm install --frozen-lockfile`, `pnpm audit`, `pnpm --filter @botho/snap test:snap`
   and the containment script.

## Other advisories observed (not part of #1562)

After #1766 (the #1745 fix) merged, the full `pnpm audit` (pnpm 9.15.9)
reports 3 findings (2 low, 1 moderate): elliptic (this report), `deepmerge`
4.3.1 (GHSA-gcqx-w48m-w2h4, low, no patch) and `minimatch` 3.1.5 (moderate,
no 3.x fix). `pnpm audit --prod` reports none. glob and the other vulnerable
minimatch lines are fixed with scoped overrides. The deepmerge and minimatch
3.x exposure is analysed in
[2026-10-31-dev-advisories-glob-minimatch-deepmerge.md](2026-10-31-dev-advisories-glob-minimatch-deepmerge.md).
