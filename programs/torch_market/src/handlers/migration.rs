use anchor_lang::prelude::*;

use crate::contexts::*;
use crate::migration::{fund_migration_wsol_handler, migrate_to_dex_handler};

// Fund bonding curve's WSOL ATA with bonding curve SOL.
// Must be called BEFORE migrate_to_dex in the same transaction.
// Isolates direct lamport manipulation from CPIs.
pub fn fund_migration_wsol(ctx: Context<FundMigrationWsol>) -> Result<()> {
    fund_migration_wsol_handler(ctx)
}

// Migrate bonded token to Raydium CPMM DEX.
//
// Permissionless — anyone can call once bonding completes.
// bc_wsol must be pre-funded via fund_migration_wsol, then:
// 1. Handles vote vault (burn or return tokens based on community vote)
// 2. Creates Raydium CPMM pool with tokens + WSOL
// 3. Burns LP tokens to lock liquidity forever
// 4. Marks token as migrated
// 5. Sets baseline for sell cycle ratio monitoring
//
// Caller pays rent for new accounts (~0.02 SOL) + Raydium fee (0.15 SOL).
// Treasury must have at least 0.15 SOL accumulated.
pub fn migrate_to_dex(ctx: Context<MigrateToDex>) -> Result<()> {
    migrate_to_dex_handler(ctx)
}
