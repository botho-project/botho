/**
 * Wire the node-identical `bth-wasm-signer` (wasm-pack `--target bundler`
 * output, base64-inlined into the snap bundle by the snaps-cli wasm loader) into
 * the shared `@botho/wasm-signer` high-level orchestrators.
 *
 * `buildSendTransaction` / `spendableBalance` (the SAME code the web wallet
 * runs) call `loadSigner()` internally, which by default instantiates the wasm
 * via a browser `fetch` of its URL — that does not work inside the Snaps SES
 * executor. Instead we import the already-instantiated bundler module and inject
 * it once via `setSigner`, so all downstream crypto uses the inlined wasm.
 *
 * The Phase-0 spike (PR #1055) proved this module loads, instantiates and runs
 * correctly under SES, with `getrandom`'s `crypto.getRandomValues` endowment
 * live (build/scan/sign ~26-28 ms).
 */

import { setSigner, type WasmSigner } from '@botho/wasm-signer';

// The wasm-pack `--target bundler` glue. Its inner `import * as wasm from
// './bth_wasm_signer_bg.wasm'` is handled by the snaps-cli wasm loader
// (`experimental.wasm: true`), which base64-inlines the module and instantiates
// it synchronously at bundle load.
import * as wasm from '@botho/wasm-signer/pkg-bundler/bth_wasm_signer.js';

/**
 * Adapt the raw wasm exports to the `WasmSigner` interface the shared send /
 * balance orchestrators expect.
 */
function makeSigner(): WasmSigner {
  return {
    buildAndSign: (request) => wasm.buildAndSign(request),
    scanOwnedOutputs: (request) =>
      wasm.scanOwnedOutputs(request) as ReturnType<WasmSigner['scanOwnedOutputs']>,
    computeOwnedOutputKeyImages: (request) =>
      wasm.computeOwnedOutputKeyImages(request) as ReturnType<
        WasmSigner['computeOwnedOutputKeyImages']
      >,
    derivePqPublicKeysFromSeed: (seedHex) =>
      wasm.derivePqPublicKeysFromSeed(seedHex) as {
        kemPublicKey: string;
        dsaPublicKey: string;
      },
    deriveAddressFromSeed: (seedHex, viewHex, spendHex, testnet) =>
      wasm.deriveAddressFromSeed(seedHex, viewHex, spendHex, testnet),
    encodeAddress: (viewHex, spendHex, kemHex, dsaHex, testnet) =>
      wasm.encodeAddress(viewHex, spendHex, kemHex, dsaHex, testnet),
    decodeAddress: (address) =>
      wasm.decodeAddress(address) as ReturnType<NonNullable<WasmSigner['decodeAddress']>>,
    ringSize: () => wasm.ringSize(),
    minFee: () => wasm.minFee(),
  };
}

let injected = false;

/** Inject the inlined wasm signer once (idempotent). */
export function ensureSigner(): void {
  if (!injected) {
    setSigner(makeSigner());
    injected = true;
  }
}

/** Direct access to the raw wasm module (for address derivation / constants). */
export { wasm };
