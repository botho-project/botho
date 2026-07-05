import { defineConfig } from 'vitest/config'

// Package-local config so `pnpm --filter @botho/adapters test` discovers the
// adapter tests when run from this package's directory. The repo-root
// vitest.config.ts still picks these up under `packages/**` when the whole
// suite runs from `web/` (e.g. `pnpm test:run` / `pnpm check:ci`).
export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
})
