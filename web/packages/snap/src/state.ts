/**
 * Persisted Snap state + windowed/incremental scanning (Phase-2 foundation,
 * issue #1091).
 *
 * The Phase-1 MVP kept **no** persisted state: every `botho_getBalance` /
 * `botho_showBalance` re-scanned the whole chain from genesis
 * (`spendableBalance` -> `getOutputs(0, tip)`), which is O(chain height) per read
 * and forgets everything the moment the Snap execution context ends. This module
 * introduces the Snap's first persisted surface via `snap_manageState` and turns
 * scanning into an incremental, resumable operation.
 *
 * Design (see the issue for the full rationale):
 *
 *  - **Encrypted at rest.** Owned outputs leak amounts and one-time target keys,
 *    so the blob is stored with MetaMask's default `encrypted: true` state (cf.
 *    the web-wallet at-rest lessons #474/#475). Keys never leave the sandbox.
 *
 *  - **Namespaced + versioned.** The top-level blob is `{ version, scan }` so the
 *    sibling Phase-2 consumers can extend it WITHOUT a migration: #1092 (history)
 *    reads `scan.ownedOutputs` (which already carry `blockHeight`/`txHash`), and
 *    #1093 (contacts) adds a `contacts` sibling key. `version` guards a future
 *    breaking schema change.
 *
 *  - **Network-bound + invalidated.** `scan.network` records the node network the
 *    outputs were discovered on. If a later read sees a different reported network
 *    (or a `version` mismatch), the persisted scan is discarded and we rescan from
 *    genesis. This prevents cross-network / stale-schema contamination even for
 *    loopback nodes that the wrong-network guard intentionally exempts.
 *
 *  - **Only immutable receive facts are persisted.** We store the owned outputs
 *    and their receive `blockHeight`/`txHash` — facts that never change once an
 *    output is mined. We deliberately do NOT persist spent/unspent status: an
 *    output can be spent later, so spent status is recomputed LIVE on every
 *    balance read via `chain_areKeyImagesSpent` (the `spendableOwnedOutputs`
 *    split already modelled in `wasm-signer/src/send.ts`).
 *
 *  - **Reorg safety.** betanet can reorg. On resume we conservatively re-scan a
 *    small trailing `REORG_BUFFER` of blocks below the checkpoint so a shallow
 *    reorg near the tip is picked up; merges dedupe by one-time target key so the
 *    overlap never double-counts.
 */

import type {
  ChainOutputWithMeta,
  OwnedOutput,
  SendRpc,
  SignerKeys,
  WasmSigner,
} from '@botho/wasm-signer';
import { spendableOwnedOutputs } from '@botho/wasm-signer';

declare const snap: {
  request(args: { method: string; params?: unknown }): Promise<unknown>;
};

/**
 * Persisted-state schema version. Bump ONLY for a breaking change to the shape
 * below; a mismatch triggers a clean discard + full rescan (never a silent
 * mis-parse of an older blob).
 */
export const STATE_VERSION = 1;

/**
 * How many blocks per `chain_getOutputs` window. The scan walks `(start, tip]` in
 * fixed-size windows rather than one whole-chain fetch, so a resumed scan only
 * pulls the new tail. Large enough that a short chain is a single request.
 */
export const WINDOW_SIZE = 1000;

/**
 * Trailing blocks re-scanned below the checkpoint on resume, to absorb a shallow
 * tip reorg (betanet can reorg). Small: the overlap is re-fetched every read, and
 * merges dedupe by target key so it never double-counts. Chosen over trusting a
 * finalized depth because betanet has no explicit finality signal in the Snap's
 * RPC surface today.
 */
export const REORG_BUFFER = 10;

/**
 * A single owned output persisted across sessions. Only IMMUTABLE receive facts
 * live here — never spent status (recomputed live each read).
 */
export interface PersistedOwnedOutput {
  /** Hex one-time target key (unique per output; the dedupe/merge key). */
  targetKey: string;
  /** Hex ephemeral public key of the output. */
  publicKey: string;
  /** JSON-safe u64 picocredits (bigint serialised as a decimal string). */
  amount: string;
  /**
   * Subaddress index that received the output (0 = default, 1 = change),
   * serialised as a decimal string. Persisted because key-image recovery (hence
   * the live spent-status check) is subaddress-dependent — dropping it would
   * mis-derive change outputs' key images.
   */
  subaddressIndex: string;
  /** Output position within its creating tx — hybrid recovery (#988). */
  outputIndex: number;
  /** Hex ML-KEM-768 ciphertext, or null for a classical output — #988. */
  kemCiphertext: string | null;
  /** Receive block height. Carried so #1092 renders history with no rescan. */
  blockHeight: number;
  /** Creating tx hash (hex). Carried so #1092 renders history with no rescan. */
  txHash: string;
}

/** The incremental-scan checkpoint + discovered owned outputs. */
export interface ScanState {
  /** Node network id these outputs were discovered on (invalidation key). */
  network: string;
  /** Highest block height fully scanned (the resume checkpoint). */
  lastScannedHeight: number;
  /** Every owned output discovered so far (spent AND unspent — filtered live). */
  ownedOutputs: PersistedOwnedOutput[];
}

/**
 * The top-level persisted blob. Namespaced so Phase-2 siblings extend it without
 * a migration:
 *   - `scan`      — this issue (#1091)
 *   - `contacts`  — #1093 (reserved; do NOT implement here)
 *   - `settings`  — in-Snap ingress selection (reserved)
 */
export interface SnapState {
  /** Schema version; see {@link STATE_VERSION}. */
  version: number;
  /** Incremental-scan state (absent until the first balance read). */
  scan?: ScanState;
  // Reserved for sibling consumers (do NOT implement here):
  // contacts?: ContactBook;          // #1093
  // settings?: { rpcUrl?: string };  // in-Snap ingress selection
}

/* ------------------------------------------------------------------ */
/* snap_manageState wrappers (the only impure surface in this module) */
/* ------------------------------------------------------------------ */

/**
 * Read the persisted (encrypted) state blob, or `null` if the Snap has never
 * written one (fresh install / after a clear).
 */
export async function readState(): Promise<SnapState | null> {
  const state = (await snap.request({
    method: 'snap_manageState',
    params: { operation: 'get', encrypted: true },
  })) as SnapState | null;
  return state ?? null;
}

/** Persist the (encrypted) state blob, replacing any previous value. */
export async function writeState(state: SnapState): Promise<void> {
  await snap.request({
    method: 'snap_manageState',
    params: { operation: 'update', newState: state as unknown as Record<string, unknown>, encrypted: true },
  });
}

/* ------------------------------------------------------------------ */
/* Pure helpers (unit-testable without the SES `snap` global)         */
/* ------------------------------------------------------------------ */

/** A fresh, empty scan checkpoint bound to `network` (nothing scanned yet). */
export function emptyScanState(network: string): ScanState {
  return { network, lastScannedHeight: 0, ownedOutputs: [] };
}

/**
 * Return the persisted scan state IFF it is safe to resume from: schema version
 * matches AND it was discovered on the same network. Otherwise `null` — the
 * caller must discard it and rescan from genesis (stale-schema / cross-network
 * invalidation).
 */
export function usableScanState(state: SnapState | null, network: string): ScanState | null {
  if (!state || state.version !== STATE_VERSION) return null;
  if (!state.scan || state.scan.network !== network) return null;
  return state.scan;
}

/**
 * First block height to re-scan on resume: `lastScannedHeight - REORG_BUFFER`,
 * clamped at 0. The buffer re-scans a few trailing blocks so a shallow tip reorg
 * is absorbed; merges dedupe by target key so it never double-counts.
 */
export function scanStartHeight(lastScannedHeight: number): number {
  return Math.max(0, lastScannedHeight - REORG_BUFFER);
}

/**
 * Split the inclusive block range `[start, tip]` into fixed-size windows for
 * `chain_getOutputs`. Returns `[]` when there is nothing to scan (`tip < start`,
 * e.g. a chain that shrank below the checkpoint), so a no-op resume issues no
 * output-window fetches at all.
 */
export function windows(start: number, tip: number, size: number = WINDOW_SIZE): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  if (tip < start) return out;
  for (let lo = start; lo <= tip; lo += size) {
    out.push([lo, Math.min(lo + size - 1, tip)]);
  }
  return out;
}

/** Convert a wasm `OwnedOutput` + its chain meta into a persisted record. */
export function toPersistedOwnedOutput(
  owned: OwnedOutput,
  meta: { blockHeight: number; txHash: string },
): PersistedOwnedOutput {
  return {
    targetKey: owned.targetKey,
    publicKey: owned.publicKey,
    amount: owned.amount.toString(),
    subaddressIndex: owned.subaddressIndex.toString(),
    outputIndex: owned.outputIndex ?? 0,
    kemCiphertext: owned.kemCiphertext ?? null,
    blockHeight: meta.blockHeight,
    txHash: meta.txHash,
  };
}

/** Rehydrate a persisted record into the `OwnedOutput` the signer/spent-check consumes. */
export function toOwnedOutput(p: PersistedOwnedOutput): OwnedOutput {
  return {
    targetKey: p.targetKey,
    publicKey: p.publicKey,
    amount: BigInt(p.amount),
    subaddressIndex: BigInt(p.subaddressIndex),
    outputIndex: p.outputIndex,
    kemCiphertext: p.kemCiphertext,
  };
}

/**
 * Merge newly-discovered owned outputs into the persisted set, deduped by
 * one-time target key (which is globally unique per output). Existing records
 * win, so a re-scanned reorg-buffer block never appends a duplicate. Returns a
 * new array (does not mutate `existing`).
 */
export function mergeOwnedOutputs(
  existing: PersistedOwnedOutput[],
  discovered: PersistedOwnedOutput[],
): PersistedOwnedOutput[] {
  const byKey = new Map<string, PersistedOwnedOutput>();
  for (const o of existing) byKey.set(o.targetKey, o);
  for (const o of discovered) if (!byKey.has(o.targetKey)) byKey.set(o.targetKey, o);
  return Array.from(byKey.values());
}

/* ------------------------------------------------------------------ */
/* Orchestrator (keeps index.ts thin)                                 */
/* ------------------------------------------------------------------ */

/** Inputs to {@link incrementalScanBalance}. */
export interface IncrementalScanArgs {
  /** The injected wasm signer (node-identical ownership + key-image checks). */
  signer: WasmSigner;
  /** The wallet's private keys (stay in the sandbox). */
  keys: SignerKeys;
  /** The connected node's reported network id (the state-binding key). */
  network: string;
  /** The connected node's current chain tip. */
  tip: number;
  /** Windowed meta fetch over `chain_getOutputs` (block height + tx hash). */
  fetchWindow: (lo: number, hi: number) => Promise<ChainOutputWithMeta[]>;
  /** RPC slice for the LIVE spent-status filter (`chain_areKeyImagesSpent`). */
  sendRpc: SendRpc;
}

/**
 * The persisted-state, incremental-scan replacement for `spendableBalance`.
 *
 *   read state -> (in)validate vs network/version -> windowed scan of the
 *   `(checkpoint - reorg buffer, tip]` tail -> merge + persist -> live
 *   spent-status filter over the FULL persisted owned set -> summed balance.
 *
 * The first read on a fresh wallet scans `[0, tip]` and persists a checkpoint; a
 * second read at the same tip fetches only the reorg-buffer window (or nothing)
 * and reuses the persisted outputs, yet returns the same balance — the
 * observable incremental win.
 */
export async function incrementalScanBalance(args: IncrementalScanArgs): Promise<bigint> {
  const { signer, keys, network, tip, fetchWindow, sendRpc } = args;

  const persisted = await readState();
  const usable = usableScanState(persisted, network);
  let ownedOutputs = usable ? usable.ownedOutputs : [];
  const start = usable ? scanStartHeight(usable.lastScannedHeight) : 0;

  for (const [lo, hi] of windows(start, tip)) {
    const raw = await fetchWindow(lo, hi);
    if (raw.length === 0) continue;

    const metaByTargetKey = new Map<string, ChainOutputWithMeta>();
    for (const c of raw) metaByTargetKey.set(c.targetKey, c);

    const discovered = signer.scanOwnedOutputs({
      spendPrivateKey: keys.spendPrivateKey,
      viewPrivateKey: keys.viewPrivateKey,
      seed: keys.seed ?? '',
      outputs: raw.map((c) => ({
        targetKey: c.targetKey,
        publicKey: c.publicKey,
        amount: c.amount,
        outputIndex: c.outputIndex,
        kemCiphertext: c.kemCiphertext,
      })),
    });

    const persistedDiscovered = discovered.map((o) => {
      const meta = metaByTargetKey.get(o.targetKey);
      return toPersistedOwnedOutput(o, {
        blockHeight: meta?.height ?? 0,
        txHash: meta?.txHash ?? o.targetKey,
      });
    });
    ownedOutputs = mergeOwnedOutputs(ownedOutputs, persistedDiscovered);
  }

  await writeState({
    version: STATE_VERSION,
    // Preserve any sibling namespaces a future consumer may have written.
    ...(persisted ?? {}),
    scan: { network, lastScannedHeight: Math.max(tip, 0), ownedOutputs },
  });

  // Spent status is NEVER persisted — recompute it live over the FULL owned set
  // so a previously-counted output that has since been spent drops out.
  const spendable = await spendableOwnedOutputs(
    signer,
    keys,
    ownedOutputs.map(toOwnedOutput),
    sendRpc,
  );
  return spendable.reduce((sum, o) => sum + BigInt(o.amount), 0n);
}
