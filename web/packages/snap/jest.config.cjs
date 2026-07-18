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
  transform: {
    '^.+\\.(t|j)sx?$': [
      'ts-jest',
      {
        isolatedModules: true,
        tsconfig: {
          module: 'commonjs',
          moduleResolution: 'node',
          esModuleInterop: true,
          allowJs: true,
          target: 'es2022',
        },
      },
    ],
  },
  // Workspace deps (@botho/*) resolve to raw TS via pnpm symlinks, and the
  // @scure/@noble crypto deps are ESM-only — all must go through ts-jest.
  // The `.pnpm` branch lets the check recurse into pnpm's store layout.
  transformIgnorePatterns: ['node_modules/(?!(\\.pnpm|@scure|@noble|@botho)/)'],
  // Snap install + SES lockdown + wasm instantiation are slow; generous timeout.
  testTimeout: 120_000,
};
