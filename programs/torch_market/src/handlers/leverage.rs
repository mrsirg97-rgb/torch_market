//! [V21] Per-token closed leveraged markets — handlers.
//!
//! Six handlers (open/close/liquidate × short/long), each routing through
//! deep_pool::swap in a single CPI so open/close are atomic. Per-position
//! custody (D-2): the held asset IS the position vault balance, never tracked
//! state. Mirrors `sim/torch_sim.py` (the economic spec) one-to-one.
//!
//! deep_pool's `Swap` declares BOTH `user` and `sol_source` as `Signer`, so
//! every PDA that fills those slots signs via `CpiContext::new_with_signer`:
//!   - short open/close: user = treasury_lock (lock ATA authority),
//!     sol_source = position_sol_vault.
//!   - long  open/close: user = position (its ATA), sol_source = long_sol_vault.

use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::token_interface::{transfer_checked, TransferChecked};

use crate::constants::*;
use crate::contexts::{
    CloseLongPosition, CloseShortPosition, LiquidateLongPosition, LiquidateShortPosition,
    OpenLongPosition, OpenShortPosition,
};
use crate::errors::TorchMarketError;
use crate::math::{
    apply_bps, apply_interest_accrual, apply_short_interest_accrual,
    calc_close_pool_amount_in, calc_collateral_value, calc_ltv_bps,
    calc_short_debt_value, calc_short_partial_seize_proration, calc_short_sol_to_seize,
    calc_sol_to_token_value, effective_liq_bonus_bps,
    gross_up_for_transfer_fee, twap_tokens_to_seize, twap_value_in_sol,
};
use crate::pool_validation::{
    get_depth_max_ltv_bps, max_debt_value_for_depth, read_deep_pool_reserves, treasury_physical_sol,
    vault_physical_sol,
};
use crate::state::PositionSide;

// Manual conditional close of a program-owned data account: drain its lamports
// to `destination`, reassign to the system program, and shrink to 0 so Anchor's
// exit doesn't re-serialize it. Used for full position close (partial close
// leaves the account open), where Anchor's unconditional `close =` can't be used.
fn close_account(account: &AccountInfo, destination: &AccountInfo) -> Result<()> {
    let dest_start = destination.lamports();
    **destination.try_borrow_mut_lamports()? = dest_start
        .checked_add(account.lamports())
        .ok_or(TorchMarketError::MathOverflow)?;
    **account.try_borrow_mut_lamports()? = 0;
    account.assign(&system_program::ID);
    account.resize(0)?;
    Ok(())
}


// [F-1] Borrow the UserRisk loader for mutation. A fresh init_if_needed
// account is claimed via load_init (writes the zero-copy discriminator); an
// existing account falls through to load_mut. Scope every borrow tightly —
// a RefMut held across a CPI would alias the account buffer.
fn user_risk_mut<'a, 'info>(
    loader: &'a AccountLoader<'info, crate::state::UserRisk>,
) -> Result<std::cell::RefMut<'a, crate::state::UserRisk>> {
    match loader.load_init() {
        Ok(risk) => Ok(risk),
        Err(_) => loader.load_mut(),
    }
}

// ============================================================================
// [V21][D-10] TWAP mark — read from DeepPool (oracle lives there now).
//
// DeepPool owns the keeperless oracle (it accumulates a Q64.64 price on every
// swap; docs/twap-oracle.md). Torch reads it at liquidation time: deserialize the
// `deep_pool::Pool` already present in the liquidation context and call its read
// method with the CURRENT reserves (for the lazy head-extension to `now`) over
// torch's own `LIQ_TWAP_LOOKBACK_SLOTS`. Returns a Q64.64 SOL-per-token price, or
// `None` if the pool's ring is younger than the lookback (warmup) → the caller
// fails closed (refuses to liquidate). No CPI, no crank, no torch-side ring.
//
// Safety: the pool account is address-constrained in the context (the exact PDA
// under DEEP_POOL_PROGRAM_ID), and `try_deserialize` checks the Anchor
// discriminator — so the bytes are an authentic deep_pool::Pool, not attacker
// data. The reserves are read the same way every other torch path reads them
// (PDA lamports − rent, vault balance), so the "now" price the extension uses is
// the real spot, matching what DeepPool itself would record on the next swap.
fn read_twap_price_q64(
    deep_pool: &AccountInfo,
    deep_pool_token_vault: &AccountInfo,
    now: u64,
) -> Result<Option<u128>> {
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(deep_pool, deep_pool_token_vault)?;
    let data = deep_pool.try_borrow_data()?;
    let pool = deep_pool::Pool::try_deserialize(&mut &data[..])
        .map_err(|_| TorchMarketError::InvalidPoolAccount)?;
    Ok(pool.read_twap_sol_per_tok(pool_sol, pool_tokens, now, LIQ_TWAP_LOOKBACK_SLOTS))
}

// ============================================================================
// open_short (D-4) — atomic custodied short.
//
// Collateral SOL in → 0.5% open fee to treasury → net collateral to the
// position SOL vault → atomically borrow tokens from the lock, sell them on
// deep_pool, sale proceeds land in the same vault. Debt = gross tokens borrowed
// (owed back to the lock on close). No tokens ever reach the user's wallet.
// ============================================================================
pub fn open_short(ctx: Context<OpenShortPosition>, args: crate::contexts::OpenPositionArgs) -> Result<()> {
    let collateral = args.collateral;
    require!(collateral > 0, TorchMarketError::EmptyBorrowRequest);

    // Depth-band LTV cap, intersected with the per-token max (D-4 step 1).
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(ctx.accounts.treasury.max_ltv_bps);

    // Oracle: the deep_pool swap this open triggers advances DeepPool's TWAP —
    // torch records nothing (the oracle lives there now). See docs/twap-oracle.md.

    // Borrow plan at pre-swap price (D-4 step 2), all clamps applied BEFORE the
    // fee so the fee prices the capacity actually consumed (D-9 / F-5).
    let borrow_value_sol = apply_bps(collateral, effective_max_ltv)
        .ok_or(TorchMarketError::MathOverflow)?
        // [V21] Rail 2 size cap: SOL-debt-value ≤ ρ_max of pool depth (clamp, not
        // reject — mirrors the lock/per-user caps; keeps unwind slippage bounded).
        .min(max_debt_value_for_depth(pool_sol));
    let mut tokens_to_borrow = calc_sol_to_token_value(borrow_value_sol, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;

    // Cap by the physical lock balance and the per-user wallet cap (D-4
    // step 1). The lock ATA balance IS the lendable amount — tokens already
    // lent have physically left it, and the lock may drain to zero by design
    // (closes/liquidations only pay INTO it; the real aggregate brake is pool
    // depth — every short open drains pool SOL, shrinking rail-2 and the
    // depth-LTV curve until PoolTooThin stops new opens).
    let lock_available = ctx.accounts.treasury_lock_token_account.amount;
    // [F-1] Per-USER wallet cap: the remaining allowance across ALL of the
    // owner's open shorts (any position_index), not per position — without
    // the aggregate, each new index re-granted the full 2% cap.
    let user_short_remaining = {
        let risk = user_risk_mut(&ctx.accounts.user_risk)?;
        MAX_WALLET_TOKENS.saturating_sub(risk.short_tokens_debt)
    };
    tokens_to_borrow = tokens_to_borrow
        .min(lock_available)
        .min(user_short_remaining);
    require!(tokens_to_borrow > 0, TorchMarketError::ShortTooSmall);
    require!(tokens_to_borrow >= MIN_SHORT_TOKENS, TorchMarketError::ShortTooSmall);

    // [F-5] Open fee on the REALIZED borrow value — the SOL value of the
    // clamped token borrow at the pre-swap price — so a Rail-2/lock/wallet-
    // clamped short pays in proportion to what it actually borrows, exactly
    // like the long side's post-clamp fee. realized ≤ collateral × LTV, so
    // the fee can never exceed the posted collateral.
    let realized_borrow_value = calc_collateral_value(tokens_to_borrow, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let open_fee = apply_bps(realized_borrow_value, OPEN_FEE_BPS)
        .ok_or(TorchMarketError::MathOverflow)?;
    let net_collateral = collateral
        .checked_sub(open_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    // ---- Transfer 1: open fee shorter → treasury_sol_vault (unified custody) ----
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.shorter.to_account_info(),
                to: ctx.accounts.treasury_sol_vault.to_account_info(),
            },
        ),
        open_fee,
    )?;

    // ---- Transfer 2: net collateral shorter → position SOL vault ----
    system_program::transfer(
        CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.shorter.to_account_info(),
                to: ctx.accounts.position_sol_vault.to_account_info(),
            },
        ),
        net_collateral,
    )?;

    // ---- Atomic sell: lock ATA → pool, SOL proceeds → position SOL vault ----
    // user = treasury_lock (authority of the lock ATA = token source);
    // sol_source = position_sol_vault (SOL sink). Both are PDAs → both sign.
    let mint_key = ctx.accounts.mint.key();
    let shorter_key = ctx.accounts.shorter.key();
    let lock_seeds: &[&[u8]] = &[
        TREASURY_LOCK_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury_lock.bump],
    ];
    let vault_bump = ctx.bumps.position_sol_vault;
    let idx_le = args.position_index.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        shorter_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[vault_bump],
    ];
    let cpi_signers = &[lock_seeds, vault_seeds][..];

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.treasury_lock.to_account_info(),
        sol_source: ctx.accounts.position_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.treasury_lock_token_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: tokens_to_borrow,
            minimum_out: args.min_out, // slippage guard: min SOL out from the sale
            buy: false,
        },
    )?;

    // ---- Persist position + aggregate counters (D-4 steps 6–7) ----
    let position = &mut ctx.accounts.position;
    position.user = ctx.accounts.shorter.key();
    position.mint = mint_key;
    position.side = PositionSide::Short;
    position.position_index = args.position_index;
    position.collateral_amount = net_collateral; // record-keeping (post-fee)
    position.debt_amount = tokens_to_borrow; // gross tokens owed to the lock
    position.accrued_interest = 0;
    position.last_slot = Clock::get()?.slot;
    position.bump = ctx.bumps.position;
    position.vault_bump = vault_bump;

    let treasury = &mut ctx.accounts.treasury;
    treasury.total_tokens_lent = treasury
        .total_tokens_lent
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.active_shorts = treasury
        .active_shorts
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    // [F-1] Aggregate per-user exposure (owner/mint/bump idempotent — zeroed
    // on a fresh init_if_needed, rewritten cheaply otherwise).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.owner = shorter_key;
        risk.mint = mint_key;
        risk.bump = ctx.bumps.user_risk;
        risk.short_tokens_debt = risk
            .short_tokens_debt
            .checked_add(tokens_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit_cpi!(OpenShortEvent {
        user: position.user,
        mint: mint_key,
        position_index: args.position_index,
        collateral_sol_gross: collateral,
        open_fee_sol: open_fee,
        net_collateral_sol: net_collateral,
        tokens_borrowed: tokens_to_borrow,
        vault_sol: ctx.accounts.position_sol_vault.lamports(),
    });
    Ok(())
}

#[event]
pub struct OpenShortEvent {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub collateral_sol_gross: u64,
    pub open_fee_sol: u64,
    pub net_collateral_sol: u64,
    pub tokens_borrowed: u64,
    pub vault_sol: u64,
}

// ============================================================================
// open_short_via_vault (D-12) — open_short funded by a TorchVault.
//
// Identical economics to open_short; the only differences are the funding edges:
// collateral comes from the vault's System-owned vault_sol (seed-signed transfers,
// not a wallet), the position is VAULT-SEEDED (owned by the vault), and the signer
// is a linked wallet that only pays the position rent. See docs D-12 + the vault
// security model: a linked wallet can trade vault funds (vault → position → vault)
// but can never extract them.
// ============================================================================
pub fn open_short_via_vault(
    ctx: Context<crate::contexts::OpenShortViaVault>,
    args: crate::contexts::OpenPositionArgs,
) -> Result<()> {
    let collateral = args.collateral;
    require!(collateral > 0, TorchMarketError::EmptyBorrowRequest);
    // Vault funds the collateral (derived balance — vault_sol lamports − rent).
    require!(
        vault_physical_sol(&ctx.accounts.vault_sol)? >= collateral,
        TorchMarketError::InsufficientVaultBalance
    );

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(ctx.accounts.treasury.max_ltv_bps);

    // Borrow plan at pre-swap price (identical to open_short): clamps first,
    // then the fee on the REALIZED borrow value (F-4/F-5 — see open_short).
    let borrow_value_sol = apply_bps(collateral, effective_max_ltv)
        .ok_or(TorchMarketError::MathOverflow)?
        // [V21] Rail 2 size cap: SOL-debt-value ≤ ρ_max of pool depth (clamp, not
        // reject — mirrors the lock/per-user caps; keeps unwind slippage bounded).
        .min(max_debt_value_for_depth(pool_sol));
    let mut tokens_to_borrow = calc_sol_to_token_value(borrow_value_sol, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let lock_available = ctx.accounts.treasury_lock_token_account.amount;
    // [F-1] Per-USER (here: per-vault) aggregate wallet cap — see open_short.
    let user_short_remaining = {
        let risk = user_risk_mut(&ctx.accounts.user_risk)?;
        MAX_WALLET_TOKENS.saturating_sub(risk.short_tokens_debt)
    };
    tokens_to_borrow = tokens_to_borrow
        .min(lock_available)
        .min(user_short_remaining);
    require!(tokens_to_borrow > 0, TorchMarketError::ShortTooSmall);
    require!(tokens_to_borrow >= MIN_SHORT_TOKENS, TorchMarketError::ShortTooSmall);

    let realized_borrow_value = calc_collateral_value(tokens_to_borrow, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let open_fee = apply_bps(realized_borrow_value, OPEN_FEE_BPS)
        .ok_or(TorchMarketError::MathOverflow)?;
    let net_collateral = collateral
        .checked_sub(open_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let creator_key = ctx.accounts.torch_vault.creator;
    let vsol_bump = ctx.bumps.vault_sol;
    let vsol_seeds: &[&[u8]] = &[TORCH_VAULT_SOL_SEED, creator_key.as_ref(), &[vsol_bump]];

    // ---- Transfer 1: open fee vault_sol → treasury_sol_vault (seed-signed) ----
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.vault_sol.to_account_info(),
                to: ctx.accounts.treasury_sol_vault.to_account_info(),
            },
            &[vsol_seeds],
        ),
        open_fee,
    )?;
    // ---- Transfer 2: net collateral vault_sol → position SOL vault (seed-signed) ----
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.vault_sol.to_account_info(),
                to: ctx.accounts.position_sol_vault.to_account_info(),
            },
            &[vsol_seeds],
        ),
        net_collateral,
    )?;

    // ---- Atomic sell: lock ATA → pool, SOL proceeds → position SOL vault ----
    let lock_seeds: &[&[u8]] = &[
        TREASURY_LOCK_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury_lock.bump],
    ];
    let pos_vault_bump = ctx.bumps.position_sol_vault;
    let idx_le = args.position_index.to_le_bytes();
    // VAULT-SEEDED position vault: keyed by torch_vault, not a wallet.
    let pos_vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[pos_vault_bump],
    ];
    let cpi_signers = &[lock_seeds, pos_vault_seeds][..];

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.treasury_lock.to_account_info(),
        sol_source: ctx.accounts.position_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.treasury_lock_token_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: tokens_to_borrow,
            minimum_out: args.min_out,
            buy: false,
        },
    )?;

    // ---- Persist position (owner = the vault) + counters ----
    let position = &mut ctx.accounts.position;
    position.user = vault_key; // the vault owns the position
    position.mint = mint_key;
    position.side = PositionSide::Short;
    position.position_index = args.position_index;
    position.collateral_amount = net_collateral;
    position.debt_amount = tokens_to_borrow;
    position.accrued_interest = 0;
    position.last_slot = Clock::get()?.slot;
    position.bump = ctx.bumps.position;
    position.vault_bump = pos_vault_bump;

    let treasury = &mut ctx.accounts.treasury;
    treasury.total_tokens_lent = treasury
        .total_tokens_lent
        .checked_add(tokens_to_borrow)
        .ok_or(TorchMarketError::MathOverflow)?;
    treasury.active_shorts = treasury
        .active_shorts
        .checked_add(1)
        .ok_or(TorchMarketError::MathOverflow)?;

    // [F-1] Aggregate per-vault exposure.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.owner = vault_key;
        risk.mint = mint_key;
        risk.bump = ctx.bumps.user_risk;
        risk.short_tokens_debt = risk
            .short_tokens_debt
            .checked_add(tokens_to_borrow)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    emit_cpi!(OpenShortEvent {
        user: vault_key,
        mint: mint_key,
        position_index: args.position_index,
        collateral_sol_gross: collateral,
        open_fee_sol: open_fee,
        net_collateral_sol: net_collateral,
        tokens_borrowed: tokens_to_borrow,
        vault_sol: ctx.accounts.position_sol_vault.lamports(),
    });
    Ok(())
}

// ============================================================================
// close_short (D-5) — atomic pool-routed close, exits in SOL.
//
// Vault SOL buys exactly enough tokens (grossed up for the Token-2022 fee on the
// pool→lock leg) to repay the borrow, bought tokens land back in the lock. Any
// leftover vault SOL is the user's PnL surplus. Asymmetric vs close_long
// (compute-exact buy here vs sell-all there) but both exit in SOL. Partial close
// (`repay_fraction_bps` < 10000) scales the repay + swap and leaves the position
// open; full close drains the surplus and closes the position + vault.
// ============================================================================
pub fn close_short(ctx: Context<CloseShortPosition>, args: crate::contexts::ClosePositionArgs) -> Result<()> {
    // ---- Accrue interest, size the repay (D-5 steps 1–3) ----
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_short_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }

    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveShort);
    let debt_to_repay = apply_bps(total_debt, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_repay > 0, TorchMarketError::ZeroAmount);

    // Gross up so the lock receives net == debt_to_repay after the transfer fee
    // on the pool→lock leg (D-5 step 3 — V20 lock-conservation invariant).
    let debt_gross = gross_up_for_transfer_fee(debt_to_repay)
        .ok_or(TorchMarketError::MathOverflow)?;

    // Exact SOL the vault must spend to buy debt_gross tokens, incl. deep_pool's
    // input-side swap fee (D-5 step 4). Inverse of deep_pool's forward buy.
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let sol_needed = calc_close_pool_amount_in(
        debt_gross,
        pool_sol,
        pool_tokens,
        deep_pool::constants::SWAP_FEE_BPS as u16,
    )
    .ok_or(TorchMarketError::MathOverflow)?;

    // Oracle: the deep_pool swap this close triggers advances DeepPool's TWAP.

    // Vault must cover the buy; otherwise the position is underwater → liquidate.
    let vault_sol = ctx.accounts.position_sol_vault.lamports();
    require!(vault_sol >= sol_needed, TorchMarketError::NotLiquidatable);

    // ---- Atomic buy: vault SOL → pool, tokens → lock ATA (D-5 step 6) ----
    // user = treasury_lock (lock ATA = repay sink); sol_source = position_sol_vault
    // (SOL source). Both PDAs sign.
    let mint_key = ctx.accounts.mint.key();
    let shorter_key = ctx.accounts.shorter.key();
    let lock_seeds: &[&[u8]] = &[
        TREASURY_LOCK_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury_lock.bump],
    ];
    let idx_le = args.position_index.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        shorter_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[ctx.accounts.position.vault_bump],
    ];
    let cpi_signers = &[lock_seeds, vault_seeds][..];

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.treasury_lock.to_account_info(),
        sol_source: ctx.accounts.position_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.treasury_lock_token_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: sol_needed,
            minimum_out: debt_gross, // guarantee the lock receives >= debt_to_repay net
            buy: true,
        },
    )?;

    // [F-6] Slippage guard on BOTH partial and full close. `sol_needed` is
    // quoted from live reserves in this same instruction, so the swap's
    // min_out only guarantees token delivery — not the price paid. Requiring
    // the post-buy vault balance to clear the caller's floor bounds the SOL
    // spent on a partial close too (on a full close this balance IS the
    // surplus paid out, so the semantic is unchanged).
    require!(
        ctx.accounts.position_sol_vault.lamports() >= args.min_surplus_sol_out,
        TorchMarketError::SlippageExceeded
    );

    // ---- Apply debt credit: interest first, then principal (D-5 step ~) ----
    // The buy delivers >= debt_to_repay net to the lock by construction (gross-up
    // + min_out), so we credit exactly debt_to_repay; any excess (interest overpay
    // + gross-up rounding) stays in the lock as protocol revenue, never lost.
    let interest_paid = debt_to_repay.min(ctx.accounts.position.accrued_interest);
    let principal_paid = debt_to_repay - interest_paid;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
    }

    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_tokens_lent = treasury
            .total_tokens_lent
            .saturating_sub(principal_paid);
        treasury.short_interest_collected = treasury
            .short_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the owner's per-user cap headroom.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.short_tokens_debt = risk.short_tokens_debt.saturating_sub(principal_paid);
    }

    // ---- Settle: surplus SOL → user; close position + vault on full close ----
    let fully_closed =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    let mut surplus_sol = 0u64;
    if fully_closed {
        // (min_surplus_sol_out already enforced right after the swap — F-6.)
        surplus_sol = ctx.accounts.position_sol_vault.lamports();
        // Drain the system-owned vault → user via SIGNED system transfer. The
        // vault is owned by the System program (bare PDA, 0 data), so this
        // program cannot debit it by direct lamport manipulation — only a
        // seed-signed system_program::transfer can move SOL out (same mechanism
        // deep_pool uses to pull `sol_source`). Draining to 0 deallocates it.
        if surplus_sol > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.position_sol_vault.to_account_info(),
                        to: ctx.accounts.shorter.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                surplus_sol,
            )?;
        }
        ctx.accounts.treasury.active_shorts =
            ctx.accounts.treasury.active_shorts.saturating_sub(1);
        // Reclaim Position rent → user, close the account.
        let position_ai = ctx.accounts.position.to_account_info();
        let shorter_ai = ctx.accounts.shorter.to_account_info();
        close_account(&position_ai, &shorter_ai)?;
    }

    emit_cpi!(CloseShortEvent {
        user: shorter_key,
        mint: mint_key,
        position_index: args.position_index,
        debt_repaid: debt_to_repay,
        sol_spent_on_buyback: sol_needed,
        interest_paid,
        principal_paid,
        surplus_sol_to_user: surplus_sol,
        fully_closed,
    });
    Ok(())
}

#[event]
pub struct CloseShortEvent {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub debt_repaid: u64,
    pub sol_spent_on_buyback: u64,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub surplus_sol_to_user: u64,
    pub fully_closed: bool,
}

// ============================================================================
// close_short_via_vault (D-12) — close_short for a vault-owned position.
//
// Identical to close_short except: the position is vault-seeded, surplus SOL on
// full close returns to vault_sol (the vault's SOL home, not a wallet), and the
// Position-account rent is reclaimed to `signer` (the linked wallet pays its own
// gas). A linked wallet can close but never extract value to a wallet.
// ============================================================================
pub fn close_short_via_vault(
    ctx: Context<crate::contexts::CloseShortViaVault>,
    args: crate::contexts::ClosePositionArgs,
) -> Result<()> {
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_short_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }

    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveShort);
    let debt_to_repay = apply_bps(total_debt, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_repay > 0, TorchMarketError::ZeroAmount);

    let debt_gross = gross_up_for_transfer_fee(debt_to_repay)
        .ok_or(TorchMarketError::MathOverflow)?;

    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let sol_needed = calc_close_pool_amount_in(
        debt_gross,
        pool_sol,
        pool_tokens,
        deep_pool::constants::SWAP_FEE_BPS as u16,
    )
    .ok_or(TorchMarketError::MathOverflow)?;

    let vault_balance = ctx.accounts.position_sol_vault.lamports();
    require!(vault_balance >= sol_needed, TorchMarketError::NotLiquidatable);

    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let lock_seeds: &[&[u8]] = &[
        TREASURY_LOCK_SEED,
        mint_key.as_ref(),
        &[ctx.accounts.treasury_lock.bump],
    ];
    let idx_le = args.position_index.to_le_bytes();
    // Vault-seeded position vault.
    let pos_vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[ctx.accounts.position.vault_bump],
    ];
    let cpi_signers = &[lock_seeds, pos_vault_seeds][..];

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.treasury_lock.to_account_info(),
        sol_source: ctx.accounts.position_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.treasury_lock_token_account.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: sol_needed,
            minimum_out: debt_gross,
            buy: true,
        },
    )?;

    // [F-6] Slippage guard on BOTH partial and full close (see close_short).
    require!(
        ctx.accounts.position_sol_vault.lamports() >= args.min_surplus_sol_out,
        TorchMarketError::SlippageExceeded
    );

    let interest_paid = debt_to_repay.min(ctx.accounts.position.accrued_interest);
    let principal_paid = debt_to_repay - interest_paid;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
    }
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_tokens_lent = treasury.total_tokens_lent.saturating_sub(principal_paid);
        treasury.short_interest_collected = treasury
            .short_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the vault's per-user cap headroom.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.short_tokens_debt = risk.short_tokens_debt.saturating_sub(principal_paid);
    }

    // ---- Settle: surplus SOL → vault_sol; rent → signer on full close ----
    let fully_closed =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    let mut surplus_sol = 0u64;
    if fully_closed {
        // (min_surplus_sol_out already enforced right after the swap — F-6.)
        surplus_sol = ctx.accounts.position_sol_vault.lamports();
        // Surplus P&L returns to the VAULT (seed-signed; the linked wallet can't
        // skim it). Draining to 0 deallocates the System-owned position vault.
        if surplus_sol > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.position_sol_vault.to_account_info(),
                        to: ctx.accounts.vault_sol.to_account_info(),
                    },
                    &[pos_vault_seeds],
                ),
                surplus_sol,
            )?;
        }
        ctx.accounts.treasury.active_shorts =
            ctx.accounts.treasury.active_shorts.saturating_sub(1);
        // Position rent → signer (the linked wallet's own gas, not vault funds).
        let position_ai = ctx.accounts.position.to_account_info();
        let signer_ai = ctx.accounts.signer.to_account_info();
        close_account(&position_ai, &signer_ai)?;
    }

    emit_cpi!(CloseShortEvent {
        user: vault_key,
        mint: mint_key,
        position_index: args.position_index,
        debt_repaid: debt_to_repay,
        sol_spent_on_buyback: sol_needed,
        interest_paid,
        principal_paid,
        surplus_sol_to_user: surplus_sol,
        fully_closed,
    });
    Ok(())
}

// ============================================================================
// liquidate_short (D-6 + D-10) — liquidator repays token debt, seizes vault SOL.
//
// Trigger AND seize are marked against the hardened TWAP, never raw spot:
//   - Trigger: LTV-at-TWAP must exceed the threshold (an atomic spot pump can't
//     move the ratchet ring, so a manufactured liquidation is refused). The raw
//     spot LTV must ALSO exceed it (belt-and-suspenders; TWAP is the binding
//     gate since it lags a real move). Warmup (mark not ready) → fail closed.
//   - Seize: priced at the TWAP mark so a spot pump at liquidation time can't
//     inflate the SOL seized.
//   - Bonus: distress-scaled on the TWAP LTV (effective_liq_bonus_bps), so a
//     barely-over (manufactured) liquidation earns ~0 prize.
// Liquidator supplies cover tokens (grossed-up) → lock; seizes SOL + bonus from
// the position vault. Bad debt (vault can't fund the bonus) is written off
// against treasury.total_tokens_lent. Per-position custody bounds the loss.
// ============================================================================
pub fn liquidate_short(
    ctx: Context<LiquidateShortPosition>,
    _args: crate::contexts::LiquidatePositionArgs,
) -> Result<()> {
    // ---- Accrue interest ----
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_short_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveShort);

    let vault_sol = ctx.accounts.position_sol_vault.lamports();
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let threshold = ctx.accounts.treasury.liquidation_threshold_bps as u64;

    // ---- D-10 trigger: DeepPool TWAP mark (fail closed on warmup) ----
    let price_q64 = read_twap_price_q64(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
        now,
    )?
    .ok_or(TorchMarketError::ShortNotLiquidatable)?;
    let debt_value_at_mark = twap_value_in_sol(total_debt, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let twap_ltv = calc_ltv_bps(debt_value_at_mark, vault_sol)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(twap_ltv > threshold, TorchMarketError::ShortNotLiquidatable);

    // Asymmetric spot veto (D-10): the TWAP is the binding trigger; spot may only
    // REFUSE the liquidation when it is CLEARLY healthy (a full LIQ_SPOT_VETO_MARGIN_BPS
    // below the threshold), so a genuinely-recovered borrower isn't liquidated on a
    // stale-high TWAP — but spot can't be atomically nudged just under the line to dodge.
    let debt_value_spot = calc_short_debt_value(total_debt, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let spot_ltv =
        calc_ltv_bps(debt_value_spot, vault_sol).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        spot_ltv > threshold.saturating_sub(LIQ_SPOT_VETO_MARGIN_BPS),
        TorchMarketError::ShortNotLiquidatable
    );

    // ---- Size the cover + seize, priced at the mark (D-10 seize clamp) ----
    let debt_to_cover = apply_bps(total_debt, ctx.accounts.treasury.liquidation_close_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_cover > 0, TorchMarketError::ZeroAmount);

    // SOL value of the covered token debt at the TWAP mark.
    let debt_value_sol = twap_value_in_sol(debt_to_cover, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    // Distress-scaled bonus on the TWAP LTV (manufactured liq earns ~0).
    let bonus_bps = effective_liq_bonus_bps(
        twap_ltv,
        ctx.accounts.treasury.liquidation_threshold_bps,
        LIQ_FULL_BONUS_LTV_BPS,
        ctx.accounts.treasury.liquidation_bonus_bps,
    ) as u16;
    let target_sol_seize = calc_short_sol_to_seize(debt_value_sol, bonus_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    let actual_sol_seize = target_sol_seize.min(vault_sol);

    // Insolvent iff the vault can't fund the full target seize: the ENTIRE vault is
    // taken (actual == vault) and no collateral remains. The liquidator covers a
    // proportional slice; the rest is forgiven in full in the apply block below — so
    // the position fully resolves in one liquidation rather than leaving an
    // un-liquidatable tail that permanently inflates total_tokens_lent.
    let insolvent = actual_sol_seize < target_sol_seize;
    let actual_tokens_covered = if insolvent && target_sol_seize > 0 {
        calc_short_partial_seize_proration(debt_to_cover, actual_sol_seize, target_sol_seize)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        debt_to_cover
    };

    // ---- Liquidator supplies cover tokens (grossed-up) → lock ATA ----
    // Skipped when there's nothing left to cover (a fully-drained insolvent vault
    // still resolves via the write-off below).
    if actual_tokens_covered > 0 {
        let cover_gross =
            gross_up_for_transfer_fee(actual_tokens_covered).ok_or(TorchMarketError::MathOverflow)?;
        require!(cover_gross > 0, TorchMarketError::ZeroAmount);
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_2022_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.liquidator_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: ctx.accounts.liquidator.to_account_info(),
                },
            ),
            cover_gross,
            ctx.accounts.mint.decimals,
        )?;
    }

    // ---- Seize vault SOL → liquidator (signed transfer; vault is system-owned) ----
    let mint_key = ctx.accounts.mint.key();
    let borrower_key = ctx.accounts.borrower.key();
    let idx_le = _args.position_index.to_le_bytes();
    let vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[ctx.accounts.position.vault_bump],
    ];
    if actual_sol_seize > 0 {
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.position_sol_vault.to_account_info(),
                    to: ctx.accounts.liquidator.to_account_info(),
                },
                &[vault_seeds],
            ),
            actual_sol_seize,
        )?;
    }

    // ---- Apply debt credit (interest first, then principal) + bad-debt write-off ----
    // The cover transfer delivers net == actual_tokens_covered to the lock by the
    // gross-up; credit exactly that, excess stays in lock as protocol revenue.
    let applied = actual_tokens_covered;
    let interest_paid = applied.min(ctx.accounts.position.accrued_interest);
    let principal_paid = applied - interest_paid;
    let bad_debt;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
        if insolvent {
            // Vault fully seized, no collateral remains — forgive the ENTIRE residual
            // (principal + interest) so the position fully resolves. Without this the
            // unrecoverable tail would sit as live debt against an empty vault and keep
            // total_tokens_lent permanently overstated.
            bad_debt = position.debt_amount;
            position.debt_amount = 0;
            position.accrued_interest = 0;
        } else {
            bad_debt = 0;
        }
    }
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_tokens_lent = treasury
            .total_tokens_lent
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
        treasury.short_interest_collected = treasury
            .short_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the owner's per-user cap headroom (repaid + written off).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.short_tokens_debt = risk
            .short_tokens_debt
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
    }

    // ---- Full liquidation: residual vault SOL → borrower, close position+vault ----
    let fully_liquidated =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    let mut residual_sol = 0u64;
    if fully_liquidated {
        residual_sol = ctx.accounts.position_sol_vault.lamports();
        if residual_sol > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.position_sol_vault.to_account_info(),
                        to: ctx.accounts.borrower.to_account_info(),
                    },
                    &[vault_seeds],
                ),
                residual_sol,
            )?;
        }
        ctx.accounts.treasury.active_shorts =
            ctx.accounts.treasury.active_shorts.saturating_sub(1);
        let position_ai = ctx.accounts.position.to_account_info();
        let borrower_ai = ctx.accounts.borrower.to_account_info();
        close_account(&position_ai, &borrower_ai)?;
    }

    emit_cpi!(LiquidateShortEvent {
        liquidator: ctx.accounts.liquidator.key(),
        borrower: borrower_key,
        mint: mint_key,
        position_index: _args.position_index,
        tokens_covered: actual_tokens_covered,
        sol_seized: actual_sol_seize,
        bad_debt,
        bonus_bps,
        twap_ltv,
        residual_sol_to_borrower: residual_sol,
        fully_liquidated,
    });
    Ok(())
}

#[event]
pub struct LiquidateShortEvent {
    pub liquidator: Pubkey,
    pub borrower: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub tokens_covered: u64,
    pub sol_seized: u64,
    pub bad_debt: u64,
    pub bonus_bps: u16,
    pub twap_ltv: u64,
    pub residual_sol_to_borrower: u64,
    pub fully_liquidated: bool,
}

// [V21] Vault-routed liquidation of a vault-owned short. Mirrors liquidate_short,
// but the position is vault-seeded (keyed by torch_vault), the liquidator is an
// external actor (no vault_wallet_link), seized SOL → liquidator, and residual
// SOL + position rent on full liquidation flow back to the vault's vault_sol.
pub fn liquidate_short_via_vault(
    ctx: Context<crate::contexts::LiquidateShortViaVault>,
    _args: crate::contexts::LiquidatePositionArgs,
) -> Result<()> {
    // ---- Accrue interest ----
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_short_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveShort);

    let vault_sol_bal = ctx.accounts.position_sol_vault.lamports();
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let threshold = ctx.accounts.treasury.liquidation_threshold_bps as u64;

    // ---- D-10 trigger: DeepPool TWAP mark (fail closed on warmup) ----
    let price_q64 = read_twap_price_q64(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
        now,
    )?
    .ok_or(TorchMarketError::ShortNotLiquidatable)?;
    let debt_value_at_mark = twap_value_in_sol(total_debt, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let twap_ltv = calc_ltv_bps(debt_value_at_mark, vault_sol_bal)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(twap_ltv > threshold, TorchMarketError::ShortNotLiquidatable);

    // Asymmetric spot veto (D-10): TWAP is the binding trigger; spot may only refuse
    // when CLEARLY healthy (LIQ_SPOT_VETO_MARGIN_BPS below threshold) — see liquidate_short.
    let debt_value_spot = calc_short_debt_value(total_debt, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let spot_ltv =
        calc_ltv_bps(debt_value_spot, vault_sol_bal).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        spot_ltv > threshold.saturating_sub(LIQ_SPOT_VETO_MARGIN_BPS),
        TorchMarketError::ShortNotLiquidatable
    );

    // ---- Size the cover + seize, priced at the mark (D-10 seize clamp) ----
    let debt_to_cover = apply_bps(total_debt, ctx.accounts.treasury.liquidation_close_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_cover > 0, TorchMarketError::ZeroAmount);

    let debt_value_sol = twap_value_in_sol(debt_to_cover, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let bonus_bps = effective_liq_bonus_bps(
        twap_ltv,
        ctx.accounts.treasury.liquidation_threshold_bps,
        LIQ_FULL_BONUS_LTV_BPS,
        ctx.accounts.treasury.liquidation_bonus_bps,
    ) as u16;
    let target_sol_seize = calc_short_sol_to_seize(debt_value_sol, bonus_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    let actual_sol_seize = target_sol_seize.min(vault_sol_bal);

    // Insolvent iff the vault can't fund the full target seize — entire vault taken,
    // no collateral remains; forgive the residual in full below so the position fully
    // resolves (no un-liquidatable tail, total_tokens_lent stays true). See liquidate_short.
    let insolvent = actual_sol_seize < target_sol_seize;
    let actual_tokens_covered = if insolvent && target_sol_seize > 0 {
        calc_short_partial_seize_proration(debt_to_cover, actual_sol_seize, target_sol_seize)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        debt_to_cover
    };

    // ---- Liquidator supplies cover tokens (grossed-up) → lock ATA ----
    if actual_tokens_covered > 0 {
        let cover_gross =
            gross_up_for_transfer_fee(actual_tokens_covered).ok_or(TorchMarketError::MathOverflow)?;
        require!(cover_gross > 0, TorchMarketError::ZeroAmount);
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_2022_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.liquidator_token_account.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.treasury_lock_token_account.to_account_info(),
                    authority: ctx.accounts.liquidator.to_account_info(),
                },
            ),
            cover_gross,
            ctx.accounts.mint.decimals,
        )?;
    }

    // ---- Seize vault SOL → liquidator (signed; position vault is system-owned) ----
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let idx_le = _args.position_index.to_le_bytes();
    let pos_vault_seeds: &[&[u8]] = &[
        SHORT_VAULT_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[ctx.accounts.position.vault_bump],
    ];
    if actual_sol_seize > 0 {
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.position_sol_vault.to_account_info(),
                    to: ctx.accounts.liquidator.to_account_info(),
                },
                &[pos_vault_seeds],
            ),
            actual_sol_seize,
        )?;
    }

    // ---- Apply debt credit (interest first, then principal) + bad-debt write-off ----
    let applied = actual_tokens_covered;
    let interest_paid = applied.min(ctx.accounts.position.accrued_interest);
    let principal_paid = applied - interest_paid;
    let bad_debt;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
        if insolvent {
            // No collateral remains — forgive the entire residual so the position
            // fully resolves (no tail, total_tokens_lent stays true).
            bad_debt = position.debt_amount;
            position.debt_amount = 0;
            position.accrued_interest = 0;
        } else {
            bad_debt = 0;
        }
    }
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_tokens_lent = treasury
            .total_tokens_lent
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
        treasury.short_interest_collected = treasury
            .short_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the vault's per-user cap headroom (repaid + written off).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.short_tokens_debt = risk
            .short_tokens_debt
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
    }

    // ---- Full liquidation: residual SOL → vault_sol, rent → vault_sol, close ----
    let fully_liquidated =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    let mut residual_sol = 0u64;
    if fully_liquidated {
        residual_sol = ctx.accounts.position_sol_vault.lamports();
        if residual_sol > 0 {
            system_program::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.position_sol_vault.to_account_info(),
                        to: ctx.accounts.vault_sol.to_account_info(),
                    },
                    &[pos_vault_seeds],
                ),
                residual_sol,
            )?;
        }
        ctx.accounts.treasury.active_shorts =
            ctx.accounts.treasury.active_shorts.saturating_sub(1);
        // Position rent flows back to the vault (the position's economic owner).
        let position_ai = ctx.accounts.position.to_account_info();
        let vault_sol_ai = ctx.accounts.vault_sol.to_account_info();
        close_account(&position_ai, &vault_sol_ai)?;
    }

    emit_cpi!(LiquidateShortEvent {
        liquidator: ctx.accounts.liquidator.key(),
        borrower: vault_key,
        mint: mint_key,
        position_index: _args.position_index,
        tokens_covered: actual_tokens_covered,
        sol_seized: actual_sol_seize,
        bad_debt,
        bonus_bps,
        twap_ltv,
        residual_sol_to_borrower: residual_sol,
        fully_liquidated,
    });
    Ok(())
}

// ============================================================================
// open_long (D-4) — atomic custodied long, structural mirror of open_short.
//
// Token collateral in → treasury funds a SOL borrow → 0.5% open fee stays in
// treasury (revenue), (borrow − fee) atomically buys tokens on deep_pool that
// land in the position token vault alongside the collateral. Debt = the FULL
// gross borrow (user owes back what was lent on their behalf, incl. the fee
// portion). `vault_tokens` is NOT stored — it IS position_token_vault.amount
// (D-3, derived state). No SOL reaches the user's wallet on open.
// ============================================================================
pub fn open_long(ctx: Context<OpenLongPosition>, args: crate::contexts::OpenPositionArgs) -> Result<()> {
    let collateral = args.collateral;
    require!(collateral > 0, TorchMarketError::EmptyBorrowRequest);

    // ---- Lending unlock gate + depth-band LTV (D-4 step 1) ----
    // [F-2] STICKY gate on the principal pool (physical float + outstanding
    // receivables) — the V21 translation of V20's tracked `sol_balance`
    // (docs/lending-unlock.md). Normal borrow/repay moves lamports between the
    // two terms without changing the sum, so once the protocol has EARNED the
    // threshold the gate stays open; only a bad-debt write-off (a real loss)
    // re-locks it. The gate certifies meaningful lending scale; the physical
    // float below is the first-come-first-serve capacity within it.
    let physical_sol = treasury_physical_sol(&ctx.accounts.treasury_sol_vault)?;
    let lending_assets = crate::math::calc_lending_assets(
        physical_sol,
        ctx.accounts.treasury.total_sol_lent_to_longs,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        lending_assets >= MIN_TREASURY_SOL_FOR_LENDING,
        TorchMarketError::LendingNotYetUnlocked
    );
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(ctx.accounts.treasury.max_ltv_bps);

    // Oracle: the deep_pool swap this open triggers advances DeepPool's TWAP.
    let now = Clock::get()?.slot;

    // ---- Deposit collateral: borrower ATA → position token vault ----
    // The vault is freshly init'd (balance 0), so its post-transfer amount IS the
    // net collateral after the Token-2022 fee on this leg.
    anchor_spl::token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_2022_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.borrower_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.position_token_vault.to_account_info(),
                authority: ctx.accounts.borrower.to_account_info(),
            },
        ),
        collateral,
        ctx.accounts.mint.decimals,
    )?;
    ctx.accounts.position_token_vault.reload()?;
    let net_collateral = ctx.accounts.position_token_vault.amount;
    require!(net_collateral > 0, TorchMarketError::InsufficientTokens);

    // ---- Borrow plan at pre-swap price (D-4 steps 2–3) ----
    let collateral_value_sol = calc_collateral_value(net_collateral, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(collateral_value_sol > 0, TorchMarketError::EmptyBorrowRequest);
    let mut desired_borrow_sol = apply_bps(collateral_value_sol, effective_max_ltv)
        .ok_or(TorchMarketError::MathOverflow)?;

    // Bound the borrow by the caps (clamp, not reject — the UI shows the
    // clamped size before signing, and min_out guards the swap output). Mirrors
    // open_short's `.min(lock_available).min(user cap)`.
    // No utilization cap: the float is fully lendable, first-come-first-serve
    // (full-drain by design, mirroring the short side's lock). The per-user
    // caps below are the fairness rail, based on the principal pool.
    let max_lendable = lending_assets;
    let global_headroom = physical_sol;
    // [F-1][F-3] Per-USER allowance = min(formula cap, absolute 20% cap) on the
    // owner's AGGREGATE long debt across position_index values, minus what's
    // already borrowed. The formula cap (calc_user_borrow_cap — scales with the
    // owner's collateral share of total supply) is measured on aggregate
    // collateral including this deposit; the absolute cap clamps any single
    // owner at MAX_USER_BORROW_SHARE_BPS of lendable regardless of collateral.
    let (prior_long_collateral, prior_long_debt) = {
        let risk = user_risk_mut(&ctx.accounts.user_risk)?;
        (risk.long_collateral_tokens, risk.long_sol_debt)
    };
    let aggregate_collateral = prior_long_collateral
        .checked_add(net_collateral)
        .ok_or(TorchMarketError::MathOverflow)?;
    let user_cap =
        crate::math::calc_user_borrow_cap(max_lendable, aggregate_collateral, TOTAL_SUPPLY)
            .ok_or(TorchMarketError::MathOverflow)?;
    let user_allowance = user_cap.saturating_sub(prior_long_debt);
    // Global utilization: an exhausted pool is its OWN failure (LendingCapExceeded),
    // distinct from the dust floor below — the error matches the intent. Only the
    // clamp below is silent; a tapped-out pool still rejects explicitly.
    require!(
        global_headroom >= MIN_BORROW_AMOUNT,
        TorchMarketError::LendingCapExceeded
    );
    desired_borrow_sol = desired_borrow_sol
        .min(user_allowance)
        .min(global_headroom)
        // [V21] Rail 2 size cap: SOL-debt-value ≤ ρ_max of pool depth (clamp).
        .min(max_debt_value_for_depth(pool_sol));
    // Dust floor: collateral too small to borrow meaningfully — or the owner's
    // per-user allowance is exhausted (distinct from the pool-exhausted case above).
    require!(
        desired_borrow_sol >= MIN_BORROW_AMOUNT,
        TorchMarketError::BorrowTooSmall
    );

    let open_fee = apply_bps(desired_borrow_sol, OPEN_FEE_BPS)
        .ok_or(TorchMarketError::MathOverflow)?;
    let atomic_buy_sol = desired_borrow_sol
        .checked_sub(open_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    // ---- Atomic buy: long_sol_vault SOL → pool, tokens → position token vault ----
    // user = position (its ATA = token sink); sol_source = long_sol_vault. Both PDAs sign.
    let mint_key = ctx.accounts.mint.key();
    let borrower_key = ctx.accounts.borrower.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_bump = ctx.bumps.position;
    let vault_bump = ctx.bumps.long_sol_vault;
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[position_bump],
    ];
    let vault_seeds: &[&[u8]] = &[
        LONG_SOL_VAULT_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[vault_bump],
    ];
    let cpi_signers = &[position_seeds, vault_seeds][..];

    // ---- Stage atomic_buy_sol: treasury_sol_vault → long_sol_vault ----
    // The lendable SOL already lives in treasury_sol_vault (System-owned), so this
    // is a plain seed-signed transfer between two System PDAs — and long_sol_vault
    // is then a valid `sol_source` for deep_pool's buy. The open fee never leaves
    // the vault (revenue); only atomic_buy_sol moves. Debt records the full
    // desired_borrow (user owes gross). The transfer fails closed if the vault is
    // short, so available-to-lend is enforced by both the derived treasury_sol_vault
    // balance gate above and real lamports here.
    require!(
        ctx.accounts.treasury_sol_vault.lamports() >= atomic_buy_sol,
        TorchMarketError::InsufficientTreasury
    );
    let tsv_bump = ctx.bumps.treasury_sol_vault;
    let tsv_seeds: &[&[u8]] = &[TREASURY_SOL_VAULT_SEED, mint_key.as_ref(), &[tsv_bump]];
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.treasury_sol_vault.to_account_info(),
                to: ctx.accounts.long_sol_vault.to_account_info(),
            },
            &[tsv_seeds],
        ),
        atomic_buy_sol,
    )?;

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.position.to_account_info(),
        sol_source: ctx.accounts.long_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.position_token_vault.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: atomic_buy_sol,
            minimum_out: args.min_out, // slippage guard: min tokens out from the buy
            buy: true,
        },
    )?;

    // ---- Treasury accounting ----
    // Physical SOL already moved: atomic_buy_sol left treasury_sol_vault for the
    // buy; open_fee stayed in the vault as revenue. Only the tracked obligation
    // (gross debt owed back) is recorded here.
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs = treasury
            .total_sol_lent_to_longs
            .checked_add(desired_borrow_sol)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.total_token_collateral_locked = treasury
            .total_token_collateral_locked
            .checked_add(net_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.active_longs = treasury
            .active_longs
            .checked_add(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    // [F-1][F-3] Aggregate per-user exposure (owner/mint/bump idempotent).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.owner = borrower_key;
        risk.mint = mint_key;
        risk.bump = ctx.bumps.user_risk;
        risk.long_sol_debt = risk
            .long_sol_debt
            .checked_add(desired_borrow_sol)
            .ok_or(TorchMarketError::MathOverflow)?;
        risk.long_collateral_tokens = risk
            .long_collateral_tokens
            .checked_add(net_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    // ---- Persist position (vault_tokens NOT stored — it's the vault balance) ----
    let position = &mut ctx.accounts.position;
    position.user = borrower_key;
    position.mint = mint_key;
    position.side = PositionSide::Long;
    position.position_index = args.position_index;
    position.collateral_amount = net_collateral; // record-keeping (post-deposit-fee)
    position.debt_amount = desired_borrow_sol; // gross SOL owed back to treasury
    position.accrued_interest = 0;
    position.last_slot = now;
    position.bump = position_bump;
    position.vault_bump = vault_bump; // long_sol_vault bump (token vault is an ATA)

    ctx.accounts.position_token_vault.reload()?;
    emit_cpi!(OpenLongEvent {
        user: borrower_key,
        mint: mint_key,
        position_index: args.position_index,
        collateral_tokens: net_collateral,
        borrowed_sol_gross: desired_borrow_sol,
        open_fee_sol: open_fee,
        atomic_buy_sol,
        vault_tokens: ctx.accounts.position_token_vault.amount,
    });
    Ok(())
}

#[event]
pub struct OpenLongEvent {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub collateral_tokens: u64,
    pub borrowed_sol_gross: u64,
    pub open_fee_sol: u64,
    pub atomic_buy_sol: u64,
    pub vault_tokens: u64,
}

// ============================================================================
// close_long (D-5) — atomic pool-routed close, exits in SOL. Mirror of
// close_short but ASYMMETRIC: sells ALL vault tokens (scaled by repay_fraction)
// in one swap, then SPLITS the SOL output debt→treasury / surplus→user. (Short
// computes an exact buy; long sells-all and splits. Both exit in SOL.)
//
// SOL-debt repayment needs NO gross-up — SOL transfers are lamport shifts with
// no Token-2022 fee (unlike the short's token-debt repayment to the lock).
// ============================================================================
pub fn close_long(ctx: Context<CloseLongPosition>, args: crate::contexts::ClosePositionArgs) -> Result<()> {
    // ---- Accrue interest (SOL-denominated), size the repay (D-5 steps 1–2) ----
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveLoan);
    let debt_to_repay = apply_bps(total_debt, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_repay > 0, TorchMarketError::ZeroAmount);

    // Tokens to sell = repay_fraction of the vault (sell-all on full close).
    let vault_tokens = ctx.accounts.position_token_vault.amount;
    let tokens_to_sell = apply_bps(vault_tokens, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(tokens_to_sell > 0, TorchMarketError::ZeroAmount);

    // Oracle: the deep_pool swap this close triggers advances DeepPool's TWAP.

    // ---- Atomic sell: vault tokens → pool, SOL proceeds → long_sol_vault ----
    // user = position (its ATA = token source); sol_source = long_sol_vault (SOL
    // sink). Both PDAs sign. min_out=1: the sell-all output is split below; the
    // sufficiency check (sol_out >= debt) is the real guard, not pool slippage.
    let mint_key = ctx.accounts.mint.key();
    let borrower_key = ctx.accounts.borrower.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[ctx.accounts.position.bump],
    ];
    let vault_bump = ctx.bumps.long_sol_vault;
    let vault_seeds: &[&[u8]] = &[
        LONG_SOL_VAULT_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[vault_bump],
    ];
    let cpi_signers = &[position_seeds, vault_seeds][..];

    let vault_sol_before = ctx.accounts.long_sol_vault.lamports();
    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.position.to_account_info(),
        sol_source: ctx.accounts.long_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.position_token_vault.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: tokens_to_sell,
            minimum_out: 1,
            buy: false,
        },
    )?;
    // SOL the sale delivered into the vault.
    let sol_out = ctx
        .accounts
        .long_sol_vault
        .lamports()
        .checked_sub(vault_sol_before)
        .ok_or(TorchMarketError::MathOverflow)?;

    // Sale must cover the debt; else position is underwater → liquidate.
    require!(sol_out >= debt_to_repay, TorchMarketError::NotLiquidatable);

    // ---- Split: debt → treasury, surplus → user (D-5 steps 6–9) ----
    let surplus_sol = sol_out - debt_to_repay;
    require!(
        surplus_sol >= args.min_surplus_sol_out,
        TorchMarketError::SlippageExceeded
    );

    // debt_to_repay: long_sol_vault → treasury_sol_vault (both System-owned PDAs;
    // signed system transfer). Treasury balance is derived from the vault lamports.
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.long_sol_vault.to_account_info(),
                to: ctx.accounts.treasury_sol_vault.to_account_info(),
            },
            &[vault_seeds],
        ),
        debt_to_repay,
    )?;
    if surplus_sol > 0 {
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.long_sol_vault.to_account_info(),
                    to: ctx.accounts.borrower.to_account_info(),
                },
                &[vault_seeds],
            ),
            surplus_sol,
        )?;
    }

    // ---- Apply debt credit + treasury accounting ----
    let interest_paid = debt_to_repay.min(ctx.accounts.position.accrued_interest);
    let principal_paid = debt_to_repay - interest_paid;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
    }
    {
        // Repaid SOL already landed in treasury_sol_vault (the long_sol_vault →
        // vault transfer above). Only the tracked obligations update here.
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs =
            treasury.total_sol_lent_to_longs.saturating_sub(principal_paid);
        treasury.long_interest_collected = treasury
            .long_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the owner's per-user cap headroom.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.long_sol_debt = risk.long_sol_debt.saturating_sub(principal_paid);
    }

    // ---- Full close: release collateral bookkeeping, close position + vaults ----
    let fully_closed =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    if fully_closed {
        {
            let treasury = &mut ctx.accounts.treasury;
            treasury.total_token_collateral_locked = treasury
                .total_token_collateral_locked
                .saturating_sub(ctx.accounts.position.collateral_amount);
            treasury.active_longs = treasury.active_longs.saturating_sub(1);
        }
        // [F-1] Release the closed position's collateral from the formula-cap basis.
        {
            let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
            risk.long_collateral_tokens = risk
                .long_collateral_tokens
                .saturating_sub(ctx.accounts.position.collateral_amount);
        }
        // Full close sells 100% of the vault (tokens_to_sell == vault_tokens
        // exactly at 10000 bps), so the token vault is empty here — no residual
        // to sweep (unlike liquidate_long's partial seize). Close the now-empty
        // ATA → borrower (reclaims rent; the long's held-asset vault is a token
        // ATA with no auto-GC, the one close-path asymmetry vs close_short's
        // bare SOL vault). Then close the Position data account → borrower.
        //
        // Token-2022 refuses to close an account that still holds withheld
        // transfer fees (accrued when collateral was deposited in). Harvest them
        // to the mint first (permissionless), then the empty vault can close.
        anchor_lang::solana_program::program::invoke(
            &crate::token_2022_utils::build_harvest_withheld_tokens_to_mint_instruction(
                &ctx.accounts.mint.key(),
                &[ctx.accounts.position_token_vault.key()],
            ),
            &[
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.position_token_vault.to_account_info(),
            ],
        )?;
        ctx.accounts.position_token_vault.reload()?;
        anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            anchor_spl::token_interface::CloseAccount {
                account: ctx.accounts.position_token_vault.to_account_info(),
                destination: ctx.accounts.borrower.to_account_info(),
                authority: ctx.accounts.position.to_account_info(),
            },
            &[position_seeds],
        ))?;
        let position_ai = ctx.accounts.position.to_account_info();
        let borrower_ai = ctx.accounts.borrower.to_account_info();
        close_account(&position_ai, &borrower_ai)?;
    }

    emit_cpi!(CloseLongEvent {
        user: borrower_key,
        mint: mint_key,
        position_index: args.position_index,
        tokens_sold: tokens_to_sell,
        sol_out,
        debt_repaid: debt_to_repay,
        interest_paid,
        principal_paid,
        surplus_sol_to_user: surplus_sol,
        fully_closed,
    });
    Ok(())
}

#[event]
pub struct CloseLongEvent {
    pub user: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub tokens_sold: u64,
    pub sol_out: u64,
    pub debt_repaid: u64,
    pub interest_paid: u64,
    pub principal_paid: u64,
    pub surplus_sol_to_user: u64,
    pub fully_closed: bool,
}

// ============================================================================
// liquidate_long (D-6 + D-10) — mirror of liquidate_short. Liquidator pays SOL
// debt → treasury, seizes vault TOKENS + bonus from the position token vault.
// Trigger + seize marked against the hardened TWAP (same four-layer stack).
// ============================================================================
pub fn liquidate_long(
    ctx: Context<LiquidateLongPosition>,
    args: crate::contexts::LiquidatePositionArgs,
) -> Result<()> {
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveLoan);

    let vault_tokens = ctx.accounts.position_token_vault.amount;
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let threshold = ctx.accounts.treasury.liquidation_threshold_bps as u64;

    // ---- D-10 trigger: LTV-at-TWAP (vault token value at the DeepPool mark) ----
    let price_q64 = read_twap_price_q64(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
        now,
    )?
    .ok_or(TorchMarketError::NotLiquidatable)?;
    let vault_value_at_mark = twap_value_in_sol(vault_tokens, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let twap_ltv =
        calc_ltv_bps(total_debt, vault_value_at_mark).ok_or(TorchMarketError::MathOverflow)?;
    require!(twap_ltv > threshold, TorchMarketError::NotLiquidatable);

    // Asymmetric spot veto (D-10): TWAP is the binding trigger; spot may only refuse
    // when CLEARLY healthy (LIQ_SPOT_VETO_MARGIN_BPS below threshold) — see liquidate_short.
    let vault_value_spot = calc_collateral_value(vault_tokens, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let spot_ltv =
        calc_ltv_bps(total_debt, vault_value_spot).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        spot_ltv > threshold.saturating_sub(LIQ_SPOT_VETO_MARGIN_BPS),
        TorchMarketError::NotLiquidatable
    );

    // ---- Size cover + seize, priced at the mark (D-10 seize clamp) ----
    let debt_to_cover = apply_bps(total_debt, ctx.accounts.treasury.liquidation_close_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_cover > 0, TorchMarketError::ZeroAmount);
    let bonus_bps = effective_liq_bonus_bps(
        twap_ltv,
        ctx.accounts.treasury.liquidation_threshold_bps,
        LIQ_FULL_BONUS_LTV_BPS,
        ctx.accounts.treasury.liquidation_bonus_bps,
    ) as u16;
    // Tokens to seize for debt_to_cover SOL + bonus, priced at the TWAP mark.
    let target_seize = twap_tokens_to_seize(debt_to_cover, bonus_bps, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let actual_seize = target_seize.min(vault_tokens);

    // Insolvent iff the vault can't fund the full target seize (entire token vault
    // taken, no collateral remains) — or the degenerate target==0. Either way the
    // residual is forgiven in full in the apply block so the position fully resolves
    // (no un-liquidatable tail, total_sol_lent_to_longs stays true). See liquidate_short.
    let insolvent = target_seize == 0 || actual_seize < target_seize;
    let actual_debt_covered = if target_seize == 0 {
        0u64
    } else if actual_seize < target_seize {
        // covered = debt_to_cover * actual_seize / target_seize
        calc_short_partial_seize_proration(debt_to_cover, actual_seize, target_seize)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        debt_to_cover
    };

    // ---- Liquidator pays SOL → treasury_sol_vault (recoups principal + interest) ----
    // Skipped when there's nothing to cover (a fully-drained insolvent vault still
    // resolves via the write-off below).
    if actual_debt_covered > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.liquidator.to_account_info(),
                    to: ctx.accounts.treasury_sol_vault.to_account_info(),
                },
            ),
            actual_debt_covered,
        )?;
    }

    // ---- Seize vault tokens → liquidator (position ATA → liquidator ATA) ----
    let mint_key = ctx.accounts.mint.key();
    let borrower_key = ctx.accounts.borrower.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        borrower_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[ctx.accounts.position.bump],
    ];
    if actual_seize > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_2022_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.position_token_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.liquidator_token_account.to_account_info(),
                    authority: ctx.accounts.position.to_account_info(),
                },
                &[position_seeds],
            ),
            actual_seize,
            ctx.accounts.mint.decimals,
        )?;
    }

    // ---- Apply debt credit + bad-debt write-off ----
    let interest_paid = actual_debt_covered.min(ctx.accounts.position.accrued_interest);
    let principal_paid = actual_debt_covered - interest_paid;
    let bad_debt;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
        if insolvent {
            // No collateral remains — forgive the ENTIRE residual (principal +
            // interest) so the position fully resolves; otherwise the tail would sit
            // as live debt against an empty vault and overstate total_sol_lent_to_longs.
            bad_debt = position.debt_amount;
            position.debt_amount = 0;
            position.accrued_interest = 0;
        } else {
            bad_debt = 0;
        }
    }
    {
        // Liquidator's repayment already landed in treasury_sol_vault. Only the
        // tracked obligations update here.
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs = treasury
            .total_sol_lent_to_longs
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
        treasury.long_interest_collected = treasury
            .long_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the owner's per-user cap headroom (repaid + written off).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.long_sol_debt = risk
            .long_sol_debt
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
    }

    // ---- Full liquidation: residual vault tokens → borrower, close vault + position ----
    let fully_liquidated =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    if fully_liquidated {
        ctx.accounts.position_token_vault.reload()?;
        let residual = ctx.accounts.position_token_vault.amount;
        if residual > 0 {
            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_2022_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.position_token_vault.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.borrower_token_account.to_account_info(),
                        authority: ctx.accounts.position.to_account_info(),
                    },
                    &[position_seeds],
                ),
                residual,
                ctx.accounts.mint.decimals,
            )?;
        }
        // Token-2022 refuses to close an account holding withheld transfer fees;
        // harvest them to the mint first (permissionless), then the vault can close.
        anchor_lang::solana_program::program::invoke(
            &crate::token_2022_utils::build_harvest_withheld_tokens_to_mint_instruction(
                &ctx.accounts.mint.key(),
                &[ctx.accounts.position_token_vault.key()],
            ),
            &[
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.position_token_vault.to_account_info(),
            ],
        )?;
        ctx.accounts.position_token_vault.reload()?;
        anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            anchor_spl::token_interface::CloseAccount {
                account: ctx.accounts.position_token_vault.to_account_info(),
                destination: ctx.accounts.borrower.to_account_info(),
                authority: ctx.accounts.position.to_account_info(),
            },
            &[position_seeds],
        ))?;
        {
            let treasury = &mut ctx.accounts.treasury;
            treasury.total_token_collateral_locked = treasury
                .total_token_collateral_locked
                .saturating_sub(ctx.accounts.position.collateral_amount);
            treasury.active_longs = treasury.active_longs.saturating_sub(1);
        }
        // [F-1] Release the resolved position's collateral from the formula-cap basis.
        {
            let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
            risk.long_collateral_tokens = risk
                .long_collateral_tokens
                .saturating_sub(ctx.accounts.position.collateral_amount);
        }
        let position_ai = ctx.accounts.position.to_account_info();
        let borrower_ai = ctx.accounts.borrower.to_account_info();
        close_account(&position_ai, &borrower_ai)?;
    }

    emit_cpi!(LiquidateLongEvent {
        liquidator: ctx.accounts.liquidator.key(),
        borrower: borrower_key,
        mint: mint_key,
        position_index: args.position_index,
        debt_covered: actual_debt_covered,
        tokens_seized: actual_seize,
        bad_debt,
        bonus_bps,
        twap_ltv,
        fully_liquidated,
    });
    Ok(())
}

#[event]
pub struct LiquidateLongEvent {
    pub liquidator: Pubkey,
    pub borrower: Pubkey,
    pub mint: Pubkey,
    pub position_index: u32,
    pub debt_covered: u64,
    pub tokens_seized: u64,
    pub bad_debt: u64,
    pub bonus_bps: u16,
    pub twap_ltv: u64,
    pub fully_liquidated: bool,
}

// ============================================================================
// [V21] Vault-routed long handlers. Mirror open/close/liquidate_long, but the
// token collateral is the vault's (seed-signed by torch_vault), the position is
// vault-seeded, and P&L / residuals flow back to the vault (vault_sol /
// vault_token_account) rather than to a wallet. Borrowed SOL still comes from
// treasury_sol_vault — longs borrow from the treasury, never the vault.
// ============================================================================
pub fn open_long_via_vault(
    ctx: Context<crate::contexts::OpenLongViaVault>,
    args: crate::contexts::OpenPositionArgs,
) -> Result<()> {
    let collateral = args.collateral;
    require!(collateral > 0, TorchMarketError::EmptyBorrowRequest);

    // ---- Lending unlock gate + depth-band LTV (D-4 step 1) ----
    // [F-2] Sticky gate on the principal pool (physical + lent) — see open_long.
    let physical_sol = treasury_physical_sol(&ctx.accounts.treasury_sol_vault)?;
    let lending_assets = crate::math::calc_lending_assets(
        physical_sol,
        ctx.accounts.treasury.total_sol_lent_to_longs,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    require!(
        lending_assets >= MIN_TREASURY_SOL_FOR_LENDING,
        TorchMarketError::LendingNotYetUnlocked
    );
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let depth_max_ltv = get_depth_max_ltv_bps(pool_sol);
    require!(depth_max_ltv > 0, TorchMarketError::PoolTooThin);
    let effective_max_ltv = depth_max_ltv.min(ctx.accounts.treasury.max_ltv_bps);

    let now = Clock::get()?.slot;

    // ---- Deposit collateral: vault token ATA → position token vault (seed-signed) ----
    let vault_creator = ctx.accounts.torch_vault.creator;
    let vault_bump_v = ctx.accounts.torch_vault.bump;
    let vault_auth_seeds: &[&[u8]] = &[TORCH_VAULT_SEED, vault_creator.as_ref(), &[vault_bump_v]];
    anchor_spl::token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.position_token_vault.to_account_info(),
                authority: ctx.accounts.torch_vault.to_account_info(),
            },
            &[vault_auth_seeds],
        ),
        collateral,
        ctx.accounts.mint.decimals,
    )?;
    ctx.accounts.position_token_vault.reload()?;
    let net_collateral = ctx.accounts.position_token_vault.amount;
    require!(net_collateral > 0, TorchMarketError::InsufficientTokens);

    // ---- Borrow plan at pre-swap price (D-4 steps 2–3) ----
    let collateral_value_sol = calc_collateral_value(net_collateral, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(collateral_value_sol > 0, TorchMarketError::EmptyBorrowRequest);
    let mut desired_borrow_sol = apply_bps(collateral_value_sol, effective_max_ltv)
        .ok_or(TorchMarketError::MathOverflow)?;

    // No utilization cap — float is FCFS capacity (see open_long).
    let max_lendable = lending_assets;
    let global_headroom = physical_sol;
    // [F-1][F-3] Per-VAULT aggregate allowance — see open_long.
    let (prior_long_collateral, prior_long_debt) = {
        let risk = user_risk_mut(&ctx.accounts.user_risk)?;
        (risk.long_collateral_tokens, risk.long_sol_debt)
    };
    let aggregate_collateral = prior_long_collateral
        .checked_add(net_collateral)
        .ok_or(TorchMarketError::MathOverflow)?;
    let user_cap =
        crate::math::calc_user_borrow_cap(max_lendable, aggregate_collateral, TOTAL_SUPPLY)
            .ok_or(TorchMarketError::MathOverflow)?;
    let user_allowance = user_cap.saturating_sub(prior_long_debt);
    require!(
        global_headroom >= MIN_BORROW_AMOUNT,
        TorchMarketError::LendingCapExceeded
    );
    desired_borrow_sol = desired_borrow_sol
        .min(user_allowance)
        .min(global_headroom)
        // [V21] Rail 2 size cap: SOL-debt-value ≤ ρ_max of pool depth (clamp).
        .min(max_debt_value_for_depth(pool_sol));
    require!(
        desired_borrow_sol >= MIN_BORROW_AMOUNT,
        TorchMarketError::BorrowTooSmall
    );

    let open_fee = apply_bps(desired_borrow_sol, OPEN_FEE_BPS)
        .ok_or(TorchMarketError::MathOverflow)?;
    let atomic_buy_sol = desired_borrow_sol
        .checked_sub(open_fee)
        .ok_or(TorchMarketError::MathOverflow)?;

    // ---- Atomic buy: long_sol_vault SOL → pool, tokens → position token vault ----
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_bump = ctx.bumps.position;
    let vault_bump = ctx.bumps.long_sol_vault;
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[position_bump],
    ];
    let vault_seeds: &[&[u8]] = &[
        LONG_SOL_VAULT_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[vault_bump],
    ];
    let cpi_signers = &[position_seeds, vault_seeds][..];

    // ---- Stage atomic_buy_sol: treasury_sol_vault → long_sol_vault ----
    require!(
        ctx.accounts.treasury_sol_vault.lamports() >= atomic_buy_sol,
        TorchMarketError::InsufficientTreasury
    );
    let tsv_bump = ctx.bumps.treasury_sol_vault;
    let tsv_seeds: &[&[u8]] = &[TREASURY_SOL_VAULT_SEED, mint_key.as_ref(), &[tsv_bump]];
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.treasury_sol_vault.to_account_info(),
                to: ctx.accounts.long_sol_vault.to_account_info(),
            },
            &[tsv_seeds],
        ),
        atomic_buy_sol,
    )?;

    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.position.to_account_info(),
        sol_source: ctx.accounts.long_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.position_token_vault.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: atomic_buy_sol,
            minimum_out: args.min_out,
            buy: true,
        },
    )?;

    // ---- Treasury accounting ----
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs = treasury
            .total_sol_lent_to_longs
            .checked_add(desired_borrow_sol)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.total_token_collateral_locked = treasury
            .total_token_collateral_locked
            .checked_add(net_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
        treasury.active_longs = treasury
            .active_longs
            .checked_add(1)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    // [F-1][F-3] Aggregate per-vault exposure.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.owner = vault_key;
        risk.mint = mint_key;
        risk.bump = ctx.bumps.user_risk;
        risk.long_sol_debt = risk
            .long_sol_debt
            .checked_add(desired_borrow_sol)
            .ok_or(TorchMarketError::MathOverflow)?;
        risk.long_collateral_tokens = risk
            .long_collateral_tokens
            .checked_add(net_collateral)
            .ok_or(TorchMarketError::MathOverflow)?;
    }

    // ---- Persist position (owned by the vault) ----
    let position = &mut ctx.accounts.position;
    position.user = vault_key;
    position.mint = mint_key;
    position.side = PositionSide::Long;
    position.position_index = args.position_index;
    position.collateral_amount = net_collateral;
    position.debt_amount = desired_borrow_sol;
    position.accrued_interest = 0;
    position.last_slot = now;
    position.bump = position_bump;
    position.vault_bump = vault_bump;

    ctx.accounts.position_token_vault.reload()?;
    emit_cpi!(OpenLongEvent {
        user: vault_key,
        mint: mint_key,
        position_index: args.position_index,
        collateral_tokens: net_collateral,
        borrowed_sol_gross: desired_borrow_sol,
        open_fee_sol: open_fee,
        atomic_buy_sol,
        vault_tokens: ctx.accounts.position_token_vault.amount,
    });
    Ok(())
}

pub fn close_long_via_vault(
    ctx: Context<crate::contexts::CloseLongViaVault>,
    args: crate::contexts::ClosePositionArgs,
) -> Result<()> {
    // ---- Accrue interest, size the repay (D-5 steps 1–2) ----
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveLoan);
    let debt_to_repay = apply_bps(total_debt, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_repay > 0, TorchMarketError::ZeroAmount);

    let vault_tokens = ctx.accounts.position_token_vault.amount;
    let tokens_to_sell = apply_bps(vault_tokens, args.repay_fraction_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(tokens_to_sell > 0, TorchMarketError::ZeroAmount);

    // ---- Atomic sell: vault tokens → pool, SOL proceeds → long_sol_vault ----
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[ctx.accounts.position.bump],
    ];
    let vault_bump = ctx.bumps.long_sol_vault;
    let vault_seeds: &[&[u8]] = &[
        LONG_SOL_VAULT_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &idx_le,
        &[vault_bump],
    ];
    let cpi_signers = &[position_seeds, vault_seeds][..];

    let vault_sol_before = ctx.accounts.long_sol_vault.lamports();
    let swap_accounts = deep_pool::cpi::accounts::Swap {
        user: ctx.accounts.position.to_account_info(),
        sol_source: ctx.accounts.long_sol_vault.to_account_info(),
        pool: ctx.accounts.deep_pool.to_account_info(),
        token_mint: ctx.accounts.mint.to_account_info(),
        token_vault: ctx.accounts.deep_pool_token_vault.to_account_info(),
        user_token_account: ctx.accounts.position_token_vault.to_account_info(),
        token_program: ctx.accounts.token_2022_program.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        event_authority: ctx.accounts.deep_pool_event_authority.to_account_info(),
        program: ctx.accounts.deep_pool_program.to_account_info(),
    };
    deep_pool::cpi::swap(
        CpiContext::new_with_signer(
            ctx.accounts.deep_pool_program.to_account_info(),
            swap_accounts,
            cpi_signers,
        ),
        deep_pool::SwapArgs {
            amount_in: tokens_to_sell,
            minimum_out: 1,
            buy: false,
        },
    )?;
    let sol_out = ctx
        .accounts
        .long_sol_vault
        .lamports()
        .checked_sub(vault_sol_before)
        .ok_or(TorchMarketError::MathOverflow)?;

    require!(sol_out >= debt_to_repay, TorchMarketError::NotLiquidatable);

    // ---- Split: debt → treasury, surplus → vault_sol (D-5 steps 6–9) ----
    let surplus_sol = sol_out - debt_to_repay;
    require!(
        surplus_sol >= args.min_surplus_sol_out,
        TorchMarketError::SlippageExceeded
    );

    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.long_sol_vault.to_account_info(),
                to: ctx.accounts.treasury_sol_vault.to_account_info(),
            },
            &[vault_seeds],
        ),
        debt_to_repay,
    )?;
    if surplus_sol > 0 {
        // Surplus P&L returns to the VAULT (the linked wallet can't skim it).
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.long_sol_vault.to_account_info(),
                    to: ctx.accounts.vault_sol.to_account_info(),
                },
                &[vault_seeds],
            ),
            surplus_sol,
        )?;
    }

    // ---- Apply debt credit + treasury accounting ----
    let interest_paid = debt_to_repay.min(ctx.accounts.position.accrued_interest);
    let principal_paid = debt_to_repay - interest_paid;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
    }
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs =
            treasury.total_sol_lent_to_longs.saturating_sub(principal_paid);
        treasury.long_interest_collected = treasury
            .long_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the vault's per-user cap headroom.
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.long_sol_debt = risk.long_sol_debt.saturating_sub(principal_paid);
    }

    // ---- Full close: release collateral bookkeeping, close vaults + position ----
    let fully_closed =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    if fully_closed {
        {
            let treasury = &mut ctx.accounts.treasury;
            treasury.total_token_collateral_locked = treasury
                .total_token_collateral_locked
                .saturating_sub(ctx.accounts.position.collateral_amount);
            treasury.active_longs = treasury.active_longs.saturating_sub(1);
        }
        // [F-1] Release the closed position's collateral from the formula-cap basis.
        {
            let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
            risk.long_collateral_tokens = risk
                .long_collateral_tokens
                .saturating_sub(ctx.accounts.position.collateral_amount);
        }
        // Harvest withheld fees, then close the empty token vault → signer (rent).
        anchor_lang::solana_program::program::invoke(
            &crate::token_2022_utils::build_harvest_withheld_tokens_to_mint_instruction(
                &ctx.accounts.mint.key(),
                &[ctx.accounts.position_token_vault.key()],
            ),
            &[
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.position_token_vault.to_account_info(),
            ],
        )?;
        ctx.accounts.position_token_vault.reload()?;
        anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            anchor_spl::token_interface::CloseAccount {
                account: ctx.accounts.position_token_vault.to_account_info(),
                destination: ctx.accounts.signer.to_account_info(),
                authority: ctx.accounts.position.to_account_info(),
            },
            &[position_seeds],
        ))?;
        // Position rent → signer (the linked wallet's own gas, not vault funds).
        let position_ai = ctx.accounts.position.to_account_info();
        let signer_ai = ctx.accounts.signer.to_account_info();
        close_account(&position_ai, &signer_ai)?;
    }

    emit_cpi!(CloseLongEvent {
        user: vault_key,
        mint: mint_key,
        position_index: args.position_index,
        tokens_sold: tokens_to_sell,
        sol_out,
        debt_repaid: debt_to_repay,
        interest_paid,
        principal_paid,
        surplus_sol_to_user: surplus_sol,
        fully_closed,
    });
    Ok(())
}

pub fn liquidate_long_via_vault(
    ctx: Context<crate::contexts::LiquidateLongViaVault>,
    args: crate::contexts::LiquidatePositionArgs,
) -> Result<()> {
    let now = Clock::get()?.slot;
    let (accrued, _) = apply_interest_accrual(
        ctx.accounts.position.debt_amount,
        ctx.accounts.position.accrued_interest,
        ctx.accounts.position.last_slot,
        now,
        ctx.accounts.treasury.interest_rate_bps,
    )
    .ok_or(TorchMarketError::MathOverflow)?;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest = accrued;
        position.last_slot = now;
    }
    let total_debt = ctx
        .accounts
        .position
        .debt_amount
        .checked_add(accrued)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(total_debt > 0, TorchMarketError::NoActiveLoan);

    let vault_tokens = ctx.accounts.position_token_vault.amount;
    let (pool_sol, pool_tokens) = read_deep_pool_reserves(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
    )?;
    let threshold = ctx.accounts.treasury.liquidation_threshold_bps as u64;

    // ---- D-10 trigger: LTV-at-TWAP (vault token value at the DeepPool mark) ----
    let price_q64 = read_twap_price_q64(
        &ctx.accounts.deep_pool,
        &ctx.accounts.deep_pool_token_vault,
        now,
    )?
    .ok_or(TorchMarketError::NotLiquidatable)?;
    let vault_value_at_mark = twap_value_in_sol(vault_tokens, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let twap_ltv =
        calc_ltv_bps(total_debt, vault_value_at_mark).ok_or(TorchMarketError::MathOverflow)?;
    require!(twap_ltv > threshold, TorchMarketError::NotLiquidatable);

    // Asymmetric spot veto (D-10): TWAP is the binding trigger; spot may only refuse
    // when CLEARLY healthy (LIQ_SPOT_VETO_MARGIN_BPS below threshold) — see liquidate_short.
    let vault_value_spot = calc_collateral_value(vault_tokens, pool_sol, pool_tokens)
        .ok_or(TorchMarketError::MathOverflow)?;
    let spot_ltv =
        calc_ltv_bps(total_debt, vault_value_spot).ok_or(TorchMarketError::MathOverflow)?;
    require!(
        spot_ltv > threshold.saturating_sub(LIQ_SPOT_VETO_MARGIN_BPS),
        TorchMarketError::NotLiquidatable
    );

    // ---- Size cover + seize, priced at the mark (D-10 seize clamp) ----
    let debt_to_cover = apply_bps(total_debt, ctx.accounts.treasury.liquidation_close_bps)
        .ok_or(TorchMarketError::MathOverflow)?;
    require!(debt_to_cover > 0, TorchMarketError::ZeroAmount);
    let bonus_bps = effective_liq_bonus_bps(
        twap_ltv,
        ctx.accounts.treasury.liquidation_threshold_bps,
        LIQ_FULL_BONUS_LTV_BPS,
        ctx.accounts.treasury.liquidation_bonus_bps,
    ) as u16;
    let target_seize = twap_tokens_to_seize(debt_to_cover, bonus_bps, price_q64)
        .ok_or(TorchMarketError::MathOverflow)?;
    let actual_seize = target_seize.min(vault_tokens);

    // Insolvent iff the vault can't fund the full target seize (or degenerate
    // target==0) — residual forgiven in full below so the position fully resolves.
    let insolvent = target_seize == 0 || actual_seize < target_seize;
    let actual_debt_covered = if target_seize == 0 {
        0u64
    } else if actual_seize < target_seize {
        calc_short_partial_seize_proration(debt_to_cover, actual_seize, target_seize)
            .ok_or(TorchMarketError::MathOverflow)?
    } else {
        debt_to_cover
    };

    // ---- Liquidator pays SOL → treasury_sol_vault (recoups principal + interest) ----
    if actual_debt_covered > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.liquidator.to_account_info(),
                    to: ctx.accounts.treasury_sol_vault.to_account_info(),
                },
            ),
            actual_debt_covered,
        )?;
    }

    // ---- Seize vault tokens → liquidator (position ATA → liquidator ATA) ----
    let mint_key = ctx.accounts.mint.key();
    let vault_key = ctx.accounts.torch_vault.key();
    let idx_le = args.position_index.to_le_bytes();
    let position_seeds: &[&[u8]] = &[
        POSITION_SEED,
        vault_key.as_ref(),
        mint_key.as_ref(),
        &[POSITION_SIDE_LONG],
        &idx_le,
        &[ctx.accounts.position.bump],
    ];
    if actual_seize > 0 {
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_2022_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.position_token_vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.liquidator_token_account.to_account_info(),
                    authority: ctx.accounts.position.to_account_info(),
                },
                &[position_seeds],
            ),
            actual_seize,
            ctx.accounts.mint.decimals,
        )?;
    }

    // ---- Apply debt credit + bad-debt write-off ----
    let interest_paid = actual_debt_covered.min(ctx.accounts.position.accrued_interest);
    let principal_paid = actual_debt_covered - interest_paid;
    let bad_debt;
    {
        let position = &mut ctx.accounts.position;
        position.accrued_interest -= interest_paid;
        position.debt_amount = position.debt_amount.saturating_sub(principal_paid);
        if insolvent {
            // No collateral remains — forgive the entire residual so the position
            // fully resolves (no tail, total_sol_lent_to_longs stays true).
            bad_debt = position.debt_amount;
            position.debt_amount = 0;
            position.accrued_interest = 0;
        } else {
            bad_debt = 0;
        }
    }
    {
        let treasury = &mut ctx.accounts.treasury;
        treasury.total_sol_lent_to_longs = treasury
            .total_sol_lent_to_longs
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
        treasury.long_interest_collected = treasury
            .long_interest_collected
            .checked_add(interest_paid)
            .ok_or(TorchMarketError::MathOverflow)?;
    }
    // [F-1] Release the vault's per-user cap headroom (repaid + written off).
    {
        let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
        risk.long_sol_debt = risk
            .long_sol_debt
            .saturating_sub(principal_paid)
            .saturating_sub(bad_debt);
    }

    // ---- Full liquidation: residual tokens → vault ATA, rent → vault_sol, close ----
    let fully_liquidated =
        ctx.accounts.position.debt_amount == 0 && ctx.accounts.position.accrued_interest == 0;
    if fully_liquidated {
        ctx.accounts.position_token_vault.reload()?;
        let residual = ctx.accounts.position_token_vault.amount;
        if residual > 0 {
            // Residual equity tokens flow back to the vault's own ATA (stay in vault).
            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_2022_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.position_token_vault.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        to: ctx.accounts.vault_token_account.to_account_info(),
                        authority: ctx.accounts.position.to_account_info(),
                    },
                    &[position_seeds],
                ),
                residual,
                ctx.accounts.mint.decimals,
            )?;
        }
        // Harvest withheld fees so the now-empty vault can close → vault_sol (rent).
        anchor_lang::solana_program::program::invoke(
            &crate::token_2022_utils::build_harvest_withheld_tokens_to_mint_instruction(
                &ctx.accounts.mint.key(),
                &[ctx.accounts.position_token_vault.key()],
            ),
            &[
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.position_token_vault.to_account_info(),
            ],
        )?;
        ctx.accounts.position_token_vault.reload()?;
        anchor_spl::token_interface::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_2022_program.to_account_info(),
            anchor_spl::token_interface::CloseAccount {
                account: ctx.accounts.position_token_vault.to_account_info(),
                destination: ctx.accounts.vault_sol.to_account_info(),
                authority: ctx.accounts.position.to_account_info(),
            },
            &[position_seeds],
        ))?;
        {
            let treasury = &mut ctx.accounts.treasury;
            treasury.total_token_collateral_locked = treasury
                .total_token_collateral_locked
                .saturating_sub(ctx.accounts.position.collateral_amount);
            treasury.active_longs = treasury.active_longs.saturating_sub(1);
        }
        // [F-1] Release the resolved position's collateral from the formula-cap basis.
        {
            let mut risk = user_risk_mut(&ctx.accounts.user_risk)?;
            risk.long_collateral_tokens = risk
                .long_collateral_tokens
                .saturating_sub(ctx.accounts.position.collateral_amount);
        }
        // Position rent → vault_sol (the position's economic owner).
        let position_ai = ctx.accounts.position.to_account_info();
        let vault_sol_ai = ctx.accounts.vault_sol.to_account_info();
        close_account(&position_ai, &vault_sol_ai)?;
    }

    emit_cpi!(LiquidateLongEvent {
        liquidator: ctx.accounts.liquidator.key(),
        borrower: vault_key,
        mint: mint_key,
        position_index: args.position_index,
        debt_covered: actual_debt_covered,
        tokens_seized: actual_seize,
        bad_debt,
        bonus_bps,
        twap_ltv,
        fully_liquidated,
    });
    Ok(())
}
