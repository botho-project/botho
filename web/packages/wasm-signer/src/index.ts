/**
 * @botho/wasm-signer
 *
 * Typed TypeScript facade over the WebAssembly Botho transaction builder +
 * CLSAG signer. The heavy cryptography (stealth-key recovery, CLSAG ring
 * signature, bincode serialization) runs in wasm compiled from the same Rust
 * code the node uses, so the produced transaction round-trips through the same
 * verifier the node runs.
 *
 * The private keys are passed in by the caller and used only inside wasm — they
 * never leave the browser and are never sent to the node.
 *
 * ## Building the wasm artifact
 *
 * The wasm package under `pkg/` is generated (and git-ignored). Build it with:
 *
 * ```sh
 * pnpm --filter @botho/wasm-signer build:wasm
 * ```
 *
 * which runs `wasm-pack build --target web`. Until that has been run, importing
 * the wasm module will fail; {@link loadSigner} surfaces a clear error.
 */

/** A ring member (decoy or real output) sourced from the chain. */
export interface DecoyOutput {
  /** Hex-encoded 32-byte one-time target key of the output. */
  target_key: string
  /** Hex-encoded 32-byte ephemeral public key of the output. */
  public_key: string
  /** Amount in picocredits committed by this output. */
  amount: bigint | number
}

/** One of the wallet's own outputs being spent, with its decoy ring. */
export interface SpendInput {
  /** Hex-encoded 32-byte one-time target key of the owned output. */
  target_key: string
  /** Hex-encoded 32-byte ephemeral public key of the owned output. */
  public_key: string
  /** Amount in picocredits of the owned output. */
  amount: bigint | number
  /** Subaddress index that received this output (0 = default, 1 = change). */
  subaddress_index: bigint | number
  /**
   * Decoys for this input's ring. Must contain at least `ringSize() - 1`
   * distinct outputs.
   */
  decoys: DecoyOutput[]
}

/** A recipient address, as the two 32-byte Ristretto public keys (hex). */
export interface RecipientAddress {
  /** Hex-encoded 32-byte spend public key (`D`). */
  spend_public_key: string
  /** Hex-encoded 32-byte view public key (`C`). */
  view_public_key: string
}

/**
 * The full client-side signing request. Field names are camelCase to match the
 * wasm `serde` binding.
 */
export interface SignRequest {
  /** Hex-encoded 32-byte account spend private key. Stays client-side. */
  spendPrivateKey: string
  /** Hex-encoded 32-byte account view private key. Stays client-side. */
  viewPrivateKey: string
  /** Owned outputs being spent (one ring per input). */
  inputs: SpendInput[]
  /** Recipient of the transfer. */
  recipient: RecipientAddress
  /** Amount to send to the recipient, in picocredits. */
  amount: bigint | number
  /** Transaction fee in picocredits. */
  fee: bigint | number
  /** Chain height to stamp the transaction with (replay protection). */
  createdAtHeight: bigint | number
}

/** The wasm module's exported surface. */
export interface WasmSigner {
  /**
   * Build and CLSAG-sign a transaction. Returns the hex-encoded bincode bytes
   * of the signed transaction, ready to submit via `tx_submit`.
   * Throws on any failure (bad keys, insufficient decoys, balance mismatch).
   */
  buildAndSign(request: SignRequest): string
  /** The CLSAG ring size the network requires (decoys + 1 real input). */
  ringSize(): number
  /** The minimum transaction fee in picocredits. */
  minFee(): bigint
}

let cached: Promise<WasmSigner> | null = null

/**
 * Load and initialize the wasm signer. Cached after the first call.
 *
 * Requires the wasm package to have been built (`build:wasm`). Throws a
 * descriptive error if the generated module is missing.
 */
export async function loadSigner(): Promise<WasmSigner> {
  if (cached) return cached
  cached = (async () => {
    let mod: {
      default: (init?: unknown) => Promise<unknown>
      buildAndSign: (request: unknown) => string
      ringSize: () => number
      minFee: () => bigint
    }
    try {
      // The generated package is git-ignored and produced by `build:wasm`.
      // Use a dynamic import with a variable specifier so bundlers/typecheck
      // don't hard-fail when the artifact is absent.
      const spec = '../pkg/bth_wasm_signer.js'
      mod = (await import(/* @vite-ignore */ spec)) as typeof mod
    } catch (err) {
      throw new Error(
        '@botho/wasm-signer: wasm artifact not found. Run ' +
          '`pnpm --filter @botho/wasm-signer build:wasm` first. ' +
          `Underlying error: ${err instanceof Error ? err.message : String(err)}`,
      )
    }
    // `default` is the wasm init function; awaiting it instantiates the module.
    await mod.default()
    return {
      buildAndSign: (request: SignRequest) => mod.buildAndSign(request),
      ringSize: () => mod.ringSize(),
      minFee: () => mod.minFee(),
    }
  })()
  return cached
}

/** Reset the cached signer (primarily for tests). */
export function resetSigner(): void {
  cached = null
}
