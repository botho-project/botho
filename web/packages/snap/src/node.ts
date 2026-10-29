/**
 * Botho node JSON-RPC access over the Snap's `endowment:network-access`
 * (`fetch`) endowment, plus the wallet's node-trust / wrong-network guard.
 *
 * The Snap still needs an ingress node for decoys + submit; it carries over the
 * web wallet's trust model (user-selected endpoint) and its wrong-network guard
 * (cf. #811 / `validateRpcEndpointForNetwork`): before any balance/send the Snap
 * confirms the node reports the EXPECTED network id, so the user cannot silently
 * point a testnet wallet at a wrong-network (e.g. mainnet) node. Loopback hosts
 * are exempt (local dev nodes may report other network names).
 */

import type {
  ChainOutput,
  ChainOutputWithMeta,
  KeyImageSpentStatus,
  LotteryContext,
  SendRpc,
} from '@botho/wasm-signer';

/**
 * The network id a node must report (via `node_getStatus`'s `network` field) to
 * be accepted as this Snap's ingress. The MVP targets testnet, matching the web
 * wallet's `EXPECTED_NETWORK_ID` and the Snap's `tbotho://2/` addresses.
 */
export const EXPECTED_NETWORK_ID = 'botho-testnet';

/** The node reports a coinbase output's index as u32::MAX; its one-time key is
 * bound to MINTING_OUTPUT_INDEX = 0 (mirrors the web wallet adapter, #988). */
const COINBASE_OUTPUT_INDEX_SENTINEL = 0xffffffff;

/** A minimal JSON-RPC caller bound to a single node endpoint. */
export type NodeCall = <T>(method: string, params: Record<string, unknown>) => Promise<T>;

/**
 * Validate a candidate RPC endpoint URL shape. Accepts `https://` anywhere and
 * `http://localhost` / `http://127.0.0.1` for local development. Mirrors the web
 * wallet's `isValidRpcUrl` so an insecure plain-http ingress is rejected.
 */
export function isValidRpcUrl(candidate: string): boolean {
  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    return false;
  }
  if (url.protocol === 'https:') return true;
  if (url.protocol === 'http:') {
    return url.hostname === 'localhost' || url.hostname === '127.0.0.1';
  }
  return false;
}

function isLoopbackHost(endpoint: string): boolean {
  try {
    const host = new URL(endpoint).hostname.toLowerCase();
    return host === 'localhost' || host === '127.0.0.1';
  } catch {
    return false;
  }
}

/**
 * Enforce the wrong-network guard for a resolved node: throw a user-facing error
 * if a non-loopback node reports a network id other than {@link EXPECTED_NETWORK_ID}.
 * Every node must report an identity. Loopback hosts may report a different
 * network name for local development, matching the web wallet's
 * `validateRpcEndpointForNetwork` (#811). Pure and
 * network-free so it is directly unit-testable.
 */
export function assertNetworkAllowed(rpcUrl: string, reportedNetwork: unknown): asserts reportedNetwork is string {
  if (
    typeof reportedNetwork !== 'string' || !reportedNetwork.trim() ||
    reportedNetwork !== reportedNetwork.trim()
  ) {
    throw new Error('This node did not report a valid network identity.');
  }
  if (isLoopbackHost(rpcUrl)) return;
  if (reportedNetwork !== EXPECTED_NETWORK_ID) {
    throw new Error(
      `This node is on a different network (${reportedNetwork}); expected ` +
        `${EXPECTED_NETWORK_ID}. Refusing to use it as an ingress.`,
    );
  }
}

/** The node's `node_getStatus` result fields the Snap relies on. */
export interface NodeStatus {
  chainHeight: number;
  network: string;
  synced?: boolean;
  version?: string;
}

/** Build a JSON-RPC caller for a node endpoint. Throws on RPC-level errors. */
export function makeNodeCall(url: string): NodeCall {
  let id = 1;
  return async function call<T>(
    method: string,
    params: Record<string, unknown>,
  ): Promise<T> {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method, params, id: id++ }),
    });
    if (!res.ok) {
      throw new Error(`node RPC ${method} failed: HTTP ${res.status}`);
    }
    const json = (await res.json()) as { result?: T; error?: { message: string } };
    if (json.error) throw new Error(`${method}: ${json.error.message}`);
    return json.result as T;
  };
}

/**
 * Fetch the node's status AND enforce the wrong-network guard. Returns the
 * status on success; throws a user-facing error if the endpoint is malformed,
 * unreachable, or on a different network than expected.
 */
export async function connectAndGuard(rpcUrl: string): Promise<{ call: NodeCall; status: NodeStatus }> {
  if (!isValidRpcUrl(rpcUrl)) {
    throw new Error('Enter a valid https:// node endpoint.');
  }
  const call = makeNodeCall(rpcUrl);
  let status: NodeStatus;
  try {
    status = await call<NodeStatus>('node_getStatus', {});
  } catch (err) {
    throw new Error(
      `Could not reach the Botho node at ${new URL(rpcUrl).host}: ` +
        `${err instanceof Error ? err.message : String(err)}`,
    );
  }
  assertNetworkAllowed(rpcUrl, status?.network);
  return { call, status };
}

interface RawOutput {
  targetKey: string;
  publicKey: string;
  amountCommitment: string;
  outputIndex: number;
  /** Original derivation index; for a V2 lottery payout, `lotteryContext.baseIndex`. */
  cryptoOutputIndex?: number;
  kemCiphertext?: string | null;
  /** Lottery payout row; legacy (V1) when it has no `lotteryContext` (#1450). */
  lottery?: boolean;
  /** V2 lottery payout derivation context (frozen #1641 contract, #1645). */
  lotteryContext?: LotteryContext | null;
  /** For a V2 payout: the winning source outpoint (`hash:index`). */
  winnerOutpoint?: string;
  /** Stored cluster tags (`[clusterId, weight]` pairs). */
  clusterTags?: Array<[number, number]>;
}

/**
 * The scan/send view of a row shared by both fetchers. The index bound into
 * the one-time key is `cryptoOutputIndex` when served (equal to `outputIndex`
 * for ordinary outputs, 0 for the coinbase, `baseIndex` for a V2 payout), else
 * the normalized `outputIndex` (older nodes). The lottery flag + context are
 * passed through so the shared orchestrator keeps legacy payouts out of inputs
 * and decoys (#1450) and discovers/spends V2 payouts client-side (#1645).
 */
function decodeRow(o: RawOutput): ChainOutput {
  const row: ChainOutput = {
    targetKey: o.targetKey,
    publicKey: o.publicKey,
    amount: leHexToBigInt(o.amountCommitment),
    outputIndex:
      typeof o.cryptoOutputIndex === 'number'
        ? o.cryptoOutputIndex
        : o.outputIndex === COINBASE_OUTPUT_INDEX_SENTINEL
          ? 0
          : o.outputIndex,
    kemCiphertext: o.kemCiphertext ?? null,
  };
  if (o.lottery === true) row.lottery = true;
  if (o.lotteryContext != null) row.lotteryContext = o.lotteryContext;
  if (o.winnerOutpoint !== undefined) row.winnerOutpoint = o.winnerOutpoint;
  if (o.clusterTags !== undefined) row.clusterTags = o.clusterTags;
  return row;
}

/** A `chain_getOutputs` output row that also carries its creating tx hash. */
interface RawOutputWithMeta extends RawOutput {
  txHash: string;
}

/** Decode a little-endian hex `u64` amount commitment into a bigint. */
function leHexToBigInt(hex: string): bigint {
  let result = 0n;
  for (let i = hex.length - 2; i >= 0; i -= 2) {
    result = (result << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16));
  }
  return result;
}

/**
 * Adapt a node caller to the `SendRpc` slice the shared balance/send
 * orchestrators need (`getChainHeight`, `getOutputs`, `areKeyImagesSpent`).
 */
export function makeSendRpc(call: NodeCall): SendRpc {
  return {
    getChainHeight: async () =>
      (await call<{ chainHeight: number }>('node_getStatus', {})).chainHeight,
    getOutputs: async (start, end) => {
      const blocks = await call<Array<{ outputs: RawOutput[] }>>('chain_getOutputs', {
        start_height: start,
        end_height: end,
      });
      return blocks.flatMap((b) => b.outputs.map(decodeRow));
    },
    areKeyImagesSpent: (keyImages) =>
      call<KeyImageSpentStatus[]>('chain_areKeyImagesSpent', { keyImages }),
  };
}

/**
 * Fetch every output in the inclusive block range `[start, end]` WITH the
 * metadata client-side history + the persisted scan-state need: each block's
 * `height` and each output's creating `txHash`. This is the same
 * `chain_getOutputs` RPC as {@link makeSendRpc}'s `getOutputs`, but preserves the
 * per-block height + per-output txHash that the flat `getOutputs` drops. The
 * windowed/incremental scanner (`state.ts`) calls this one window at a time.
 */
export function getOutputsWithMeta(call: NodeCall) {
  return async (start: number, end: number): Promise<ChainOutputWithMeta[]> => {
    const blocks = await call<Array<{ height: number; outputs: RawOutputWithMeta[] }>>(
      'chain_getOutputs',
      { start_height: start, end_height: end },
    );
    return blocks.flatMap((b) =>
      b.outputs.map(
        (o): ChainOutputWithMeta => ({
          ...decodeRow(o),
          txHash: o.txHash,
          height: b.height,
        }),
      ),
    );
  };
}
