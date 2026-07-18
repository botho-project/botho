import type { SnapConfig } from '@metamask/snaps-cli';
import { resolve } from 'path';

const config: SnapConfig = {
  input: resolve(__dirname, 'src/index.ts'),
  server: {
    // Distinct from the Phase-0 spike's dev server (8090) so both can run.
    port: 8091,
  },
  stats: {
    buffer: false,
  },
  // Enables direct `import * as wasm from './x.wasm'` — the snaps-cli wasm
  // loader base64-inlines the module into the bundle and resolves its JS
  // imports, which is exactly the shape wasm-pack `--target bundler` emits.
  // Proven in the Phase-0 spike (issue #815, PR #1055).
  experimental: {
    wasm: true,
  },
};

export default config;
