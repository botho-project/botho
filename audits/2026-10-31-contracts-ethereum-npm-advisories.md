# contracts/ethereum npm advisories: what was fixed and the zod residual

**Date**: 2026-10-31
**Auditor**: Loom Builder (#1767, split from #1765)
**Scope**: Every finding from `npm audit --package-lock-only` in
`contracts/ethereum` (npm lockfile v3, Node 22 in CI). This covers the lock
graph, the Hardhat 3 toolchain that compiles and tests the contracts
(`contracts-ci.yml`), the local Hardhat node used by `bridge-e2e.yml`, and the
deploy scripts under `contracts/ethereum/scripts/`. Out of scope: Solidity
source (not changed), deployed addresses (not changed), and the other JS
lockfiles (`web/` is #1745, `contracts/solana` and `mobile/app` are tracked in
#1765).
**Commit**: origin/main @ `f11488ec`
**Status**: **glob, minimatch and serialize-javascript RESOLVED. zod
UNRESOLVED: no patched 3.x exists, Hardhat 3 still requires `zod ^3`, and
forcing zod 4 breaks Hardhat's config validation.** Dev-only, and no untrusted
input reaches it. No audit exception or ignore entry was added.

---

## Summary

| | Before | After |
|---|---|---|
| `npm audit --package-lock-only` | 19 (1 low, 5 moderate, 13 high) | 13 (13 high) |
| `npm audit --package-lock-only --omit=dev` | 0 | 0 |
| glob (GHSA-3g3m-2fjr-6pw4) | 13.0.6 (mocha), 7.1.7 (typechain) | none (14.0.3) |
| minimatch (GHSA-g3hv-75wg-m249) | 10.2.6 (mocha, glob 13), 3.1.5 (typechain's glob 7) | none (11.0.3) |
| serialize-javascript (GHSA-m5mm-p8h7-m29v) | 7.1.2 | none (7.1.4) |
| zod (GHSA-mrhx-c7xc-m3w5) | 3.25.76 | 3.25.76 (no 3.x fix) |

The 13 remaining findings all come from the one zod advisory. npm reports zod
itself plus every Hardhat package that depends on it:
`hardhat`, `@nomicfoundation/hardhat-zod-utils`, `-ethers`,
`-ethers-chai-matchers`, `-ignition`, `-ignition-ethers`, `-keystore`,
`-mocha`, `-network-helpers`, `-toolbox-mocha-ethers`, `-typechain`, `-verify`.
`mocha`, `typechain` and `@typechain/ethers-v6` were reported only because of
glob/minimatch, and they are clean now.

**Shipped vs dev-only.** Everything in `contracts/ethereum/package.json` is a
`devDependency`. The package is not published, and nothing from
`node_modules` ends up on chain. The deployed artifact is the solc 0.8.20
bytecode of `contracts/*.sol` plus `@openzeppelin/contracts`. Neither the
compiler nor OpenZeppelin changed version here, so the bytecode is unchanged.
`--omit=dev` shows 0 findings before and after.

## Findings

| Advisory | Package / range | Defect | Patched |
|---|---|---|---|
| [GHSA-3g3m-2fjr-6pw4](https://github.com/advisories/GHSA-3g3m-2fjr-6pw4) (moderate) | `glob >=9.0.0 <=14.0.1` (npm also flags 3.x–8.x through minimatch) | An aborted async walk keeps reading directories in the background | 14.0.2 |
| [GHSA-g3hv-75wg-m249](https://github.com/advisories/GHSA-g3hv-75wg-m249) (moderate) | `minimatch <11.0.1` | Attacker-supplied pattern amplifies `globParts` (`firstPhasePreProcess`, `optimizationLevel >= 2`) | 11.0.1 |
| [GHSA-m5mm-p8h7-m29v](https://github.com/advisories/GHSA-m5mm-p8h7-m29v) (low) | `serialize-javascript <7.1.3` | XSS through an unescaped `</script` at the end of a serialized function body | 7.1.3 |
| [GHSA-mrhx-c7xc-m3w5](https://github.com/advisories/GHSA-mrhx-c7xc-m3w5) (high) | `zod <4.6.6` | `$ZodArray` collects one issue per failing element with no cap, so a huge array with failing elements can exhaust memory | 4.6.6 (no 3.x backport) |

## Dependency trace (`npm explain`, before)

```
glob@13.0.6            <- mocha@12.0.3 (glob ^13.0.0)
glob@7.1.7             <- typechain@8.3.2 (glob "7.1.7", exact pin)
minimatch@10.2.6       <- mocha@12.0.3 (^10.2.2), glob@13.0.6 (^10.2.2)
minimatch@3.1.5        <- typechain's glob@7.1.7 (^3.0.4)
serialize-javascript@7.1.2 <- mocha@12.0.3 (^7.1.1)
zod@3.25.76            <- hardhat@3.20.0, hardhat-zod-utils@3.0.8 (peer),
                          hardhat-ethers, -keystore, -mocha, -typechain,
                          -verify (all "zod": "^3.23.8")
```

mocha, hardhat, hardhat-toolbox-mocha-ethers and hardhat-zod-utils are
the root packages. All four are top-level devDependencies (or come in through
the toolbox), so none of these findings reaches production.

## Upstream fix check (2026-10-31)

| Parent | Latest on npm | Still pins a vulnerable version? |
|---|---|---|
| `mocha` | 12.0.3 (in use) | yes, `glob ^13.0.0`, `minimatch ^10.2.2` |
| `typechain` | 8.3.2 (in use, last release 2023-10) | yes, `glob 7.1.7` exact |
| `@nomicfoundation/hardhat-typechain` | 3.1.3 | yes, `typechain ^8.3.1` |
| `hardhat` | 3.21.2 | yes, `zod ^3.23.8` |
| `@nomicfoundation/hardhat-zod-utils` | 3.0.8 (in use) | yes, peer `zod ^3.23.8` |
| `zod` 3.x | 3.25.76 (in use, last 3.x) | n/a, no 3.x fix was released |

No parent bump moves any of these consumers to a patched version.

## Resolution

### 1. In-range parent bumps (`npm audit fix`, no `--force`)

These stay inside the existing `package.json` ranges:

| Package | Before → after |
|---|---|
| `serialize-javascript` | 7.1.2 → 7.1.4 (**fixes GHSA-m5mm-p8h7-m29v**) |
| `hardhat` | 3.20.0 → 3.21.2 |
| `@nomicfoundation/edr` (+ platform binaries) | 0.23.0 → 0.25.0 |
| `@nomicfoundation/hardhat-ethers` | 4.2.1 → 4.2.2 |
| `@nomicfoundation/hardhat-typechain` | 3.1.2 → 3.1.3 |
| `@nomicfoundation/hardhat-utils` | 5.0.0 → 5.1.1 |
| `@nomicfoundation/hardhat-verify` | 3.1.3 → 3.1.4 |

### 2. Scoped overrides (`contracts/ethereum/package.json#overrides`)

```json
"overrides": {
  "mocha": { "glob": "14.0.3", "minimatch": "11.0.3" },
  "typechain": { "glob": "14.0.3" }
}
```

glob 14 brings its own `minimatch ^11.0.2`, so this removes both the 10.2.6
and the 3.1.5 minimatch lines, along with glob 7's helpers (`inflight`,
`once`, `wrappy`, `fs.realpath`, `path-is-absolute`, `concat-map`).

**API compatibility check** (glob 14 and minimatch 11 are majors):

- **mocha 12.0.3** calls `glob.hasMagic(p, {windowsPathsNoEscape})`
  (`lib/cli/lookup-files.js`), `new glob.Glob(p, {...})` (`watch-run.cjs`, only
  in `--watch`), and `minimatch(path, pattern, {dot, windowsPathsNoEscape})`
  (`collect-files.cjs`, `watch-run.cjs`). glob 14 and minimatch 11 still
  export all of these with the same signatures. Hardhat's mocha runner
  (`@nomicfoundation/hardhat-mocha`) uses the programmatic `Mocha` API.
- **typechain 8.3.2** uses glob in one place, `dist/utils/glob.js`:
  `require("glob").sync(p, {ignore, absolute, cwd})`. glob 14's CommonJS build
  still exports `sync` with those options. That helper only runs from the
  typechain CLI. `@nomicfoundation/hardhat-typechain` calls `runTypeChain`
  with an explicit list of artifact paths and never globs. A direct call
  `require("typechain").glob(cwd, ["artifacts/contracts/**/+([a-zA-Z0-9_]).json"])`
  returns the same 3 artifacts that `find` does.
- Generated types (`types/ethers-contracts/`, gitignored) have the same
  content as before. The only difference is the order of the
  `WrappedBTH`/`WbthPeerToken` exports, and that order changes between
  repeated `hardhat clean && hardhat compile` runs on the same lockfile, so it
  is existing nondeterminism and not caused by this change.

## zod: why it stays

**No compatible patched version exists.** The fix is only in zod 4.6.6+.
Every Hardhat 3 package, including the newest `hardhat@3.21.2`, imports the
root `"zod"` entry point and depends on `zod ^3.23.8`. In zod 4 the root entry
point is the v4 API.

**Forcing zod 4 was tested and breaks Hardhat.** With
`"overrides": {"zod": "4.6.7"}`, compile and all 39 tests still pass, because
a valid config never reaches the error path. But an invalid config (for
example `chainId: "x"`, `url: 12345`) makes Hardhat crash instead of reporting
the error:

```
zod 3 (current):  Error HHE15: Invalid config:
                    * Config error in config.networks.hardhat.chainId: Expected number, received string
                    * Config error in config.networks.localhost.url: Expected a URL or a Configuration Variable
zod 4 (forced):   An unexpected error occurred:
                  TypeError: Cannot read properties of undefined (reading 'map')
                      at validateUserConfigZodType (hardhat-zod-utils/src/index.ts:219:32)
```

`hardhat-zod-utils` also uses v3-only APIs (`errorMap:`, `z.ZodIssueCode`,
`z.function()` as a schema). A forced major override would leave the
toolchain only partly working, so it was not added. Downgrading to Hardhat 2
(what `npm audit fix --force` suggests: `hardhat@2.29.1`) would undo the
Hardhat 3 migration (#1174). That is not a security fix.

### Where zod runs, and what input it sees

Every file in the installed tree that imports zod:

| Call site | Input | Untrusted? |
|---|---|---|
| `hardhat` config validation (`solidity/config`, `network-manager/type-validation`, `coverage/config`, `solidity-test/config`) and plugin config hooks (`hardhat-ethers`, `-mocha`, `-typechain`, `-verify`) | the committed `hardhat.config.ts` plus `process.env` values from the developer's git-ignored `.env` | No. Anyone who controls these already runs arbitrary code, since the config is executable TypeScript |
| `hardhat-zod-utils/rpc` `validateParams`, used only by `LocalAccountsHandler` | params of `eth_sign`, `personal_sign`, `eth_signTypedData_v4`, `eth_sendTransaction` that **our own scripts** send through the Hardhat provider on `type: "http"` networks with local `accounts` (`fork`, `sepolia`, `mainnet`) | No. The requests come from our process, not from the remote RPC. Server responses are not validated with zod |

The advisory needs an attacker to send an oversized array with failing
elements into a zod array schema. Nothing on this list takes input from the
network, users or RPC responses. The worst result is that a developer's own
Hardhat process runs out of memory on input the developer wrote. `hardhat node`
(used by `bridge-e2e.yml`) listens on 127.0.0.1 by default, and its JSON-RPC
server sends requests straight to EDR, so zod is not involved there.

**Disposition:** accepted dev-only residual. It stays visible in `npm audit`
(no exception added). Re-check when Hardhat moves to zod 4 (or ships
`hardhat-zod-utils` with a v4 peer range), or if zod releases a 3.x fix.

## Verification

All commands run in `contracts/ethereum`, Node 22, on the final lockfile:

| Check | Result |
|---|---|
| `rm -rf node_modules && npm ci` | OK. A follow-up `npm install` leaves `package-lock.json` byte-identical |
| `npx hardhat clean && npx hardhat compile` | `Compiled 3 Solidity files with solc 0.8.20` |
| `npx hardhat test` (what `contracts-ci.yml` runs) | 39 passing (same as before) |
| `npx hardhat node` + `npm run deploy:local` (smoke test of EDR 0.25 in node mode, which `bridge-e2e.yml` uses) | `WrappedBTH deployed at: 0x5FbDB2315678afecb367f032d93F642f64180aa3` |
| `npm audit --package-lock-only` | 13 high (all zod) |
| `npm audit --package-lock-only --omit=dev` | 0 |

The full `scripts/bridge-e2e-local.sh` (nightly `bridge-e2e.yml`) was not run
locally because the Rust build needs more free disk than the host had. It runs
on the next nightly.

## Re-check procedure

```sh
cd contracts/ethereum
npm ci
npm audit --package-lock-only            # expect only the zod chain (13 high)
npm audit --package-lock-only --omit=dev # expect 0
npm explain zod glob minimatch           # confirm the graph above
npm view hardhat@latest dependencies.zod
npm view @nomicfoundation/hardhat-zod-utils@latest peerDependencies
npm view mocha@latest dependencies       # drop the mocha override once glob >=14.0.2 / minimatch >=11.0.1
npm view typechain@latest dependencies.glob
```

Remove each override once its parent ships a patched dependency range.
