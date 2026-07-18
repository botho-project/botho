/**
 * MetaMask-SRP -> Botho-key derivation (issue #815, deliverable 2).
 *
 * The Snap derives the Botho wallet from MetaMask-managed entropy through a
 * DEFINED, DOCUMENTED SLIP-10 path, plugging the entropy in as the BIP39 entropy
 * of the EXISTING Botho `RootIdentity` pipeline. Nothing downstream changes, so
 * the same derived mnemonic imported into the web wallet / node / mobile wallet
 * recovers the identical wallet.
 *
 *   MetaMask SRP
 *     -- SIP-6 snap_getEntropy (version 1, salt "botho-root") ----> 32-byte entropy
 *     -- BIP39 entropyToMnemonic (english) ----------------------> 24-word mnemonic
 *     -- BIP39 seed (empty passphrase) --------------------------> 64-byte seed
 *     -- SLIP-10 ed25519 m/44'/866'/0' + HKDF domain separation -> Ristretto view/spend keys
 *     -- node-identical derive_pq_keys_from_seed (wasm) ---------> ML-KEM-768 / ML-DSA-65
 *     -----------------------------------------------------------> tbotho://2/ address
 *
 * SECURITY TRADE-OFF (SRP-derived — chosen — vs Snap-local seed): see README.md.
 * In short: the SRP path gives the user one backup (their MetaMask Secret
 * Recovery Phrase also recovers the Botho wallet) at the cost of coupling the
 * two secrets, and the derived entropy is pinned to this Snap's npm id.
 *
 * Keys NEVER leave the sandbox: private material lives only in `SignerKeys` /
 * the derived mnemonic, which are consumed by the inlined wasm signer and, for
 * the mnemonic, shown only in an explicit user-confirmed backup dialog.
 */

import { entropyToMnemonic, mnemonicToSeedSync } from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english.js';
import {
  deriveKeypairs,
  deriveDefaultSubaddressPublicKeys,
} from '@botho/core';
import type { SignerKeys } from '@botho/wasm-signer';

import { wasm } from './signer';

declare const snap: {
  request(args: { method: string; params?: unknown }): Promise<unknown>;
};

/** The SIP-6 entropy salt that binds this Snap's derived wallet. Changing it
 * (or the Snap's npm id) derives a DIFFERENT wallet — treat as consensus config. */
export const ENTROPY_SALT = 'botho-root';

/** Human-readable description of the derivation path (surfaced by getAddress). */
export const DERIVATION_DESCRIPTION =
  "SIP-6 snap_getEntropy(salt='botho-root') -> BIP39(24 words) -> seed -> " +
  "SLIP-10 ed25519 m/44'/866'/0' (node-identical Botho RootIdentity pipeline)";

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

/** The Snap's fully derived wallet material. */
export interface SnapWallet {
  /** Private signing keys (spend/view/seed, hex). Stay inside the sandbox. */
  keys: SignerKeys;
  /** The wallet's testnet v2 receive address (`tbotho://2/…`). */
  address: string;
  /** Raw ML-KEM-768 public key (hex), for change-output encapsulation. */
  kemPublicKey: string;
}

/**
 * Derive the full Botho wallet material from a BIP39 mnemonic — the shared tail
 * of the RootIdentity pipeline (seed -> SLIP-10 ed25519 m/44'/866'/0' ->
 * Ristretto keys, plus node-identical PQ keys + v2 address in wasm).
 */
export function walletFromMnemonic(mnemonic: string): SnapWallet {
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
let cachedMnemonic: string | null = null;

/** Fetch the SIP-6 entropy and turn it into the 24-word Botho mnemonic. */
async function deriveMnemonic(): Promise<string> {
  if (cachedMnemonic) return cachedMnemonic;
  // SIP-6 entropy: deterministic in (SRP, snap id, salt). 32 bytes.
  const entropyHex = (await snap.request({
    method: 'snap_getEntropy',
    params: { version: 1, salt: ENTROPY_SALT },
  })) as string;
  const entropy = hexToBytes(entropyHex);
  if (entropy.length !== 32) {
    throw new Error(`snap_getEntropy returned ${entropy.length} bytes, expected 32`);
  }
  cachedMnemonic = entropyToMnemonic(entropy, wordlist);
  return cachedMnemonic;
}

/**
 * Derive (and cache) the Snap's SRP-backed Botho wallet. The heavy derivation
 * runs once per Snap execution context.
 */
export async function deriveWallet(): Promise<SnapWallet> {
  if (cachedWallet) return cachedWallet;
  cachedWallet = walletFromMnemonic(await deriveMnemonic());
  return cachedWallet;
}

/**
 * Reveal the derived 24-word Botho mnemonic (recovery / off-MetaMask backup).
 *
 * This is the mitigation for the Snap-id-pinning risk (README.md): the same
 * words recover the wallet in the web/CLI/mobile wallet even if the Snap is
 * republished under a different id. Callers MUST gate this behind an explicit
 * user confirmation dialog — the mnemonic is full spending authority.
 */
export async function revealMnemonic(): Promise<string> {
  return deriveMnemonic();
}
