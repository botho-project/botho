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

/**
 * The V2 lottery payout derivation context, exactly as `chain_getOutputs`
 * serves it on a lottery payout row (`lotteryContext`, frozen RPC contract of
 * #1641). A V2 payout's one-time key is `P = P_base + T·G`; the signer scans the
 * base target `P − T·G` at `baseIndex` and recovers `x = x_base + T`, checking
 * `x·G == P` (#1645). Pass the RPC object through unchanged.
 *
 * The context is not chain authentication — the wallet trusts its node. A
 * forged context can only cause a missed payout or a spend the ledger rejects;
 * it can never make the signer use a key that does not own the output.
 */
export interface LotteryContext {
  /** Context encoding version; only 2 is accepted. */
  version: number
  /** Original derivation index `j` (equals the row's `cryptoOutputIndex`). */
  baseIndex: number
  /** Hex-encoded 32-byte canonical scalar `T` (the cumulative tweak). */
  tweak: string
  /** Award ordinal within the block's payout manifest. */
  ordinal: number
  /** Height the payout was created at. */
  height: number
}

/** True when a chain row is a V2 lottery payout (it carries a derivation context). */
export function isV2LotteryPayout(row: { lotteryContext?: LotteryContext | null }): boolean {
  return row.lotteryContext != null
}

/**
 * True when a chain row is a LEGACY (V1) lottery payout: flagged `lottery` but
 * with no derivation context. Legacy payouts alias their source's key, so they
 * stay excluded from inputs and decoys (#1450). A V2 chain never emits them.
 */
export function isLegacyLotteryPayout(row: {
  lottery?: boolean
  lotteryContext?: LotteryContext | null
}): boolean {
  return row.lottery === true && row.lotteryContext == null
}

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
   * The output's position within its creating transaction. Bound into the
   * 6.0.0 hybrid one-time key, so it MUST be supplied (from the scan's
   * `outputIndex`) to spend a hybrid output (#988). Optional (defaults to 0)
   * for classical/legacy callers.
   */
  output_index?: number
  /**
   * Hex-encoded ML-KEM-768 ciphertext of the owned output (from the scan's
   * `kemCiphertext`), or null/undefined for a classical KEM-less output.
   * Required to recover a hybrid output's one-time spend key (#988).
   */
  kem_ciphertext?: string | null
  /**
   * V2 lottery payout context (from the scan's `lotteryContext`), or
   * null/undefined for an ordinary output. Required to recover a V2 payout's
   * one-time key; the signer checks `x·G == P` before signing (#1645).
   */
  lottery_context?: LotteryContext | null
  /**
   * Decoys for this input's ring. Must contain at least `ringSize() - 1`
   * distinct outputs.
   */
  decoys: DecoyOutput[]
}

/**
 * A recipient address: the two 32-byte Ristretto stealth keys PLUS the
 * recipient's raw ML-KEM-768 public key (all hex).
 *
 * Under protocol 6.0.0 every send output is a hybrid post-quantum stealth
 * output: the signer encapsulates a shared secret against `kem_public_key` and
 * attaches the resulting 1,088-byte ML-KEM ciphertext (issue #978). Decode a
 * `botho://2/` address with `parseAddress` (`@botho/core`) or `decodeAddress`
 * and pass `kemPublic` here. A missing/malformed key is a hard error — a
 * KEM-less output is rejected by 6.0.0 consensus.
 */
export interface RecipientAddress {
  /** Hex-encoded 32-byte spend public key (`D`). */
  spend_public_key: string
  /** Hex-encoded 32-byte view public key (`C`). */
  view_public_key: string
  /** Hex-encoded raw ML-KEM-768 public key (1184 bytes) from the v2 address. */
  kem_public_key: string
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
  /**
   * Hex-encoded 64-byte BIP39 seed. Stays client-side. Used to derive the
   * wallet's ML-KEM secret so a HYBRID owned input's one-time spend key can be
   * recovered at signing time (#988) — without it a 6.0.0 output cannot be
   * spent. Omit / empty for a classical-only spend.
   */
  seed?: string
  /** Owned outputs being spent (one ring per input). */
  inputs: SpendInput[]
  /** Recipient of the transfer. */
  recipient: RecipientAddress
  /**
   * Hex-encoded raw ML-KEM-768 public key (1184 bytes) of the SENDER's own v2
   * address. The change output is a self-send encapsulated against this key so
   * the sender can later recover its change under the hybrid scheme (#978).
   * Derive it from the wallet seed via `derivePqPublicKeysFromSeed`.
   */
  senderKemPublicKey: string
  /** Amount to send to the recipient, in picocredits. */
  amount: bigint | number
  /** Transaction fee in picocredits. */
  fee: bigint | number
  /** Chain height to stamp the transaction with (replay protection). */
  createdAtHeight: bigint | number
  /**
   * Optional BRIDGE DEPOSIT memo (hex-encoded 64 bytes) embedded on the
   * RECIPIENT output. This is a DEDICATED, typed channel for the bridge deposit
   * hook (#1037) — distinct from any human free-text "note" a wallet UI
   * collects, which must NEVER be routed here. A BTH→wBTH mint deposit carries
   * the order memo (first 16 bytes = the mint-order UUID, from
   * `BridgeOrder::generate_memo`, returned as a 128-char hex string by the
   * public order API) so the bridge watcher can view-key-match the deposit to
   * its order (`bth_scan.rs`). Encrypted to the recipient's view key like any
   * memo. A present value MUST be exactly 64 bytes of hex (the signer hard-errors
   * otherwise). Omit / empty for an ordinary send — no memo, unchanged privacy.
   */
  bridgeDepositMemo?: string
}

/** A chain output (as returned by `chain_getOutputs`) to test for ownership. */
export interface ChainOutput {
  /**
   * Lottery payout row. Without a `lotteryContext` it is a LEGACY (V1) payout
   * envelope: retained in history, excluded from independent inputs and decoys
   * (#1450). With a context it is an independently keyed V2 payout (#1645).
   */
  lottery?: boolean
  /**
   * V2 lottery payout derivation context (`lotteryContext`), or null/undefined
   * for an ordinary row. When present the scan checks the base target
   * `P − T·G` at `baseIndex` instead of the plain check at `P` (#1645).
   */
  lotteryContext?: LotteryContext | null
  /**
   * The output's original derivation index (`cryptoOutputIndex`). For a V2
   * payout this is `lotteryContext.baseIndex`, NOT the payout outpoint index.
   * Informational; adapters already map it into `outputIndex`.
   */
  cryptoOutputIndex?: number
  /** For a V2 payout: the winning source outpoint, `"<hash>:<index>"`. */
  winnerOutpoint?: string
  /**
   * Block height the output was confirmed at (the per-block `height` of
   * `chain_getOutputs`). Optional in the type so scan-only callers keep
   * compiling, but REQUIRED by the send path: ring decoys are drawn from the
   * real input's age band (#1693), and a send whose rows lack heights fails
   * closed rather than falling back to chain-ordered decoys.
   */
  height?: number
  /** The output's stored cluster tags (`[clusterId, weight]` pairs). */
  clusterTags?: Array<[number, number]>
  /** Hex-encoded 32-byte one-time target key of the output. */
  targetKey: string
  /** Hex-encoded 32-byte ephemeral public key of the output. */
  publicKey: string
  /** Amount in picocredits (recovered from the transparent commitment). */
  amount: bigint | number
  /**
   * The output's position within its creating transaction (`outputIndex` from
   * `chain_getOutputs`). Under protocol 6.0.0 this index is bound into the
   * hybrid one-time key, so it must be supplied for hybrid detection to work.
   * Optional (defaults to 0) so legacy callers that only scanned classical
   * outputs keep compiling (#988).
   */
  outputIndex?: number
  /**
   * Hex-encoded ML-KEM-768 ciphertext (`kemCiphertext` from
   * `chain_getOutputs`), or null/undefined for a classical/legacy KEM-less
   * output. When present (and a `seed` is supplied), the scan decapsulates it
   * to detect the hybrid one-time key (#970/#988).
   */
  kemCiphertext?: string | null
}

/** A request to scan candidate outputs for ones owned by the account. */
export interface ScanRequest {
  /** Hex-encoded 32-byte account spend private key. Stays client-side. */
  spendPrivateKey: string
  /** Hex-encoded 32-byte account view private key. Stays client-side. */
  viewPrivateKey: string
  /**
   * Hex-encoded 64-byte BIP39 seed. Stays client-side. Used to derive the
   * wallet's ML-KEM secret (node-identical `derive_pq_keys_from_seed`) so the
   * scan can decapsulate hybrid outputs and detect 6.0.0 incoming payments and
   * change. Omit / empty for a classical-only scan (#988).
   */
  seed?: string
  /** Candidate outputs to test for ownership. */
  outputs: ChainOutput[]
}

/** An output the scan determined belongs to the account. */
export interface OwnedOutput {
  /** Hex-encoded 32-byte one-time target key of the owned output. */
  targetKey: string
  /** Hex-encoded 32-byte ephemeral public key of the owned output. */
  publicKey: string
  /** Amount in picocredits of the owned output. */
  amount: bigint
  /** Subaddress index that received this output (0 = default, 1 = change). */
  subaddressIndex: bigint
  /**
   * The output's position within its creating transaction, carried through from
   * the scan so a later spend recovers the hybrid one-time key. Optional for
   * back-compat (#988).
   */
  outputIndex?: number
  /**
   * The owned output's ML-KEM-768 ciphertext (hex), or null for a classical
   * output — preserved from the scan for hybrid spend-key recovery (#988).
   */
  kemCiphertext?: string | null
  /**
   * The V2 lottery payout context the output was discovered under, or absent
   * for an ordinary output — preserved for key-image and spend recovery (#1645).
   */
  lotteryContext?: LotteryContext | null
}

/**
 * A request to compute key images for owned outputs. The wallet supplies the
 * outputs returned by `scanOwnedOutputs`.
 */
export interface KeyImageRequest {
  /** Hex-encoded 32-byte account spend private key. Stays client-side. */
  spendPrivateKey: string
  /** Hex-encoded 32-byte account view private key. Stays client-side. */
  viewPrivateKey: string
  /**
   * Hex-encoded 64-byte BIP39 seed. Stays client-side. Used to recover a hybrid
   * owned output's one-time spend key (hence its key image). Omit / empty for
   * classical recovery (#988).
   */
  seed?: string
  /** The wallet's owned outputs to derive key images for. */
  outputs: OwnedOutput[]
}

/** An owned output annotated with its derived key image. */
export interface OwnedOutputKeyImage {
  /** Hex-encoded 32-byte one-time target key of the owned output. */
  targetKey: string
  /** Hex-encoded 32-byte ephemeral public key of the owned output. */
  publicKey: string
  /** Amount in picocredits of the owned output. */
  amount: bigint
  /** Subaddress index that received this output (0 = default, 1 = change). */
  subaddressIndex: bigint
  /** The output's position within its creating transaction (#988). */
  outputIndex?: number
  /** The owned output's ML-KEM-768 ciphertext (hex), or null (#988). */
  kemCiphertext?: string | null
  /** The V2 lottery payout context, preserved from the scan (#1645). */
  lotteryContext?: LotteryContext | null
  /**
   * Hex-encoded 32-byte key image. Querying the node's
   * `chain_areKeyImagesSpent` RPC with this value reveals whether the output
   * has already been spent.
   */
  keyImage: string
}

/** A v2 address decoded into its raw hex components (from `decodeAddress`). */
export interface DecodedV2Address {
  /** `"mainnet"` or `"testnet"`. */
  network: string
  /** Hex-encoded 32-byte view public key. */
  viewPublicKey: string
  /** Hex-encoded 32-byte spend public key. */
  spendPublicKey: string
  /** Hex-encoded raw ML-KEM-768 public key (1184 bytes). */
  kemPublicKey: string
  /** Hex-encoded raw ML-DSA-65 public key (1952 bytes). */
  dsaPublicKey: string
}

/** A wallet's post-quantum public keys, derived from its BIP39 seed (hex). */
export interface DerivedPqPublicKeys {
  /** Hex-encoded raw ML-KEM-768 public key (1184 bytes). */
  kemPublicKey: string
  /** Hex-encoded raw ML-DSA-65 public key (1952 bytes). */
  dsaPublicKey: string
}

/** The wasm module's exported surface. */
export interface WasmSigner {
  /**
   * Build and CLSAG-sign a transaction. Returns the hex-encoded bincode bytes
   * of the signed transaction, ready to submit via `tx_submit`.
   * Throws on any failure (bad keys, insufficient decoys, balance mismatch).
   */
  buildAndSign(request: SignRequest): string
  /**
   * Identify which of `request.outputs` belong to the account, using the
   * node-identical stealth-address ownership check. Returns the owned outputs
   * with their recovered subaddress index. The keys never leave the client.
   */
  scanOwnedOutputs(request: ScanRequest): OwnedOutput[]
  /**
   * Compute the key image for each owned output, using the node-identical
   * derivation. Pass the resulting key images to `chain_areKeyImagesSpent` to
   * learn which owned outputs are already spent so they can be excluded from
   * balance and spendable selection. The keys never leave the client.
   */
  computeOwnedOutputKeyImages(request: KeyImageRequest): OwnedOutputKeyImage[]
  /**
   * Derive the account's post-quantum public keys from its 64-byte BIP39 seed
   * (hex), using the node-identical `derive_pq_keys_from_seed`. Returns the raw
   * ML-KEM-768 / ML-DSA-65 public keys (hex). Throws on a malformed seed.
   *
   * Optional in the type only so lightweight test fakes (which stub just the
   * transaction methods) still satisfy `WasmSigner`; the real wasm module
   * loaded by {@link loadSigner} always provides it.
   */
  derivePqPublicKeysFromSeed?(seedHex: string): DerivedPqPublicKeys
  /**
   * Derive the wallet's full v2 address string (`botho://2/…` / `tbotho://2/…`)
   * from its 64-byte BIP39 seed (hex) and its classical default-subaddress
   * view/spend public keys (hex). Combines the node-identical PQ derivation with
   * the shared address codec so the string is byte-identical to the node.
   *
   * Optional in the type only (see {@link derivePqPublicKeysFromSeed}).
   */
  deriveAddressFromSeed?(
    seedHex: string,
    viewHex: string,
    spendHex: string,
    testnet: boolean,
  ): string
  /**
   * Encode a v2 address string from hex key components via the shared codec
   * (`view`, `spend`, raw `kem`, raw `dsa`). Routes through the same Rust the
   * node uses, so it cannot drift from the node/mobile/CLI encoders.
   *
   * Optional in the type only (see {@link derivePqPublicKeysFromSeed}).
   */
  encodeAddress?(
    viewHex: string,
    spendHex: string,
    kemHex: string,
    dsaHex: string,
    testnet: boolean,
  ): string
  /**
   * Decode a `botho://2/…` / `tbotho://2/…` address string into its hex
   * components via the shared codec. Rejects retired v1 / quantum prefixes.
   *
   * Optional in the type only (see {@link derivePqPublicKeysFromSeed}).
   */
  decodeAddress?(address: string): DecodedV2Address
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
      scanOwnedOutputs: (request: unknown) => unknown
      computeOwnedOutputKeyImages: (request: unknown) => unknown
      derivePqPublicKeysFromSeed: (seedHex: string) => unknown
      deriveAddressFromSeed: (
        seedHex: string,
        viewHex: string,
        spendHex: string,
        testnet: boolean,
      ) => string
      encodeAddress: (
        viewHex: string,
        spendHex: string,
        kemHex: string,
        dsaHex: string,
        testnet: boolean,
      ) => string
      decodeAddress: (address: string) => unknown
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
      scanOwnedOutputs: (request: ScanRequest) =>
        mod.scanOwnedOutputs(request) as OwnedOutput[],
      computeOwnedOutputKeyImages: (request: KeyImageRequest) =>
        mod.computeOwnedOutputKeyImages(request) as OwnedOutputKeyImage[],
      derivePqPublicKeysFromSeed: (seedHex: string) =>
        mod.derivePqPublicKeysFromSeed(seedHex) as DerivedPqPublicKeys,
      deriveAddressFromSeed: (
        seedHex: string,
        viewHex: string,
        spendHex: string,
        testnet: boolean,
      ) => mod.deriveAddressFromSeed(seedHex, viewHex, spendHex, testnet),
      encodeAddress: (
        viewHex: string,
        spendHex: string,
        kemHex: string,
        dsaHex: string,
        testnet: boolean,
      ) => mod.encodeAddress(viewHex, spendHex, kemHex, dsaHex, testnet),
      decodeAddress: (address: string) => mod.decodeAddress(address) as DecodedV2Address,
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

/**
 * Inject a pre-initialized {@link WasmSigner}, bypassing {@link loadSigner}'s
 * browser `fetch`-based wasm instantiation.
 *
 * The browser code path ({@link loadSigner}) instantiates the wasm by fetching
 * its URL, which does not work under Node/vitest. Node-backed end-to-end tests
 * load the wasm via `readFileSync` + `default({ module_or_path })` instead, then
 * call this to make the wallet's real high-level orchestration
 * ({@link buildSendTransaction}) use that already-initialized module. This is a
 * test seam only; production always goes through {@link loadSigner}.
 */
export function setSigner(signer: WasmSigner): void {
  cached = Promise.resolve(signer)
}

export { deriveV2Address, decodeV2Address, deriveKemPublicKey, mnemonicToSeedHex } from './address'

export {
  buildSendTransaction,
  buildOwnedHistory,
  netOwnedHistory,
  spendableBalance,
  spendableOwnedOutputs,
  ownedOutputTargetKeys,
  type BuildSendParams,
  type BuildSendResult,
  type ChainOutputWithMeta,
  type HistoryEntry,
  type NettedHistoryEntry,
  type HistoryRpc,
  type KeyImageSpentStatus,
  type SendRpc,
  type SignerKeys,
} from './send'
export {
  AGE_SIMILARITY_SPREAD_BPS,
  MIN_DECOY_AGE_BLOCKS,
  ageSimilarityBand,
  ageSimilarityEnvelope,
  ageSimilarityWindow,
  ageSimilarityWindowWidth,
  drawAgeSimilarityWindow,
  outputAge,
  randomIndex,
  randomInteger,
  selectDecoysForInput,
  type RandomFill,
} from './decoys'
