use anchor_lang::prelude::*;
use anchor_spl::token::Mint;

pub mod states;
pub mod math;
pub mod error;
pub use states::*;

declare_id!("FLASH6Lo6h3iasJKWDs2F8TkW2UKf3s15C8PMGuVfgBn");

#[program]
pub mod flash_read {
    use super::*;

    pub fn get_lp_token_price(
        _ctx: Context<GetLpTokenPrice>,
    ) -> Result<u64> {
        // We only need the interface, not the actual implementation here.
        unimplemented!("Just an interface")
    }
}

#[derive(Accounts)]
pub struct GetLpTokenPrice<'info> {
    #[account(
        seeds = [b"perpetuals"],
        bump = perpetuals.perpetuals_bump,
    )]
    pub perpetuals: Box<Account<'info, Perpetuals>>,

    #[account(
        seeds = [b"pool",
                 pool.name.as_bytes()],
        bump = pool.bump
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [b"lp_token_mint",
                 pool.key().as_ref()],
        bump = pool.lp_mint_bump
    )]
    pub lp_token_mint: Box<Account<'info, Mint>>,

    // remaining accounts:
    //   pool.custodies.len() custody accounts (read-only, unsigned)
    //   pool.custodies.len() custody oracles (read-only, unsigned)
    //   pool.markets.len() market accounts (read-only, unsigned)
}
