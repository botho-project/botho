/**
 * Botho MetaMask Snap — Phase-0 feasibility spike (issue #815).
 *
 * Runs the node-identical Botho transaction pipeline (`bth-wasm-signer`,
 * wasm-pack `--target bundler` output, inlined into the bundle by the
 * snaps-cli wasm loader) inside the MetaMask Snaps execution environment
 * (SES / Hardened JavaScript), and derives the Botho wallet from
 * MetaMask-managed entropy via SIP-6 `snap_getEntropy`.
 *
 * Key derivation (deliverable 2 of the spike):
 *
 *   MetaMask SRP
 *     --SIP-6 snap_getEntropy (deterministic in SRP + snap id, salt "botho-root")-->
 *   32-byte entropy
 *     --BIP39 entropyToMnemonic (english)-->
 *   24-word mnemonic
 *     --BIP39 seed (empty passphrase)-->
 *   64-byte seed
 *     --SLIP-10 ed25519 m/44'/866'/0' + HKDF domain separation-->
 *   Ristretto view/spend keys   (identical to @botho/core `deriveKeypairs`)
 *     --node-identical derive_pq_keys_from_seed (wasm)-->
 *   ML-KEM-768 / ML-DSA-65 keys --> botho://2/ address
 *
 * i.e. the snap plugs MetaMask entropy in as the BIP39 entropy of the
 * EXISTING RootIdentity pipeline — nothing downstream changes, and the same
 * mnemonic imported into the web wallet / node recovers the same wallet.
 *
 * RPC methods (invoked from the snaps-jest harness):
 *   - botho_probe      — environment probe: wasm constants, crypto/RNG endowments
 *   - botho_getAddress — SRP-derived testnet address
 *   - botho_benchSign  — build+CLSAG-sign N transactions (no submit), timed
 *   - botho_send       — build, sign and SUBMIT a real transaction
 *
 * All numeric RPC params/results are strings (JSON-safe; amounts are u64
 * picocredits).
 */

import type { Json, OnRpcRequestHandler, SnapsProvider } from '@metamask/snaps-sdk';
import { MethodNotFoundError } from '@metamask/snaps-sdk';
import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english.js';
import {
  deriveKeypairs,
  deriveDefaultSubaddressPublicKeys,
  parseAddress,
} from '@botho/core';
import {
  buildSendTransaction,
  setSigner,
  type ChainOutput,
  type KeyImageSpentStatus,
  type SendRpc,
  type SignerKeys,
  type WasmSigner,
} from '@botho/wasm-signer';

// The wasm-pack `--target bundler` glue. Its inner `import * as wasm from
// './bth_wasm_signer_bg.wasm'` is handled by the snaps-cli wasm loader
// (`experimental.wasm: true`), which base64-inlines the module and
// instantiates it synchronously at bundle load.
import * as wasm from '@botho/wasm-signer/pkg-bundler/bth_wasm_signer.js';

declare const snap: SnapsProvider;

/* -------------------------------------------------------------------------- */
/* Helpers                                                                    */
/* -------------------------------------------------------------------------- */

const toHex = (b: Uint8Array): string =>
  Array.from(b)
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function leHexToBigInt(hex: string): bigint {
  let result = 0n;
  for (let i = hex.length - 2; i >= 0; i -= 2) {
    result = (result << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16));
  }
  return result;
}

/** `Date.now()` if the SES environment endows a live clock, else 0. */
function now(): number {
  try {
    const t = Date.now();
    return Number.isFinite(t) ? t : 0;
  } catch {
    return 0;
  }
}

/* -------------------------------------------------------------------------- */
/* Instrumented signer                                                        */
/* -------------------------------------------------------------------------- */

interface Timings {
  scanMs: number[];
  keyImagesMs: number[];
  signMs: number[];
}

const timings: Timings = { scanMs: [], keyImagesMs: [], signMs: [] };

function resetTimings(): void {
  timings.scanMs.length = 0;
  timings.keyImagesMs.length = 0;
  timings.signMs.length = 0;
}

/**
 * Wrap the raw wasm exports into the `WasmSigner` interface the shared send
 * orchestrator expects, timing the three heavy wasm calls. Injected via
 * `setSigner` so `buildSendTransaction` (the SAME code the web wallet runs)
 * uses the bundler-target module instead of its browser `fetch` loader.
 */
function makeInstrumentedSigner(): WasmSigner {
  return {
    buildAndSign: (request) => {
      const t0 = now();
      const result = wasm.buildAndSign(request);
      timings.signMs.push(now() - t0);
      return result;
    },
    scanOwnedOutputs: (request) => {
      const t0 = now();
      const result = wasm.scanOwnedOutputs(request) as ReturnType<
        WasmSigner['scanOwnedOutputs']
      >;
      timings.scanMs.push(now() - t0);
      return result;
    },
    computeOwnedOutputKeyImages: (request) => {
      const t0 = now();
      const result = wasm.computeOwnedOutputKeyImages(request) as ReturnType<
        WasmSigner['computeOwnedOutputKeyImages']
      >;
      timings.keyImagesMs.push(now() - t0);
      return result;
    },
    derivePqPublicKeysFromSeed: (seedHex) =>
      wasm.derivePqPublicKeysFromSeed(seedHex) as {
        kemPublicKey: string;
        dsaPublicKey: string;
      },
    deriveAddressFromSeed: (seedHex, viewHex, spendHex, testnet) =>
      wasm.deriveAddressFromSeed(seedHex, viewHex, spendHex, testnet),
    encodeAddress: (viewHex, spendHex, kemHex, dsaHex, testnet) =>
      wasm.encodeAddress(viewHex, spendHex, kemHex, dsaHex, testnet),
    decodeAddress: (address) =>
      wasm.decodeAddress(address) as ReturnType<
        NonNullable<WasmSigner['decodeAddress']>
      >,
    ringSize: () => wasm.ringSize(),
    minFee: () => wasm.minFee(),
  };
}

let signerInjected = false;
function ensureSigner(): void {
  if (!signerInjected) {
    setSigner(makeInstrumentedSigner());
    signerInjected = true;
  }
}

/* -------------------------------------------------------------------------- */
/* Wallet derivation from MetaMask entropy (SIP-6)                            */
/* -------------------------------------------------------------------------- */

interface SnapWallet {
  keys: SignerKeys;
  address: string;
  kemPublicKey: string;
}

/** Derive the full Botho wallet material from a BIP39 mnemonic (the shared
 * tail of the RootIdentity pipeline: seed -> SLIP-10 ed25519 m/44'/866'/0'
 * -> Ristretto keys, plus node-identical PQ keys + v2 address in wasm). */
function walletFromMnemonic(mnemonic: string): SnapWallet {
  const seed = mnemonicToSeedSync(mnemonic, '');
  const kp = deriveKeypairs(mnemonic, 0);
  const sub = deriveDefaultSubaddressPublicKeys(mnemonic, 0);

  const seedHex = toHex(seed);
  const address = wasm.deriveAddressFromSeed(
    seedHex,
    toHex(sub.viewPublic),
    toHex(sub.spendPublic),
    true, // testnet
  );
  const pq = wasm.derivePqPublicKeysFromSeed(seedHex) as {
    kemPublicKey: string;
    dsaPublicKey: string;
  };

  return {
    keys: {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
      seed: seedHex,
    },
    address,
    kemPublicKey: pq.kemPublicKey,
  };
}

let cachedWallet: SnapWallet | null = null;

async function deriveWallet(): Promise<SnapWallet> {
  if (cachedWallet) return cachedWallet;

  // SIP-6 entropy: deterministic in (SRP, snap id, salt). 32 bytes.
  const entropyHex = (await snap.request({
    method: 'snap_getEntropy',
    params: { version: 1, salt: 'botho-root' },
  })) as string;
  const entropy = hexToBytes(entropyHex);
  if (entropy.length !== 32) {
    throw new Error(`snap_getEntropy returned ${entropy.length} bytes, expected 32`);
  }

  // Map the MetaMask entropy in as the BIP39 entropy of the existing Botho
  // RootIdentity pipeline (24-word mnemonic -> 64-byte seed -> SLIP-10
  // ed25519 m/44'/866'/0').
  const mnemonic = entropyToMnemonic(entropy, wordlist);
  cachedWallet = walletFromMnemonic(mnemonic);
  return cachedWallet;
}

/* -------------------------------------------------------------------------- */
/* Node RPC over the network-access endowment                                 */
/* -------------------------------------------------------------------------- */

// The node reports a coinbase output's outputIndex as u32::MAX; its one-time
// key is bound to MINTING_OUTPUT_INDEX = 0 (mirrors the web wallet adapter).
const COINBASE_OUTPUT_INDEX_SENTINEL = 0xffffffff;

function makeJsonRpc(url: string) {
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

interface RawOutput {
  targetKey: string;
  publicKey: string;
  amountCommitment: string;
  outputIndex: number;
  kemCiphertext?: string | null;
}

function makeSendRpc(call: ReturnType<typeof makeJsonRpc>): SendRpc {
  return {
    getChainHeight: async () =>
      (await call<{ chainHeight: number }>('node_getStatus', {})).chainHeight,
    getOutputs: async (start, end) => {
      const blocks = await call<Array<{ outputs: RawOutput[] }>>(
        'chain_getOutputs',
        { start_height: start, end_height: end },
      );
      return blocks.flatMap((b) =>
        b.outputs.map(
          (o): ChainOutput => ({
            targetKey: o.targetKey,
            publicKey: o.publicKey,
            amount: leHexToBigInt(o.amountCommitment),
            outputIndex:
              o.outputIndex === COINBASE_OUTPUT_INDEX_SENTINEL ? 0 : o.outputIndex,
            kemCiphertext: o.kemCiphertext ?? null,
          }),
        ),
      );
    },
    areKeyImagesSpent: (keyImages) =>
      call<KeyImageSpentStatus[]>('chain_areKeyImagesSpent', { keyImages }),
  };
}

/* -------------------------------------------------------------------------- */
/* Build (and optionally submit) a send                                       */
/* -------------------------------------------------------------------------- */

interface SendParams {
  rpcUrl: string;
  recipientAddress: string;
  amountPicocredits: string;
  feePicocredits?: string;
  /**
   * SPIKE-ONLY test hook: sign with an explicitly supplied mnemonic instead
   * of the SRP-derived wallet, so the test harness can fund the snap wallet
   * from the throwaway node wallet THROUGH the same snap pipeline. A real
   * snap must never accept caller-supplied key material.
   */
  senderMnemonic?: string;
}

async function buildOnce(params: SendParams): Promise<{
  txHex: string;
  totalMs: number;
  scanMs: number;
  keyImagesMs: number;
  signMs: number;
}> {
  ensureSigner();
  resetTimings();
  const wallet = params.senderMnemonic
    ? walletFromMnemonic(params.senderMnemonic)
    : await deriveWallet();
  const call = makeJsonRpc(params.rpcUrl);
  const rpc = makeSendRpc(call);

  const parsed = parseAddress(params.recipientAddress);
  const recipient = {
    spend_public_key: toHex(parsed.spendPublic),
    view_public_key: toHex(parsed.viewPublic),
    kem_public_key: toHex(parsed.kemPublic),
  };

  const fee = params.feePicocredits ? BigInt(params.feePicocredits) : wasm.minFee();

  const t0 = now();
  const { txHex } = await buildSendTransaction({
    keys: wallet.keys,
    recipient,
    senderKemPublicKey: wallet.kemPublicKey,
    amount: BigInt(params.amountPicocredits),
    fee,
    rpc,
  });
  const totalMs = now() - t0;

  const sum = (xs: number[]) => xs.reduce((s, x) => s + x, 0);
  return {
    txHex,
    totalMs,
    scanMs: sum(timings.scanMs),
    keyImagesMs: sum(timings.keyImagesMs),
    signMs: sum(timings.signMs),
  };
}

/* -------------------------------------------------------------------------- */
/* RPC handler                                                                */
/* -------------------------------------------------------------------------- */

export const onRpcRequest: OnRpcRequestHandler = async ({ request }) => {
  return handle(request as { method: string; params?: unknown }) as Promise<Json>;
};

async function handle(request: { method: string; params?: unknown }): Promise<unknown> {
  switch (request.method) {
    case 'botho_probe': {
      // Deliverable 4: verify the RNG endowments the `getrandom` js/wasm_js
      // backends resolve against. (The definitive in-wasm proof is that
      // botho_benchSign/botho_send succeed AND produce differing signed
      // transactions for the same wallet state.)
      const a = new Uint8Array(16);
      const b = new Uint8Array(16);
      let randomOk = false;
      let hasGetRandomValues = false;
      try {
        crypto.getRandomValues(a);
        crypto.getRandomValues(b);
        hasGetRandomValues = true;
        randomOk = toHex(a) !== toHex(b) && toHex(a) !== '00'.repeat(16);
      } catch {
        /* left false */
      }
      return {
        wasmLoaded: true,
        ringSize: wasm.ringSize(),
        minFee: wasm.minFee().toString(),
        hasWebAssembly: typeof WebAssembly !== 'undefined',
        hasCrypto: typeof crypto !== 'undefined',
        hasSubtleCrypto: typeof crypto !== 'undefined' && !!crypto.subtle,
        hasGetRandomValues,
        randomOk,
        hasLiveClock: now() > 0,
        hasFetch: typeof fetch === 'function',
      };
    }

    case 'botho_getAddress': {
      const wallet = await deriveWallet();
      return {
        address: wallet.address,
        derivation:
          "SIP-6 snap_getEntropy(salt='botho-root') -> BIP39(24 words) -> " +
          "seed -> SLIP-10 ed25519 m/44'/866'/0' (node-identical RootIdentity pipeline)",
      };
    }

    case 'botho_deriveAddress': {
      // SPIKE-ONLY test helper: derive the testnet address for a supplied
      // mnemonic (used by the harness to compute the node wallet's address
      // without loading wasm on the jest side).
      const { mnemonic } = request.params as unknown as { mnemonic: string };
      return { address: walletFromMnemonic(mnemonic).address };
    }

    case 'botho_benchSign': {
      // Build + CLSAG-sign (ring construction, decoy shuffle, hybrid ML-KEM
      // outputs) N times WITHOUT submitting. Timed per iteration.
      const params = request.params as unknown as SendParams & {
        iterations?: number;
      };
      const iterations = Math.max(1, Math.min(10, params.iterations ?? 3));
      const results: Array<Record<string, number>> = [];
      const txHexes: string[] = [];
      for (let i = 0; i < iterations; i++) {
        const { txHex, ...t } = await buildOnce(params);
        txHexes.push(txHex);
        results.push({ ...t, txBytes: txHex.length / 2 });
      }
      // getrandom-in-wasm proof: identical wallet state must still yield
      // distinct transactions (fresh blinding factors, ring shuffle, KEM
      // encapsulation randomness). All-identical => RNG is broken/zeroed.
      const allIdentical = txHexes.every((h) => h === txHexes[0]);
      return { iterations: results, allIdentical };
    }

    case 'botho_send': {
      const params = request.params as unknown as SendParams;
      const { txHex, ...t } = await buildOnce(params);
      const call = makeJsonRpc(params.rpcUrl);
      const submitT0 = now();
      const { txHash } = await call<{ txHash: string }>('tx_submit', {
        tx_hex: txHex,
      });
      return {
        txHash,
        ...t,
        submitMs: now() - submitT0,
        txBytes: txHex.length / 2,
      };
    }

    default:
      throw new MethodNotFoundError({ method: request.method });
  }
}
