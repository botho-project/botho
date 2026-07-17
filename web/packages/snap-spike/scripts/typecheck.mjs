// Typecheck gate for the snap spike. The spike imports the GENERATED
// wasm-pack `--target bundler` artifact (`@botho/wasm-signer/pkg-bundler`),
// which is git-ignored. In CI (fresh checkout) it does not exist, so `tsc`
// cannot resolve the import — skip with a note instead of failing the
// workspace-wide `pnpm -r typecheck`.
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const pkgBundler = join(here, '..', '..', 'wasm-signer', 'pkg-bundler', 'bth_wasm_signer.js');

if (!existsSync(pkgBundler)) {
  console.log(
    '@botho/snap-spike: typecheck skipped (wasm artifact not built — run ' +
      '`pnpm --filter @botho/wasm-signer build:wasm:bundler`)',
  );
  process.exit(0);
}

const result = spawnSync('tsc', ['--noEmit'], {
  cwd: join(here, '..'),
  stdio: 'inherit',
  shell: true,
});
process.exit(result.status ?? 1);
