use anchor_lang::prelude::*;

use crate::contexts::*;
use crate::migration::migrate_to_dex_handler;

// Migrate bonded token to DeepPool.
// Permissionless — anyone can call once bonding completes. The bonded SOL is
// sourced directly from the System-owned bonding_curve_sol PDA (seed-signed), so
// there is no separate fund step and the raise never transits a user wallet.
// 1. Handles vote vault (burn or return tokens based on community vote)
// 2. Creates DeepPool with tokens + native SOL
// 3. Burns LP tokens to lock liquidity forever
// 4. Revokes mint/freeze/transfer_fee authorities
// 5. Records baseline for sell cycle ratio monitoring
// Caller pays rent for new accounts (~0.003 SOL), reimbursed from treasury.
pub fn migrate_to_dex(ctx: Context<MigrateToDex>) -> Result<()> {
    migrate_to_dex_handler(ctx)
}
