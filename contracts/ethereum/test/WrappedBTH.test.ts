import { expect } from "chai";
import { ethers } from "hardhat";
import {
  loadFixture,
  time,
} from "@nomicfoundation/hardhat-toolbox/network-helpers";

/**
 * WrappedBTH unit + invariant tests (#826).
 *
 * The Safes from ADR 0002 are simulated with EOAs here: threshold
 * enforcement lives inside the Gnosis Safe, not in this contract, so the
 * contract-level property under test is "only the MINTER_ROLE holder can
 * mint" (plus: the deployer holds no roles at all).
 */

const PICO = 10n ** 12n; // 1 BTH = 10^12 picocredits (12 decimals)
const MAX_TX = 1_000_000n * PICO;
const DAILY = 10_000_000n * PICO;

/** Deterministic order ids. */
function oid(n: number | string): string {
  return ethers.keccak256(ethers.toUtf8Bytes(`order-${n}`));
}

describe("WrappedBTH", function () {
  async function deployFixture() {
    const [deployer, admin, minter, pauser, user, other] =
      await ethers.getSigners();

    const WrappedBTH = await ethers.getContractFactory("WrappedBTH", deployer);
    const wbth = await WrappedBTH.deploy(
      admin.address,
      minter.address,
      pauser.address
    );
    await wbth.waitForDeployment();

    // Pin the clock to one hour past a UTC-day boundary so tests that
    // issue several mints "on the same day" can never straddle midnight.
    const now = await time.latest();
    const nextDayStart = (Math.floor(now / 86400) + 1) * 86400;
    await time.increaseTo(nextDayStart + 3600);

    return { wbth, deployer, admin, minter, pauser, user, other };
  }

  describe("deployment & role custody (ADR 0002)", function () {
    it("has 12 decimals (1 base unit == 1 picocredit)", async function () {
      const { wbth } = await loadFixture(deployFixture);
      expect(await wbth.decimals()).to.equal(12);
      expect(await wbth.name()).to.equal("Wrapped BTH");
      expect(await wbth.symbol()).to.equal("wBTH");
    });

    it("treats amounts as raw picocredits (5 BTH = 5 * 10^12 units)", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, 5n * PICO, oid(1));
      expect(await wbth.balanceOf(user.address)).to.equal(5n * PICO);
      expect(ethers.formatUnits(await wbth.balanceOf(user.address), 12)).to.equal(
        "5.0"
      );
    });

    it("grants NO roles to the deployer", async function () {
      const { wbth, deployer } = await loadFixture(deployFixture);
      const adminRole = await wbth.DEFAULT_ADMIN_ROLE();
      const minterRole = await wbth.MINTER_ROLE();
      const pauserRole = await wbth.PAUSER_ROLE();
      expect(await wbth.hasRole(adminRole, deployer.address)).to.equal(false);
      expect(await wbth.hasRole(minterRole, deployer.address)).to.equal(false);
      expect(await wbth.hasRole(pauserRole, deployer.address)).to.equal(false);
    });

    it("grants each role to its designated Safe only", async function () {
      const { wbth, admin, minter, pauser } = await loadFixture(deployFixture);
      const adminRole = await wbth.DEFAULT_ADMIN_ROLE();
      const minterRole = await wbth.MINTER_ROLE();
      const pauserRole = await wbth.PAUSER_ROLE();
      expect(await wbth.hasRole(adminRole, admin.address)).to.equal(true);
      expect(await wbth.hasRole(minterRole, minter.address)).to.equal(true);
      expect(await wbth.hasRole(pauserRole, pauser.address)).to.equal(true);
      // No cross-holding.
      expect(await wbth.hasRole(minterRole, admin.address)).to.equal(false);
      expect(await wbth.hasRole(adminRole, minter.address)).to.equal(false);
      expect(await wbth.hasRole(pauserRole, minter.address)).to.equal(false);
    });

    it("rejects zero addresses for any role holder", async function () {
      const [_, admin, minter, pauser] = await ethers.getSigners();
      const WrappedBTH = await ethers.getContractFactory("WrappedBTH");
      await expect(
        WrappedBTH.deploy(ethers.ZeroAddress, minter.address, pauser.address)
      ).to.be.revertedWith("Invalid admin");
      await expect(
        WrappedBTH.deploy(admin.address, ethers.ZeroAddress, pauser.address)
      ).to.be.revertedWith("Invalid minter");
      await expect(
        WrappedBTH.deploy(admin.address, minter.address, ethers.ZeroAddress)
      ).to.be.revertedWith("Invalid pauser");
    });

    it("ships with breaker defaults: autoPauseThreshold == dailyMintLimit", async function () {
      const { wbth } = await loadFixture(deployFixture);
      expect(await wbth.maxMintPerTx()).to.equal(MAX_TX);
      expect(await wbth.dailyMintLimit()).to.equal(DAILY);
      expect(await wbth.autoPauseThreshold()).to.equal(DAILY);
    });
  });

  describe("ABI compatibility with the Rust bindings", function () {
    // bridge/service/src/mint/ethereum.rs binds these exact signatures;
    // bridge/service/src/watchers/ethereum.rs binds BridgeBurn.
    it("pins bridgeMint(address,uint256,bytes32) selector", async function () {
      const { wbth } = await loadFixture(deployFixture);
      const fn = wbth.interface.getFunction("bridgeMint")!;
      expect(fn.selector).to.equal(
        ethers.id("bridgeMint(address,uint256,bytes32)").slice(0, 10)
      );
    });

    it("pins BridgeMint/BridgeBurn event signatures and indexing", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      const mintEv = wbth.interface.getEvent("BridgeMint")!;
      expect(mintEv.topicHash).to.equal(
        ethers.id("BridgeMint(address,uint256,bytes32)")
      );
      const burnEv = wbth.interface.getEvent("BridgeBurn")!;
      expect(burnEv.topicHash).to.equal(
        ethers.id("BridgeBurn(address,uint256,string)")
      );

      // Topic layout relied on by find_bridge_mint_event():
      // [signature, to (indexed), orderId (indexed)].
      const orderId = oid("abi");
      const tx = await wbth
        .connect(minter)
        .bridgeMint(user.address, PICO, orderId);
      const receipt = await tx.wait();
      const log = receipt!.logs.find((l) => l.topics[0] === mintEv.topicHash)!;
      expect(log.topics[1]).to.equal(
        ethers.zeroPadValue(user.address.toLowerCase(), 32)
      );
      expect(log.topics[2]).to.equal(orderId);
    });

    it("exposes NO open burn/burnFrom (bridgeBurn is the only burn path)", async function () {
      const { wbth } = await loadFixture(deployFixture);
      expect(wbth.interface.hasFunction("burn(uint256)")).to.equal(false);
      expect(wbth.interface.hasFunction("burnFrom(address,uint256)")).to.equal(
        false
      );
    });
  });

  describe("access control", function () {
    it("only the minter Safe can bridgeMint", async function () {
      const { wbth, deployer, admin, pauser, user } =
        await loadFixture(deployFixture);
      for (const caller of [deployer, admin, pauser, user]) {
        await expect(
          wbth.connect(caller).bridgeMint(user.address, PICO, oid("ac"))
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
      }
    });

    it("minter mints; balances and totalSupply update", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await expect(wbth.connect(minter).bridgeMint(user.address, PICO, oid(1)))
        .to.emit(wbth, "BridgeMint")
        .withArgs(user.address, PICO, oid(1));
      expect(await wbth.balanceOf(user.address)).to.equal(PICO);
      expect(await wbth.totalSupply()).to.equal(PICO);
    });

    it("only the admin Safe administers roles", async function () {
      const { wbth, admin, minter, other } = await loadFixture(deployFixture);
      const minterRole = await wbth.MINTER_ROLE();
      await expect(
        wbth.connect(minter).grantRole(minterRole, other.address)
      ).to.be.revertedWithCustomError(wbth, "AccessControlUnauthorizedAccount");
      await wbth.connect(admin).grantRole(minterRole, other.address);
      expect(await wbth.hasRole(minterRole, other.address)).to.equal(true);
      await wbth.connect(admin).revokeRole(minterRole, other.address);
      expect(await wbth.hasRole(minterRole, other.address)).to.equal(false);
    });

    it("only the admin Safe can change limits and the breaker", async function () {
      const { wbth, admin, minter, pauser } = await loadFixture(deployFixture);
      for (const caller of [minter, pauser]) {
        await expect(
          wbth.connect(caller).setMaxMintPerTx(1)
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
        await expect(
          wbth.connect(caller).setDailyMintLimit(1)
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
        await expect(
          wbth.connect(caller).setAutoPauseThreshold(1)
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
      }
      await expect(wbth.connect(admin).setMaxMintPerTx(123n))
        .to.emit(wbth, "RateLimitUpdated")
        .withArgs(123n, DAILY, DAILY);
    });
  });

  describe("mint validation", function () {
    it("rejects zero recipient, zero amount, zero order id, oversize tx", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await expect(
        wbth.connect(minter).bridgeMint(ethers.ZeroAddress, PICO, oid(1))
      ).to.be.revertedWith("Invalid recipient");
      await expect(
        wbth.connect(minter).bridgeMint(user.address, 0, oid(1))
      ).to.be.revertedWith("Amount must be positive");
      await expect(
        wbth.connect(minter).bridgeMint(user.address, PICO, ethers.ZeroHash)
      ).to.be.revertedWith("Invalid order id");
      await expect(
        wbth.connect(minter).bridgeMint(user.address, MAX_TX + 1n, oid(1))
      ).to.be.revertedWith("Exceeds max mint per tx");
      // Boundary: exactly maxMintPerTx is allowed.
      await wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid(1));
    });
  });

  describe("order-id replay guard", function () {
    it("rejects a duplicate order id (idempotent mint)", async function () {
      const { wbth, minter, user, other } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid("dup"));
      expect(await wbth.processedOrders(oid("dup"))).to.equal(true);

      // Same id replayed — identical args, different args, different
      // recipient: all must revert.
      await expect(
        wbth.connect(minter).bridgeMint(user.address, PICO, oid("dup"))
      ).to.be.revertedWith("Order already processed");
      await expect(
        wbth.connect(minter).bridgeMint(other.address, 2n * PICO, oid("dup"))
      ).to.be.revertedWith("Order already processed");
    });

    it("allows distinct order ids", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid(1));
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid(2));
      expect(await wbth.balanceOf(user.address)).to.equal(2n * PICO);
    });

    it("keeps the guard across day rollovers (permanent, not daily)", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid("perm"));
      await time.increase(3 * 86400);
      await expect(
        wbth.connect(minter).bridgeMint(user.address, PICO, oid("perm"))
      ).to.be.revertedWith("Order already processed");
    });
  });

  describe("daily limit accounting", function () {
    it("enforces the daily cap cumulatively (no recipient-rotation bypass)", async function () {
      const { wbth, admin, minter, user, other } =
        await loadFixture(deployFixture);
      await wbth.connect(admin).setAutoPauseThreshold(0); // isolate the cap

      // 10 x 1M BTH to alternating recipients consumes the full cap.
      for (let i = 0; i < 10; i++) {
        const to = i % 2 === 0 ? user.address : other.address;
        await wbth.connect(minter).bridgeMint(to, MAX_TX, oid(`cap-${i}`));
      }
      expect(await wbth.dailyMinted()).to.equal(DAILY);
      expect(await wbth.remainingDailyMint()).to.equal(0);
      await expect(
        wbth.connect(minter).bridgeMint(user.address, 1n, oid("cap-over"))
      ).to.be.revertedWith("Daily limit exceeded");
    });

    it("resets after one day and after multi-day gaps", async function () {
      const { wbth, admin, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(admin).setAutoPauseThreshold(0);

      await wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid("d0"));
      expect(await wbth.remainingDailyMint()).to.equal(DAILY - MAX_TX);

      // Next day: full capacity again.
      await time.increase(86400);
      expect(await wbth.remainingDailyMint()).to.equal(DAILY);
      await wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid("d1"));
      expect(await wbth.dailyMinted()).to.equal(MAX_TX);

      // Multi-day gap with NO mint on the intermediate days: the lazy
      // reset must still fire (strictly-greater comparison).
      await time.increase(5 * 86400);
      expect(await wbth.remainingDailyMint()).to.equal(DAILY);
      for (let i = 0; i < 10; i++) {
        await wbth
          .connect(minter)
          .bridgeMint(user.address, MAX_TX, oid(`gap-${i}`));
      }
      expect(await wbth.dailyMinted()).to.equal(DAILY);
    });

    it("keeps remainingDailyMint consistent with the mutating branch", async function () {
      const { wbth, admin, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(admin).setAutoPauseThreshold(0);
      await wbth
        .connect(minter)
        .bridgeMint(user.address, 3n * PICO, oid("view"));
      expect(await wbth.remainingDailyMint()).to.equal(DAILY - 3n * PICO);

      // Lowering the limit below what was already minted floors at 0.
      await wbth.connect(admin).setDailyMintLimit(PICO);
      expect(await wbth.remainingDailyMint()).to.equal(0);
      await expect(
        wbth.connect(minter).bridgeMint(user.address, 1n, oid("view2"))
      ).to.be.revertedWith("Daily limit exceeded");
    });
  });

  describe("pause (manual circuit breaker)", function () {
    it("pauser pauses/unpauses; mint AND burn are gated", async function () {
      const { wbth, minter, pauser, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, 2n * PICO, oid(1));

      await wbth.connect(pauser).pause();
      await expect(
        wbth.connect(minter).bridgeMint(user.address, PICO, oid(2))
      ).to.be.revertedWithCustomError(wbth, "EnforcedPause");
      await expect(
        wbth.connect(user).bridgeBurn(PICO, "bth1destination")
      ).to.be.revertedWithCustomError(wbth, "EnforcedPause");

      await wbth.connect(pauser).unpause();
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid(2));
      await wbth.connect(user).bridgeBurn(PICO, "bth1destination");
    });

    it("only the pauser Safe can pause/unpause", async function () {
      const { wbth, admin, minter, pauser, user } =
        await loadFixture(deployFixture);
      for (const caller of [admin, minter, user]) {
        await expect(
          wbth.connect(caller).pause()
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
      }
      await wbth.connect(pauser).pause();
      for (const caller of [admin, minter, user]) {
        await expect(
          wbth.connect(caller).unpause()
        ).to.be.revertedWithCustomError(
          wbth,
          "AccessControlUnauthorizedAccount"
        );
      }
    });
  });

  describe("auto-pause circuit breaker", function () {
    it("trips when cumulative daily volume reaches the threshold", async function () {
      const { wbth, admin, minter, pauser, user } =
        await loadFixture(deployFixture);
      await wbth.connect(admin).setAutoPauseThreshold(3n * MAX_TX);

      await wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid(1));
      await wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid(2));
      expect(await wbth.paused()).to.equal(false);

      // The crossing mint SUCCEEDS (it is within the daily limit) but
      // flips the breaker for everything after it.
      await expect(wbth.connect(minter).bridgeMint(user.address, MAX_TX, oid(3)))
        .to.emit(wbth, "AutoPaused")
        .withArgs(3n * MAX_TX, 3n * MAX_TX);
      expect(await wbth.paused()).to.equal(true);
      expect(await wbth.balanceOf(user.address)).to.equal(3n * MAX_TX);
      await expect(
        wbth.connect(minter).bridgeMint(user.address, PICO, oid(4))
      ).to.be.revertedWithCustomError(wbth, "EnforcedPause");

      // Guardian review + unpause resumes (daily accounting continues).
      await wbth.connect(pauser).unpause();
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid(4));
    });

    it("trips at the full daily limit by default", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      for (let i = 0; i < 10; i++) {
        await wbth
          .connect(minter)
          .bridgeMint(user.address, MAX_TX, oid(`def-${i}`));
      }
      expect(await wbth.paused()).to.equal(true);
    });

    it("is disabled when the threshold is 0", async function () {
      const { wbth, admin, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(admin).setAutoPauseThreshold(0);
      for (let i = 0; i < 10; i++) {
        await wbth
          .connect(minter)
          .bridgeMint(user.address, MAX_TX, oid(`off-${i}`));
      }
      expect(await wbth.paused()).to.equal(false);
    });
  });

  describe("bridgeBurn", function () {
    it("burns and emits the redemption event the watcher binds", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, 5n * PICO, oid(1));
      await expect(wbth.connect(user).bridgeBurn(2n * PICO, "bth1stealthaddr"))
        .to.emit(wbth, "BridgeBurn")
        .withArgs(user.address, 2n * PICO, "bth1stealthaddr");
      expect(await wbth.balanceOf(user.address)).to.equal(3n * PICO);
      expect(await wbth.totalSupply()).to.equal(3n * PICO);
    });

    it("rejects zero amount, empty destination, and over-balance burns", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      await wbth.connect(minter).bridgeMint(user.address, PICO, oid(1));
      await expect(
        wbth.connect(user).bridgeBurn(0, "bth1x")
      ).to.be.revertedWith("Amount must be positive");
      await expect(wbth.connect(user).bridgeBurn(PICO, "")).to.be.revertedWith(
        "Invalid BTH address"
      );
      await expect(
        wbth.connect(user).bridgeBurn(2n * PICO, "bth1x")
      ).to.be.revertedWithCustomError(wbth, "ERC20InsufficientBalance");
    });
  });

  describe("reentrancy", function () {
    // OZ ERC-20 v5 _mint/_burn make no external calls (no ERC-777 hooks),
    // so no reentrant path can be triggered from a test. These tests pin
    // the defense-in-depth posture: contract recipients get no callback,
    // and state (replay guard, daily accounting) is committed before the
    // token movement (checks-effects-interactions).
    it("mints to a contract recipient without invoking any hook", async function () {
      const { wbth, admin, minter, pauser, user } =
        await loadFixture(deployFixture);
      // Any contract address works as a hook-less recipient; use a second
      // token instance rather than shipping a mock.
      const WrappedBTH = await ethers.getContractFactory("WrappedBTH");
      const recipient = await WrappedBTH.deploy(
        admin.address,
        minter.address,
        pauser.address
      );
      await recipient.waitForDeployment();

      const to = await recipient.getAddress();
      await wbth.connect(minter).bridgeMint(to, PICO, oid("contract"));
      expect(await wbth.balanceOf(to)).to.equal(PICO);
    });

    it("commits the replay guard before the mint (CEI)", async function () {
      const { wbth, minter, user } = await loadFixture(deployFixture);
      const tx = await wbth
        .connect(minter)
        .bridgeMint(user.address, PICO, oid("cei"));
      await tx.wait();
      expect(await wbth.processedOrders(oid("cei"))).to.equal(true);
    });
  });

  describe("supply accounting invariant (randomized)", function () {
    it("totalSupply == sum(mints) - sum(burns) under random ops", async function () {
      const { wbth, admin, minter, user, other } =
        await loadFixture(deployFixture);
      // Disable the breaker so the run exercises many days of flow.
      await wbth.connect(admin).setAutoPauseThreshold(0);

      // Deterministic PRNG (mulberry32) so failures reproduce.
      let seed = 0x5eed826;
      const rand = () => {
        seed |= 0;
        seed = (seed + 0x6d2b79f5) | 0;
        let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };

      let minted = 0n;
      let burned = 0n;
      const holders = [user, other];

      for (let i = 0; i < 120; i++) {
        const roll = rand();
        if (roll < 0.55) {
          // Mint a random amount to a random holder.
          const amount = (BigInt(Math.floor(rand() * 1_000_000)) + 1n) * PICO;
          const to = holders[Math.floor(rand() * holders.length)];
          try {
            await wbth
              .connect(minter)
              .bridgeMint(to.address, amount, oid(`fuzz-${i}`));
            minted += amount;
          } catch (e: any) {
            // Only the daily cap may reject an otherwise-valid mint here.
            expect(String(e.message)).to.contain("Daily limit exceeded");
          }
        } else if (roll < 0.85) {
          // Burn a random fraction of a random holder's balance.
          const from = holders[Math.floor(rand() * holders.length)];
          const balance = await wbth.balanceOf(from.address);
          if (balance > 0n) {
            const amount =
              (balance * BigInt(1 + Math.floor(rand() * 100))) / 100n;
            await wbth.connect(from).bridgeBurn(amount, "bth1fuzz");
            burned += amount;
          }
        } else {
          // Jump time by 1..72 hours (exercises the daily reset).
          await time.increase(3600 * (1 + Math.floor(rand() * 72)));
        }

        // Invariants after every operation.
        expect(await wbth.totalSupply()).to.equal(minted - burned);
        expect(await wbth.dailyMinted()).to.be.lte(await wbth.dailyMintLimit());
      }

      // The run must have actually exercised both paths.
      expect(minted).to.be.gt(0n);
      expect(burned).to.be.gt(0n);
    });
  });

  // Adversarial rate-limit accounting fuzz (bridge epic #816, Phase 3,
  // issue #829). The #851 supply invariant above disables the breaker and
  // only bounds `dailyMinted <= dailyMintLimit`. This test attacks the
  // rate-limit ACCOUNTING itself with the breaker ARMED: it drives
  // randomized mints and time-jumps across UTC-day boundaries and asserts,
  // after every operation, that `dailyMinted`, `remainingDailyMint()`, and
  // the auto-pause state exactly match an independent model — including the
  // reset-is-rolled-back-on-revert and no-reset-on-unpause subtleties.
  describe("rate-limit accounting fuzz (randomized, breaker armed)", function () {
    it("dailyMinted / remainingDailyMint / auto-pause track a model under random ops", async function () {
      const { wbth, admin, minter, pauser, user } = await loadFixture(
        deployFixture
      );

      // A tight regime so a handful of max-size mints fill a day. The run
      // has two phases: with the breaker ARMED below the cap (phase 1,
      // exercises auto-pause + recovery) and with the breaker DISABLED
      // (phase 2, lets the counter reach the cap so the daily-limit revert
      // fires). `currentThreshold` mirrors the on-chain breaker setting.
      const DAILY_LIMIT = 5n * MAX_TX;
      let currentThreshold = 4n * MAX_TX;
      const PHASE2_AT = 70;
      await wbth.connect(admin).setDailyMintLimit(DAILY_LIMIT);
      await wbth.connect(admin).setAutoPauseThreshold(currentThreshold);

      // Deterministic PRNG (mulberry32) so failures reproduce.
      let seed = 0x0829a11;
      const rand = () => {
        seed |= 0;
        seed = (seed + 0x6d2b79f5) | 0;
        let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
      };

      // Independent model of the contract's rate-limit state.
      let modelMinted = 0n;
      let modelResetDay = BigInt(Math.floor((await time.latest()) / 86400));
      let modelPaused = false;
      let sawLimitRevert = false;
      let sawAutoPause = false;
      let orderNonce = 0;

      for (let i = 0; i < 140; i++) {
        // Enter phase 2: disable the breaker so the counter can reach the
        // daily cap and exercise the "Daily limit exceeded" revert (which
        // the breaker would otherwise pre-empt at the threshold).
        if (i === PHASE2_AT && currentThreshold !== 0n) {
          if (modelPaused) {
            await wbth.connect(pauser).unpause();
            modelPaused = false;
          }
          await wbth.connect(admin).setAutoPauseThreshold(0);
          currentThreshold = 0n;
        }

        // Recover from an auto-pause: unpause (no reset happens) and jump a
        // full day so the NEXT mint's lazy reset clears the counter.
        if (modelPaused) {
          await wbth.connect(pauser).unpause();
          await time.increase(86400 + 3600);
          modelPaused = false;
        }

        const roll = rand();
        const nowTs = await time.latest();
        const today = BigInt(Math.floor(nowTs / 86400));

        if (roll < 0.72) {
          // Attempt a mint of 1..maxMintPerTx picocredits.
          const amount = (BigInt(Math.floor(rand() * 1_000_000)) + 1n) * PICO;
          // The contract resets the counter at the top of bridgeMint, but a
          // revert on the limit check rolls that reset back.
          const base = today > modelResetDay ? 0n : modelMinted;

          if (base + amount <= DAILY_LIMIT) {
            await wbth
              .connect(minter)
              .bridgeMint(user.address, amount, oid(`rl-${orderNonce++}`));
            modelMinted = base + amount;
            modelResetDay = today;
            if (currentThreshold !== 0n && modelMinted >= currentThreshold) {
              modelPaused = true;
              sawAutoPause = true;
            }
          } else {
            await expect(
              wbth
                .connect(minter)
                .bridgeMint(user.address, amount, oid(`rl-${orderNonce++}`))
            ).to.be.revertedWith("Daily limit exceeded");
            sawLimitRevert = true;
            // State (including the day counter) is unchanged by the revert.
          }
        } else {
          // Jump 1..48 hours (often crossing a UTC-day boundary).
          await time.increase(3600 * (1 + Math.floor(rand() * 48)));
        }

        // Accounting invariants after every operation.
        const tsAfter = await time.latest();
        const todayAfter = BigInt(Math.floor(tsAfter / 86400));
        expect(await wbth.dailyMinted()).to.equal(modelMinted);
        expect(await wbth.paused()).to.equal(modelPaused);

        // remainingDailyMint() folds in the pending (not-yet-persisted) reset.
        let expectedRemaining: bigint;
        if (todayAfter > modelResetDay) {
          expectedRemaining = DAILY_LIMIT;
        } else if (modelMinted >= DAILY_LIMIT) {
          expectedRemaining = 0n;
        } else {
          expectedRemaining = DAILY_LIMIT - modelMinted;
        }
        expect(await wbth.remainingDailyMint()).to.equal(expectedRemaining);
        expect(await wbth.dailyMinted()).to.be.lte(await wbth.dailyMintLimit());
      }

      // The run must have actually exercised the interesting branches.
      expect(sawLimitRevert, "fuzz never hit the daily-limit revert").to.equal(
        true
      );
      expect(
        sawAutoPause,
        "fuzz never tripped the auto-pause breaker"
      ).to.equal(true);
    });
  });
});
