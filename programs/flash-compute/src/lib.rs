use anchor_lang::prelude::*;
use solana_program::pubkey;
use anchor_spl::token::Mint;
use flash_read::states::*;
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use flash_read::math;


declare_id!("Fcmp5ZQ1wR5swZ87aRQyHfUiHYxrfrRVhCWrV2yYA6QG");

#[cfg(feature = "mainnet")]
pub const FLASH_PROGRAM: Pubkey = pubkey!("FLASH6Lo6h3iasJKWDs2F8TkW2UKf3s15C8PMGuVfgBn");

#[cfg(not(feature = "mainnet"))]
pub const FLASH_PROGRAM: Pubkey = pubkey!("FTN6rgbaaxwT8mpRuC55EFTwpHB3BwnHJ91Lqv4ZVCfW");

#[program]
pub mod flash_compute {
    use super::*;

    pub fn get_pool_token_prices(
        ctx: Context<GetPoolTokenPrices>,
    ) -> Result<(u64, u64)> {
        let pool = &ctx.accounts.pool;
        let mut custody_details: Box<Vec<CustodyDetails>> = Box::new(Vec::new());
        let mut pool_equity: u128 = 0;

        // Computing the raw AUM of the pool
        for (idx, &custody) in pool.custodies.iter().enumerate() {

            require_keys_eq!(ctx.remaining_accounts[idx].key(), custody);
            let custody = Box::new(Account::<Custody>::try_from(&ctx.remaining_accounts[idx])?);
            let oracle_idx = idx + pool.custodies.len();  
            if oracle_idx >= ctx.remaining_accounts.len() {
                return Err(ProgramError::NotEnoughAccountKeys.into());
            }
            require_keys_eq!(ctx.remaining_accounts[oracle_idx].key(), custody.oracle.ext_oracle_account);

            let pyth_price = Account::<PriceUpdateV2>::try_from(&ctx.remaining_accounts[oracle_idx])?;

            custody_details.push(CustodyDetails { 
                trade_spread_min: custody.pricing.trade_spread_min,
                trade_spread_max: custody.pricing.trade_spread_max,
                delay_seconds: custody.pricing.delay_seconds,
                min_price: OraclePrice {
                    price: pyth_price.price_message.price as u64,
                    exponent: pyth_price.price_message.exponent as i32,
                },
                max_price: OraclePrice {
                    price: pyth_price.price_message.price as u64,
                    exponent: pyth_price.price_message.exponent as i32,
                },
            });

            let token_amount_usd =
                custody_details[idx].min_price.get_asset_amount_usd(custody.assets.owned, custody.decimals)?;
            pool_equity = math::checked_add(pool_equity, token_amount_usd as u128)?;

        }

        // Computing the unrealsied PnL pending against the pool

        
        for (idx, &market) in pool.markets.iter().enumerate() {
            require_keys_eq!(ctx.remaining_accounts[(pool.custodies.len() * 2) + idx].key(), market);
            let market = Box::new(Account::<Market>::try_from(&ctx.remaining_accounts[(pool.custodies.len() * 2) + idx])?);
            // Get the collective position against the pool
            let position = Box::new(market.get_collective_position()?);
            if market.side == Side::Short {
                let exit_price = OraclePrice {
                    price: math::checked_add(
                        custody_details[market.target_custody_id].max_price.price,
                        math::checked_decimal_ceil_mul(
                            custody_details[market.target_custody_id].max_price.price,
                            custody_details[market.target_custody_id].max_price.exponent,
                            custody_details[market.target_custody_id].trade_spread_max,
                            -6, // Spread is in 100th of a bip
                            custody_details[market.target_custody_id].max_price.exponent,
                        )?,
                    )?,
                    exponent: custody_details[market.target_custody_id].max_price.exponent,
                };
                pool_equity = if exit_price < position.entry_price {
                    // Shorts are in collective profit
                     pool_equity.saturating_sub(std::cmp::min(
                        position.entry_price.checked_sub(&exit_price)?.get_asset_amount_usd(position.size_amount, position.size_decimals)? as u128,
                        custody_details[market.collateral_custody_id].min_price.get_asset_amount_usd(position.locked_amount, position.locked_decimals)? as u128
                    ))
                } else {
                    // Shorts are in collective loss
                    pool_equity.checked_add(std::cmp::min(
                        exit_price.checked_sub(&position.entry_price)?.get_asset_amount_usd(position.size_amount, position.size_decimals)? as u128,
                        custody_details[market.collateral_custody_id].min_price.get_asset_amount_usd(position.collateral_amount, position.collateral_decimals)? as u128
                    )).unwrap()
                };
            } else {
                let spread = math::checked_decimal_mul(
                    custody_details[market.target_custody_id].min_price.price,
                    custody_details[market.target_custody_id].min_price.exponent,
                    custody_details[market.target_custody_id].trade_spread_min,
                    -6, // Spread is in 100th of a bip
                    custody_details[market.target_custody_id].min_price.exponent,
                )?;
    
                let price = if spread < custody_details[market.target_custody_id].min_price.price {
                    math::checked_sub(custody_details[market.target_custody_id].min_price.price, spread)?
                } else {
                    0
                };

                let exit_price = OraclePrice {
                    price,
                    exponent: custody_details[market.target_custody_id].min_price.exponent,
                };

                pool_equity = if exit_price > position.entry_price {
                    // Longs are in collective profit
                    pool_equity.saturating_sub(std::cmp::min(
                        exit_price.checked_sub(&position.entry_price)?.get_asset_amount_usd(position.size_amount, position.size_decimals)? as u128,
                        custody_details[market.collateral_custody_id].min_price.get_asset_amount_usd(position.locked_amount, position.locked_decimals)? as u128
                    ))
                } else {
                    // Longs are in collective loss
                    pool_equity.checked_add(std::cmp::min(
                        position.entry_price.checked_sub(&exit_price)?.get_asset_amount_usd(position.size_amount, position.size_decimals)? as u128,
                        custody_details[market.collateral_custody_id].min_price.get_asset_amount_usd(position.collateral_amount, position.collateral_decimals)? as u128
                    )).unwrap()
                };

            };
        }

        let lp_supply = ctx.accounts.lp_token_mint.supply;

        let sflp_price_usd = math::checked_decimal_div(
            math::checked_as_u64(pool_equity)?,
            -(Perpetuals::USD_DECIMALS as i32),
            lp_supply,
            -(Perpetuals::LP_DECIMALS as i32),
            -(Perpetuals::USD_DECIMALS as i32),
        )?;

        let compounding_factor = math::checked_decimal_div(
            pool.compounding_stats.active_amount, 
            -(Perpetuals::LP_DECIMALS as i32), 
            pool.compounding_stats.total_supply,
            -(Perpetuals::LP_DECIMALS as i32),
            -(Perpetuals::LP_DECIMALS as i32),
        )?;

        let flp_price = math::checked_decimal_mul(
            sflp_price_usd,
            -(Perpetuals::USD_DECIMALS as i32),
            compounding_factor,
            -(Perpetuals::LP_DECIMALS as i32),
            -(Perpetuals::USD_DECIMALS as i32),
        )?;

        msg!("SFLP Price: {}, FLP Price: {}", sflp_price_usd, flp_price);

        Ok((sflp_price_usd, flp_price))
    }

    pub fn get_liquidation_price(
        ctx: Context<GetLiquidationPrice>,
    ) -> Result<OraclePrice> {
        let position = &ctx.accounts.position;
        let pool = &ctx.accounts.pool;
        let market = &ctx.accounts.market;
        let target_custody = &ctx.accounts.target_custody;
        let collateral_custody = &ctx.accounts.collateral_custody;
        
        let pyth_price = Account::<PriceUpdateV2>::try_from(&ctx.accounts.collateral_oracle_account)?;

        let collateral_price = OraclePrice {
            price: pyth_price.price_message.price as u64,
            exponent: pyth_price.price_message.exponent as i32,
        };
    
        let liabilities_usd = math::checked_add(
            math::checked_add(
                pool.get_fee_amount(position.size_usd, target_custody.fees.close_position)?,
                collateral_custody.get_lock_fee_usd(position, solana_program::sysvar::clock::Clock::get()?.unix_timestamp)?
            )?,
            math::checked_add(
                position.unsettled_fees_usd,
                math::checked_as_u64(math::checked_div(
                    math::checked_mul(position.size_usd as u128, Perpetuals::BPS_POWER)?,
                    target_custody.pricing.max_leverage as u128,
                )?)?
            )?,
        )?;

        if market.correlation && market.side == Side::Long {
                // For Correlated Long Markets, if notional size value is assumed to correspond to liabilities then current size vlaue corresponds to assets 
                // Liq Price = (size_usd + liabilities_usd) / (size_amount + collateral_amount) subject to constraints
                let liq_price = OraclePrice::new(
                    math::checked_as_u64(math::checked_div(
                        math::checked_mul(
                            math::checked_add(position.size_usd, liabilities_usd)? as u128,
                            math::checked_pow(10_u128, (position.size_decimals + 3) as usize)?, // USD to Rate decimals for granularity 
                        )?,
                        math::checked_add(position.size_amount, position.collateral_amount)? as u128,
                    )?)?,
                    -(Perpetuals::RATE_DECIMALS as i32),
                );
                Ok(liq_price.scale_to_exponent(position.entry_price.exponent)?)
        //  } else if market.correlation && market.side == Side::Short {
                // Invalid combination of market side and correlation as anti-correlated short markets do not exist 
        } else {
            // For uncorrelated markets, assume assets_usd corresponding to collateral value to be constant
            let assets_usd = collateral_price.get_asset_amount_usd(position.collateral_amount, position.collateral_decimals)?;

            if assets_usd >= liabilities_usd {
                // Position is nominally solvent and shall be liqudaited in case of loss
                let mut price_diff_loss = OraclePrice::new(
                    math::checked_as_u64(math::checked_div(
                        math::checked_mul(
                            math::checked_sub(assets_usd, liabilities_usd)? as u128,
                            math::checked_pow(10_u128, (position.size_decimals + 3) as usize)?,
                        )?,
                        position.size_amount as u128,
                    )?)?,
                    -(Perpetuals::RATE_DECIMALS as i32),
                ).scale_to_exponent(position.entry_price.exponent)?;
                if market.side == Side::Long {
                    // For Longs, loss implies price drop
                    price_diff_loss.price = position.entry_price.price.saturating_sub(price_diff_loss.price);
                } else {
                    // For Shorts, loss implies price rise
                    price_diff_loss.price = position.entry_price.price.saturating_add(price_diff_loss.price);
                }
                Ok(price_diff_loss)
            } else {
                // Position is nominally insolvent and shall be liqudaited with profit to cover outstanding liabilities
                let mut price_diff_profit = OraclePrice::new(
                    math::checked_as_u64(math::checked_div(
                        math::checked_mul(
                            math::checked_sub(liabilities_usd, assets_usd)? as u128,
                            math::checked_pow(10_u128, (position.size_decimals + 3) as usize)?,
                        )?,
                        position.size_amount as u128,
                    )?)?,
                    -(Perpetuals::RATE_DECIMALS as i32),
                ).scale_to_exponent(position.entry_price.exponent)?;
                if market.side == Side::Long {
                    // For Longs, profit implies price rise
                    price_diff_profit.price = position.entry_price.price.saturating_add(price_diff_profit.price);
                } else {
                    // For Shorts, profit implies price drop
                    price_diff_profit.price = position.entry_price.price.saturating_sub(price_diff_profit.price);
                }
                Ok(price_diff_profit)
            }
        }
    }
}

#[derive(Accounts)]
pub struct GetPoolTokenPrices<'info> {
    #[account(
        seeds = [b"perpetuals"],
        bump = perpetuals.perpetuals_bump,
        seeds::program = FLASH_PROGRAM,
    )]
    pub perpetuals: Box<Account<'info, Perpetuals>>,

    #[account(
        seeds = [b"pool",
                 pool.name.as_bytes()],
        bump = pool.bump,
        seeds::program = FLASH_PROGRAM,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [b"lp_token_mint",
                 pool.key().as_ref()],
        bump = pool.lp_mint_bump,
        seeds::program = FLASH_PROGRAM,
    )]
    pub lp_token_mint: Box<Account<'info, Mint>>,

    // remaining accounts:
    //   pool.custodies.len() custody accounts (read-only, unsigned)
    //   pool.custodies.len() custody oracles (read-only, unsigned)
    //   pool.markets.len() market accounts (read-only, unsigned)
}

#[derive(Accounts)]
pub struct GetLiquidationPrice<'info> {
    #[account(
        seeds = [b"perpetuals"],
        bump = perpetuals.perpetuals_bump
    )]
    pub perpetuals: Box<Account<'info, Perpetuals>>,

    #[account(
        seeds = [b"pool",
                 pool.name.as_bytes()],
        bump = pool.bump
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(
        seeds = [b"position",
                 position.owner.as_ref(),
                 market.key().as_ref()],
        bump = position.bump
    )]
    pub position: Box<Account<'info, Position>>,

    #[account(
        seeds = [b"market",
                 target_custody.key().as_ref(),
                 collateral_custody.key().as_ref(),
                 &[market.side as u8]],
        bump = market.bump
    )]
    pub market: Box<Account<'info, Market>>,

    #[account(
        seeds = [b"custody",
                 pool.key().as_ref(),
                 target_custody.mint.key().as_ref()],
        bump = target_custody.bump
    )]
    pub target_custody: Box<Account<'info, Custody>>,

    #[account(
        seeds = [b"custody",
                 pool.key().as_ref(),
                 collateral_custody.mint.key().as_ref()],
        bump = collateral_custody.bump,
    )]
    pub collateral_custody: Box<Account<'info, Custody>>,

    /// CHECK: oracle account for the collateral token
    #[account(
        constraint = collateral_oracle_account.key() == collateral_custody.oracle.ext_oracle_account 
    )]
    pub collateral_oracle_account: AccountInfo<'info>
}
