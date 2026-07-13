import { expect } from "chai";
import { ethers } from "hardhat";
import { loadFixture } from "@nomicfoundation/hardhat-toolbox/network-helpers";
import type { Signer } from "ethers";

/**
 * Bridge happy-path integration tests (#828): the full Ethereum leg at the
 * contract level, with the mint authority modeled as a REAL t-of-n
 * signature-verifying multisig (`SafeStub`, Gnosis-Safe-compatible) instead
 * of an EOA.
 *
 * This mirrors exactly what the Rust service submits
 * (`bridge/service/src/mint/ethereum.rs`):
 *
 *   relayer EOA -> SafeStub.execTransaction(
 *       to = WrappedBTH, data = bridgeMint(to, amount, orderId),
 *       signatures = threshold EIP-712 SafeTx owner signatures)
 *
 * followed by the user's `bridgeBurn` redemption. The EIP-712 digest is
 * pinned against the Rust `safe_tx_hash` implementation via a shared
 * cross-language test vector (see bridge/service/src/fork_tests.rs).
 */

const PICO = 10n ** 12n; // 1 BTH = 10^12 picocredits (12 decimals)

/** Deterministic order ids. */
function oid(n: number | string): string {
  return ethers.keccak256(ethers.toUtf8Bytes(`flow-order-${n}`));
}

/** EIP-712 SafeTx type (Gnosis Safe v1.3; matches the Rust sol! struct). */
const SAFE_TX_TYPES = {
  SafeTx: [
    { name: "to", type: "address" },
    { name: "value", type: "uint256" },
    { name: "data", type: "bytes" },
    { name: "operation", type: "uint8" },
    { name: "safeTxGas", type: "uint256" },
    { name: "baseGas", type: "uint256" },
    { name: "gasPrice", type: "uint256" },
    { name: "gasToken", type: "address" },
    { name: "refundReceiver", type: "address" },
    { name: "nonce", type: "uint256" },
  ],
};

/** Zeroed SafeTx fields the bridge always uses (value/gas refund unused). */
function safeTx(to: string, data: string, nonce: bigint) {
  return {
    to,
    value: 0n,
    data,
    operation: 0,
    safeTxGas: 0n,
    baseGas: 0n,
    gasPrice: 0n,
    gasToken: ethers.ZeroAddress,
    refundReceiver: ethers.ZeroAddress,
    nonce,
  };
}

/**
 * Collect owner signatures over the SafeTx digest and concatenate them in
 * ascending owner-address order — byte-identical to the Rust
 * `assemble_safe_signatures`.
 */
async function signSafeTx(
  owners: Signer[],
  safeAddress: string,
  tx: ReturnType<typeof safeTx>
): Promise<string> {
  const domain = { chainId: 31337, verifyingContract: safeAddress };
  const withAddr = await Promise.all(
    owners.map(async (owner) => ({
      address: (await owner.getAddress()).toLowerCase(),
      sig: await owner.signTypedData(domain, SAFE_TX_TYPES, tx),
    }))
  );
  withAddr.sort((a, b) => (a.address < b.address ? -1 : 1));
  return ethers.concat(withAddr.map((o) => o.sig));
}

describe("Bridge flow through the Safe (happy paths, #828)", function () {
  async function deployFixture() {
    const [deployer, admin, pauser, relayer, user, owner1, owner2, owner3] =
      await ethers.getSigners();

    // 2-of-3 validator federation Safe (ADR 0002).
    const SafeStub = await ethers.getContractFactory("SafeStub", deployer);
    const safe = await SafeStub.deploy(
      [owner1.address, owner2.address, owner3.address],
      2
    );
    await safe.waitForDeployment();

    const WrappedBTH = await ethers.getContractFactory("WrappedBTH", deployer);
    const wbth = await WrappedBTH.deploy(
      admin.address,
      await safe.getAddress(), // MINTER_ROLE -> the Safe, never an EOA
      pauser.address
    );
    await wbth.waitForDeployment();

    return { safe, wbth, deployer, admin, pauser, relayer, user, owner1, owner2, owner3 };
  }

  /** Build the bridgeMint calldata the Rust EthMinter encodes. */
  async function mintCalldata(
    wbth: Awaited<ReturnType<typeof deployFixture>>["wbth"],
    to: string,
    amount: bigint,
    orderId: string
  ): Promise<string> {
    return wbth.interface.encodeFunctionData("bridgeMint", [to, amount, orderId]);
  }

  describe("cross-language EIP-712 digest pin", function () {
    // Shared vector with bridge/service/src/fork_tests.rs
    // (test_safe_tx_digest_cross_language_vector). If either side changes,
    // Rust-signed attestations stop verifying on-chain — this pin makes
    // that drift a red test instead of a production incident.
    it("matches the Rust safe_tx_hash vector", async function () {
      const iface = new ethers.Interface([
        "function bridgeMint(address to, uint256 amount, bytes32 orderId)",
      ]);
      const data = iface.encodeFunctionData("bridgeMint", [
        "0x1111111111111111111111111111111111111111",
        5n * PICO,
        "0x2222222222222222222222222222222222222222222222222222222222222222",
      ]);
      const digest = ethers.TypedDataEncoder.hash(
        {
          chainId: 31337,
          verifyingContract: "0x0000000000000000000000000000000000005afe",
        },
        SAFE_TX_TYPES,
        safeTx("0x00000000000000000000000000000000000b0170", data, 7n)
      );
      expect(digest).to.equal(
        "0x5e70bedc7f0afce2208fd231d402628090aa65b017c3b0bd9d5aa0382197c4c3"
      );
    });

    it("matches SafeStub.getTransactionHash on-chain", async function () {
      const { safe, wbth, user } = await loadFixture(deployFixture);
      const data = await mintCalldata(wbth, user.address, 5n * PICO, oid("pin"));
      const tx = safeTx(await wbth.getAddress(), data, 3n);

      const onChain = await safe.getTransactionHash(
        tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
        tx.gasPrice, tx.gasToken, tx.refundReceiver, tx.nonce
      );
      const offChain = ethers.TypedDataEncoder.hash(
        { chainId: 31337, verifyingContract: await safe.getAddress() },
        SAFE_TX_TYPES,
        tx
      );
      expect(onChain).to.equal(offChain);
    });
  });

  describe("BTH -> wBTH: threshold mint through the Safe", function () {
    it("mints with 2-of-3 owner signatures submitted by a plain relayer", async function () {
      const { safe, wbth, relayer, user, owner1, owner2 } =
        await loadFixture(deployFixture);
      const amount = 100n * PICO;
      const orderId = oid(1);

      const data = await mintCalldata(wbth, user.address, amount, orderId);
      const tx = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const signatures = await signSafeTx(
        [owner1, owner2],
        await safe.getAddress(),
        tx
      );

      // The relayer holds NO role anywhere — it only pays gas.
      await expect(
        safe.connect(relayer).execTransaction(
          tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
          tx.gasPrice, tx.gasToken, tx.refundReceiver, signatures
        )
      )
        .to.emit(wbth, "BridgeMint")
        .withArgs(user.address, amount, orderId)
        .and.to.emit(safe, "ExecutionSuccess");

      expect(await wbth.balanceOf(user.address)).to.equal(amount);
      expect(await wbth.totalSupply()).to.equal(amount);
      expect(await wbth.processedOrders(orderId)).to.equal(true);
      expect(await safe.nonce()).to.equal(1n);
    });

    it("rejects below-threshold and non-owner signatures", async function () {
      const { safe, wbth, relayer, user, owner1 } =
        await loadFixture(deployFixture);
      const data = await mintCalldata(wbth, user.address, PICO, oid(2));
      const tx = safeTx(await wbth.getAddress(), data, await safe.nonce());

      // 1-of-3 is below threshold.
      const oneSig = await signSafeTx([owner1], await safe.getAddress(), tx);
      await expect(
        safe.connect(relayer).execTransaction(
          tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
          tx.gasPrice, tx.gasToken, tx.refundReceiver, oneSig
        )
      ).to.be.revertedWith("SafeStub: signatures too short");

      // Two signatures, but one from a non-owner.
      const outsider = ethers.Wallet.createRandom().connect(ethers.provider);
      const mixed = await signSafeTx(
        [owner1, outsider],
        await safe.getAddress(),
        tx
      );
      await expect(
        safe.connect(relayer).execTransaction(
          tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
          tx.gasPrice, tx.gasToken, tx.refundReceiver, mixed
        )
      ).to.be.reverted;

      expect(await wbth.totalSupply()).to.equal(0n);
    });

    it("consumes the Safe nonce: a replayed execTransaction fails", async function () {
      const { safe, wbth, relayer, user, owner1, owner2 } =
        await loadFixture(deployFixture);
      const data = await mintCalldata(wbth, user.address, PICO, oid(3));
      const tx = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const signatures = await signSafeTx(
        [owner1, owner2],
        await safe.getAddress(),
        tx
      );

      await safe.connect(relayer).execTransaction(
        tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
        tx.gasPrice, tx.gasToken, tx.refundReceiver, signatures
      );

      // Same signatures again: the Safe nonce moved, so the digest no
      // longer matches and recovery yields non-owners.
      await expect(
        safe.connect(relayer).execTransaction(
          tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
          tx.gasPrice, tx.gasToken, tx.refundReceiver, signatures
        )
      ).to.be.reverted;

      expect(await wbth.balanceOf(user.address)).to.equal(PICO);
    });

    it("duplicate order id at a fresh nonce: ExecutionFailure, no mint", async function () {
      const { safe, wbth, relayer, user, owner1, owner2 } =
        await loadFixture(deployFixture);
      const orderId = oid(4);
      const data = await mintCalldata(wbth, user.address, PICO, orderId);

      // First mint succeeds.
      const tx1 = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const sigs1 = await signSafeTx([owner1, owner2], await safe.getAddress(), tx1);
      await safe.connect(relayer).execTransaction(
        tx1.to, tx1.value, tx1.data, tx1.operation, tx1.safeTxGas, tx1.baseGas,
        tx1.gasPrice, tx1.gasToken, tx1.refundReceiver, sigs1
      );

      // A re-authorized submission of the SAME order id at the new nonce:
      // the Safe swallows the inner revert (no outer revert!) and emits
      // ExecutionFailure with no BridgeMint — exactly the case
      // EthMinter::check_confirmation refuses to treat as confirmed.
      const tx2 = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const sigs2 = await signSafeTx([owner1, owner2], await safe.getAddress(), tx2);
      await expect(
        safe.connect(relayer).execTransaction(
          tx2.to, tx2.value, tx2.data, tx2.operation, tx2.safeTxGas, tx2.baseGas,
          tx2.gasPrice, tx2.gasToken, tx2.refundReceiver, sigs2
        )
      )
        .to.emit(safe, "ExecutionFailure")
        .and.not.to.emit(wbth, "BridgeMint");

      // Exactly one mint happened; the idempotency guard held.
      expect(await wbth.balanceOf(user.address)).to.equal(PICO);
      expect(await wbth.totalSupply()).to.equal(PICO);
    });
  });

  describe("full round trip: mint -> hold -> burn", function () {
    it("wBTH -> BTH redemption restores the supply invariant exactly", async function () {
      const { safe, wbth, relayer, user, owner1, owner2 } =
        await loadFixture(deployFixture);
      const amount = 250n * PICO;
      const orderId = oid("round-trip");

      // Leg 1: threshold mint (as if a BTH deposit had confirmed).
      const data = await mintCalldata(wbth, user.address, amount, orderId);
      const tx = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const signatures = await signSafeTx(
        [owner1, owner2],
        await safe.getAddress(),
        tx
      );
      await safe.connect(relayer).execTransaction(
        tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
        tx.gasPrice, tx.gasToken, tx.refundReceiver, signatures
      );
      expect(await wbth.balanceOf(user.address)).to.equal(amount);

      // Leg 2: user redeems. The declared destination is re-shielded by
      // the bridge to a FRESH one-time stealth address per ADR 0004 — the
      // contract only transports the string.
      const bthDest = "bth_stealth_destination_address_for_redeem";
      await expect(wbth.connect(user).bridgeBurn(amount, bthDest))
        .to.emit(wbth, "BridgeBurn")
        .withArgs(user.address, amount, bthDest);

      // Peg accounting (ADR 0003, factor-1 exact): supply returns to zero,
      // 1 base unit == 1 picocredit throughout, no scaling anywhere.
      expect(await wbth.totalSupply()).to.equal(0n);
      expect(await wbth.balanceOf(user.address)).to.equal(0n);
    });

    it("supports partial redemptions", async function () {
      const { safe, wbth, relayer, user, owner1, owner2 } =
        await loadFixture(deployFixture);
      const amount = 100n * PICO;

      const data = await mintCalldata(wbth, user.address, amount, oid("partial"));
      const tx = safeTx(await wbth.getAddress(), data, await safe.nonce());
      const signatures = await signSafeTx(
        [owner1, owner2],
        await safe.getAddress(),
        tx
      );
      await safe.connect(relayer).execTransaction(
        tx.to, tx.value, tx.data, tx.operation, tx.safeTxGas, tx.baseGas,
        tx.gasPrice, tx.gasToken, tx.refundReceiver, signatures
      );

      await wbth.connect(user).bridgeBurn(30n * PICO, "bth_dest_one");
      await wbth.connect(user).bridgeBurn(20n * PICO, "bth_dest_two");

      expect(await wbth.balanceOf(user.address)).to.equal(50n * PICO);
      expect(await wbth.totalSupply()).to.equal(50n * PICO);
    });
  });
});
