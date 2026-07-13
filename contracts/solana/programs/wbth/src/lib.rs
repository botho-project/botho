use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount};

declare_id!("wBTH111111111111111111111111111111111111111");

/// Wrapped BTH (wBTH) SPL token bridge program.
///
/// This program manages the wBTH token on Solana, allowing:
/// - The validator federation (via a multisig) to mint wBTH when BTH is locked
///   on the native chain.
/// - Users to burn wBTH to redeem BTH on the native chain.
/// - Rate limiting, a circuit breaker, and pause functionality for security.
///
/// # Security model — parity with `WrappedBTH.sol` (#826, ADR 0002/0005)
///
/// The Ethereum side (`contracts/ethereum/contracts/WrappedBTH.sol`) was
/// hardened in #826; this program mirrors those decisions on Solana:
///
/// - **No single-key mint (ADR 0002).** The `authority` fields are the ONLY
///   privileged signers, and each is expected to be a *multisig* account — an
///   SPL Token multisig or a Squads multisig PDA whose members are the
///   validators' Ed25519 keys (which Solana verifies natively, so no secp256k1
///   detour is needed). Threshold `t`-of-`n` enforcement lives inside that
///   multisig program, NOT in this contract — exactly as the Gnosis Safe holds
///   the threshold on the Ethereum side. The three roles are held by three
///   DISTINCT multisigs so that minting, configuration, and pausing require
///   different quorums:
///     * `mint_authority`  — validator multisig (t ≥ SCP safety threshold).
///     * `admin_authority` — governance multisig (limits / breaker / role
///       rotation).
///     * `pauser_authority`— guardian multisig (may be lower-threshold for fast
///       incident response; pausing can only halt, never move funds).
///   The mint-authority PDA (`seeds = [b"bridge"]`) remains the SPL
///   `MintTo` signer; the multisig only *authorizes* the instruction.
///
/// - **Replay-proof, order-bound minting.** `bridge_mint` takes an `order_id:
///   [u8; 32]` (the attestation-bound bridge order id, #824, from
///   `bridge_core::derive_order_id`) and creates a per-order marker PDA
///   (`init`, `seeds = [b"order", order_id]`). A duplicate order id fails at
///   `init` (account already exists) — the Solana equivalent of Ethereum's
///   `processedOrders` mapping. The instruction signature and borsh arg order
///   (`amount: u64` then `order_id: [u8; 32]`) are pinned by the service side
///   (`bridge/service/src/mint/solana.rs`,
///   `encode_bridge_mint_instruction_data`); do not reorder them.
///
/// - **Immutable program (upgrade authority).** To match the Ethereum IMMUTABLE
///   posture (a proxy/upgrade admin is a rug vector that negates the multisig
///   custody model), the deployed program's BPF upgrade authority MUST be
///   revoked at deploy time via `solana program set-upgrade-authority
///   <PROGRAM_ID> --final`. This is a deploy-time operation (documented in
///   `contracts/solana/README.md`), not something the program enforces. On
///   testnet the upgrade authority MAY be retained for iteration; who holds it
///   is recorded in the README.
///
/// - **Units / rate limits (picocredits).** `mint::decimals = 12` (1 base unit
///   == 1 picocredit == 1:1 with native BTH). `MAX_MINT_PER_TX` (1M BTH) and
///   the default `daily_mint_limit` (10M BTH) are the same raw picocredit
///   literals as the EVM contract. The daily counter resets on a UTC-day
///   boundary using `Clock::unix_timestamp` (parity with the EVM
///   `block.timestamp / 1 days`), not slots.
///
/// - **Auto-pause circuit breaker.** When cumulative daily volume reaches
///   `auto_pause_threshold` (default = the daily limit; 0 disables), the bridge
///   pauses itself — the triggering mint still succeeds (it is within the daily
///   limit) but a guardian must investigate and unpause.
#[program]
pub mod wbth_bridge {
    use super::*;

    /// Maximum amount a single `bridge_mint` may mint (picocredits, 1M BTH).
    /// Matches `WrappedBTH.maxMintPerTx`.
    pub const MAX_MINT_PER_TX: u64 = 1_000_000 * 1_000_000_000_000;

    /// Default cumulative daily mint limit (picocredits, 10M BTH).
    /// Matches `WrappedBTH.dailyMintLimit`.
    pub const DEFAULT_DAILY_MINT_LIMIT: u64 = 10_000_000 * 1_000_000_000_000;

    /// Length of a UTC day in seconds (parity with EVM `1 days`).
    pub const SECONDS_PER_DAY: i64 = 86_400;

    /// Maximum accepted length of a native BTH destination address string.
    /// Stealth-address strings are well under this; the bound prevents a
    /// griefer from bloating the burn event / transaction.
    pub const MAX_BTH_ADDRESS_LEN: usize = 128;

    /// Initialize the bridge with a new wBTH mint.
    ///
    /// `mint_authority`, `admin_authority`, and `pauser_authority` should each
    /// be a distinct multisig account (SPL/Squads) per ADR 0002; the program
    /// only checks that the presented signer matches the configured pubkey —
    /// the multisig enforces the threshold.
    pub fn initialize(
        ctx: Context<Initialize>,
        bump: u8,
        mint_authority: Pubkey,
        admin_authority: Pubkey,
        pauser_authority: Pubkey,
    ) -> Result<()> {
        let bridge = &mut ctx.accounts.bridge;
        bridge.mint_authority = mint_authority;
        bridge.admin_authority = admin_authority;
        bridge.pauser_authority = pauser_authority;
        bridge.mint = ctx.accounts.mint.key();
        bridge.bump = bump;
        bridge.paused = false;
        bridge.daily_mint_limit = DEFAULT_DAILY_MINT_LIMIT;
        bridge.auto_pause_threshold = DEFAULT_DAILY_MINT_LIMIT;
        bridge.daily_minted = 0;
        // UTC-day index of the last reset (parity with EVM lastResetDay).
        bridge.last_reset_day = Clock::get()?.unix_timestamp / SECONDS_PER_DAY;

        msg!("Bridge initialized with mint: {}", bridge.mint);
        msg!(
            "Authorities — mint: {}, admin: {}, pauser: {}",
            bridge.mint_authority,
            bridge.admin_authority,
            bridge.pauser_authority
        );
        Ok(())
    }

    /// Mint wBTH to a user for a locked-BTH bridge order.
    ///
    /// Only callable by the `mint_authority` multisig. Replay-proof: the
    /// per-order marker PDA (`seeds = [b"order", order_id]`) is created with
    /// `init`, so a duplicate `order_id` fails here.
    ///
    /// Argument order (`amount` then `order_id`) and borsh encoding are pinned
    /// by `bridge/service/src/mint/solana.rs`.
    pub fn bridge_mint(ctx: Context<BridgeMint>, amount: u64, order_id: [u8; 32]) -> Result<()> {
        let bridge = &mut ctx.accounts.bridge;

        require!(!bridge.paused, BridgeError::Paused);
        require!(amount > 0, BridgeError::InvalidAmount);
        require!(amount <= MAX_MINT_PER_TX, BridgeError::ExceedsMaxMint);
        require!(order_id != [0u8; 32], BridgeError::InvalidOrderId);

        // Lazily reset the daily counter on the first mint of a later UTC day
        // (strictly-greater comparison, so multi-day gaps reset correctly —
        // parity with the EVM contract).
        let clock = Clock::get()?;
        let today = clock.unix_timestamp / SECONDS_PER_DAY;
        if today > bridge.last_reset_day {
            bridge.daily_minted = 0;
            bridge.last_reset_day = today;
        }

        // Effects before interaction (checks-effects-interactions). Overflow
        // is a hard error rather than a silent wrap / panic.
        let new_daily = bridge
            .daily_minted
            .checked_add(amount)
            .ok_or(BridgeError::MathOverflow)?;
        require!(
            new_daily <= bridge.daily_mint_limit,
            BridgeError::DailyLimitExceeded
        );
        bridge.daily_minted = new_daily;

        // Record the order id on the marker PDA for auditability. The replay
        // guard itself is enforced by `init` on `order_marker` in the accounts
        // struct; storing the id makes the account self-describing.
        ctx.accounts.order_marker.order_id = order_id;

        // Mint tokens using the bridge PDA as the SPL mint authority.
        let seeds = &[b"bridge".as_ref(), &[bridge.bump]];
        let signer = &[&seeds[..]];

        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                authority: bridge.to_account_info(),
            },
            signer,
        );
        token::mint_to(cpi_ctx, amount)?;

        emit!(BridgeMintEvent {
            user: ctx.accounts.user.key(),
            amount,
            order_id,
            timestamp: clock.unix_timestamp,
        });

        msg!(
            "Minted {} wBTH to {} (order: {})",
            amount,
            ctx.accounts.user.key(),
            hex::encode(&order_id[..8])
        );

        // Circuit breaker: anomalous cumulative daily volume halts the bridge
        // instead of merely reverting later mints (parity with EVM AutoPaused).
        if bridge.auto_pause_threshold != 0 && bridge.daily_minted >= bridge.auto_pause_threshold {
            bridge.paused = true;
            emit!(AutoPausedEvent {
                daily_minted: bridge.daily_minted,
                threshold: bridge.auto_pause_threshold,
            });
            msg!(
                "Auto-paused: daily volume {} reached threshold {}",
                bridge.daily_minted,
                bridge.auto_pause_threshold
            );
        }

        Ok(())
    }

    /// Burn wBTH to redeem BTH on the native chain.
    ///
    /// This is the only burn path; the emitted `BridgeBurnEvent` is what the
    /// bridge watchers rely on to release native BTH.
    pub fn bridge_burn(ctx: Context<BridgeBurn>, amount: u64, bth_address: String) -> Result<()> {
        let bridge = &ctx.accounts.bridge;

        require!(!bridge.paused, BridgeError::Paused);
        require!(amount > 0, BridgeError::InvalidAmount);
        require!(!bth_address.is_empty(), BridgeError::InvalidBthAddress);
        require!(
            bth_address.len() <= MAX_BTH_ADDRESS_LEN,
            BridgeError::InvalidBthAddress
        );

        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.mint.to_account_info(),
                from: ctx.accounts.user_token_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::burn(cpi_ctx, amount)?;

        emit!(BridgeBurnEvent {
            user: ctx.accounts.user.key(),
            amount,
            bth_address: bth_address.clone(),
            timestamp: Clock::get()?.unix_timestamp,
        });

        msg!(
            "Burned {} wBTH from {} to BTH address: {}",
            amount,
            ctx.accounts.user.key(),
            bth_address
        );

        Ok(())
    }

    /// Pause the bridge (emergency only). Callable by the pauser multisig.
    pub fn pause(ctx: Context<PauserOnly>) -> Result<()> {
        ctx.accounts.bridge.paused = true;
        msg!("Bridge paused by {}", ctx.accounts.pauser_authority.key());
        Ok(())
    }

    /// Unpause the bridge. Callable by the pauser multisig.
    pub fn unpause(ctx: Context<PauserOnly>) -> Result<()> {
        ctx.accounts.bridge.paused = false;
        msg!("Bridge unpaused by {}", ctx.accounts.pauser_authority.key());
        Ok(())
    }

    /// Update the daily mint limit (picocredits). Admin multisig only.
    pub fn set_daily_limit(ctx: Context<AdminOnly>, new_limit: u64) -> Result<()> {
        ctx.accounts.bridge.daily_mint_limit = new_limit;
        msg!("Daily mint limit updated to {}", new_limit);
        Ok(())
    }

    /// Update the auto-pause breaker threshold (0 disables). Admin only.
    pub fn set_auto_pause_threshold(ctx: Context<AdminOnly>, new_threshold: u64) -> Result<()> {
        ctx.accounts.bridge.auto_pause_threshold = new_threshold;
        msg!("Auto-pause threshold updated to {}", new_threshold);
        Ok(())
    }

    /// Transfer the mint authority to a new multisig. Admin only.
    ///
    /// Mirrors the EVM role administration living under `DEFAULT_ADMIN_ROLE`:
    /// the mint quorum cannot rotate itself; the (distinct) governance quorum
    /// does.
    pub fn transfer_authority(ctx: Context<AdminOnly>, new_authority: Pubkey) -> Result<()> {
        ctx.accounts.bridge.mint_authority = new_authority;
        msg!("Mint authority transferred to {}", new_authority);
        Ok(())
    }

    /// Transfer the admin authority to a new multisig. Admin only.
    pub fn transfer_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.bridge.admin_authority = new_admin;
        msg!("Admin authority transferred to {}", new_admin);
        Ok(())
    }

    /// Transfer the pauser authority to a new multisig. Admin only.
    pub fn transfer_pauser(ctx: Context<AdminOnly>, new_pauser: Pubkey) -> Result<()> {
        ctx.accounts.bridge.pauser_authority = new_pauser;
        msg!("Pauser authority transferred to {}", new_pauser);
        Ok(())
    }
}

/// Bridge state account.
#[account]
pub struct Bridge {
    /// Validator multisig authorized to mint (ADR 0002).
    pub mint_authority: Pubkey,
    /// Governance multisig authorized to change limits and rotate roles.
    pub admin_authority: Pubkey,
    /// Guardian multisig authorized to pause / unpause.
    pub pauser_authority: Pubkey,
    /// The wBTH mint address.
    pub mint: Pubkey,
    /// PDA bump seed.
    pub bump: u8,
    /// Whether the bridge is paused.
    pub paused: bool,
    /// Daily mint limit in picocredits.
    pub daily_mint_limit: u64,
    /// Auto-pause circuit-breaker threshold in picocredits (0 disables).
    pub auto_pause_threshold: u64,
    /// Cumulative amount minted during the current UTC day.
    pub daily_minted: u64,
    /// UTC-day index (unix_timestamp / 86400) of the last reset.
    pub last_reset_day: i64,
}

impl Bridge {
    pub const LEN: usize = 8 + // discriminator
        32 + // mint_authority
        32 + // admin_authority
        32 + // pauser_authority
        32 + // mint
        1 +  // bump
        1 +  // paused
        8 +  // daily_mint_limit
        8 +  // auto_pause_threshold
        8 +  // daily_minted
        8; // last_reset_day
}

/// Per-order replay-guard marker. Created with `init` in `bridge_mint`; a
/// duplicate `order_id` fails because the PDA already exists (the Solana
/// analogue of the EVM `processedOrders` mapping).
#[account]
pub struct OrderMarker {
    /// The order id this marker records (for auditability).
    pub order_id: [u8; 32],
}

impl OrderMarker {
    pub const LEN: usize = 8 + // discriminator
        32; // order_id
}

// === Account Contexts ===

#[derive(Accounts)]
#[instruction(bump: u8)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = payer,
        space = Bridge::LEN,
        seeds = [b"bridge"],
        bump
    )]
    pub bridge: Account<'info, Bridge>,

    #[account(
        init,
        payer = payer,
        mint::decimals = 12, // Match BTH's 12 decimals (1 unit == 1 picocredit)
        mint::authority = bridge,
        mint::freeze_authority = bridge,
    )]
    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(amount: u64, order_id: [u8; 32])]
pub struct BridgeMint<'info> {
    #[account(
        mut,
        seeds = [b"bridge"],
        bump = bridge.bump,
        has_one = mint,
        has_one = mint_authority,
    )]
    pub bridge: Account<'info, Bridge>,

    /// Per-order replay guard. `init` fails if this order id was already
    /// minted — mirrors the EVM `processedOrders` check.
    #[account(
        init,
        payer = mint_authority,
        space = OrderMarker::LEN,
        seeds = [b"order", order_id.as_ref()],
        bump,
    )]
    pub order_marker: Account<'info, OrderMarker>,

    #[account(mut)]
    pub mint: Account<'info, Mint>,

    /// The recipient's associated token account. Constrained to
    /// `associated_token::authority = user`, so a mint cannot be redirected to
    /// an account the recipient does not own.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    /// CHECK: The recipient. Not a signer, but it is bound to
    /// `user_token_account` via the `associated_token::authority = user`
    /// constraint above, so mints can only land in the recipient's own ATA.
    pub user: UncheckedAccount<'info>,

    /// The validator multisig authorized to mint (ADR 0002). Must sign and
    /// equal `bridge.mint_authority`; the multisig program enforces the
    /// t-of-n threshold. Also the rent payer for the order-marker PDA.
    #[account(mut)]
    pub mint_authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BridgeBurn<'info> {
    #[account(
        seeds = [b"bridge"],
        bump = bridge.bump,
        has_one = mint,
    )]
    pub bridge: Account<'info, Bridge>,

    #[account(mut)]
    pub mint: Account<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(
        mut,
        seeds = [b"bridge"],
        bump = bridge.bump,
        has_one = admin_authority,
    )]
    pub bridge: Account<'info, Bridge>,

    /// Governance multisig. Must sign and equal `bridge.admin_authority`.
    pub admin_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct PauserOnly<'info> {
    #[account(
        mut,
        seeds = [b"bridge"],
        bump = bridge.bump,
        has_one = pauser_authority,
    )]
    pub bridge: Account<'info, Bridge>,

    /// Guardian multisig. Must sign and equal `bridge.pauser_authority`.
    pub pauser_authority: Signer<'info>,
}

// === Events ===

#[event]
pub struct BridgeMintEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub order_id: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct BridgeBurnEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub bth_address: String,
    pub timestamp: i64,
}

#[event]
pub struct AutoPausedEvent {
    pub daily_minted: u64,
    pub threshold: u64,
}

// === Errors ===

#[error_code]
pub enum BridgeError {
    #[msg("Bridge is paused")]
    Paused,

    #[msg("Exceeds maximum mint per transaction")]
    ExceedsMaxMint,

    #[msg("Daily mint limit exceeded")]
    DailyLimitExceeded,

    #[msg("Invalid BTH address")]
    InvalidBthAddress,

    #[msg("Amount must be greater than zero")]
    InvalidAmount,

    #[msg("Invalid order id")]
    InvalidOrderId,

    #[msg("Arithmetic overflow")]
    MathOverflow,
}
