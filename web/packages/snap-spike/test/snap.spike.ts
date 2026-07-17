/**
 * Phase-0 feasibility spike for issue #815, exercised through the OFFICIAL
 * `@metamask/snaps-jest` harness: the snap bundle (dist/bundle.js, with the
 * `bth-wasm-signer` wasm inlined) runs inside the real MetaMask Snaps
 * execution environment — SES / Hardened JavaScript under
 * `@metamask/snaps-simulation` — NOT in plain Node.
 *
 * Two layers:
 *
 *  1. Environment probes (always run once the snap is built):
 *     - wasm loads + instantiates under SES (`endowment:webassembly`)
 *     - `getrandom` endowments resolve (crypto.getRandomValues)
 *     - MetaMask-entropy -> Botho RootIdentity derivation produces a valid
 *       tbotho://2/ address
 *
 *  2. Live-node end-to-end (gated on BOTHO_SNAP_E2E=1 + a built node):
 *     - spin up a throwaway solo-minting botho node (same harness as the
 *       wasm-signer live tests)
 *     - fund the snap's SRP-derived wallet (through the snap's own pipeline,
 *       using the spike-only `senderMnemonic` hook with the node wallet's
 *       throwaway mnemonic)
 *     - `botho_benchSign`: the SNAP builds + CLSAG-signs transactions against
 *       the live RPC, timed inside the SES sandbox (the send-latency number)
 *     - `botho_send`: the SNAP submits a real transaction; we assert it is
 *       accepted and MINED
 *
 * Run:
 *   pnpm --filter @botho/wasm-signer build:wasm:bundler
 *   cargo build --release --bin botho
 *   BOTHO_SNAP_E2E=1 BOTHO_BIN=/path/to/target/release/botho \
 *     pnpm --filter @botho/snap-spike test:snap
 */

import { afterAll, beforeAll, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import {
  startNodeBackedHarness,
  type NodeHarness,
} from '../../wasm-signer/test/node-harness';

const env = process.env;
const E2E = env.BOTHO_SNAP_E2E === '1';

/** Unwrap a snaps-jest response, failing loudly on a snap-side error. */
function unwrap<T>(response: { response: unknown }): T {
  const res = response.response as { result?: T; error?: { message?: string } };
  if (res.error) {
    throw new Error(`snap returned error: ${JSON.stringify(res.error)}`);
  }
  return res.result as T;
}

/* ------------------------------------------------------------------------ */
/* Layer 1: environment probes (SES + wasm + entropy derivation)            */
/* ------------------------------------------------------------------------ */

describe('snap runtime probes (SES executor via snaps-jest)', () => {
  it('loads bth-wasm-signer wasm under SES and finds live RNG endowments', async () => {
    const { request } = await installSnap();
    const probe = unwrap<{
      wasmLoaded: boolean;
      ringSize: number;
      minFee: string;
      hasWebAssembly: boolean;
      hasCrypto: boolean;
      hasGetRandomValues: boolean;
      randomOk: boolean;
      hasLiveClock: boolean;
      hasFetch: boolean;
    }>(await request({ method: 'botho_probe' }));

    // eslint-disable-next-line no-console
    console.log('[#815] probe:', JSON.stringify(probe));

    // wasm module instantiated inside the SES executor.
    expect(probe.wasmLoaded).toBe(true);
    expect(probe.hasWebAssembly).toBe(true);
    // Node-identical protocol constants round-trip out of the wasm.
    expect(probe.ringSize).toBe(20);
    expect(probe.minFee).toBe('100000000');
    // Deliverable 4: the getrandom js backend's endowment resolves.
    expect(probe.hasCrypto).toBe(true);
    expect(probe.hasGetRandomValues).toBe(true);
    expect(probe.randomOk).toBe(true);
    // Network endowment for talking to a node ingress.
    expect(probe.hasFetch).toBe(true);
  });

  it('derives a valid testnet address from MetaMask entropy (SIP-6)', async () => {
    const { request } = await installSnap();
    const { address, derivation } = unwrap<{ address: string; derivation: string }>(
      await request({ method: 'botho_getAddress' }),
    );

    // eslint-disable-next-line no-console
    console.log('[#815] snap address:', `${address.slice(0, 48)}…`, '|', derivation);

    // A structurally valid v2 testnet address (full parse happens node-side
    // when the address actually receives, in the gated E2E below).
    expect(address.startsWith('tbotho://2/')).toBe(true);
    expect(address.length).toBeGreaterThan(1000); // carries ML-KEM + ML-DSA keys

    // Deterministic: a second install of the same snap under the same
    // (simulated) SRP derives the same wallet.
    const second = await installSnap();
    const again = unwrap<{ address: string }>(
      await second.request({ method: 'botho_getAddress' }),
    );
    expect(again.address).toBe(address);
  });
});

/* ------------------------------------------------------------------------ */
/* Layer 2: live-node end-to-end (gated)                                    */
/* ------------------------------------------------------------------------ */

const maybe = E2E ? describe : describe.skip;

maybe('snap -> live botho node: bench + real send (BOTHO_SNAP_E2E=1)', () => {
  let harness: NodeHarness;
  let rpc: <T>(method: string, params: Record<string, unknown>) => Promise<T>;

  function makeRpc(url: string) {
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
      const json = (await res.json()) as { result?: T; error?: { message: string } };
      if (json.error) throw new Error(`${method}: ${json.error.message}`);
      return json.result as T;
    };
  }

  async function waitMined(txHash: string): Promise<number> {
    for (let i = 0; i < 120; i++) {
      const status = await rpc<{ status: string; blockHeight: number | null }>(
        'tx_get',
        { tx_hash: txHash },
      );
      if (status.status === 'confirmed' && status.blockHeight != null) {
        return status.blockHeight;
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
    throw new Error(`tx ${txHash} not mined within 120s`);
  }

  beforeAll(async () => {
    // Pre-mine enough coinbase outputs for a full CLSAG decoy ring (20) plus
    // funds to move around.
    harness = await startNodeBackedHarness({
      minBlocks: 23,
      binPath: env.BOTHO_BIN,
    });
    rpc = makeRpc(harness.rpcUrl);
  }, 300_000);

  afterAll(async () => {
    if (harness) await harness.stop();
  });

  it('snap builds, signs and submits a REAL testnet send (with latency numbers)', async () => {
    const { request } = await installSnap();

    // --- 1. Snap's SRP-derived address --------------------------------------
    const { address: snapAddress } = unwrap<{ address: string }>(
      await request({ method: 'botho_getAddress' }),
    );
    expect(snapAddress.startsWith('tbotho://2/')).toBe(true);

    // The node wallet's address (send-back target), derived through the same
    // wasm pipeline inside the snap.
    const { address: nodeWalletAddress } = unwrap<{ address: string }>(
      await request({
        method: 'botho_deriveAddress',
        params: { mnemonic: harness.mnemonic },
      }),
    );
    expect(nodeWalletAddress.startsWith('tbotho://2/')).toBe(true);

    // --- 2. Fund the snap wallet from the node's minting wallet -------------
    // (Through the snap's own pipeline, via the spike-only senderMnemonic
    // hook — the jest side never touches key material or wasm.)
    const fund = unwrap<{ txHash: string }>(
      await request({
        method: 'botho_send',
        params: {
          rpcUrl: harness.rpcUrl,
          recipientAddress: snapAddress,
          amountPicocredits: '5000000000000', // 5 BTH
          senderMnemonic: harness.mnemonic,
        },
      }),
    );
    const fundedHeight = await waitMined(fund.txHash);
    // eslint-disable-next-line no-console
    console.log('[#815] snap wallet funded at block', fundedHeight);

    // --- 3. Bench: snap builds + signs (no submit), timed inside SES --------
    const benchT0 = Date.now();
    const bench = unwrap<{
      iterations: Array<{
        totalMs: number;
        scanMs: number;
        keyImagesMs: number;
        signMs: number;
        txBytes: number;
      }>;
      allIdentical: boolean;
    }>(
      await request({
        method: 'botho_benchSign',
        params: {
          rpcUrl: harness.rpcUrl,
          recipientAddress: nodeWalletAddress,
          amountPicocredits: '1000000000000', // 1 BTH
          iterations: 3,
        },
      }),
    );
    const benchWallMs = Date.now() - benchT0;
    // eslint-disable-next-line no-console
    console.log(
      '[#815] BENCH (inside SES executor):',
      JSON.stringify(bench.iterations),
      '| harness wall-clock for 3 iterations:',
      benchWallMs,
      'ms | allIdentical:',
      bench.allIdentical,
    );
    expect(bench.iterations.length).toBe(3);
    // Every iteration produced a signed tx of plausible size.
    for (const iter of bench.iterations) {
      expect(iter.txBytes).toBeGreaterThan(1000);
    }
    // getrandom-in-wasm is LIVE: identical wallet state, distinct signed txs.
    expect(bench.allIdentical).toBe(false);

    // --- 4. The snap submits a REAL send back to the node wallet ------------
    const sendT0 = Date.now();
    const send = unwrap<{
      txHash: string;
      totalMs: number;
      scanMs: number;
      keyImagesMs: number;
      signMs: number;
      submitMs: number;
      txBytes: number;
    }>(
      await request({
        method: 'botho_send',
        params: {
          rpcUrl: harness.rpcUrl,
          recipientAddress: nodeWalletAddress,
          amountPicocredits: '1000000000000', // 1 BTH
        },
      }),
    );
    const sendWallMs = Date.now() - sendT0;
    // eslint-disable-next-line no-console
    console.log(
      '[#815] SEND (inside SES executor):',
      JSON.stringify(send),
      '| harness wall-clock:',
      sendWallMs,
      'ms',
    );
    expect(send.txHash).toBeTruthy();

    // --- 5. The node MINES the snap-built transaction ------------------------
    const minedHeight = await waitMined(send.txHash);
    // eslint-disable-next-line no-console
    console.log('[#815] snap-built tx mined at block', minedHeight);
    expect(minedHeight).toBeGreaterThan(fundedHeight);
  });
});
