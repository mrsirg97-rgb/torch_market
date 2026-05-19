// One-off: measure on-wire size of every program tx after Tier B/B+ cleanup.
//
// Tx wire format = [signatures] + [message]. Message = recent_blockhash +
// header + account_keys + ixs. None of our cleanup changed account lists or
// arg shapes, so we expect numbers very close to pre-cleanup. Run with:
//   cargo test --test litesvm tx_size -- --nocapture

use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, instruction::Instruction,
    native_token::LAMPORTS_PER_SOL, signature::Keypair, signer::Signer,
    transaction::Transaction,
};

use crate::harness::{Env, TokenCtx, VaultCtx};

fn size(ixs: &[Instruction], signers: &[&Keypair], blockhash: solana_sdk::hash::Hash) -> usize {
    let mut tx = Transaction::new_with_payer(ixs, Some(&signers[0].pubkey()));
    tx.sign(signers, blockhash);
    bincode::serialize(&tx).expect("serialize").len()
}

#[test]
#[ignore = "diagnostic: run with `cargo test --test litesvm tx_size -- --ignored --nocapture`"]
fn tx_sizes_report() {
    let mut env = Env::new();
    let bh = env.latest_blockhash();
    let creator = env.new_funded(2 * LAMPORTS_PER_SOL);
    let t: TokenCtx = env.create_token(&creator, torch_market::constants::BONDING_TARGET_FLAME, false);

    // --- Pre-migration txs ---
    let buyer = env.new_funded(LAMPORTS_PER_SOL);
    let buy_ix = build_buy_ix(&env, &buyer, &t);
    println!("buy                       : {} bytes", size(&[buy_ix], &[&buyer], bh));

    let sell_ix = build_sell_ix(&env, &buyer, &t);
    println!("sell                      : {} bytes", size(&[sell_ix], &[&buyer], bh));

    // --- Vault ---
    let vault_owner = env.new_funded(2 * LAMPORTS_PER_SOL);
    let vault: VaultCtx = env.create_vault(&vault_owner);
    let buy_vault_ix = build_buy_via_vault_ix(&env, &vault_owner, &vault, &t);
    println!("buy_via_vault             : {} bytes", size(&[buy_vault_ix], &[&vault_owner], bh));

    // --- Migration (full 4-ix bundle) ---
    env.bond_to_completion(&t);
    let payer = env.new_funded(2 * LAMPORTS_PER_SOL);
    let migrate_ixs = build_migrate_ixs(&env, &payer, &t);
    println!("migrate (4-ix bundle)     : {} bytes", size(&migrate_ixs, &[&payer], bh));

    println!("(tx limit                 : 1232 bytes)");
}

// --- ix builders (mirror the harness internals so we can measure pre-send) ---

fn build_buy_ix(env: &Env, buyer: &Keypair, t: &TokenCtx) -> Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    use solana_sdk::pubkey::Pubkey;
    use torch_market::{constants::*, token_2022_utils::*};
    let buyer_token = get_associated_token_address_2022(&buyer.pubkey(), &t.mint);
    let (user_position, _) = Pubkey::find_program_address(
        &[USER_POSITION_SEED, t.bonding_curve.as_ref(), buyer.pubkey().as_ref()],
        &torch_market::ID,
    );
    let (user_stats, _) = Pubkey::find_program_address(
        &[USER_STATS_SEED, buyer.pubkey().as_ref()],
        &torch_market::ID,
    );
    Instruction {
        program_id: torch_market::ID,
        accounts: torch_market::accounts::Buy {
            buyer: buyer.pubkey(),
            global_config: env.global_config,
            dev_wallet: env.dev_wallet.pubkey(),
            mint: t.mint,
            bonding_curve: t.bonding_curve,
            token_vault: t.token_vault,
            token_treasury: t.treasury,
            treasury_token_account: t.treasury_token_account,
            buyer_token_account: buyer_token,
            user_position,
            user_stats: Some(user_stats),
            protocol_treasury: env.protocol_treasury,
            creator: t.creator,
            token_program: TOKEN_2022_PROGRAM_ID,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: torch_market::instruction::Buy {
            args: torch_market::contexts::BuyArgs {
                sol_amount: 100_000_000,
                min_tokens_out: 0,
            },
        }
        .data(),
    }
}

fn build_sell_ix(env: &Env, seller: &Keypair, t: &TokenCtx) -> Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    use solana_sdk::pubkey::Pubkey;
    use torch_market::{constants::*, token_2022_utils::*};
    let _ = env;
    let seller_token = get_associated_token_address_2022(&seller.pubkey(), &t.mint);
    let (user_position, _) = Pubkey::find_program_address(
        &[USER_POSITION_SEED, t.bonding_curve.as_ref(), seller.pubkey().as_ref()],
        &torch_market::ID,
    );
    let (user_stats, _) = Pubkey::find_program_address(
        &[USER_STATS_SEED, seller.pubkey().as_ref()],
        &torch_market::ID,
    );
    Instruction {
        program_id: torch_market::ID,
        accounts: torch_market::accounts::Sell {
            seller: seller.pubkey(),
            mint: t.mint,
            bonding_curve: t.bonding_curve,
            token_vault: t.token_vault,
            seller_token_account: seller_token,
            user_position: Some(user_position),
            token_treasury: t.treasury,
            user_stats: Some(user_stats),
            protocol_treasury: Some(crate::harness::Env::new().protocol_treasury), // not used in size calc
            token_program: TOKEN_2022_PROGRAM_ID,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: torch_market::instruction::Sell {
            args: torch_market::contexts::SellArgs {
                token_amount: 100_000_000,
                min_sol_out: 0,
            },
        }
        .data(),
    }
}

fn build_buy_via_vault_ix(env: &Env, signer: &Keypair, vault: &VaultCtx, t: &TokenCtx) -> Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    use solana_sdk::pubkey::Pubkey;
    use torch_market::{constants::*, token_2022_utils::*};
    let vault_token = get_associated_token_address_2022(&vault.vault, &t.mint);
    let (user_position, _) = Pubkey::find_program_address(
        &[USER_POSITION_SEED, t.bonding_curve.as_ref(), signer.pubkey().as_ref()],
        &torch_market::ID,
    );
    let (user_stats, _) = Pubkey::find_program_address(
        &[USER_STATS_SEED, signer.pubkey().as_ref()],
        &torch_market::ID,
    );
    let (link, _) = Pubkey::find_program_address(
        &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
        &torch_market::ID,
    );
    Instruction {
        program_id: torch_market::ID,
        accounts: torch_market::accounts::BuyViaVault {
            buyer: signer.pubkey(),
            global_config: env.global_config,
            dev_wallet: env.dev_wallet.pubkey(),
            mint: t.mint,
            bonding_curve: t.bonding_curve,
            token_vault: t.token_vault,
            token_treasury: t.treasury,
            treasury_token_account: t.treasury_token_account,
            user_position,
            user_stats: Some(user_stats),
            protocol_treasury: env.protocol_treasury,
            creator: t.creator,
            torch_vault: vault.vault,
            vault_wallet_link: link,
            vault_token_account: vault_token,
            token_program: TOKEN_2022_PROGRAM_ID,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: torch_market::instruction::BuyViaVault {
            args: torch_market::contexts::BuyArgs {
                sol_amount: 100_000_000,
                min_tokens_out: 0,
            },
        }
        .data(),
    }
}

fn build_migrate_ixs(env: &Env, payer: &Keypair, t: &TokenCtx) -> Vec<Instruction> {
    use anchor_lang::{InstructionData, ToAccountMetas};
    use torch_market::{pool_validation, token_2022_utils::*};
    let payer_token = get_associated_token_address_2022(&payer.pubkey(), &t.mint);
    let payer_lp = get_associated_token_address_2022(&payer.pubkey(), &t.deep_pool_lp_mint);
    let pool_lp = get_associated_token_address_2022(&t.deep_pool, &t.deep_pool_lp_mint);

    let cu = ComputeBudgetInstruction::set_compute_unit_limit(600_000);
    let ata = build_create_associated_token_account_instruction(
        &payer.pubkey(),
        &payer.pubkey(),
        &t.mint,
    );
    let fund = Instruction {
        program_id: torch_market::ID,
        accounts: torch_market::accounts::FundMigrationSol {
            payer: payer.pubkey(),
            mint: t.mint,
            bonding_curve: t.bonding_curve,
        }
        .to_account_metas(None),
        data: torch_market::instruction::FundMigrationSol {}.data(),
    };
    let migrate = Instruction {
        program_id: torch_market::ID,
        accounts: torch_market::accounts::MigrateToDex {
            payer: payer.pubkey(),
            global_config: env.global_config,
            mint: t.mint,
            bonding_curve: t.bonding_curve,
            treasury: t.treasury,
            token_vault: t.token_vault,
            treasury_token_account: t.treasury_token_account,
            treasury_lock_token_account: t.treasury_lock_token_account,
            treasury_lock: t.treasury_lock,
            payer_token,
            deep_pool_program: deep_pool::ID,
            torch_config: env.torch_config,
            deep_pool: t.deep_pool,
            deep_pool_token_vault: t.deep_pool_token_vault,
            deep_pool_lp_mint: t.deep_pool_lp_mint,
            payer_lp_account: payer_lp,
            deep_pool_lp_account: pool_lp,
            deep_pool_event_authority: pool_validation::derive_deep_pool_event_authority(),
            token_program: TOKEN_2022_PROGRAM_ID,
            token_2022_program: TOKEN_2022_PROGRAM_ID,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: torch_market::instruction::MigrateToDex {}.data(),
    };
    vec![cu, ata, fund, migrate]
}
