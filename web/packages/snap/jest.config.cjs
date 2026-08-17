/**
 * Jest config for the Botho Snap MVP. `@metamask/snaps-jest` runs the built
 * snap bundle (dist/bundle.js, with the `bth-wasm-signer` wasm inlined) inside
 * the REAL Snaps execution environment — the same SES (Hardened JavaScript)
 * executor MetaMask ships, via `@metamask/snaps-simulation`. This is the
 * accepted headless proxy for a real MetaMask instance (issue #815; the harness
 * was validated in the Phase-0 spike, PR #1055).
 *
 * Tests are named `*.snap.ts` (NOT `*.test.ts`) so the workspace-root vitest run
 * (`packages/**​/*.test.ts`) never picks them up — they only run via
 * `pnpm --filter @botho/snap test:snap`, which first builds the bundle. The node
 * RPC is MOCKED with an in-process JSON-RPC server (`test/mock-node.ts`); no live
 * betanet or node binary is required (live-testnet send validation is a
 * follow-up, deferred behind betanet resume #1051 — see README.md).
 */
module.exports = {
  preset: '@metamask/snaps-jest',
  testMatch: ['<rootDir>/test/**/*.snap.ts'],
  // `@swc/jest`, NOT `ts-jest`: ts-jest drives the TypeScript *JavaScript*
  // compiler API, which TypeScript 7 (the Go-native compiler) no longer ships
  // — under TS 7 every suite dies at load with "The TypeScript compiler
  // `typescript` (version 7.x) does not expose the JavaScript compiler API
  // required by ts-jest". swc transpiles TS without that API and is already in
  // the tree (`mm-snap build` uses it), so it is the TS7-compatible swap. Type
  // errors are still caught by `pnpm typecheck` (tsc --noEmit); the previous
  // ts-jest config ran with `isolatedModules: true`, so it never type-checked
  // here either — this is transpile-only, same as before.
  transform: {
    '^.+\\.(t|j)sx?$': [
      '@swc/jest',
      {
        jsc: { target: 'es2022' },
        module: { type: 'commonjs' },
      },
    ],
  },
  // Workspace deps (@botho/*) resolve to raw TS via pnpm symlinks, and the
  // @scure/@noble crypto deps are ESM-only — all must go through the transform.
  // The `.pnpm` branch lets the check recurse into pnpm's store layout.
  transformIgnorePatterns: ['node_modules/(?!(\\.pnpm|@scure|@noble|@botho)/)'],
  // Snap install + SES lockdown + wasm instantiation are slow; generous timeout.
  testTimeout: 120_000,
};
