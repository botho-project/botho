/**
 * An in-process JSON-RPC server that stands in for a Botho node in the Snap
 * tests. This is the MOCKED ingress the MVP tests drive the Snap against — no
 * live betanet or `botho` binary is required (live-testnet send validation is a
 * follow-up deferred behind betanet resume #1051; see README.md).
 *
 * The Snap runs inside the SES executor in a worker thread and reaches this
 * server over its real `endowment:network-access` `fetch`, so the server binds
 * to `127.0.0.1` and the test passes its `http://127.0.0.1:<port>` URL as the
 * Snap's `rpcUrl` param.
 */

import { createServer, type Server } from 'node:http';
import { AddressInfo } from 'node:net';

/** A single JSON-RPC method handler: receives params, returns a result. */
export type RpcHandler = (params: unknown) => unknown;

export interface MockNodeOptions {
  /** The `network` id reported by `node_getStatus` (default `botho-testnet`). */
  network?: string;
  /** The `chainHeight` reported by `node_getStatus` (default 100). */
  chainHeight?: number;
  /**
   * Outputs returned by `chain_getOutputs`, grouped by block. Default: none
   * (an empty chain — a freshly-derived wallet then has a 0 balance and no
   * spendable outputs, which is the deterministic happy path the MVP tests
   * assert without needing real owned-output fixtures).
   *
   * A block MAY carry a `height`; when present, the mock honours the RPC's
   * `start_height`/`end_height` range (returning only in-range blocks), so the
   * Snap's windowed/incremental scan (#1091) can be exercised against real
   * window boundaries. Blocks without a `height` are always returned (back-compat
   * with the flat MVP fixtures). Each output MAY carry a `txHash` for the
   * meta-carrying fetch history + persisted scan-state rely on.
   */
  outputs?: Array<{ height?: number; outputs: unknown[] }>;
  /** Extra / overriding method handlers (e.g. a custom `tx_submit`). */
  handlers?: Record<string, RpcHandler>;
}

export interface MockNode {
  /** Base URL to pass as the Snap's `rpcUrl`, e.g. `http://127.0.0.1:54321`. */
  url: string;
  /** Every JSON-RPC call the Snap made, in order (method + params). */
  calls: Array<{ method: string; params: unknown }>;
  /** Stop the server. */
  close(): Promise<void>;
}

/** Start a mock node. Remember to `await node.close()` in `afterEach`/`afterAll`. */
export async function startMockNode(options: MockNodeOptions = {}): Promise<MockNode> {
  const network = options.network ?? 'botho-testnet';
  const chainHeight = options.chainHeight ?? 100;
  const outputs = options.outputs ?? [];
  const calls: MockNode['calls'] = [];

  const defaults: Record<string, RpcHandler> = {
    node_getStatus: () => ({ chainHeight, network, synced: true, version: 'mock-1.0.0' }),
    chain_getOutputs: (params) => {
      // Honour the RPC's inclusive [start_height, end_height] window for blocks
      // that declare a height, so the Snap's windowed scan sees real boundaries.
      // Height-less blocks (flat MVP fixtures) are always returned.
      const { start_height, end_height } = (params ?? {}) as {
        start_height?: number;
        end_height?: number;
      };
      if (start_height === undefined || end_height === undefined) return outputs;
      return outputs.filter(
        (b) => b.height === undefined || (b.height >= start_height && b.height <= end_height),
      );
    },
    chain_areKeyImagesSpent: (params) => {
      const keyImages = (params as { keyImages?: string[] })?.keyImages ?? [];
      return keyImages.map((keyImage) => ({
        keyImage,
        spent: false,
        spentHeight: null,
        pending: false,
      }));
    },
    tx_submit: () => ({ txHash: 'mocktxhash' }),
  };
  const handlers = { ...defaults, ...(options.handlers ?? {}) };

  const server: Server = createServer((req, res) => {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      let parsed: { id?: number; method?: string; params?: unknown };
      try {
        parsed = JSON.parse(body || '{}');
      } catch {
        res.writeHead(400).end();
        return;
      }
      const { id, method, params } = parsed;
      calls.push({ method: method ?? '', params });
      const handler = method ? handlers[method] : undefined;
      res.writeHead(200, { 'content-type': 'application/json' });
      if (!handler) {
        res.end(
          JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32601, message: `no mock for ${method}` } }),
        );
        return;
      }
      try {
        res.end(JSON.stringify({ jsonrpc: '2.0', id, result: handler(params) }));
      } catch (err) {
        res.end(
          JSON.stringify({
            jsonrpc: '2.0',
            id,
            error: { code: -32000, message: err instanceof Error ? err.message : String(err) },
          }),
        );
      }
    });
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;

  return {
    url: `http://127.0.0.1:${port}`,
    calls,
    close: () =>
      new Promise<void>((resolve, reject) =>
        server.close((err) => (err ? reject(err) : resolve())),
      ),
  };
}
