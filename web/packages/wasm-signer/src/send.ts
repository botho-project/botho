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
  isLegacyLotteryPayout,
  isV2LotteryPayout,
  loadSigner,
  type ChainOutput,
  type LotteryContext,
  type OwnedOutput,
  type RecipientAddress,
  type SignRequest,
  type SpendInput,
  type WasmSigner,
} from './index'
import { outputAge, selectDecoysForInput } from './decoys'

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
   * recovered from the output's commitment. Preserve validated chain order
   * (height, then ledger insertion order) so the first occurrence of a target
   * is its canonical output. The RemoteNodeAdapter preserves this RPC order.
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
  /**
   * Quote a signed draft (`txHex`, as for `tx_submit`) with the node's
   * `estimateFee` RPC (`tx_hex` form, #2001). The node prices it with its own
   * consensus fee floor (`Ledger::consensus_fee_floor`) and relay floor, which
   * include the demurrage, the class-exit charge (#1685) and the import floor
   * read from the inputs (#1687). Those depend on the draft's ring and on
   * node-side cluster wealth, so no client-side estimate can see them; since
   * protocol 10.0.0 a spend that skips this quote is refused as `Fee too low`.
   *
   * When present, {@link buildSendTransaction} signs, quotes and re-signs at
   * `resignFee` until the fee settles. Every production send path must supply
   * it ({@link quoteDraftFeeVia} builds it from a JSON-RPC caller); it is
   * optional only so offline test doubles can sign a single pass.
   */
  quoteDraftFee?(txHex: string): Promise<DraftFeeQuote>
}

/**
 * The node's quote for a signed draft: `estimateFee` with `tx_hex` (#2001).
 * Mirrors `botho_wallet::fee_estimation::DraftFeeQuote`.
 */
export interface DraftFeeQuote {
  /** `max(relayFeeFloor, MIN_TX_FEE)`: the least fee the node admits. */
  minimumFee: bigint
  /** The block-validity floor at the next height. */
  consensusFeeFloor: bigint
  /** The relay admission floor (>= the consensus floor). */
  relayFeeFloor: bigint
  /** A draft whose fee is at least this is priced. */
  settleFee: bigint
  /** The fee to re-sign at when the draft does not settle. */
  resignFee: bigint
  /** The fee the quoted draft carries (absent on a test double). */
  txFee?: bigint
}

/** Most sign-quote passes before a send gives up (as `botho send`). */
export const MAX_FEE_PASSES = 8

/** {@link FeeAboveApprovedError.code}, stable across bundles (#2001). */
export const FEE_ABOVE_APPROVED = 'FEE_ABOVE_APPROVED'

/**
 * The node's fee floor for this send exceeds the fee the user approved
 * (`maxFee`), so {@link buildSendTransaction} stopped BEFORE signing at it
 * (#2001). Nothing above `approvedFee` was signed, quoted or submitted. The UI
 * must show `requiredFee` (and, for a sweep, the reduced amount) and ask the
 * user again; on approval it rebuilds with `fee = maxFee = requiredFee`.
 */
export class FeeAboveApprovedError extends Error {
  readonly code = FEE_ABOVE_APPROVED
  constructor(
    /** The fee the node requires (its `resignFee`: floor plus headroom). */
    readonly requiredFee: bigint,
    /** The fee the user approved (`maxFee`). */
    readonly approvedFee: bigint,
  ) {
    super(
      `The node's fee floor requires ${requiredFee} picocredits, above the approved ` +
        `${approvedFee}; nothing was submitted. Confirm the higher fee to continue.`,
    )
    this.name = 'FeeAboveApprovedError'
  }
}

/**
 * True for a {@link FeeAboveApprovedError}, including one from another copy
 * of this module (bundles may duplicate the class, so match on `code`).
 */
export function isFeeAboveApprovedError(e: unknown): e is FeeAboveApprovedError {
  return (
    typeof e === 'object' &&
    e !== null &&
    (e as { code?: unknown }).code === FEE_ABOVE_APPROVED &&
    typeof (e as { requiredFee?: unknown }).requiredFee === 'bigint'
  )
}

/** Most times a send re-asks the user for a higher fee before giving up. */
export const MAX_FEE_RECONFIRMS = 3

/**
 * Run a priced build under user fee consent (#2001). `build(fee)` must call
 * {@link buildSendTransaction} with `fee` AND `maxFee` both set to `fee`, so
 * nothing above the approved fee is ever signed. When the node requires more,
 * `reconfirm(requiredFee, approvedFee)` asks the user; on approval the build
 * reruns at exactly `requiredFee`, on refusal `onReject()` is thrown. The
 * caller submits only the returned result, whose fee is the last approved fee.
 */
export async function buildWithFeeConsent<T>(opts: {
  /** The fee the user already approved. */
  approvedFee: bigint
  build: (fee: bigint) => Promise<T>
  reconfirm: (requiredFee: bigint, approvedFee: bigint) => Promise<boolean>
  onReject: () => Error
  maxReconfirms?: number
}): Promise<{ result: T; fee: bigint }> {
  const limit = opts.maxReconfirms ?? MAX_FEE_RECONFIRMS
  let fee = opts.approvedFee
  for (let attempt = 0; ; attempt++) {
    try {
      return { result: await opts.build(fee), fee }
    } catch (e) {
      if (!isFeeAboveApprovedError(e) || attempt >= limit) throw e
      if (!(await opts.reconfirm(e.requiredFee, fee))) throw opts.onReject()
      fee = e.requiredFee
    }
  }
}

/**
 * Parse the node's `estimateFee` (`tx_hex`) result into a {@link DraftFeeQuote},
 * every fee through `BigInt(String(..))` (#2001). Throws when a field is
 * missing (a node that predates the draft quote).
 */
export function parseDraftFeeQuote(raw: unknown): DraftFeeQuote {
  const r = (raw ?? {}) as Record<string, unknown>
  const big = (key: string): bigint => {
    const v = r[key]
    if (typeof v !== 'number' && typeof v !== 'string' && typeof v !== 'bigint') {
      throw new Error(
        `Node fee quote is missing ${key}: the node predates estimateFee tx_hex (#2001)`,
      )
    }
    return BigInt(String(v))
  }
  return {
    minimumFee: big('minimumFee'),
    consensusFeeFloor: big('consensusFeeFloor'),
    relayFeeFloor: big('relayFeeFloor'),
    settleFee: big('settleFee'),
    resignFee: big('resignFee'),
    txFee: r.txFee === undefined ? undefined : big('txFee'),
  }
}

/**
 * Build {@link SendRpc.quoteDraftFee} from a JSON-RPC caller: `estimateFee`
 * with `tx_hex` (#2001).
 */
export function quoteDraftFeeVia(
  call: (method: string, params: Record<string, unknown>) => Promise<unknown>,
): (txHex: string) => Promise<DraftFeeQuote> {
  return async (txHex: string) =>
    parseDraftFeeQuote(await call('estimateFee', { tx_hex: txHex }))
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
    seed: keys.seed ?? '',
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
      // Preserve the hybrid recovery fields (#988) so a later SPEND of this
      // output can recover its one-time key (outputIndex is bound into the
      // 6.0.0 one-time key; the ciphertext is decapsulated at signing time).
      outputIndex: o.outputIndex,
      kemCiphertext: o.kemCiphertext ?? null,
      // A V2 lottery payout recovers through its derivation context (#1645).
      ...(o.lotteryContext ? { lotteryContext: o.lotteryContext } : {}),
    }))
}

/**
 * The cluster tags a selected input passes to the signer, ids as decimal
 * strings, from the row's lossless `clusterTagsDec` (#1687). A row that has
 * tags but only in the lossy numeric `clusterTags` form (a node older than
 * 10.0.0) fails the send closed: rounded ids would name clusters the input
 * does not carry, and dropping the tags makes the spend invalid.
 */
export function inputClusterTags(row: ChainOutput): Array<[string, number]> {
  if (row.clusterTagsDec !== undefined) return row.clusterTagsDec
  if (row.clusterTags === undefined || row.clusterTags.length === 0) return []
  throw new Error(
    'Node did not serve lossless cluster tags (clusterTagsDec) for a tagged input; ' +
      'upgrade the node to protocol 10.0.0',
  )
}

/** Equal when both are absent, or both present with the same derivation. */
function sameLotteryContext(
  a: LotteryContext | null | undefined,
  b: LotteryContext | null | undefined,
): boolean {
  if (a == null || b == null) return a == null && b == null
  return (
    a.version === b.version &&
    a.baseIndex === b.baseIndex &&
    a.tweak.toLowerCase() === b.tweak.toLowerCase()
  )
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
    seed: keys.seed ?? '',
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
    seed: keys.seed ?? '',
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
    seed: keys.seed ?? '',
    outputs: candidates.map((c) => ({
      targetKey: c.targetKey,
      publicKey: c.publicKey,
      amount: c.amount,
      // Preserve the hybrid metadata so 6.0.0 outputs are detected (#988).
      outputIndex: c.outputIndex,
      kemCiphertext: c.kemCiphertext,
      // V2 lottery payouts are discovered through their context (#1645).
      lotteryContext: c.lotteryContext ?? null,
    })),
  })
  if (owned.length === 0) return []

  // Derive key images for the owned outputs and ask the node which are spent.
  const withImages = signer.computeOwnedOutputKeyImages({
    spendPrivateKey: keys.spendPrivateKey,
    viewPrivateKey: keys.viewPrivateKey,
    seed: keys.seed ?? '',
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
  /**
   * Seed fee, in picocredits (>= the network minimum). With
   * {@link SendRpc.quoteDraftFee} the send re-signs at the node's quote
   * until the fee settles, so the paid fee ({@link BuildSendResult.fee}) may
   * be higher (#2001), but never above {@link BuildSendParams.maxFee}.
   */
  fee: bigint
  /**
   * The most fee the user approved, in picocredits (#2001). The sign-quote
   * loop never signs above it: when the node's quote (with headroom) exceeds
   * it, the build throws a {@link FeeAboveApprovedError} carrying the
   * required fee before signing again, so a hostile or buggy node cannot
   * spend the wallet into fees, and inputs are only ever selected for
   * `amount + fee` with `fee <= maxFee`. Defaults to `fee`: a caller that
   * shows `fee` for confirmation pays exactly what it showed.
   */
  maxFee?: bigint
  /**
   * Sweep mode: keep `amount + fee` fixed, so a re-sign at a higher quoted fee
   * lowers the amount sent instead of needing more inputs (#2001). A claim
   * link sweeps its whole balance this way; the paid amount is
   * {@link BuildSendResult.amount}.
   */
  subtractFeeFromAmount?: boolean
  /**
   * Optional BRIDGE DEPOSIT memo (hex-encoded 64 bytes) to embed on the
   * recipient output. This is a DEDICATED channel for a BTH→wBTH bridge deposit
   * ONLY: pass the mint order's `memo` (from the public order API) so the bridge
   * watcher can match the deposit to its order (#1037). It is NOT a free-text
   * note — a present value must be exactly 64 bytes of hex. Omit for an ordinary
   * send (including any human note, which is handled cosmetically upstream and
   * never reaches the signer).
   */
  bridgeDepositMemo?: string
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
  /** The fee the signed tx pays (the settled node quote, #2001). */
  fee: bigint
  /**
   * The amount the recipient output carries: `amount`, or with
   * `subtractFeeFromAmount` the original `amount + fee` less the paid fee.
   */
  amount: bigint
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
  const { keys, recipient, senderKemPublicKey, amount, fee, bridgeDepositMemo, rpc } =
    params
  const spendTotal = amount + fee
  const maxFee = params.maxFee ?? fee

  if (amount <= 0n) throw new Error('Amount must be greater than 0')
  if (fee > maxFee) throw new FeeAboveApprovedError(fee, maxFee)
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
    seed: keys.seed ?? '',
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

  // Resolve each target once in chain order before selecting inputs or decoys.
  // Preserve the complete scan for accounting, including unsupported receipts.
  // Legacy (context-less) lottery payouts alias their source's key, so they are
  // never canonical (#1450). V2 payouts carry a derivation context and their own
  // one-time key, so they are canonical inputs (#1645).
  const canonical = new Map<string, ChainOutput>()
  for (const candidate of candidates) {
    if (!isLegacyLotteryPayout(candidate) && !canonical.has(candidate.targetKey)) {
      canonical.set(candidate.targetKey, candidate)
    }
  }
  const seenInputs = new Set<string>()
  const supported = spendable.filter((input) => {
    const original = canonical.get(input.targetKey)
    if (!original || original.publicKey !== input.publicKey ||
        toBigInt(original.amount) !== toBigInt(input.amount) ||
        (original.outputIndex ?? 0) !== (input.outputIndex ?? 0) ||
        !sameLotteryContext(original.lotteryContext, input.lotteryContext) ||
        seenInputs.has(input.targetKey)) return false
    seenInputs.add(input.targetKey)
    return true
  })
  const signAt = (
    fee: bigint,
  ): { txHex: string; inputs: OwnedOutput[]; inputTotal: bigint; amount: bigint } => {
    const amount = params.subtractFeeFromAmount ? spendTotal - fee : params.amount
    if (amount <= 0n) {
      throw new Error(`The amount does not cover the node's fee floor (fee ${fee} picocredits)`)
    }
    const target = amount + fee
    const inputs = selectInputs(supported, target)
    if (!inputs) {
      if (supported.length !== spendable.length) {
        throw new Error('Legacy lottery payouts cannot be spent independently yet')
      }
      const have = supported.reduce((s, o) => s + toBigInt(o.amount), 0n)
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
    // Lottery payouts (legacy AND V2) stay out of the decoy set for now, matching
    // the native wallet's initial V2 policy (#1642).
    const inputKeys = new Set(inputs.map((o) => o.targetKey))
    const isZeroKey = (k: string) => /^0+$/.test(k)
    const decoyPool = [...canonical.values()].filter(
      (c) =>
        c.lottery !== true &&
        !isV2LotteryPayout(c) &&
        !inputKeys.has(c.targetKey) &&
        !isZeroKey(c.targetKey),
    )
    // Per-input decoy selection (#1693, #1692): confirmed (>= 10 blocks), drawn
    // uniformly with a CSPRNG from one random-offset ±5% age window that holds
    // the real input's age at a uniform position, nearest-age fill when the
    // window is thin — the same policy as the native
    // `GammaDecoySelector::select_decoys_for_input`. Replaces the
    // former chain-ordered `pool[(i*k + j) % len]` window that gave every ring
    // the same oldest outputs. Heights are mandatory: a missing height fails the
    // send closed rather than silently reverting to chain order.
    const spendInputs: SpendInput[] = inputs.map((input) => {
      const original = canonical.get(input.targetKey)
      if (!original) throw new Error('Selected input is missing from the chain scan')
      const realAge = outputAge(original, height)
      const decoys = selectDecoysForInput(decoyPool, decoysPerInput, realAge, height)
      return {
        target_key: input.targetKey,
        public_key: input.publicKey,
        amount: toBigInt(input.amount),
        subaddress_index: toBigInt(input.subaddressIndex),
        // Hybrid one-time-key recovery inputs (#988): the output's tx position
        // (bound into the 6.0.0 one-time key) and its ML-KEM ciphertext, both
        // preserved from the scan. Without these a hybrid input would sign with
        // the wrong one-time key and fail CLSAG verification.
        output_index: input.outputIndex ?? 0,
        kem_ciphertext: input.kemCiphertext ?? null,
        // V2 lottery payout input: the signer recovers x = x_base + T and checks
        // x·G == P before signing (#1645).
        lottery_context: input.lotteryContext ?? null,
        // The input's stored cluster tags, which the signer's outputs inherit
        // (protocol 10.0.0 retained-weight floor, #1687).
        cluster_tags: inputClusterTags(original),
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
      // The wallet seed (when supplied) enables hybrid spend-key recovery for
      // 6.0.0 outputs inside the signer (#988).
      seed: keys.seed ?? '',
      inputs: spendInputs,
      recipient,
      senderKemPublicKey,
      amount,
      fee,
      createdAtHeight: height,
      // Thread the (optional) bridge-deposit memo through to the signer so a
      // bridge deposit carries the order memo the watcher matches on (#1037).
      // Free-text UI notes never reach here — they use a separate cosmetic
      // channel and are not embedded on-chain.
      bridgeDepositMemo,
    }

    // 4. Build + CLSAG-sign inside wasm. The signer self-verifies the produced tx
    // against the node's verifier before returning, so a returned hex is a tx the
    // node should accept (subject to mempool policy like double-spend checks).
    const txHex = signer.buildAndSign(request)

    return { txHex, inputs, inputTotal, amount }
  }

  // 5. Price against the node's own fee floor (#2001): sign, quote the draft,
  // re-sign at the quote until the fee settles. Each pass reselects inputs
  // for the new amount + fee and draws a fresh ring. The node is not trusted
  // with the fee: every pass signs at most `maxFee` (the user-approved fee),
  // so a quote above it stops the loop with a typed error BEFORE signing at
  // it, and inputs are never selected for more than `amount + maxFee`. The
  // paid fee is the fee this wallet signed, never the node's `txFee` echo.
  let passFee = fee
  for (let pass = 0; pass < MAX_FEE_PASSES; pass++) {
    const signed = signAt(passFee)
    if (!rpc.quoteDraftFee) return { ...signed, fee: passFee }
    const quote = await rpc.quoteDraftFee(signed.txHex)
    if (passFee >= quote.settleFee) return { ...signed, fee: passFee }
    const next = quote.resignFee > passFee ? quote.resignFee : passFee + 1n
    if (next > maxFee) throw new FeeAboveApprovedError(next, maxFee)
    passFee = next
  }
  throw new Error(
    `Could not settle a fee meeting the node's fee floor after ${MAX_FEE_PASSES} passes`,
  )
}
