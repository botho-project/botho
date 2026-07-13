# wBTH — Wrapped BTH on Solana

The `wbth` Anchor program (`programs/wbth`) mints the wBTH SPL token 1:1
against BTH locked in the bridge reserve (epic #816). One wBTH base unit
equals one picocredit (`mint::decimals = 12`); no scaling happens anywhere
across the bridge boundary. This is the Solana counterpart to
`contracts/ethereum/contracts/WrappedBTH.sol` and mirrors its hardening
decisions (#826 → #850).

## Security model (parity with #826, decided per ADR 0002 / 0005)

### Custody — ADR 0002

No single key can mint. The program stores three **authority pubkeys**, each
of which is expected to be a **multisig** account — an SPL Token multisig or
a [Squads](https://squads.so) multisig PDA whose members are the validators'
**Ed25519** keys. Solana verifies Ed25519 natively, so the SCP validator
federation signs Solana mint authorizations directly (no secp256k1/Gnosis
detour like the Ethereum side). **Threshold `t`-of-`n` enforcement lives
inside the multisig program, NOT in `wbth`** — exactly as the Gnosis Safe
holds the threshold on Ethereum. The program only checks that the presented
signer equals the configured authority pubkey.

| Role (Bridge field) | Holder | Purpose |
|---------------------|--------|---------|
| `mint_authority` | Validator multisig (t-of-n Ed25519, owners = SCP validators; t ≥ SCP safety threshold) | The only mint path: signs `bridge_mint` |
| `admin_authority` | Governance multisig (distinct from the validator multisig) | Rate-limit / breaker configuration, role rotation (`set_daily_limit`, `set_auto_pause_threshold`, `transfer_authority`, `transfer_admin`, `transfer_pauser`) |
| `pauser_authority` | Guardian multisig (may be lower-threshold) | Fast `pause` / `unpause` — fail-safe only, cannot move funds |

The mint-authority **PDA** (`seeds = [b"bridge"]`) is the SPL `MintTo` /
freeze authority on the wBTH mint; the multisig only *authorizes* the
`bridge_mint` instruction, while the PDA signs the CPI. `initialize` takes the
three authority pubkeys explicitly; the payer receives no standing role.

Record the concrete multisig addresses + thresholds here per deployment:

| Network | Program ID | wBTH mint | Mint multisig | Admin multisig | Pauser multisig |
|---------|-----------|-----------|---------------|----------------|-----------------|
| (none deployed yet) | | | | | |

### Deploy / init runbook — checked custody gate (ADR 0002)

The custody model above is a **deploy-time invariant** the on-chain program
cannot self-enforce: `wbth` only checks that the presented signer *equals*
`bridge.mint_authority`; the `t`-of-`n` threshold lives inside the multisig.
So if `initialize` sets `mint_authority` to the relayer's own single key, a
lone compromised key can mint. Turn the invariant into a step an operator ticks
off at deploy:

1. `[ ]` Create the **mint** multisig (distinct from admin/pauser). Its members
   are the SCP validators' Ed25519 keys, threshold `t ≥ SCP safety threshold`.
   - SPL Token multisig:
     ```bash
     spl-token create-multisig <THRESHOLD> <VALIDATOR_PUBKEY_1> <VALIDATOR_PUBKEY_2> ... <VALIDATOR_PUBKEY_N>
     # → prints the mint multisig address; record it below.
     ```
   - or a [Squads](https://squads.so) multisig PDA (members = validator keys),
     recording the resulting multisig PDA address.
2. `[ ]` Create the **admin** and **pauser** multisigs the same way (each
   distinct from the mint multisig and from each other).
3. `[ ]` Call `initialize` passing the three authority pubkeys **explicitly**
   (the payer receives no standing role):
   ```bash
   anchor run initialize -- \
     --mint-authority   <MINT_MULTISIG>   \
     --admin-authority  <ADMIN_MULTISIG>  \
     --pauser-authority <PAUSER_MULTISIG>
   # (or the equivalent `solana`/TS client call that invokes `initialize`
   #  with those three pubkeys)
   ```
4. `[ ]` **Hard gate — verify before any value flows:** `mint_authority` is a
   **DISTINCT multisig** (Squads PDA or SPL Token multisig), **NOT** the
   relayer / `bridge.solana.keypair_file` key, **and NOT** `admin_authority`.
   The bridge service also enforces this at startup: it reads the on-chain
   `mint_authority` and, in a production posture
   (`solana.mint_signers`/`mint_threshold` configured), **fails to start** if it
   equals this node's local key (warns otherwise) — see
   `bridge/service/src/mint/solana.rs`
   (`verify_mint_authority_is_not_local_key`, #879).
5. `[ ]` Record the resulting addresses + thresholds in the per-deployment table
   above (Program ID / wBTH mint / Mint multisig / Admin multisig / Pauser
   multisig).
6. `[ ]` Finalize the BPF upgrade authority (`--final`) before mainnet value
   flows — see [Upgradeability](#upgradeability--immutable-at-deploy-bpf-upgrade-authority-revoked)
   below. This custody gate and the upgrade-authority gate are the two deploy
   gates that must both be closed before mainnet.

### Replay-proof, order-bound minting

`bridge_mint(amount, order_id)` takes a 32-byte `order_id` (the bridge order
id from the attestation protocol, #824, `bridge_core::derive_order_id`) and
creates a per-order marker PDA with `init` (`seeds = [b"order", order_id]`).
A duplicate order id **fails at `init`** because the account already exists —
the Solana equivalent of the Ethereum `processedOrders` mapping, closing the
residual double-mint window even if the off-chain service retries or is
compromised into re-submitting an authorization.

The instruction name (`bridge_mint`) and the borsh **arg order**
(`amount: u64` little-endian, then the raw 32-byte `order_id`) are pinned by
`bridge/service/src/mint/solana.rs`
(`encode_bridge_mint_instruction_data`) — **do not reorder or rename in a way
that changes the discriminator or the encoded bytes** without updating the
Rust side. (The Anchor discriminator hashes the instruction *name* only, so
renaming the arg is safe; reordering the args is not.)

### Upgradeability — immutable at deploy (BPF upgrade authority revoked)

To match the Ethereum IMMUTABLE posture, the deployed program's BPF upgrade
authority MUST be revoked at deploy time:

```bash
solana program set-upgrade-authority <PROGRAM_ID> --final
```

Rationale (mirrors the project's no-hard-forks / minimal-trust posture): a
retained upgrade authority is a rug vector — whoever can upgrade the program
can rewrite `bridge_mint` to mint at will or seize the mint-authority PDA,
which negates the multisig custody model. Trade-off (accepted): bugs are
handled by `pause`, deploying a corrected program + mint, and migrating
balances through the bridge itself (burn on old, mint on new) — the same
recovery path documented for the Ethereum token.

**Testnet exception:** on devnet/testnet the upgrade authority MAY be retained
for iteration. When it is, the holding key (ideally the governance multisig,
never a lone EOA) MUST be recorded in the deployment table above, and it MUST
be finalized before any mainnet value flows.

### Rate limits + circuit breaker (picocredits)

- `MAX_MINT_PER_TX` (1M BTH) caps a single `bridge_mint`; `daily_mint_limit`
  (default 10M BTH) caps cumulative mints per UTC day. Both are the same raw
  picocredit literals as the EVM contract.
- The daily counter lazily resets on the first mint of a later UTC day, using
  `Clock::unix_timestamp / 86_400` (parity with EVM `block.timestamp / 1 days`;
  correct across multi-day gaps). The previous slot-based window was replaced
  because slot time drifts and is not a wall-clock day.
- **Auto-pause breaker:** when cumulative daily volume reaches
  `auto_pause_threshold` (default = the daily limit; 0 disables), the program
  sets `paused = true` and emits `AutoPausedEvent`. The triggering mint still
  succeeds (it is within the daily limit); a guardian must investigate and
  `unpause`. Converts anomalous volume from a soft revert into a hard stop.

### Arithmetic safety

Daily-total accumulation uses `checked_add(...).ok_or(BridgeError::MathOverflow)?`
(no `.unwrap()` panic path). The release profile also sets
`overflow-checks = true`.

### Burn path

`bridge_burn(amount, bth_address)` is the only burn path; it honors `paused`,
requires `amount > 0` and a non-empty `bth_address` bounded to
`MAX_BTH_ADDRESS_LEN` (128) bytes, then emits `BridgeBurnEvent` — the event the
native-chain release watchers rely on. There is no open SPL burn surface on
this program's authority: users burn their own tokens (they sign as `user`).

### Mint-redirection guard

`bridge_mint`'s `user_token_account` is constrained
`associated_token::authority = user`, so a mint can only land in the
recipient's own associated token account; a compromised or buggy caller cannot
redirect minted wBTH to an arbitrary token account.

## Development

```bash
npm install        # or: yarn
anchor build       # compiles the program + generates target/types/wbth.ts
anchor test        # spins up a local validator and runs the ts-mocha suite
```

The `tests/wbth.ts` suite covers: `initialize` (PDA mint authority + 12
decimals + distinct roles), multisig-gated `bridge_mint`, order-id replay
(same id fails at PDA `init`, distinct ids succeed), unit pinning (raw
picocredits), the mint-redirection guard, `bridge_burn` redemption events +
address bounds, admin-only limit/threshold/authority-rotation, guardian-only
pause (mint + burn blocked while paused), and the daily-limit boundary +
auto-pause breaker.

> **Toolchain note:** running `anchor test` requires the Solana toolchain
> (`solana`, `cargo build-sbf`) and Anchor CLI 0.29. The Rust program itself
> compiles under a plain host `cargo build` (used in CI without the SBF
> toolchain) via the Anchor host-side codegen.
