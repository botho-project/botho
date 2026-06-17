#!/usr/bin/env node
/**
 * Convenience runner for the node-backed #372 + #390 + #394 + #396 tests.
 *
 * Ensures the prerequisites exist (release botho binary + wasm artifact), then
 * runs the gated `send-live-node.test.ts` (single send, #372),
 * `exchange-live-node.test.ts` (two-user bidirectional exchange, #390),
 * `lottery-live-node.test.ts` (three-user exchange until a lottery payout, #394)
 * `join-consensus-live-node.test.ts` (a fresh node joins a running local
 * chain over real libp2p and catches up 0 -> N onto the same chain, #396) and
 * `mine-after-join-live-node.test.ts` (the full #396 flavor-A demonstration:
 * a fresh node joins a running multi-minter chain, catches up 0 -> N, then
 * PARTICIPATES in consensus and MINES a block the whole network accepts,
 * earning an on-chain coinbase) with BOTHO_E2E_NODE=1 so each test spins up
 * its own throwaway node(s), asserts the ledger/sync invariants, and tears the
 * node(s) down.
 *
 *   node packages/wasm-signer/test/run-node-backed.mjs
 *
 * Run from the `web/` directory. Override the binary with BOTHO_BIN=/path.
 */

import { existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { dirname, join, resolve } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
// packages/wasm-signer/test -> web -> repo root
const webDir = resolve(here, '..', '..', '..')
const repoRoot = resolve(webDir, '..')

const pkgDir = resolve(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const binPath = process.env.BOTHO_BIN || join(repoRoot, 'target', 'release', 'botho')

function fail(msg) {
  console.error(`\n[run-node-backed] ${msg}\n`)
  process.exit(1)
}

if (!existsSync(wasmGlue)) {
  fail(
    'wasm artifact missing. Build it first:\n' +
      '  pnpm --filter @botho/wasm-signer build:wasm',
  )
}
if (!existsSync(binPath)) {
  fail(
    `botho binary missing at ${binPath}. Build it first:\n` +
      '  cargo build --release --bin botho\n' +
      'or set BOTHO_BIN=/path/to/botho',
  )
}

console.log('[run-node-backed] binary :', binPath)
console.log('[run-node-backed] wasm   :', wasmGlue)
console.log('[run-node-backed] running gated node-backed test...\n')

const result = spawnSync(
  'pnpm',
  [
    'exec',
    'vitest',
    'run',
    'packages/wasm-signer/test/send-live-node.test.ts',
    'packages/wasm-signer/test/exchange-live-node.test.ts',
    'packages/wasm-signer/test/lottery-live-node.test.ts',
    'packages/wasm-signer/test/join-consensus-live-node.test.ts',
    // mine-after-join is BLOCKED ON #417 (multi-node SCP agreement / fork) and
    // is `describe.skip` for now; kept listed so it re-activates automatically
    // once #417 lands genuine, fork-free multi-node agreement.
    'packages/wasm-signer/test/mine-after-join-live-node.test.ts',
  ],
  {
    cwd: webDir,
    stdio: 'inherit',
    env: {
      ...process.env,
      BOTHO_E2E_NODE: '1',
      BOTHO_BIN: binPath,
    },
  },
)

process.exit(result.status ?? 1)
