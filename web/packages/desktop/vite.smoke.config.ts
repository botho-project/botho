import { defineConfig, mergeConfig } from 'vite'
import desktop from './vite.config'
import path from 'node:path'

// Reuse the production transforms/target. This does not select a support floor.
export default mergeConfig(desktop, defineConfig({
  root: path.resolve(import.meta.dirname, 'smoke'),
  build: { outDir: '../smoke-dist', emptyOutDir: true },
}))
