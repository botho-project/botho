/**
 * Opt-in acceptance test for a FUNDED production Snap (#1377).
 *
 * This test deliberately uses the ordinary local testnet faucet to fund the
 * address returned by `botho_getAddress`; it does not add a sender-mnemonic,
 * key injection, mocked transaction hash, or spike-only RPC to the Snap. The
 * node is throwaway and the test is excluded from the normal fast suite.
 *
 * Run from web/ after building the node and wasm signer:
 *
 *   BOTHO_SNAP_E2E=1 BOTHO_BIN=../target/release/botho \
 *     pnpm --filter @botho/snap test:snap --runInBand
 */

import { afterAll, beforeAll, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';
import { deriveV2Address } from '@botho/wasm-signer';

import {
  startNodeBackedHarness,
  type NodeHarness,
} from '../../wasm-signer/test/node-harness';

const enabled = process.env.BOTHO_SNAP_E2E === '1';
const maybe = enabled ? describe : describe.skip;

const FAUCET_AMOUNT = 1_000_000_000_000n;
const SEND_AMOUNT = 100_000_000_000n;
const MIN_FEE = 100_000_000n;
type SnapRequest = Awaited<ReturnType<typeof installSnap>>['request'];

function unwrap<T>(response: { response: unknown }): T {
  const result = response.response as { result?: T; error?: { message?: string } };
  if (result.error) {
    throw new Error(`snap returned error: ${JSON.stringify(result.error)}`);
  }
  return result.result as T;
}

function makeRpc(url: string) {
  let id = 1;
  return async function rpc<T>(method: string, params: Record<string, unknown> = {}) {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method, params, id: id++ }),
    });
    const json = (await response.json()) as { result?: T; error?: { message: string } };
    if (json.error) throw new Error(`${method}: ${json.error.message}`);
    return json.result as T;
  };
}

maybe('production Snap -> funded local node acceptance', () => {
  let harness: NodeHarness;
  let rpc: ReturnType<typeof makeRpc>;

  beforeAll(async () => {
    // Twenty decoys plus a few spare coinbases leave enough mature balance for
    // the faucet and both spends while retaining the normal protocol ring size.
    harness = await startNodeBackedHarness({ minBlocks: 23, faucet: true });
    rpc = makeRpc(harness.rpcUrl);
  }, 300_000);

  afterAll(async () => {
    if (harness) await harness.stop();
  });

  async function waitConfirmed(txHash: string): Promise<number> {
    const deadline = Date.now() + 180_000;
    while (Date.now() < deadline) {
      const status = await rpc<{ status: string; blockHeight: number | null }>('tx_get', {
        tx_hash: txHash,
      });
      if (status.status === 'confirmed' && status.blockHeight !== null) {
        return status.blockHeight;
      }
      await new Promise((resolve) => setTimeout(resolve, 1_000));
    }
    throw new Error(`transaction ${txHash} was not confirmed within 180s`);
  }

  async function approvedSend(
    request: SnapRequest,
    rpcUrl: string,
    recipientAddress: string,
  ): Promise<{ txHash: string }> {
    const pending = request({
      method: 'botho_send',
      params: {
        rpcUrl,
        recipientAddress,
        amountPicocredits: SEND_AMOUNT.toString(),
      },
    });
    const ui = await pending.getInterface();
    expect(ui.type).toBe('confirmation');
    await ui.ok();
    return unwrap<{ txHash: string }>(await pending);
  }

  it('funds the production address, confirms two sends, and accounts for change', async () => {
    const { request } = await installSnap();
    const snapAddress = unwrap<{ address: string }>(
      await request({ method: 'botho_getAddress' }),
    ).address;
    expect(snapAddress).toMatch(/^tbotho:\/\/2\//);

    // Fund through the local node's ordinary faucet RPC. The test only passes
    // the public address returned by the production Snap to the node.
    const funding = await rpc<{ txHash: string; amount: string }>('faucet_request', {
      address: snapAddress,
    });
    expect(funding.amount).toBe(FAUCET_AMOUNT.toString());
    const fundedHeight = await waitConfirmed(funding.txHash);

    const funded = unwrap<{ spendablePicocredits: string }>(
      await request({
        method: 'botho_getBalance',
        params: { rpcUrl: harness.rpcUrl },
      }),
    );
    expect(BigInt(funded.spendablePicocredits)).toBe(FAUCET_AMOUNT);

    // Use the node wallet's ordinary v2 address as a distinct destination.
    // This derives only a public recipient address in the test process; the
    // Snap still obtains its own address through its normal API above.
    const destination = await deriveV2Address(harness.mnemonic, 'testnet');
    const destinationBefore = await rpc<{ confirmed: number }>('wallet_getBalance');

    const first = await approvedSend(request, harness.rpcUrl, destination);
    const firstHeight = await waitConfirmed(first.txHash);
    expect(firstHeight).toBeGreaterThan(fundedHeight);

    const afterFirst = unwrap<{ spendablePicocredits: string }>(
      await request({
        method: 'botho_getBalance',
        params: { rpcUrl: harness.rpcUrl },
      }),
    );
    // The exact balance proves that the change output was recovered and the
    // funded input was spent; a mocked tx hash cannot satisfy this assertion.
    expect(BigInt(afterFirst.spendablePicocredits)).toBe(
      FAUCET_AMOUNT - SEND_AMOUNT - MIN_FEE,
    );

    const destinationAfter = await rpc<{ confirmed: number }>('wallet_getBalance');
    expect(BigInt(destinationAfter.confirmed) - BigInt(destinationBefore.confirmed)).toBeGreaterThanOrEqual(
      SEND_AMOUNT,
    );

    // A second spend exercises the recovered change output, proving the first
    // accepted transaction left usable funds in the production Snap wallet.
    const second = await approvedSend(request, harness.rpcUrl, destination);
    const secondHeight = await waitConfirmed(second.txHash);
    expect(secondHeight).toBeGreaterThan(firstHeight);

    const afterSecond = unwrap<{ spendablePicocredits: string }>(
      await request({
        method: 'botho_getBalance',
        params: { rpcUrl: harness.rpcUrl },
      }),
    );
    expect(BigInt(afterSecond.spendablePicocredits)).toBe(
      FAUCET_AMOUNT - 2n * (SEND_AMOUNT + MIN_FEE),
    );
  }, 600_000);
});
