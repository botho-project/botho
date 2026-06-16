/**
 * Load the @botho/wasm-signer wasm in the Playwright test process (Node), using
 * the raw-bytes init path (`default({ module_or_path })`) because the browser
 * `fetch`-based `loadSigner()` does not work under Node. Used by the full-stack
 * send spec to run the recipient's ownership scan (the same check the recipient
 * wallet would run in the browser).
 */
import { readFileSync, existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import type { WasmSigner } from '@botho/wasm-signer'

const here = dirname(fileURLToPath(import.meta.url))
// e2e/tests/fullstack -> web/packages/wasm-signer/pkg
const pkgDir = join(here, '..', '..', '..', 'packages', 'wasm-signer', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')

interface WasmMod extends WasmSigner {
  default: (init: { module_or_path: BufferSource }) => Promise<unknown>
}

export async function loadSignerNode(): Promise<WasmSigner> {
  if (!existsSync(wasmGlue) || !existsSync(wasmBin)) {
    throw new Error(
      `wasm artifact missing at ${pkgDir}. Build it with ` +
        '`pnpm --filter @botho/wasm-signer build:wasm`.',
    )
  }
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as unknown as WasmMod
  await mod.default({ module_or_path: readFileSync(wasmBin) })
  return {
    buildAndSign: (request) => mod.buildAndSign(request),
    scanOwnedOutputs: (request) => mod.scanOwnedOutputs(request),
    ringSize: () => mod.ringSize(),
    minFee: () => mod.minFee(),
  }
}
