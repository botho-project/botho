import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    include: ['packages/**/*.test.ts', 'packages/**/*.test.tsx'],
    environmentMatchGlobs: [
      // Use jsdom for React component tests
      ['packages/**/*.test.tsx', 'jsdom'],
    ],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'html'],
      include: ['packages/*/src/**/*.ts', 'packages/*/src/**/*.tsx'],
      exclude: ['**/*.test.ts', '**/*.test.tsx', '**/index.ts'],
    },
  },
})
