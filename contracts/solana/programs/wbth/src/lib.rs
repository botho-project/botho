use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, MintTo, Token, TokenAccount};

declare_id!("wBTH111111111111111111111111111111111111111");

/// Wrapped BTH (wBTH) SPL token bridge program.
///
/// This program manages the wBTH token on Solana, allowing:
/// - Bridge operator to mint wBTH when BTH is deposited
/// - Users to burn wBTH to redeem BTH on the native chain
/// - Rate limiting and pause functionality for security
#[program]
pub mod wbth_bridge {
    use super::*;

    /// Initialize the bridge with a new wBTH mint.
    pub fn initialize(ctx: Context<Initialize>, bump: u8) -> Result<()> {
        let bridge = &mut ctx.accounts.bridge;
        bridge.authority = ctx.accounts.authority.key();
        bridge.mint = ctx.accounts.mint.key();
        bridge.bump = bump;
        bridge.paused = false;
        bridge.daily_mint_limit = 10_000_000 * 1_000_000_000_000; // 10M BTH in picocredits
        bridge.daily_minted = 0;
        bridge.last_reset_slot = Clock::get()?.slot;

        msg!("Bridge initialized with mint: {}", bridge.mint);
        Ok(())
    }

    /// Mint wBTH to a user when BTH is deposited to the bridge.
    ///
    /// Only callable by the bridge authority.
    pub fn bridge_mint(
        ctx: Context<BridgeMint>,
        amount: u64,
        bth_tx_hash: [u8; 32],
    ) -> Result<()> {
        let bridge = &mut ctx.accounts.bridge;

        require!(!bridge.paused, BridgeError::Paused);
        require!(
            amount <= 1_000_000 * 1_000_000_000_000,
            BridgeError::ExceedsMaxMint
        );

        // Reset daily limit if >24h since last reset (~216,000 slots at 400ms/slot)
        let clock = Clock::get()?;
        let slots_per_day: u64 = 216_000;
        if clock.slot > bridge.last_reset_slot + slots_per_day {
            bridge.daily_minted = 0;
            bridge.last_reset_slot = clock.slot;
        }

        require!(
            bridge.daily_minted.checked_add(amount).unwrap() <= bridge.daily_mint_limit,
            BridgeError::DailyLimitExceeded
        );
        bridge.daily_minted = bridge.daily_minted.checked_add(amount).unwrap();

        // Mint tokens using PDA signature
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
            bth_tx_hash,
            slot: clock.slot,
        });

        msg!(
            "Minted {} wBTH to {} (BTH tx: {})",
            amount,
            ctx.accounts.user.key(),
            hex::encode(&bth_tx_hash[..8])
        );

        Ok(())
    }

    /// Burn wBTH to redeem BTH on the native chain.
    ///
    /// The bridge service monitors these events and releases BTH.
    pub fn bridge_burn(
        ctx: Context<BridgeBurn>,
        amount: u64,
        bth_address: String,
    ) -> Result<()> {
        let bridge = &ctx.accounts.bridge;

        require!(!bridge.paused, BridgeError::Paused);
        require!(amount > 0, BridgeError::InvalidAmount);
        require!(!bth_address.is_empty(), BridgeError::InvalidBthAddress);

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
            slot: Clock::get()?.slot,
        });

        msg!(
            "Burned {} wBTH from {} to BTH address: {}",
            amount,
            ctx.accounts.user.key(),
            bth_address
        );

        Ok(())
    }

    /// Pause the bridge (emergency only).
    pub fn pause(ctx: Context<AdminOnly>) -> Result<()> {
        ctx.accounts.bridge.paused = true;
        msg!("Bridge paused by {}", ctx.accounts.authority.key());
        Ok(())
    }

    /// Unpause the bridge.
    pub fn unpause(ctx: Context<AdminOnly>) -> Result<()> {
        ctx.accounts.bridge.paused = false;
        msg!("Bridge unpaused by {}", ctx.accounts.authority.key());
        Ok(())
    }

    /// Update the daily mint limit.
    pub fn set_daily_limit(ctx: Context<AdminOnly>, new_limit: u64) -> Result<()> {
        ctx.accounts.bridge.daily_mint_limit = new_limit;
        msg!("Daily mint limit updated to {}", new_limit);
        Ok(())
    }

    /// Transfer bridge authority to a new account.
    pub fn transfer_authority(ctx: Context<AdminOnly>, new_authority: Pubkey) -> Result<()> {
        ctx.accounts.bridge.authority = new_authority;
        msg!("Bridge authority transferred to {}", new_authority);
        Ok(())
    }
}

/// Bridge state account.
#[account]
pub struct Bridge {
    /// The authority that can mint and manage the bridge
    pub authority: Pubkey,
    /// The wBTH mint address
    pub mint: Pubkey,
    /// PDA bump seed
    pub bump: u8,
    /// Whether the bridge is paused
    pub paused: bool,
    /// Daily mint limit in picocredits
    pub daily_mint_limit: u64,
    /// Amount minted today
    pub daily_minted: u64,
    /// Slot when daily limit was last reset
    pub last_reset_slot: u64,
}

impl Bridge {
    pub const LEN: usize = 8 + // discriminator
        32 + // authority
        32 + // mint
        1 +  // bump
        1 +  // paused
        8 +  // daily_mint_limit
        8 +  // daily_minted
        8; // last_reset_slot
}

// === Account Contexts ===

#[derive(Accounts)]
#[instruction(bump: u8)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = Bridge::LEN,
        seeds = [b"bridge"],
        bump
    )]
    pub bridge: Account<'info, Bridge>,

    #[account(
        init,
        payer = authority,
        mint::decimals = 12, // Match BTH's 12 decimals
        mint::authority = bridge,
        mint::freeze_authority = bridge,
    )]
    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct BridgeMint<'info> {
    #[account(
        mut,
        seeds = [b"bridge"],
        bump = bridge.bump,
        has_one = authority,
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

    /// CHECK: Just used for event emission
    pub user: UncheckedAccount<'info>,

    pub authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
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
        has_one = authority,
    )]
    pub bridge: Account<'info, Bridge>,

    pub authority: Signer<'info>,
}

// === Events ===

#[event]
pub struct BridgeMintEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub bth_tx_hash: [u8; 32],
    pub slot: u64,
}

#[event]
pub struct BridgeBurnEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub bth_address: String,
    pub slot: u64,
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
}
