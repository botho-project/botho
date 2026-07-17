// Shared Hyperliquid TESTNET helpers for the hl-9..13 scripts (#877):
// exchange-action signing (L1 + user-signed), info queries, and constants.
//
// Signing is a faithful port of hyperliquid-python-sdk's utils/signing.py:
//   actionHash = keccak(msgpack(action) ++ nonce_u64_be ++ 0x00[no vault])
//   phantomAgent = { source: "b" (testnet), connectionId: actionHash }
//   EIP-712 sign over domain {Exchange, version 1, chainId 1337} type Agent.
// The msgpack encoder is vendored below (subset: nil/bool/int/str/array/map,
// map keys in insertion order — matching python msgpack.packb of a dict).
// Verified byte-identical + signature-identical against the python SDK.

import { ethers } from "ethers";
import * as fs from "fs";
import * as path from "path";

export const HL_API = "https://api.hyperliquid-testnet.xyz";
export const IS_MAINNET = false;
export const HYPEREVM_RPC = "https://rpc.hyperliquid-testnet.xyz/evm";

// #876/#1026 deployment (contracts/wbth-ntt/deployment.json)
export const DEPLOYER = "0x111018cfe4523097B7f651f3A06fA9a2956CF155";
export const DEPLOYER_KEYFILE =
  process.env.HL_DEPLOYER_KEYFILE ?? path.resolve(__dirname, "../../../.secrets/bridge-testnet/eth-deployer.key");
export const PEER_TOKEN = "0x230f154Ae33A53dcFFEDedB2d92cc1F32BcE7610"; // HyperEVM wBTH (12 dec)
export const PEER_TOKEN_CREATE_NONCE = 0; // CREATE by 0x111018 at nonce 0 (verified via getCreateAddress)

// HyperCore wBTH spot listing parameters (see hyperliquid-ntt-runbook.md).
// weiDecimals 8 on Core, PeerToken 12 on EVM => evmExtraWeiDecimals 4.
export const CORE_TOKEN_NAME = "WBTH";
export const CORE_SZ_DECIMALS = 2;
export const CORE_WEI_DECIMALS = 8;
export const EVM_EXTRA_WEI_DECIMALS = 4;

export function loadWallet(keyfile: string = DEPLOYER_KEYFILE): ethers.Wallet {
  return new ethers.Wallet(fs.readFileSync(keyfile, "utf8").trim());
}

// System (asset-bridge) address for a Core token index: 0x20 then zeros, index big-endian.
export function systemAddress(tokenIndex: number): string {
  const hex = tokenIndex.toString(16).padStart(38, "0");
  return ethers.getAddress("0x20" + hex);
}

// ---------------------------------------------------------------------------
// minimal msgpack encoder (only the types Hyperliquid actions use)
// ---------------------------------------------------------------------------
function msgpackEncode(v: any, out: number[]): void {
  if (v === null || v === undefined) { out.push(0xc0); return; }
  if (typeof v === "boolean") { out.push(v ? 0xc3 : 0xc2); return; }
  if (typeof v === "number") {
    if (!Number.isSafeInteger(v)) throw new Error(`non-integer number in action: ${v} (use a string)`);
    if (v >= 0) {
      if (v < 0x80) out.push(v);
      else if (v < 0x100) out.push(0xcc, v);
      else if (v < 0x10000) out.push(0xcd, v >>> 8, v & 0xff);
      else if (v < 0x100000000) out.push(0xce, (v / 0x1000000) & 0xff, (v >>> 16) & 0xff, (v >>> 8) & 0xff, v & 0xff);
      else { // uint64
        out.push(0xcf);
        let big = BigInt(v);
        for (let i = 7; i >= 0; i--) out.push(Number((big >> BigInt(8 * i)) & 0xffn));
      }
    } else {
      if (v >= -32) out.push(0x100 + v);
      else if (v >= -128) out.push(0xd0, v & 0xff);
      else if (v >= -32768) out.push(0xd1, (v >> 8) & 0xff, v & 0xff);
      else out.push(0xd2, (v >> 24) & 0xff, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
    }
    return;
  }
  if (typeof v === "string") {
    const b = Buffer.from(v, "utf8");
    if (b.length < 32) out.push(0xa0 | b.length);
    else if (b.length < 0x100) out.push(0xd9, b.length);
    else out.push(0xda, b.length >>> 8, b.length & 0xff);
    for (const x of b) out.push(x);
    return;
  }
  if (Array.isArray(v)) {
    if (v.length < 16) out.push(0x90 | v.length);
    else out.push(0xdc, v.length >>> 8, v.length & 0xff);
    for (const e of v) msgpackEncode(e, out);
    return;
  }
  if (typeof v === "object") {
    const keys = Object.keys(v); // insertion order, matches python dict
    if (keys.length < 16) out.push(0x80 | keys.length);
    else out.push(0xde, keys.length >>> 8, keys.length & 0xff);
    for (const k of keys) { msgpackEncode(k, out); msgpackEncode(v[k], out); }
    return;
  }
  throw new Error(`unsupported msgpack type: ${typeof v}`);
}

export function packAction(action: any): Uint8Array {
  const out: number[] = [];
  msgpackEncode(action, out);
  return Uint8Array.from(out);
}

// ---------------------------------------------------------------------------
// signing
// ---------------------------------------------------------------------------
export function actionHash(action: any, vaultAddress: string | null, nonce: number): string {
  const packed = packAction(action);
  const nonceBuf = Buffer.alloc(8);
  nonceBuf.writeBigUInt64BE(BigInt(nonce));
  const vaultBuf = vaultAddress === null
    ? Buffer.from([0x00])
    : Buffer.concat([Buffer.from([0x01]), Buffer.from(vaultAddress.replace(/^0x/, ""), "hex")]);
  return ethers.keccak256(Buffer.concat([Buffer.from(packed), nonceBuf, vaultBuf]));
}

export async function signL1Action(wallet: ethers.Wallet, action: any, nonce: number, vaultAddress: string | null = null) {
  const hash = actionHash(action, vaultAddress, nonce);
  const phantomAgent = { source: IS_MAINNET ? "a" : "b", connectionId: hash };
  const domain = { chainId: 1337, name: "Exchange", verifyingContract: "0x0000000000000000000000000000000000000000", version: "1" };
  const types = { Agent: [{ name: "source", type: "string" }, { name: "connectionId", type: "bytes32" }] };
  const sig = ethers.Signature.from(await wallet.signTypedData(domain, types, phantomAgent));
  return { r: sig.r, s: sig.s, v: sig.v };
}

// user-signed actions (usdSend, usdClassTransfer): EIP-712 over the named fields.
export async function signUserSignedAction(
  wallet: ethers.Wallet, action: any, payloadTypes: Array<{ name: string; type: string }>, primaryType: string,
) {
  action.signatureChainId = "0x66eee";
  action.hyperliquidChain = IS_MAINNET ? "Mainnet" : "Testnet";
  const domain = {
    name: "HyperliquidSignTransaction", version: "1",
    chainId: parseInt(action.signatureChainId, 16),
    verifyingContract: "0x0000000000000000000000000000000000000000",
  };
  // sign over exactly the declared fields (the posted action carries extras like `type`)
  const message: any = {};
  for (const f of payloadTypes) message[f.name] = action[f.name];
  const sig = ethers.Signature.from(await wallet.signTypedData(domain, { [primaryType]: payloadTypes } as any, message));
  return { r: sig.r, s: sig.s, v: sig.v };
}

export const USD_SEND_SIGN_TYPES = [
  { name: "hyperliquidChain", type: "string" },
  { name: "destination", type: "string" },
  { name: "amount", type: "string" },
  { name: "time", type: "uint64" },
];
export const USD_CLASS_TRANSFER_SIGN_TYPES = [
  { name: "hyperliquidChain", type: "string" },
  { name: "amount", type: "string" },
  { name: "toPerp", type: "bool" },
  { name: "nonce", type: "uint64" },
];

// ---------------------------------------------------------------------------
// API I/O
// ---------------------------------------------------------------------------
export async function info(request: any): Promise<any> {
  const res = await fetch(HL_API + "/info", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(request),
  });
  const text = await res.text();
  try { return JSON.parse(text); } catch { return text; }
}

async function postExchange(payload: any): Promise<any> {
  const res = await fetch(HL_API + "/exchange", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload),
  });
  const text = await res.text();
  let json: any;
  try { json = JSON.parse(text); } catch { throw new Error(`exchange HTTP ${res.status}: ${text.slice(0, 400)}`); }
  if (json?.status === "err") throw new Error(`exchange error: ${json.response}`);
  if (json?.status !== "ok") throw new Error(`exchange unexpected: ${text.slice(0, 400)}`);
  return json;
}

// Send an L1-signed action (order, spotDeploy, requestEvmContract, finalizeEvmContract, ...).
export async function sendL1Action(wallet: ethers.Wallet, action: any): Promise<any> {
  const nonce = Date.now();
  const signature = await signL1Action(wallet, action, nonce);
  return postExchange({ action, nonce, signature, vaultAddress: null });
}

// Send a user-signed action. `action` must already contain its time/nonce field.
export async function sendUserSignedAction(
  wallet: ethers.Wallet, action: any, payloadTypes: Array<{ name: string; type: string }>, primaryType: string, nonce: number,
): Promise<any> {
  const signature = await signUserSignedAction(wallet, action, payloadTypes, primaryType);
  return postExchange({ action, nonce, signature, vaultAddress: null });
}

// convenience wrappers -------------------------------------------------------
export async function usdClassTransfer(wallet: ethers.Wallet, amount: string, toPerp: boolean) {
  const nonce = Date.now();
  const action: any = { type: "usdClassTransfer", amount, toPerp, nonce };
  return sendUserSignedAction(wallet, action, USD_CLASS_TRANSFER_SIGN_TYPES, "HyperliquidTransaction:UsdClassTransfer", nonce);
}

export async function usdSend(wallet: ethers.Wallet, destination: string, amount: string) {
  const time = Date.now();
  const action: any = { type: "usdSend", destination, amount, time };
  return sendUserSignedAction(wallet, action, USD_SEND_SIGN_TYPES, "HyperliquidTransaction:UsdSend", time);
}

export async function spotBalances(user: string): Promise<Map<string, number>> {
  const st = await info({ type: "spotClearinghouseState", user });
  const m = new Map<string, number>();
  for (const b of st.balances ?? []) m.set(b.coin, parseFloat(b.total));
  return m;
}

export async function perpUsdc(user: string): Promise<number> {
  const st = await info({ type: "clearinghouseState", user });
  return parseFloat(st.marginSummary.accountValue);
}

export function fmt(n: number): string { return n.toLocaleString("en-US", { maximumFractionDigits: 8 }); }

// Canonical Hyperliquid wire format for float fields inside L1 actions (px, sz,
// startPx, orderSz, ...): the server re-canonicalizes these strings before
// verifying the signature, so "53.0" MUST be sent as "53" (trailing zeros and
// trailing dot stripped) or the recovered signer is garbage and the exchange
// replies "User or API Wallet 0x... does not exist". Mirrors the python SDK's
// float_to_wire. Integer wei amounts (e.g. genesis balances) are exempt.
export function wire(x: number | string): string {
  const n = typeof x === "number" ? x : parseFloat(x);
  if (!isFinite(n)) throw new Error(`bad wire number: ${x}`);
  let s = n.toFixed(8);
  if (parseFloat(s) !== n) throw new Error(`wire rounding changed value: ${x}`);
  s = s.replace(/0+$/, "").replace(/\.$/, "");
  return s === "-0" ? "0" : s;
}
