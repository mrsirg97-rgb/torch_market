use anchor_lang::prelude::*;

use crate::contexts::*;
use crate::migration::{fund_migration_sol_handler, migrate_to_dex_handler};

// Fund payer with bonding curve SOL for DeepPool pool creation.
// Must be called BEFORE migrate_to_dex in the same transaction.
// Isolates direct lamport manipulation from CPIs.
pub fn fund_migration_sol(ctx: Context<FundMigrationSol>) -> Result<()> {
    fund_migration_sol_handler(ctx)
}

// Migrate bonded token to DeepPool.
// Permissionless — anyone can call once bonding completes.
// Payer must be pre-funded via fund_migration_sol, then:
// 1. Handles vote vault (burn or return tokens based on community vote)
// 2. Creates DeepPool with tokens + native SOL
// 3. Burns LP tokens to lock liquidity forever
// 4. Revokes mint/freeze/transfer_fee authorities
// 5. Records baseline for sell cycle ratio monitoring
// Caller pays rent for new accounts (~0.003 SOL), reimbursed from treasury.
pub fn migrate_to_dex(ctx: Context<MigrateToDex>) -> Result<()> {
    migrate_to_dex_handler(ctx)
}
