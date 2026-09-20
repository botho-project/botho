/** Tier-2 execution proof. All keys below are public deterministic LOCAL TEST keys. */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import * as anchor from "@coral-xyz/anchor";
import * as squads from "@sqds/multisig";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  ComputeBudgetProgram,
  SYSVAR_RENT_PUBKEY,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountInstruction,
  getAccount,
  getMint,
} from "@solana/spl-token";

const key = (n: number) => Keypair.fromSeed(Buffer.alloc(32, n));
const payer = key(1),
  second = key(2),
  third = key(3),
  outsider = key(4);
const createKey = key(10),
  mint = key(11),
  recipient = key(12);
const programId = new PublicKey("CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE");
const configPda = squads.getProgramConfigPda({})[0];
const treasury = key(20).publicKey;
const fixtureDir = "fixtures/squads-v4";
const [mode, output, endpoint = "http://127.0.0.1:18899"] =
  process.argv.slice(2);
const configData = squads.accounts.ProgramConfig.fromArgs({
  authority: key(21).publicKey,
  multisigCreationFee: 0,
  treasury,
  reserved: Array(64).fill(0),
}).serialize()[0];
assert.equal(configData.length, 144);
assert.equal(configData.subarray(0, 8).toString("hex"), "c4d25ae790958c3f");
if (mode === "genesis") {
  writeFileSync(
    output,
    JSON.stringify({
      pubkey: configPda.toBase58(),
      account: {
        lamports: 10_000_000,
        data: [configData.toString("base64"), "base64"],
        owner: squads.PROGRAM_ID.toBase58(),
        executable: false,
        rentEpoch: 0,
      },
    }),
  );
  console.log(configPda.toBase58());
} else if (mode === "test" || mode === "engine-setup") {
  run()
    .then(() => process.exit(0))
    .catch((error) => {
      console.error(error);
      process.exit(1);
    });
} else {
  throw new Error(
    "usage: squads.ts genesis CONFIG.json | test RESULT.json [loopback-rpc]",
  );
}

async function run() {
  const url = new URL(endpoint);
  assert.ok(
    ["127.0.0.1", "localhost"].includes(url.hostname),
    "local validator only",
  );
  const connection = new Connection(endpoint, "confirmed");
  const provider = new anchor.AnchorProvider(
    connection,
    new anchor.Wallet(payer),
    { commitment: "confirmed" },
  );
  const idl = JSON.parse(readFileSync("target/idl/wbth.json", "utf8"));
  const program = new anchor.Program(idl, programId, provider);
  const v = JSON.parse(readFileSync(`${fixtureDir}/mint-vectors.json`, "utf8"));
  const pub = (s: string) => new PublicKey(s);
  const ix = (raw: any) =>
    new TransactionInstruction({
      programId: pub(raw.programId),
      data: Buffer.from(raw.data, "hex"),
      keys: raw.keys.map((a: any) => ({ ...a, pubkey: pub(a.pubkey) })),
    });
  const wire = (instruction: TransactionInstruction) => ({
    programId: instruction.programId.toBase58(),
    data: instruction.data.toString("hex"),
    keys: instruction.keys.map((a) => ({ ...a, pubkey: a.pubkey.toBase58() })),
  });
  const multisig = squads.getMultisigPda({ createKey: createKey.publicKey })[0];
  const vault = squads.getVaultPda({ multisigPda: multisig, index: 0 })[0];
  assert.equal(v.multisig, multisig.toBase58());
  assert.equal(v.vault, vault.toBase58());
  assert.equal(v.mint, mint.publicKey.toBase58());
  assert.equal(v.recipient, recipient.publicKey.toBase58());
  const bridge = pub(v.bridge),
    ata = pub(v.ata);
  const events: any[] = [];
  let serial = 0;
  async function send(
    label: string,
    instructions: TransactionInstruction[],
    signers: Keypair[] = [],
    failure?: RegExp,
  ) {
    const bh = await connection.getLatestBlockhash();
    const tx = new Transaction({ feePayer: payer.publicKey, ...bh }).add(
      ComputeBudgetProgram.setComputeUnitLimit({ units: 500_000 + ++serial }),
      ...instructions,
    );
    tx.sign(
      ...[
        payer,
        ...signers.filter((s) => !s.publicKey.equals(payer.publicKey)),
      ],
    );
    const signature = await connection.sendRawTransaction(tx.serialize(), {
      skipPreflight: true,
    });
    // web3 can reject confirmation for a landed program error instead of
    // returning status.value.err. Only the confirmed transaction receipt below
    // establishes the expected rejection; a thrown RPC error alone never does.
    const commitment = mode === "engine-setup" ? "finalized" : "confirmed";
    let confirmationError: unknown;
    try {
      await connection.confirmTransaction({ ...bh, signature }, commitment);
    } catch (error) {
      confirmationError = error;
    }
    let receipt = await connection.getTransaction(signature, {
      commitment,
      maxSupportedTransactionVersion: 0,
    });
    for (let i = 0; !receipt && i < 30; i++) {
      await new Promise((r) => setTimeout(r, 100));
      receipt = await connection.getTransaction(signature, {
        commitment,
        maxSupportedTransactionVersion: 0,
      });
    }
    assert.ok(
      receipt?.meta,
      `missing confirmed receipt: ${label}; confirmation=${String(confirmationError)}`,
    );
    assert.equal(receipt.transaction.signatures[0], signature);
    const logs = receipt.meta.logMessages ?? [];
    if (failure) {
      assert.ok(receipt.meta.err, `${label}: unexpectedly succeeded`);
      assert.match(logs.join("\n"), failure, `${label}: wrong failure`);
    } else {
      assert.equal(receipt.meta.err, null, `${label}: ${logs.join("\n")}`);
    }
    events.push({
      label,
      signature,
      slot: receipt.slot,
      error: receipt.meta.err,
      logs,
    });
    console.log(`${failure ? "REJECTED" : "EXECUTED"}: ${label}`);
    return receipt;
  }
  async function balances() {
    return {
      supply: (await getMint(connection, mint.publicKey)).supply.toString(),
      recipient: (await getAccount(connection, ata)).amount.toString(),
    };
  }
  async function rejected(
    label: string,
    instructions: TransactionInstruction[],
    failure: RegExp,
    signers: Keypair[] = [],
  ) {
    const before = await balances();
    await send(label, instructions, signers, failure);
    assert.deepEqual(await balances(), before, `${label}: changed token state`);
  }
  const configInfo = await connection.getAccountInfo(configPda);
  assert.ok(configInfo && configInfo.owner.equals(squads.PROGRAM_ID));
  assert.deepEqual(configInfo.data, configData);
  const config = await squads.accounts.ProgramConfig.fromAccountAddress(
    connection,
    configPda,
  );
  assert.equal(config.multisigCreationFee.toString(), "0");
  assert.ok(config.treasury.equals(treasury));
  assert.ok(config.authority.equals(key(21).publicKey));
  for (const id of [squads.PROGRAM_ID, programId]) {
    assert.equal((await connection.getAccountInfo(id))?.executable, true);
  }
  await send(
    "create immutable 2-of-3 multisig",
    [
      squads.instructions.multisigCreateV2({
        treasury,
        createKey: createKey.publicKey,
        creator: payer.publicKey,
        multisigPda: multisig,
        configAuthority: null,
        threshold: 2,
        timeLock: 0,
        rentCollector: null,
        members: [payer, second, third].map((k) => ({
          key: k.publicKey,
          permissions: squads.types.Permissions.all(),
        })),
      }),
    ],
    [createKey],
  );
  const multisigState = await squads.accounts.Multisig.fromAccountAddress(
    connection,
    multisig,
  );
  assert.equal(multisigState.threshold, 2);
  assert.equal(multisigState.members.length, 3);
  assert.equal(
    multisigState.configAuthority.toBase58(),
    SystemProgram.programId.toBase58(),
  );
  await send("fund vault rent locally", [
    SystemProgram.transfer({
      fromPubkey: payer.publicKey,
      toPubkey: vault,
      lamports: 100_000_000,
    }),
  ]);
  const bump = PublicKey.findProgramAddressSync(
    [Buffer.from("bridge")],
    programId,
  )[1];
  await send(
    "initialize wbth with vault authority",
    [
      await program.methods
        .initialize(bump, vault, third.publicKey, outsider.publicKey)
        .accounts({
          bridge,
          mint: mint.publicKey,
          payer: payer.publicKey,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .instruction(),
    ],
    [mint],
  );
  await send("create destination ATA", [
    createAssociatedTokenAccountInstruction(
      payer.publicKey,
      ata,
      recipient.publicKey,
      mint.publicKey,
    ),
  ]);
  assert.deepEqual(await balances(), { supply: "0", recipient: "0" });
  const bridgeState: any = await program.account.bridge.fetch(bridge);
  assert.ok(bridgeState.mintAuthority.equals(vault));

  if (mode === "engine-setup") {
    await send(
      "fund independent engine member fee accounts",
      [second, third, recipient].map((k) =>
        SystemProgram.transfer({
          fromPubkey: payer.publicKey,
          toPubkey: k.publicKey,
          lamports: 1_000_000_000,
        }),
      ),
    );
    writeFileSync(
      output,
      JSON.stringify({
        genesisHash: await connection.getGenesisHash(),
        solanaVersion: await connection.getVersion(),
        nodeVersion: process.version,
        multisig: multisig.toBase58(),
        vault: vault.toBase58(),
        mint: mint.publicKey.toBase58(),
        recipient: recipient.publicKey.toBase58(),
        ata: ata.toBase58(),
      }),
    );
    console.log("Engine fixture initialized; no mint has occurred");
    return;
  }
  const squadsIdl = JSON.parse(readFileSync(`${fixtureDir}/idl.json`, "utf8"));
  const squadsCoder = new anchor.BorshInstructionCoder(squadsIdl);
  async function create(vector: any) {
    for (const [field, name] of Object.entries({
      create: "vaultTransactionCreate",
      proposal: "proposalCreate",
      approve1: "proposalApprove",
      approve2: "proposalApprove",
      execute: "vaultTransactionExecute",
    })) {
      const instruction = ix(vector[field]);
      const decoded = squadsCoder.decode(instruction.data);
      assert.equal(
        decoded?.name,
        name,
        "discriminator must match pinned official IDL",
      );
      assert.deepEqual(
        squadsCoder.encode(name, decoded!.data),
        instruction.data,
      );
      const definition = squadsIdl.instructions.find(
        (i: any) => i.name === name,
      );
      definition.accounts.forEach((a: any, i: number) => {
        assert.equal(
          instruction.keys[i].isSigner,
          a.isSigner,
          `${name} signer ${a.name}`,
        );
        assert.equal(
          instruction.keys[i].isWritable,
          a.isMut,
          `${name} writable ${a.name}`,
        );
      });
    }
    const index = BigInt(vector.index);
    // Independently compare PDA/account/data encodings against the pinned official SDK.
    assert.deepEqual(
      wire(
        squads.instructions.proposalCreate({
          multisigPda: multisig,
          transactionIndex: index,
          creator: payer.publicKey,
        }),
      ),
      vector.proposal,
    );
    for (const [member, raw] of [
      [payer, vector.approve1],
      [second, vector.approve2],
    ] as const) {
      assert.deepEqual(
        wire(
          squads.instructions.proposalApprove({
            multisigPda: multisig,
            transactionIndex: index,
            member: member.publicKey,
          }),
        ),
        raw,
      );
    }
    const inner = ix(vector.inner);
    const expected = await program.methods
      .bridgeMint(new anchor.BN(5_000_000_000_000), [
        ...Buffer.from(vector.order, "hex"),
      ])
      .accounts({
        bridge,
        orderMarker: inner.keys[1].pubkey,
        mint: mint.publicKey,
        userTokenAccount: ata,
        user: recipient.publicKey,
        mintAuthority: inner.keys[5].pubkey,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
      })
      .instruction();
    assert.deepEqual(
      wire(expected),
      vector.inner,
      "Rust inner instruction differs from built wbth IDL",
    );
    await send(`create proposal ${index} from Rust bytes`, [
      ix(vector.create),
      ix(vector.proposal),
    ]);
    const stored = await squads.accounts.VaultTransaction.fromAccountAddress(
      connection,
      squads.getTransactionPda({ multisigPda: multisig, index })[0],
    );
    assert.equal(stored.index.toString(), index.toString());
    assert.equal(stored.vaultIndex, 0);
    const compiled = stored.message.instructions[0];
    assert.deepEqual(Buffer.from(compiled.data), inner.data);
    assert.equal(
      stored.message.accountKeys[compiled.programIdIndex].toBase58(),
      programId.toBase58(),
    );
    assert.deepEqual(
      Array.from(compiled.accountIndexes, (i: number) =>
        stored.message.accountKeys[i].toBase58(),
      ),
      inner.keys.map((a) => a.pubkey.toBase58()),
    );
    const execute = (
      await squads.instructions.vaultTransactionExecute({
        connection,
        multisigPda: multisig,
        transactionIndex: index,
        member: payer.publicKey,
      })
    ).instruction;
    assert.deepEqual(
      wire(execute),
      vector.execute,
      "Rust execute differs from official SDK reading actual program state",
    );
  }
  const first = v.vectors[0];
  await create(first);
  await rejected(
    "nonmember approval",
    [
      squads.instructions.proposalApprove({
        multisigPda: multisig,
        transactionIndex: 1n,
        member: outsider.publicKey,
      }),
    ],
    /NotAMember/,
    [outsider],
  );
  await send("first distinct approval", [ix(first.approve1)]);
  await rejected(
    "one approval cannot execute",
    [ix(first.execute)],
    /InvalidProposalStatus/,
  );
  await rejected(
    "same member cannot count twice",
    [ix(first.approve1)],
    /AlreadyApproved/,
  );
  await send("second distinct approval", [ix(first.approve2)], [second]);
  const proposal = await squads.accounts.Proposal.fromAccountAddress(
    connection,
    squads.getProposalPda({ multisigPda: multisig, transactionIndex: 1n })[0],
  );
  assert.equal(proposal.approved.length, 2);
  assert.equal(proposal.status.__kind, "Approved");
  const tampered = ix(first.execute);
  tampered.keys[tampered.keys.length - 1] = {
    ...tampered.keys[tampered.keys.length - 1],
    pubkey: outsider.publicKey,
  };
  await rejected(
    "substituted execution account",
    [tampered],
    /InvalidAccount|InvalidTransactionMessage/,
  );
  const vaultBefore = await connection.getBalance(vault);
  const receipt = await send("vault invoke_signed bridge_mint", [
    ix(first.execute),
  ]);
  assert.match(
    receipt.meta!.logMessages!.join("\n"),
    /Instruction: BridgeMint/,
  );
  assert.deepEqual(await balances(), {
    supply: "5000000000000",
    recipient: "5000000000000",
  });
  const marker = await connection.getAccountInfo(
    ix(first.inner).keys[1].pubkey,
  );
  assert.ok(marker && marker.owner.equals(programId));
  const decodedMarker: any = program.coder.accounts.decode(
    "OrderMarker",
    marker.data,
  );
  assert.deepEqual(
    Buffer.from(decodedMarker.orderId),
    Buffer.from(first.order, "hex"),
  );
  const executedProposal = await squads.accounts.Proposal.fromAccountAddress(
    connection,
    squads.getProposalPda({ multisigPda: multisig, transactionIndex: 1n })[0],
  );
  assert.equal(executedProposal.status.__kind, "Executed");

  assert.equal(
    vaultBefore - (await connection.getBalance(vault)),
    marker.lamports,
    "vault must pay marker rent",
  );
  await rejected(
    "same proposal cannot execute twice",
    [ix(first.execute)],
    /InvalidProposalStatus/,
  );
  const duplicate = v.vectors[1];
  await create(duplicate);
  await send(
    "approve duplicate order with quorum",
    [ix(duplicate.approve1), ix(duplicate.approve2)],
    [second],
  );
  await rejected(
    "fresh proposal cannot mint duplicate order",
    [ix(duplicate.execute)],
    /already in use/,
  );
  const wrong = v.vectors[2];
  await create(wrong);
  await send(
    "approve wrong-authority payload",
    [ix(wrong.approve1), ix(wrong.approve2)],
    [second],
  );
  await rejected(
    "quorum cannot bypass wbth authority",
    [ix(wrong.execute)],
    /ConstraintHasOne/,
  );
  assert.equal(
    await connection.getAccountInfo(ix(wrong.inner).keys[1].pubkey),
    null,
    "rejected wrong-authority mint must not leave an order marker",
  );
  assert.deepEqual(await balances(), {
    supply: "5000000000000",
    recipient: "5000000000000",
  });
  const hash = (path: string) =>
    createHash("sha256").update(readFileSync(path)).digest("hex");
  writeFileSync(
    output,
    JSON.stringify(
      {
        result: "PASS",
        endpoint,
        validator: await connection.getVersion(),
        genesisHash: await connection.getGenesisHash(),
        node: process.version,
        squadsProgramId: squads.PROGRAM_ID.toBase58(),
        wbthProgramId: programId.toBase58(),
        hashes: {
          squads: hash(`${fixtureDir}/squads_multisig_program.so`),
          idl: hash(`${fixtureDir}/idl.json`),
          vectors: hash(`${fixtureDir}/mint-vectors.json`),
          wbth: hash("target/deploy/wbth.so"),
          wbthSource: hash("programs/wbth/src/lib.rs"),
          assembler: hash("../../bridge/service/src/mint/squads.rs"),
          harness: hash("localnet/squads.ts"),
          packageLock: hash("package-lock.json"),
        },
        balances: await balances(),
        events,
      },
      null,
      2,
    ) + "\n",
  );
  console.log(
    `PASS: real Squads + wbth CPI, threshold/replay/rent assertions; evidence ${output}`,
  );
}
