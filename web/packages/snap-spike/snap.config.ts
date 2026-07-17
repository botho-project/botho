import type { SnapConfig } from '@metamask/snaps-cli';
import { resolve } from 'path';

const config: SnapConfig = {
  input: resolve(__dirname, 'src/index.ts'),
  server: {
    port: 8090,
  },
  stats: {
    buffer: false,
  },
  // Enables direct `import * as wasm from './x.wasm'` — the snaps-cli wasm
  // loader base64-inlines the module into the bundle and resolves its JS
  // imports, which is exactly the shape wasm-pack `--target bundler` emits.
  experimental: {
    wasm: true,
  },
};

export default config;
