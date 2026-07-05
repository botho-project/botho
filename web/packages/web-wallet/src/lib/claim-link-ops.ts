/**
 * Claim-link transaction operations (#460).
 *
 * Pure client-side helpers that wire the existing wasm-signer send/scan path to
 * an ephemeral (claim-link) wallet. NOTHING here touches the node beyond the
 * RPCs the normal send/balance path already uses (`chain_getOutputs`,
 * `chain_areKeyImagesSpent`, `tx_submit`). No new RPCs, no consensus change.
 *
 * - `fundEphemeral`: a normal CLSAG send FROM the sender's wallet TO the
 *   ephemeral address (this is just `wallet.send`, factored to the context).
 * - `scanEphemeral`: derive the ephemeral keys, scan its owned outputs, and
 *   spent-filter them — exactly `spendableBalance`, but returning the gross
 *   spendable total so the claim page can subtract the sweep fee.
 * - `sweepEphemeral`: build a CLSAG send FROM the ephemeral keys to a chosen
 *   destination, paying the sweep fee from the funded output.
 */

import { deriveKeypairs, parseAddress } from '@botho/core'
import { buildSendTransaction, spendableBalance, type SendRpc } from '@botho/wasm-signer'
import type { RemoteNodeAdapter } from '@botho/adapters'

/** Signer's MIN_TX_FEE (picocredits) — mirrors wallet.tsx `send()`. */
export const MIN_TX_FEE = 100_000_000n

/**
 * Sweep-fee reserve added when funding a link so the recipient nets the round
 * number. The architect recommends reserving 2x MIN_TX_FEE to absorb a fee
 * bump between create and claim (still negligible).
 */
export const SWEEP_FEE_RESERVE = 2n * MIN_TX_FEE

function toHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return out
}

/** Build the {@link SendRpc} accessor from a connected adapter. */
function rpcFromAdapter(adapter: RemoteNodeAdapter): SendRpc {
  return {
    getChainHeight: () => adapter.getBlockHeight(),
    getOutputs: (start, end) => adapter.getRawOutputs(start, end),
    areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
  }
}

/** Resolve the send fee, clamped to the consensus floor (mirrors send()). */
async function resolveFee(adapter: RemoteNodeAdapter): Promise<bigint> {
  let fee: bigint
  try {
    fee = (await adapter.estimateFee(0)).fee
  } catch {
    fee = 0n
  }
  return fee < MIN_TX_FEE ? MIN_TX_FEE : fee
}

/**
 * Build + submit a CLSAG send from `senderMnemonic` to `recipientAddress`.
 * Shared core for both a normal send and funding a claim link.
 */
export async function buildAndSubmitSend(
  adapter: RemoteNodeAdapter,
  senderMnemonic: string,
  recipientAddress: string,
  amount: bigint,
): Promise<string> {
  if (!adapter.isConnected()) throw new Error('Not connected to a node')
  const kp = deriveKeypairs(senderMnemonic, 0)
  const recipientKeys = parseAddress(recipientAddress)
  const fee = await resolveFee(adapter)

  const { txHex } = await buildSendTransaction({
    keys: {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    },
    recipient: {
      spend_public_key: toHex(recipientKeys.spendPublic),
      view_public_key: toHex(recipientKeys.viewPublic),
    },
    amount,
    fee,
    rpc: rpcFromAdapter(adapter),
  })

  const result = await adapter.submitTransaction(hexToBytes(txHex))
  if (!result.success || !result.txHash) {
    throw new Error(result.error || 'Transaction submission failed')
  }
  return result.txHash
}

/** Result of scanning an ephemeral claim-link wallet. */
export interface EphemeralScan {
  /** Gross spendable picocredits owned by the ephemeral wallet. */
  gross: bigint
  /** Sweep fee that will be charged. */
  fee: bigint
  /** Net picocredits the recipient receives after the sweep fee. */
  net: bigint
}

/**
 * Scan an ephemeral claim-link wallet for its spendable balance.
 *
 * Returns gross/fee/net. `gross === 0n` means either the funding tx has not
 * confirmed yet OR the link was already claimed (the output's key image is
 * spent) — the caller distinguishes those two via UX/state, since both yield an
 * empty spendable set.
 */
export async function scanEphemeral(
  adapter: RemoteNodeAdapter,
  ephMnemonic: string,
): Promise<EphemeralScan> {
  if (!adapter.isConnected()) throw new Error('Not connected to a node')
  const kp = deriveKeypairs(ephMnemonic, 0)
  const gross = await spendableBalance(
    {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    },
    rpcFromAdapter(adapter),
  )
  const fee = await resolveFee(adapter)
  const net = gross > fee ? gross - fee : 0n
  return { gross, fee, net }
}

/**
 * Sweep an ephemeral claim-link wallet's funds to `destinationAddress`.
 *
 * Builds a CLSAG send FROM the ephemeral keys, paying the sweep fee out of the
 * funded output so the recipient nets `gross - fee`. Throws if there is nothing
 * spendable (already claimed / not yet confirmed). Returns the sweep tx hash.
 */
export async function sweepEphemeral(
  adapter: RemoteNodeAdapter,
  ephMnemonic: string,
  destinationAddress: string,
): Promise<{ txHash: string; net: bigint }> {
  if (!adapter.isConnected()) throw new Error('Not connected to a node')

  // Re-scan to get the current gross and fee at sweep time.
  const { gross, fee, net } = await scanEphemeral(adapter, ephMnemonic)
  if (gross === 0n) {
    throw new Error('Nothing to claim — the link is empty, already claimed, or not yet confirmed')
  }
  if (net <= 0n) {
    throw new Error('The funded amount does not cover the sweep fee')
  }

  const kp = deriveKeypairs(ephMnemonic, 0)
  const destKeys = parseAddress(destinationAddress)

  const { txHex } = await buildSendTransaction({
    keys: {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    },
    recipient: {
      spend_public_key: toHex(destKeys.spendPublic),
      view_public_key: toHex(destKeys.viewPublic),
    },
    amount: net,
    fee,
    rpc: rpcFromAdapter(adapter),
  })

  const result = await adapter.submitTransaction(hexToBytes(txHex))
  if (!result.success || !result.txHash) {
    throw new Error(result.error || 'Sweep submission failed')
  }
  return { txHash: result.txHash, net }
}
