#!/usr/bin/env node
// Containment guard for the unresolved `elliptic` advisory (#1562).
//
// GHSA-848j-6mx2-7j84 / CVE-2025-14505: elliptic <= 6.6.1 can produce faulty
// ECDSA signatures from an incorrectly truncated nonce, which can leak the
// private key. Upstream has published no patched version. In this workspace
// elliptic is reached only through MetaMask Snap build/test tooling
// (@metamask/snaps-cli and @metamask/snaps-jest). The full evidence is in
// audits/2026-10-31-elliptic-snap-tooling.md.
//
// We do not suppress the advisory, so `pnpm audit` keeps reporting it. This
// script checks the containment properties, so CI fails if any of them stops
// holding:
//
//   1. No workspace importer reaches elliptic through a production dependency.
//   2. Only the Snap packages (@botho/snap, @botho/snap-spike) reach it at all,
//      even through dev dependencies.
//   3. No Snap config enables snaps-cli's `crypto` polyfill. That polyfill is
//      the crypto-browserify -> browserify-sign/create-ecdh -> elliptic chain.
//   4. No built Snap bundle (dist/bundle.js) contains elliptic code. Detection
//      uses elliptic's own ECDSA error strings. Before trusting them, the
//      script checks that they are present in the installed elliptic source.
//
// If elliptic leaves the lock graph entirely, the script reports that #1562
// can be closed and exits 0.
//
// Usage (from web/, after `mm-snap build` for each bundle you want checked):
//   node scripts/check-elliptic-containment.mjs [--require-bundle <pkg>]...
// `--require-bundle snap` makes a missing packages/snap/dist/bundle.js an
// error. Without it, missing bundles are skipped with a notice.

import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SNAP_PACKAGES = ['snap', 'snap-spike'];
const ALLOWED_DEV_IMPORTERS = new Set(['@botho/snap', '@botho/snap-spike']);

// Error strings from elliptic/lib/elliptic/ec/index.js. Minifiers keep string
// literals, and none of these appear in @noble/* or in the wasm-signer glue.
const FINGERPRINTS = [
  'Can not sign a negative message',
  'The recovery param is more than two bits',
  'Unable to find sencond key candinate', // sic: upstream typo, highly specific
];

const requiredBundles = new Set();
for (let i = 2; i < process.argv.length; i += 1) {
  if (process.argv[i] === '--require-bundle') requiredBundles.add(process.argv[++i]);
}

const failures = [];
const fail = (msg) => failures.push(msg);
const log = (msg) => console.log(`[elliptic-containment] ${msg}`);

function pnpmWhy(scopeFlag) {
  const out = execFileSync(
    'pnpm',
    ['why', '-r', scopeFlag, 'elliptic', '--json'],
    { cwd: WEB, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] },
  );
  const trimmed = out.trim();
  return trimmed ? JSON.parse(trimmed) : [];
}

const hasDeps = (entry, keys) => keys.some((k) => entry[k] && Object.keys(entry[k]).length > 0);

// Find the installed elliptic copies (pnpm's virtual store).
const storeDir = join(WEB, 'node_modules', '.pnpm');
const ellipticDirs = existsSync(storeDir)
  ? readdirSync(storeDir).filter((d) => d.startsWith('elliptic@'))
  : [];

// 1 + 2: lock-graph reachability.
const prod = pnpmWhy('--prod').filter((e) =>
  hasDeps(e, ['dependencies', 'optionalDependencies']),
);
const dev = pnpmWhy('--dev').filter((e) => hasDeps(e, ['devDependencies']));

if (prod.length === 0 && dev.length === 0 && ellipticDirs.length === 0) {
  log('elliptic is no longer in the lock graph. Re-run `pnpm audit` and close #1562.');
  process.exit(0);
}

for (const e of prod) {
  fail(`${e.name} reaches elliptic through a production dependency`);
}
for (const e of dev) {
  if (!ALLOWED_DEV_IMPORTERS.has(e.name)) {
    fail(`${e.name} reaches elliptic through a dev dependency. Only the Snap tooling may (#1562)`);
  }
}
log(`production importers reaching elliptic: ${prod.length}`);
log(`dev importers reaching elliptic: ${dev.map((e) => e.name).join(', ') || 'none'}`);

// Check that the fingerprints are still present in each installed elliptic.
for (const d of ellipticDirs) {
  const src = join(storeDir, d, 'node_modules', 'elliptic', 'lib', 'elliptic', 'ec', 'index.js');
  if (!existsSync(src)) {
    fail(`cannot find ${src} to check the bundle fingerprints`);
    continue;
  }
  const text = readFileSync(src, 'utf8');
  for (const fp of FINGERPRINTS) {
    if (!text.includes(fp)) {
      fail(`fingerprint "${fp}" is missing from ${d}. Update FINGERPRINTS in this script`);
    }
  }
}
log(`fingerprints checked against: ${ellipticDirs.join(', ') || 'none installed'}`);

for (const pkg of SNAP_PACKAGES) {
  const dir = join(WEB, 'packages', pkg);

  // 3: crypto polyfill must stay off.
  const configPath = join(dir, 'snap.config.ts');
  if (existsSync(configPath)) {
    const cfg = readFileSync(configPath, 'utf8');
    if (/polyfills\s*:\s*true/.test(cfg) || /polyfills\s*:\s*\{[^}]*\bcrypto\s*:\s*true/s.test(cfg)) {
      fail(`${pkg}/snap.config.ts enables the crypto polyfill (crypto-browserify -> elliptic)`);
    }
  }

  // 4: shipped bundle must not contain elliptic.
  const bundle = join(dir, 'dist', 'bundle.js');
  if (!existsSync(bundle)) {
    if (requiredBundles.has(pkg)) fail(`${pkg}/dist/bundle.js is missing. Run mm-snap build first`);
    else log(`${pkg}: no dist/bundle.js, so the bundle check was skipped`);
    continue;
  }
  const text = readFileSync(bundle, 'utf8');
  const hits = FINGERPRINTS.filter((fp) => text.includes(fp));
  if (hits.length > 0) {
    fail(`${pkg}/dist/bundle.js contains elliptic code (${hits.join('; ')})`);
  } else {
    log(`${pkg}: dist/bundle.js has no elliptic fingerprints`);
  }
}

if (failures.length > 0) {
  for (const f of failures) console.error(`[elliptic-containment] FAIL: ${f}`);
  console.error('[elliptic-containment] See audits/2026-10-31-elliptic-snap-tooling.md (#1562).');
  process.exit(1);
}
log('OK: elliptic is still limited to Snap build/test tooling (#1562 remains open)');
