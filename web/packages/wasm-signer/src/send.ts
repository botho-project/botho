/**
 * High-level "build a send" orchestrator.
 *
 * Composes the low-level wasm primitives ({@link loadSigner}'s
 * `scanOwnedOutputs` / `buildAndSign`) plus the wallet's key derivation and the
 * node RPC into a single call the wallet UI can use:
 *
 *   derive keys -> scan chain for owned outputs -> select inputs ->
 *   gather decoys -> build + CLSAG-sign -> hex tx ready for `tx_submit`.
 *
 * All cryptography (ownership detection, one-time-key recovery, ring signature)
 * runs inside the wasm module compiled from the same Rust the node runs, so the
 * resulting transaction round-trips through the node's verifier. The spend/view
 * private keys are passed straight into wasm and never sent to the node.
 */

import {
  loadSigner,
  type ChainOutput,
  type OwnedOutput,
  type RecipientAddress,
  type SignRequest,
  type SpendInput,
} from './index'

/** The account private keys the caller derived from the mnemonic. */
export interface SignerKeys {
  /** Hex-encoded 32-byte account spend private key. */
  spendPrivateKey: string
  /** Hex-encoded 32-byte account view private key. */
  viewPrivateKey: string
}

/**
 * The slice of node RPC the send path needs. Implemented by the wallet's
 * `RemoteNodeAdapter`; abstracted here so this package does not depend on the
 * adapters package.
 */
export interface SendRpc {
  /** Current chain height (stamped on the tx for replay protection). */
  getChainHeight(): Promise<number>
  /**
   * Fetch every output on the chain in `[startHeight, endHeight]` as raw
   * `{ targetKey, publicKey, amount }`. The amount is the transparent value
   * recovered from the output's commitment.
   */
  getOutputs(startHeight: number, endHeight: number): Promise<ChainOutput[]>
}

/** Inputs to {@link buildSendTransaction}. */
export interface BuildSendParams {
  /** Account keys derived from the wallet mnemonic. */
  keys: SignerKeys
  /** Recipient address keys (decoded from a `tbotho://` address). */
  recipient: RecipientAddress
  /** Amount to send, in picocredits. */
  amount: bigint
  /** Fee, in picocredits. Must be >= the network minimum. */
  fee: bigint
  /** Node RPC accessor. */
  rpc: SendRpc
}

/** Result of a successful build: the signed tx plus the inputs that were used. */
export interface BuildSendResult {
  /** Hex-encoded bincode tx, ready for `tx_submit`. */
  txHex: string
  /** The owned outputs selected as inputs. */
  inputs: OwnedOutput[]
  /** Total picocredits of the selected inputs. */
  inputTotal: bigint
}

function toBigInt(v: bigint | number): bigint {
  return typeof v === 'bigint' ? v : BigInt(v)
}

/**
 * Greedily select the fewest owned outputs whose total covers `target`,
 * largest-first. Returns null if the wallet cannot cover the target.
 */
function selectInputs(owned: OwnedOutput[], target: bigint): OwnedOutput[] | null {
  const sorted = [...owned].sort((a, b) => {
    const d = toBigInt(b.amount) - toBigInt(a.amount)
    return d > 0n ? 1 : d < 0n ? -1 : 0
  })
  const chosen: OwnedOutput[] = []
  let total = 0n
  for (const o of sorted) {
    chosen.push(o)
    total += toBigInt(o.amount)
    if (total >= target) return chosen
  }
  return null
}

/**
 * Build and CLSAG-sign a send transaction. Throws a descriptive error if the
 * wallet has insufficient funds, the chain lacks enough decoys for the ring, or
 * the signer rejects the request.
 */
export async function buildSendTransaction(
  params: BuildSendParams,
): Promise<BuildSendResult> {
  const { keys, recipient, amount, fee, rpc } = params

  if (amount <= 0n) throw new Error('Amount must be greater than 0')

  const signer = await loadSigner()
  const ringSize = signer.ringSize()
  const decoysPerInput = ringSize - 1

  const height = await rpc.getChainHeight()
  // Scan the whole chain so we both (a) find every owned output and (b) have a
  // large pool of real on-chain outputs to draw ring decoys from.
  const candidates = await rpc.getOutputs(0, height)
  if (candidates.length === 0) {
    throw new Error('Node returned no outputs to scan')
  }

  // 1. Identify owned outputs via the node-identical wasm ownership check.
  const owned = signer.scanOwnedOutputs({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    outputs: candidates,
  })
  if (owned.length === 0) {
    throw new Error('No spendable outputs found for this wallet')
  }

  // 2. Select inputs covering amount + fee.
  const target = amount + fee
  const inputs = selectInputs(owned, target)
  if (!inputs) {
    const have = owned.reduce((s, o) => s + toBigInt(o.amount), 0n)
    throw new Error(
      `Insufficient funds: need ${target} picocredits (amount + fee), have ${have}`,
    )
  }
  const inputTotal = inputs.reduce((s, o) => s + toBigInt(o.amount), 0n)

  // 3. Gather decoys. A decoy ring member only needs to be a valid on-chain
  // output that is NOT one of the real inputs being spent — the node's own
  // decoy selector likewise excludes only the real inputs (see
  // `decoy_selection.rs` `select_decoys`, which filters by `exclude_keys` =
  // the real inputs). In particular decoys MAY be the wallet's own other
  // outputs; requiring foreign-only decoys would make a solo-mined / low-
  // traffic chain unspendable. We still drop the all-zero genesis placeholder.
  const inputKeys = new Set(inputs.map((o) => o.targetKey))
  const isZeroKey = (k: string) => /^0+$/.test(k)
  const decoyPool = candidates.filter(
    (c) => !inputKeys.has(c.targetKey) && !isZeroKey(c.targetKey),
  )
  if (decoyPool.length < decoysPerInput) {
    throw new Error(
      `Not enough decoys on chain for a ring of ${ringSize}: ` +
        `need ${decoysPerInput} per input, found ${decoyPool.length}. ` +
        'Mine more blocks / wait for more on-chain outputs.',
    )
  }

  const spendInputs: SpendInput[] = inputs.map((input, i) => {
    // Rotate a window over the decoy pool so each input gets distinct decoys
    // when the pool is large; if the pool is exactly the minimum, reuse it
    // (rings still differ because the real member differs).
    const decoys: ChainOutput[] = []
    for (let j = 0; j < decoysPerInput; j++) {
      decoys.push(decoyPool[(i * decoysPerInput + j) % decoyPool.length])
    }
    return {
      target_key: input.targetKey,
      public_key: input.publicKey,
      amount: toBigInt(input.amount),
      subaddress_index: toBigInt(input.subaddressIndex),
      decoys: decoys.map((d) => ({
        target_key: d.targetKey,
        public_key: d.publicKey,
        amount: toBigInt(d.amount),
      })),
    }
  })

  const request: SignRequest = {
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    inputs: spendInputs,
    recipient,
    amount,
    fee,
    createdAtHeight: height,
  }

  // 4. Build + CLSAG-sign inside wasm. The signer self-verifies the produced tx
  // against the node's verifier before returning, so a returned hex is a tx the
  // node should accept (subject to mempool policy like double-spend checks).
  const txHex = signer.buildAndSign(request)

  return { txHex, inputs, inputTotal }
}
