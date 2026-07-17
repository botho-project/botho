/**
 * Jest config for the snap spike. `@metamask/snaps-jest` runs the built snap
 * bundle (dist/bundle.js) inside the REAL Snaps execution environment — the
 * same SES (Hardened JavaScript) executor MetaMask ships, via
 * `@metamask/snaps-simulation`. This is the accepted measurement proxy for a
 * real MetaMask instance (issue #815, Phase 0).
 *
 * Tests are named `*.spike.ts` (NOT `*.test.ts`) so the workspace-root vitest
 * run never picks them up; run them with `pnpm --filter @botho/snap-spike
 * test:snap` (requires the wasm artifacts and, for the live-send test, a
 * built `botho` node binary — see README.md).
 */
module.exports = {
  preset: '@metamask/snaps-jest',
  testMatch: ['<rootDir>/test/**/*.spike.ts'],
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
  // Node spawning + block mining + snap install are slow; generous timeout.
  testTimeout: 600_000,
};
