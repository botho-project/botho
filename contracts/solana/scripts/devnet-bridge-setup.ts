// Devnet bring-up of the wBTH bridge program (#867): initialize the bridge
// state + wBTH SPL mint, then bridge_mint liquidity to the LP for the Orca
// pool (#870). Raw web3.js instructions with Anchor 8-byte discriminators — no
// IDL / anchor CLI (the anchor toolchain-switching is avoided entirely).
//
// TESTNET CUSTODY NOTE: mainnet requires the three authorities to be DISTINCT
// *multisigs* (Squads PDAs), since bridge_mint takes mint_authority as a
// transaction Signer and an SPL Token multisig cannot sign a generic ix. On
// devnet we use three distinct single-key authorities (role separation
// preserved) so we can iterate — documented in contracts/solana/README.md.
//
// Run: npx ts-node scripts/devnet-bridge-setup.ts   (or via ts-mocha's tsx)

import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, SYSVAR_RENT_PUBKEY, sendAndConfirmTransaction, LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync, createAssociatedTokenAccountIdempotentInstruction,
  getAccount, getMint,
} from "@solana/spl-token";
import { createHash } from "crypto";
import * as fs from "fs";
import * as path from "path";

const RPC = "https://api.devnet.solana.com";
const PROGRAM_ID = new PublicKey("CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE");
const SECRETS = path.resolve(__dirname, "../../../.secrets/bridge-testnet");

// Liquidity to mint for the LP: 100,000 wBTH (12 decimals) = 10^17 base units,
// matching the Ethereum-side bootstrap.
const MINT_AMOUNT = 100_000_000_000_000_000n;
const ORDER_ID = sha256Bytes("wbth-devnet-liquidity-bootstrap-2026-07-16").subarray(0, 32);

function load(name: string): Keypair {
  const raw = JSON.parse(fs.readFileSync(path.join(SECRETS, `${name}.json`), "utf8"));
  return Keypair.fromSecretKey(Uint8Array.from(raw));
}
function sha256Bytes(s: string): Buffer {
  return createHash("sha256").update(s).digest();
}
function discriminator(ixName: string): Buffer {
  return createHash("sha256").update(`global:${ixName}`).digest().subarray(0, 8);
}
function u64le(v: bigint): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(v);
  return b;
}

async function main() {
  const conn = new Connection(RPC, "confirmed");
  const deployer = load("solana-deployer");
  const mint = load("solana-wbth-mint");
  const mintAuth = load("solana-mint-auth");
  const adminAuth = load("solana-admin-auth");
  const pauserAuth = load("solana-pauser-auth");
  const lp = load("solana-lp");

  const [bridgePda, bump] = PublicKey.findProgramAddressSync([Buffer.from("bridge")], PROGRAM_ID);
  console.log("program :", PROGRAM_ID.toBase58());
  console.log("bridge  :", bridgePda.toBase58(), "bump", bump);
  console.log("mint    :", mint.publicKey.toBase58());
  console.log("mintAuth:", mintAuth.publicKey.toBase58());
  console.log("LP      :", lp.publicKey.toBase58());

  // ---- Step 0: fund the mint authority (pays order_marker rent + fees) ----
  const maBal = await conn.getBalance(mintAuth.publicKey);
  if (maBal < 0.05 * LAMPORTS_PER_SOL) {
    console.log("\n[0] Funding mint authority 0.1 SOL");
    const tx = new Transaction().add(SystemProgram.transfer({
      fromPubkey: deployer.publicKey, toPubkey: mintAuth.publicKey,
      lamports: 0.1 * LAMPORTS_PER_SOL,
    }));
    console.log("   ", await sendAndConfirmTransaction(conn, tx, [deployer]));
  } else {
    console.log(`\n[0] mint authority already funded (${maBal / LAMPORTS_PER_SOL} SOL)`);
  }

  // ---- Step 1: initialize (skip if bridge account already exists) ----
  const bridgeInfo = await conn.getAccountInfo(bridgePda);
  if (bridgeInfo) {
    console.log("\n[1] bridge already initialized — skip");
  } else {
    console.log("\n[1] initialize bridge + wBTH mint");
    const data = Buffer.concat([
      discriminator("initialize"),
      Buffer.from([bump]),
      mintAuth.publicKey.toBuffer(),
      adminAuth.publicKey.toBuffer(),
      pauserAuth.publicKey.toBuffer(),
    ]);
    const keys = [
      { pubkey: bridgePda, isSigner: false, isWritable: true },
      { pubkey: mint.publicKey, isSigner: true, isWritable: true },
      { pubkey: deployer.publicKey, isSigner: true, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      { pubkey: SYSVAR_RENT_PUBKEY, isSigner: false, isWritable: false },
    ];
    const ix = new TransactionInstruction({ programId: PROGRAM_ID, keys, data });
    const sig = await sendAndConfirmTransaction(conn, new Transaction().add(ix), [deployer, mint]);
    console.log("   initialize:", sig);
  }

  // Verify bridge state + mint config.
  const mintInfo = await getMint(conn, mint.publicKey);
  console.log(`   mint decimals=${mintInfo.decimals} authority=${mintInfo.mintAuthority?.toBase58()} freeze=${mintInfo.freezeAuthority?.toBase58()} supply=${mintInfo.supply}`);
  if (mintInfo.decimals !== 12) throw new Error("mint decimals != 12");
  if (!mintInfo.mintAuthority?.equals(bridgePda)) throw new Error("mint authority is not the bridge PDA");

  // ---- Step 2: create the LP's ATA ----
  const lpAta = getAssociatedTokenAddressSync(mint.publicKey, lp.publicKey);
  console.log("\n[2] LP ATA:", lpAta.toBase58());
  {
    const ix = createAssociatedTokenAccountIdempotentInstruction(
      deployer.publicKey, lpAta, lp.publicKey, mint.publicKey,
    );
    const sig = await sendAndConfirmTransaction(conn, new Transaction().add(ix), [deployer]);
    console.log("   ata (idempotent):", sig);
  }

  // ---- Step 3: bridge_mint 100,000 wBTH to the LP ATA ----
  const [orderMarker] = PublicKey.findProgramAddressSync(
    [Buffer.from("order"), ORDER_ID], PROGRAM_ID);
  const markerInfo = await conn.getAccountInfo(orderMarker);
  if (markerInfo) {
    console.log("\n[3] order already minted (marker exists) — skip");
  } else {
    console.log("\n[3] bridge_mint 100,000 wBTH to LP");
    const data = Buffer.concat([discriminator("bridge_mint"), u64le(MINT_AMOUNT), Buffer.from(ORDER_ID)]);
    const keys = [
      { pubkey: bridgePda, isSigner: false, isWritable: true },
      { pubkey: orderMarker, isSigner: false, isWritable: true },
      { pubkey: mint.publicKey, isSigner: false, isWritable: true },
      { pubkey: lpAta, isSigner: false, isWritable: true },
      { pubkey: lp.publicKey, isSigner: false, isWritable: false },
      { pubkey: mintAuth.publicKey, isSigner: true, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ];
    const ix = new TransactionInstruction({ programId: PROGRAM_ID, keys, data });
    const sig = await sendAndConfirmTransaction(conn, new Transaction().add(ix), [mintAuth]);
    console.log("   bridge_mint:", sig);
  }

  const lpAcct = await getAccount(conn, lpAta);
  console.log(`\n   LP wBTH balance: ${Number(lpAcct.amount) / 1e12} wBTH (${lpAcct.amount} base)`);
  const supply = (await getMint(conn, mint.publicKey)).supply;
  console.log(`   wBTH totalSupply: ${Number(supply) / 1e12} wBTH`);

  console.log("\n=== BRIDGE INITIALIZED + LIQUIDITY MINTED (devnet) ===");
  console.log("mint:", mint.publicKey.toBase58());
  console.log("explorer:", `https://explorer.solana.com/address/${mint.publicKey.toBase58()}?cluster=devnet`);
}

main().catch((e) => { console.error(e); process.exit(1); });
