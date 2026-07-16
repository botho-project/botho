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
  type WasmSigner,
} from './index'

/** The account private keys the caller derived from the mnemonic. */
export interface SignerKeys {
  /** Hex-encoded 32-byte account spend private key. */
  spendPrivateKey: string
  /** Hex-encoded 32-byte account view private key. */
  viewPrivateKey: string
  /**
   * Hex-encoded 64-byte BIP39 seed (from `mnemonicToSeedSync(mnemonic, '')`).
   * Threaded into the RECEIVE scan so it can derive the wallet's ML-KEM secret
   * and detect 6.0.0 hybrid outputs — incoming payments and the wallet's own
   * change. Optional: omit for a classical-only scan (#988).
   */
  seed?: string
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
  /**
   * Query the node's `chain_areKeyImagesSpent` RPC: given a list of hex-encoded
   * key images, return for each whether it is spent on-chain or pending in the
   * mempool. The wallet uses this to exclude already-spent owned outputs from
   * its balance and from spendable-input selection (so it never tries to
   * double-spend its own output). Order is preserved to match the input list.
   */
  areKeyImagesSpent(keyImages: string[]): Promise<KeyImageSpentStatus[]>
}

/** The spent/pending status of a single key image, as returned by the node. */
export interface KeyImageSpentStatus {
  /** The queried key image (hex). */
  keyImage: string
  /** True if the key image is recorded in the on-chain double-spend set. */
  spent: boolean
  /** Block height the key image was spent at, or null if unspent. */
  spentHeight: number | null
  /** True if the key image is currently pending in the mempool. */
  pending: boolean
}

/**
 * Filter a wallet's owned outputs down to the ones that are actually spendable:
 * those whose key image is neither spent on-chain nor pending in the mempool.
 *
 * This is the core of the thin-wallet spent-awareness fix (#392): without it,
 * the wallet counts already-spent outputs in its balance (overstating it) and
 * could select a spent output as a transaction input (a guaranteed
 * double-spend rejection). It mirrors the node's own `wallet_getBalance`
 * filtering, but works for arbitrary thin-wallet keys.
 */
export async function spendableOwnedOutputs(
  signer: WasmSigner,
  keys: SignerKeys,
  owned: OwnedOutput[],
  rpc: SendRpc,
): Promise<OwnedOutput[]> {
  if (owned.length === 0) return []

  // Derive each owned output's key image (node-identical derivation inside
  // wasm), then ask the node which are spent/pending.
  const withImages = signer.computeOwnedOutputKeyImages({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed,
    outputs: owned,
  })
  const statuses = await rpc.areKeyImagesSpent(withImages.map((o) => o.keyImage))

  // Map key image -> spendable. Treat anything we couldn't get a clear answer
  // for as spent (conservative: never overstate balance / never select it).
  const spendableByKeyImage = new Map<string, boolean>()
  for (const s of statuses) {
    spendableByKeyImage.set(s.keyImage, !s.spent && !s.pending)
  }

  return withImages
    .filter((o) => spendableByKeyImage.get(o.keyImage) === true)
    .map((o) => ({
      targetKey: o.targetKey,
      publicKey: o.publicKey,
      amount: o.amount,
      subaddressIndex: o.subaddressIndex,
    }))
}

/**
 * Compute the wallet's spendable balance: the sum of owned outputs that are
 * neither spent on-chain nor pending. This is the figure the UI should display
 * — it excludes outputs the wallet has already spent (#392).
 */
export async function spendableBalance(
  keys: SignerKeys,
  rpc: SendRpc,
): Promise<bigint> {
  const signer = await loadSigner()
  const height = await rpc.getChainHeight()
  const candidates = await rpc.getOutputs(0, height)
  if (candidates.length === 0) return 0n

  const owned = signer.scanOwnedOutputs({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed,
    outputs: candidates,
  })
  const spendable = await spendableOwnedOutputs(signer, keys, owned, rpc)
  return spendable.reduce((s, o) => s + toBigInt(o.amount), 0n)
}

/**
 * Return the target keys (hex) of the wallet's owned outputs. These identify the
 * wallet's cluster to the node's progressive-fee lookup
 * (`cluster_getWealthByTargetKeys`), so the wallet can fetch its real cluster
 * wealth before an `estimateFee` call and be charged the correct fee factor
 * (#626/#628/#634).
 *
 * Uses the same node-identical wasm ownership scan as {@link spendableBalance}.
 * Includes all owned outputs (not just currently-spendable ones): the node
 * clusters a wallet by every output it has ever touched, so spent outputs still
 * contribute to cluster identity. Returns an empty array for a wallet with no
 * owned outputs.
 */
export async function ownedOutputTargetKeys(
  keys: SignerKeys,
  rpc: SendRpc,
): Promise<string[]> {
  const signer = await loadSigner()
  const height = await rpc.getChainHeight()
  const candidates = await rpc.getOutputs(0, height)
  if (candidates.length === 0) return []
  const owned = signer.scanOwnedOutputs({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed,
    outputs: candidates,
  })
  return owned.map((o) => o.targetKey)
}

/**
 * A chain output annotated with the block it landed in and its source tx hash.
 * This is what the node returns via `chain_getOutputs` (height per block, txHash
 * per output) and is the raw material for client-side transaction history.
 */
export interface ChainOutputWithMeta extends ChainOutput {
  /** Hex-encoded hash of the transaction that created this output. */
  txHash: string
  /** Block height the output was confirmed at. */
  height: number
}

/** A single client-side transaction-history entry. */
export interface HistoryEntry {
  /** Source transaction hash (the output's creating tx). */
  txHash: string
  /** `receive` for an owned output; `spend` for an owned output later spent. */
  type: 'receive' | 'spend'
  /** Decoded amount of the owned output, in picocredits. */
  amount: bigint
  /** Block height the output landed in (its receive height). */
  blockHeight: number
  /** True if the output's key image is spent on-chain or pending. */
  spent: boolean
  /** Height the output was spent at, if known (on-chain spends only). */
  spentHeight: number | null
}

/**
 * The slice of node RPC the history path needs: every chain output in a height
 * range WITH its block height + tx hash, plus the key-image spent check.
 * Implemented by the wallet's `RemoteNodeAdapter`.
 */
export interface HistoryRpc {
  getChainHeight(): Promise<number>
  getOutputsWithMeta(
    startHeight: number,
    endHeight: number,
  ): Promise<ChainOutputWithMeta[]>
  areKeyImagesSpent(keyImages: string[]): Promise<KeyImageSpentStatus[]>
}

/**
 * Build the wallet's transaction history CLIENT-SIDE from its OWNED outputs.
 *
 * This is the keys-aware counterpart to the node adapter's old
 * `getTransactionHistory` stub, which mapped EVERY chain output to a bogus
 * "received 0 BTH" entry because the adapter has no wallet keys (#459). Here we
 * reuse the exact balance scan path:
 *
 *   fetch outputs (with block height) -> `scanOwnedOutputs` (wasm) keeps only
 *   the user's outputs with their real decoded amounts -> derive key images and
 *   ask `chain_areKeyImagesSpent` which are spent.
 *
 * Each owned output becomes ONE `receive` entry with its true amount and block
 * height. Owned outputs whose key image is spent are additionally surfaced as a
 * `spend` entry (we can prove the user spent the output even though the ring
 * hides which tx consumed it), and the receive entry is flagged `spent`. No
 * non-owned outputs are ever returned, so there is no 0-BTH spam.
 *
 * Returned newest-first (highest block height first).
 */
export async function buildOwnedHistory(
  keys: SignerKeys,
  rpc: HistoryRpc,
): Promise<HistoryEntry[]> {
  const signer = await loadSigner()
  const height = await rpc.getChainHeight()
  const candidates = await rpc.getOutputsWithMeta(0, height)
  if (candidates.length === 0) return []

  // Identify owned outputs (node-identical ownership check in wasm). The scan
  // input drops the meta, so map owned outputs back to their height/txHash by
  // their unique one-time target key.
  const metaByTargetKey = new Map<string, ChainOutputWithMeta>()
  for (const c of candidates) metaByTargetKey.set(c.targetKey, c)

  const owned = signer.scanOwnedOutputs({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed,
    outputs: candidates.map((c) => ({
      targetKey: c.targetKey,
      publicKey: c.publicKey,
      amount: c.amount,
      // Preserve the hybrid metadata so 6.0.0 outputs are detected (#988).
      outputIndex: c.outputIndex,
      kemCiphertext: c.kemCiphertext,
    })),
  })
  if (owned.length === 0) return []

  // Derive key images for the owned outputs and ask the node which are spent.
  const withImages = signer.computeOwnedOutputKeyImages({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed,
    outputs: owned,
  })
  const statuses = await rpc.areKeyImagesSpent(withImages.map((o) => o.keyImage))
  const statusByKeyImage = new Map<string, KeyImageSpentStatus>()
  for (const s of statuses) statusByKeyImage.set(s.keyImage, s)

  const entries: HistoryEntry[] = []
  for (const o of withImages) {
    const meta = metaByTargetKey.get(o.targetKey)
    const blockHeight = meta?.height ?? 0
    const txHash = meta?.txHash ?? o.targetKey
    const status = statusByKeyImage.get(o.keyImage)
    const spent = !!status && (status.spent || status.pending)
    const spentHeight = status?.spentHeight ?? null

    // The receive of this owned output (always shown, with its real amount).
    entries.push({
      txHash,
      type: 'receive',
      amount: toBigInt(o.amount),
      blockHeight,
      spent,
      spentHeight,
    })

    // If we've spent it, also record a spend entry so history shows the outflow.
    if (spent) {
      entries.push({
        txHash,
        type: 'spend',
        amount: toBigInt(o.amount),
        blockHeight: spentHeight ?? blockHeight,
        spent: true,
        spentHeight,
      })
    }
  }

  // Newest first.
  entries.sort((a, b) => b.blockHeight - a.blockHeight)
  return entries
}

/**
 * A history ROW ready for UI rendering: one row per user-visible event, with a
 * unique id, rather than one row per owned output (#675).
 */
export interface NettedHistoryEntry {
  /** Stable, unique row id (safe as a React key). */
  id: string
  type: 'receive' | 'send'
  /**
   * Net amount in picocredits, always positive. For a send this is the net
   * outflow after subtracting same-block change; for a receive it is the sum
   * of the tx's owned outputs.
   */
  amount: bigint
  /** Block height of the event; 0 for a not-yet-mined (pending) spend. */
  blockHeight: number
  /** 'pending' while an outflow's key image is only in mempools. */
  status: 'pending' | 'confirmed'
}

/**
 * Collapse per-output {@link HistoryEntry} rows into per-event rows (#675).
 *
 * `buildOwnedHistory` intentionally reports raw per-output facts; rendered
 * as-is they have three defects: (a) a spent output emits a receive AND a
 * spend entry with the SAME txHash (duplicate React keys), (b) a multi-output
 * receive emits one row per output with the same txHash, and (c) a send shows
 * "-<whole input>" plus "+<change>" instead of the net outflow.
 *
 * Netting heuristic: the ring signature hides which tx consumed an output, so
 * a spend only knows its `spentHeight`. Owned outputs RECEIVED in that same
 * block are treated as the spend's change and subtracted — for a single-user
 * wallet that is exactly the change output. If the subtraction would go
 * negative (e.g. a genuine incoming payment landed in the same block), the
 * receives are kept as their own rows instead, so an incoming amount is never
 * silently swallowed.
 */
export function netOwnedHistory(entries: HistoryEntry[]): NettedHistoryEntry[] {
  // Group receives by creating tx: one row per receiving tx.
  const receiveByTx = new Map<string, { amount: bigint; blockHeight: number }>()
  // Group spends by the block they were spent in (null = still pending).
  const spendByHeight = new Map<number, bigint>()
  let pendingSpend = 0n

  for (const e of entries) {
    if (e.type === 'receive') {
      const g = receiveByTx.get(e.txHash)
      if (g) {
        g.amount += e.amount
      } else {
        receiveByTx.set(e.txHash, { amount: e.amount, blockHeight: e.blockHeight })
      }
    } else if (e.spentHeight != null) {
      spendByHeight.set(e.spentHeight, (spendByHeight.get(e.spentHeight) ?? 0n) + e.amount)
    } else {
      pendingSpend += e.amount
    }
  }

  const rows: NettedHistoryEntry[] = []
  const consumedAsChange = new Set<string>()

  for (const [height, spentSum] of spendByHeight) {
    // Candidate change: receives that landed in the spend's block.
    const changeTxs: string[] = []
    let changeSum = 0n
    for (const [txHash, g] of receiveByTx) {
      if (g.blockHeight === height) {
        changeTxs.push(txHash)
        changeSum += g.amount
      }
    }
    if (changeSum <= spentSum) {
      for (const txHash of changeTxs) consumedAsChange.add(txHash)
      rows.push({
        id: `send-${height}`,
        type: 'send',
        amount: spentSum - changeSum,
        blockHeight: height,
        status: 'confirmed',
      })
    } else {
      // More received than spent in this block: don't guess which part is
      // change — show the gross spend and keep the receives visible.
      rows.push({
        id: `send-${height}`,
        type: 'send',
        amount: spentSum,
        blockHeight: height,
        status: 'confirmed',
      })
    }
  }

  if (pendingSpend > 0n) {
    // The unmined spend's change is also unmined, so nothing to net against;
    // the row corrects itself to the net amount once the tx confirms.
    rows.push({
      id: 'send-pending',
      type: 'send',
      amount: pendingSpend,
      blockHeight: 0,
      status: 'pending',
    })
  }

  for (const [txHash, g] of receiveByTx) {
    if (consumedAsChange.has(txHash)) continue
    rows.push({
      id: `recv-${txHash}`,
      type: 'receive',
      amount: g.amount,
      blockHeight: g.blockHeight,
      status: 'confirmed',
    })
  }

  // Pending first, then newest first.
  rows.sort((a, b) => {
    if (a.status !== b.status) return a.status === 'pending' ? -1 : 1
    return b.blockHeight - a.blockHeight
  })
  return rows
}

/** Inputs to {@link buildSendTransaction}. */
export interface BuildSendParams {
  /** Account keys derived from the wallet mnemonic. */
  keys: SignerKeys
  /**
   * Recipient address keys (decoded from a `botho://2/` address). Must include
   * the recipient's raw ML-KEM-768 public key (`kem_public_key`) so the send
   * output can be a hybrid post-quantum output (#978).
   */
  recipient: RecipientAddress
  /**
   * Hex-encoded raw ML-KEM-768 public key (1184 bytes) of the SENDER's own v2
   * address, used to encapsulate the change output back to the sender (#978).
   * Derive it from the wallet seed via `derivePqPublicKeysFromSeed`.
   */
  senderKemPublicKey: string
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
  const { keys, recipient, senderKemPublicKey, amount, fee, rpc } = params

  if (amount <= 0n) throw new Error('Amount must be greater than 0')
  if (!recipient.kem_public_key) {
    throw new Error(
      'Recipient address has no ML-KEM key: it is a retired v1 / classical-only ' +
        'address that cannot receive on the post-quantum (6.0.0) chain. Ask the ' +
        'recipient for a current botho://2/ address.',
    )
  }
  if (!senderKemPublicKey) {
    throw new Error('Missing sender ML-KEM public key for change encapsulation')
  }

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
    seed: keys.seed,
    outputs: candidates,
  })
  if (owned.length === 0) {
    throw new Error('No spendable outputs found for this wallet')
  }

  // 1b. Exclude outputs the wallet has already spent (on-chain or pending).
  // Selecting a spent output as an input is a guaranteed double-spend
  // rejection, so we filter to spendable outputs before input selection (#392).
  const spendable = await spendableOwnedOutputs(signer, keys, owned, rpc)
  if (spendable.length === 0) {
    throw new Error('No spendable outputs found for this wallet (all spent)')
  }

  // 2. Select inputs covering amount + fee.
  const target = amount + fee
  const inputs = selectInputs(spendable, target)
  if (!inputs) {
    const have = spendable.reduce((s, o) => s + toBigInt(o.amount), 0n)
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
    senderKemPublicKey,
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
