// Harness for litesvm integration tests.
//
// Env::new bootstraps a fresh LiteSVM with deep_pool + torch_market loaded
// and global_config / protocol_treasury initialized. Helpers wrap each
// handler in a typed `Result<()>` so tests stay terse.

#![allow(dead_code)] // helpers used by submodules added incrementally

use std::path::PathBuf;

use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use litesvm::LiteSVM;
use solana_sdk::{
    account::{Account, ReadableAccount},
    compute_budget::ComputeBudgetInstruction,
    hash::Hash,
    instruction::{AccountMeta, Instruction, InstructionError},
    native_token::LAMPORTS_PER_SOL,
    signature::Keypair,
    signer::Signer,
    system_program,
    transaction::{Transaction, TransactionError},
};

use torch_market::{
    constants::*,
    pool_validation,
    state::{
        BondingCurve, GlobalConfig, LoanPosition, ProtocolTreasury, ShortPosition, TorchVault,
        Treasury,
    },
    token_2022_utils::{get_associated_token_address_2022, TOKEN_2022_PROGRAM_ID},
};

// ============================================================================
// File paths
// ============================================================================

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = programs/torch_market
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // programs/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .to_path_buf()
}

fn torch_market_so() -> Vec<u8> {
    let path = workspace_root().join("target/deploy/torch_market.so");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "torch_market.so missing at {:?}: {}. Run `cargo build-sbf` first.",
            path, e
        )
    })
}

fn deep_pool_so() -> Vec<u8> {
    let path = std::env::var("DEEP_POOL_SO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            workspace_root()
                .parent()
                .unwrap()
                .join("deep_pool/target/deploy/deep_pool.so")
        });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "deep_pool.so missing at {:?}: {}. Build deep_pool first, or set DEEP_POOL_SO_PATH.",
            path, e
        )
    })
}

// ============================================================================
// Env
// ============================================================================

pub struct Env {
    pub svm: LiteSVM,
    pub authority: Keypair, // protocol authority (global_config.authority)
    pub treasury_wallet: Keypair, // global_config.treasury
    pub dev_wallet: Keypair, // global_config.dev_wallet
    pub global_config: Pubkey,
    pub protocol_treasury: Pubkey,
    pub torch_config: Pubkey, // namespace PDA for deep_pool pools
}

impl Env {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(torch_market::ID, &torch_market_so())
            .expect("add torch_market program");
        svm.add_program(deep_pool::ID, &deep_pool_so())
            .expect("add deep_pool program");

        let authority = Keypair::new();
        let treasury_wallet = Keypair::new();
        let dev_wallet = Keypair::new();
        svm.airdrop(&authority.pubkey(), 100 * LAMPORTS_PER_SOL)
            .unwrap();

        let (global_config, _) =
            Pubkey::find_program_address(&[GLOBAL_CONFIG_SEED], &torch_market::ID);
        let (protocol_treasury, _) =
            Pubkey::find_program_address(&[PROTOCOL_TREASURY_SEED], &torch_market::ID);
        let (torch_config, _) =
            Pubkey::find_program_address(&[TORCH_CONFIG_SEED], &torch_market::ID);

        let mut env = Env {
            svm,
            authority,
            treasury_wallet,
            dev_wallet,
            global_config,
            protocol_treasury,
            torch_config,
        };

        env.send_init_global_config();
        env.send_init_protocol_treasury();
        env
    }

    // -----------------------------------------------------------------------
    // Bootstrap (called by new())
    // -----------------------------------------------------------------------

    fn send_init_global_config(&mut self) {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Initialize {
                authority: self.authority.pubkey(),
                global_config: self.global_config,
                treasury: self.treasury_wallet.pubkey(),
                dev_wallet: self.dev_wallet.pubkey(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Initialize {}.data(),
        };
        let authority = clone_keypair(&self.authority);
        self.send(&[ix], &[&authority])
            .expect("init global_config failed");
    }

    fn send_init_protocol_treasury(&mut self) {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::InitializeProtocolTreasury {
                authority: self.authority.pubkey(),
                global_config: self.global_config,
                protocol_treasury: self.protocol_treasury,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::InitializeProtocolTreasury {}.data(),
        };
        let authority = clone_keypair(&self.authority);
        self.send(&[ix], &[&authority])
            .expect("init protocol_treasury failed");
    }

    // -----------------------------------------------------------------------
    // Tx send / sign helpers
    // -----------------------------------------------------------------------

    pub fn latest_blockhash(&self) -> Hash {
        self.svm.latest_blockhash()
    }

    pub fn send(
        &mut self,
        ixs: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<(), TransactionError> {
        let payer = signers
            .first()
            .expect("at least one signer (payer)")
            .pubkey();
        // Advance the blockhash so back-to-back identical txs don't dedupe to
        // `AlreadyProcessed`. In litesvm the clock doesn't auto-tick.
        self.svm.expire_blockhash();
        let mut tx = Transaction::new_with_payer(ixs, Some(&payer));
        tx.sign(signers, self.latest_blockhash());
        match self.svm.send_transaction(tx) {
            Ok(_) => Ok(()),
            Err(failed) => {
                if std::env::var("LITESVM_LOGS").is_ok() {
                    eprintln!("--- tx failed: {:?} ---", failed.err);
                    for line in &failed.meta.logs {
                        eprintln!("{}", line);
                    }
                }
                Err(failed.err)
            }
        }
    }

    pub fn airdrop(&mut self, to: &Pubkey, lamports: u64) {
        self.svm.airdrop(to, lamports).unwrap();
    }

    pub fn new_funded(&mut self, lamports: u64) -> Keypair {
        let k = Keypair::new();
        self.airdrop(&k.pubkey(), lamports);
        k
    }

    // -----------------------------------------------------------------------
    // Typed account accessors
    // -----------------------------------------------------------------------

    pub fn get_global_config(&self) -> GlobalConfig {
        deserialize_anchor(&self.svm, &self.global_config)
    }

    pub fn get_protocol_treasury(&self) -> ProtocolTreasury {
        deserialize_anchor(&self.svm, &self.protocol_treasury)
    }

    pub fn get_bonding_curve(&self, t: &TokenCtx) -> BondingCurve {
        deserialize_anchor(&self.svm, &t.bonding_curve)
    }

    pub fn get_treasury(&self, t: &TokenCtx) -> Treasury {
        deserialize_anchor(&self.svm, &t.treasury)
    }

    pub fn get_loan(&self, t: &TokenCtx, borrower: &Pubkey) -> Option<LoanPosition> {
        let (addr, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        try_deserialize_anchor(&self.svm, &addr)
    }

    pub fn get_short(&self, t: &TokenCtx, shorter: &Pubkey) -> Option<ShortPosition> {
        let (addr, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), shorter.as_ref()],
            &torch_market::ID,
        );
        try_deserialize_anchor(&self.svm, &addr)
    }

    pub fn get_torch_vault(&self, vault: &Pubkey) -> TorchVault {
        deserialize_anchor(&self.svm, vault)
    }

    // -----------------------------------------------------------------------
    // Token creation
    // -----------------------------------------------------------------------

    pub fn create_token(&mut self, creator: &Keypair, target: u64, community: bool) -> TokenCtx {
        let mint = Keypair::new();
        let mint_key = mint.pubkey();

        let (bonding_curve, _) = Pubkey::find_program_address(
            &[BONDING_CURVE_SEED, mint_key.as_ref()],
            &torch_market::ID,
        );
        let (treasury, _) =
            Pubkey::find_program_address(&[TREASURY_SEED, mint_key.as_ref()], &torch_market::ID);
        let (treasury_lock, _) = Pubkey::find_program_address(
            &[TREASURY_LOCK_SEED, mint_key.as_ref()],
            &torch_market::ID,
        );

        let token_vault = get_associated_token_address_2022(&bonding_curve, &mint_key);
        let treasury_token_account = get_associated_token_address_2022(&treasury, &mint_key);
        let treasury_lock_token_account =
            get_associated_token_address_2022(&treasury_lock, &mint_key);

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CreateToken2022 {
                creator: creator.pubkey(),
                global_config: self.global_config,
                mint: mint_key,
                bonding_curve,
                token_vault,
                treasury,
                treasury_token_account,
                treasury_lock,
                treasury_lock_token_account,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
                rent: solana_sdk::sysvar::rent::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CreateToken {
                args: torch_market::contexts::CreateTokenArgs {
                    name: "Test Token".into(),
                    symbol: "TT".into(),
                    uri: "https://example.com/t.json".into(),
                    sol_target: target,
                    community_token: community,
                },
            }
            .data(),
        };
        // create_token chains many CPIs (system_create_account, transfer-fee init,
        // metadata pointer + mint init + metadata, 3× create ATA, 2× mint_to).
        // Default 200k is tight under parallel test scheduling — bump for margin.
        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
        self.send(&[bump_cu, ix], &[creator, &mint])
            .expect("create_token failed");

        let deep_pool = pool_validation::derive_deep_pool(&self.torch_config, &mint_key);
        let deep_pool_token_vault = pool_validation::derive_deep_pool_vault(&deep_pool);
        let deep_pool_lp_mint = pool_validation::derive_deep_pool_lp_mint(&deep_pool);

        TokenCtx {
            creator: creator.pubkey(),
            mint: mint_key,
            bonding_curve,
            treasury,
            treasury_lock,
            token_vault,
            treasury_token_account,
            treasury_lock_token_account,
            deep_pool,
            deep_pool_token_vault,
            deep_pool_lp_mint,
        }
    }

    // -----------------------------------------------------------------------
    // Buy / Sell
    // -----------------------------------------------------------------------

    pub fn buy(
        &mut self,
        buyer: &Keypair,
        t: &TokenCtx,
        sol_amount: u64,
        min_tokens_out: u64,
    ) -> Result<(), TransactionError> {
        let buyer_token_account = get_associated_token_address_2022(&buyer.pubkey(), &t.mint);
        let (user_position, _) = Pubkey::find_program_address(
            &[
                USER_POSITION_SEED,
                t.bonding_curve.as_ref(),
                buyer.pubkey().as_ref(),
            ],
            &torch_market::ID,
        );
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, buyer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Buy {
                buyer: buyer.pubkey(),
                global_config: self.global_config,
                dev_wallet: self.dev_wallet.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_token_account: t.treasury_token_account,
                buyer_token_account,
                user_position,
                user_stats: Some(user_stats),
                protocol_treasury: self.protocol_treasury,
                creator: t.creator,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Buy {
                args: torch_market::contexts::BuyArgs {
                    sol_amount,
                    min_tokens_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[buyer])
    }

    /// Buy with explicit override accounts — for negative tests that need to
    /// substitute one of the constraint-checked accounts (e.g., dev_wallet).
    /// Pass `None` for any field you want defaults.
    #[allow(clippy::too_many_arguments)]
    pub fn buy_with_overrides(
        &mut self,
        buyer: &Keypair,
        t: &TokenCtx,
        sol_amount: u64,
        min_tokens_out: u64,
        dev_wallet_override: Option<Pubkey>,
        creator_override: Option<Pubkey>,
    ) -> Result<(), TransactionError> {
        let buyer_token_account = get_associated_token_address_2022(&buyer.pubkey(), &t.mint);
        let (user_position, _) = Pubkey::find_program_address(
            &[
                USER_POSITION_SEED,
                t.bonding_curve.as_ref(),
                buyer.pubkey().as_ref(),
            ],
            &torch_market::ID,
        );
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, buyer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Buy {
                buyer: buyer.pubkey(),
                global_config: self.global_config,
                dev_wallet: dev_wallet_override.unwrap_or(self.dev_wallet.pubkey()),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_token_account: t.treasury_token_account,
                buyer_token_account,
                user_position,
                user_stats: Some(user_stats),
                protocol_treasury: self.protocol_treasury,
                creator: creator_override.unwrap_or(t.creator),
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Buy {
                args: torch_market::contexts::BuyArgs {
                    sol_amount,
                    min_tokens_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[buyer])
    }

    pub fn buy_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_amount: u64,
        min_tokens_out: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        // Ensure vault ATA exists (handler doesn't init).
        if !self.account_exists(&vault_token_account) {
            use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
            let create_ata_ix = build_create_associated_token_account_instruction(
                &signer.pubkey(),
                &vault.vault,
                &t.mint,
            );
            self.send(&[create_ata_ix], &[signer])?;
        }
        let (user_position, _) = Pubkey::find_program_address(
            &[
                USER_POSITION_SEED,
                t.bonding_curve.as_ref(),
                signer.pubkey().as_ref(),
            ],
            &torch_market::ID,
        );
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::BuyViaVault {
                buyer: signer.pubkey(),
                global_config: self.global_config,
                dev_wallet: self.dev_wallet.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_token_account: t.treasury_token_account,
                user_position,
                user_stats: Some(user_stats),
                protocol_treasury: self.protocol_treasury,
                creator: t.creator,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::BuyViaVault {
                args: torch_market::contexts::BuyArgs {
                    sol_amount,
                    min_tokens_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn sell(
        &mut self,
        seller: &Keypair,
        t: &TokenCtx,
        token_amount: u64,
        min_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let seller_token_account = get_associated_token_address_2022(&seller.pubkey(), &t.mint);
        let (user_position, _) = Pubkey::find_program_address(
            &[
                USER_POSITION_SEED,
                t.bonding_curve.as_ref(),
                seller.pubkey().as_ref(),
            ],
            &torch_market::ID,
        );
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, seller.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Sell {
                seller: seller.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                seller_token_account,
                user_position: Some(user_position),
                token_treasury: t.treasury,
                user_stats: Some(user_stats),
                protocol_treasury: Some(self.protocol_treasury),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Sell {
                args: torch_market::contexts::SellArgs {
                    token_amount,
                    min_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[seller])
    }

    pub fn sell_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        token_amount: u64,
        min_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        let (user_position, _) = Pubkey::find_program_address(
            &[
                USER_POSITION_SEED,
                t.bonding_curve.as_ref(),
                signer.pubkey().as_ref(),
            ],
            &torch_market::ID,
        );
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::SellViaVault {
                seller: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                user_position: Some(user_position),
                token_treasury: t.treasury,
                user_stats: Some(user_stats),
                protocol_treasury: Some(self.protocol_treasury),
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::SellViaVault {
                args: torch_market::contexts::SellArgs {
                    token_amount,
                    min_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    // -----------------------------------------------------------------------
    // Vaults
    // -----------------------------------------------------------------------

    /// Create a vault for `creator` (creator becomes both the seed and authority,
    /// and is auto-linked). Returns a VaultCtx with the PDAs.
    pub fn create_vault(&mut self, creator: &Keypair) -> VaultCtx {
        let (vault, _) = Pubkey::find_program_address(
            &[TORCH_VAULT_SEED, creator.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, creator.pubkey().as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CreateVault {
                creator: creator.pubkey(),
                vault,
                wallet_link,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CreateVault {}.data(),
        };
        self.send(&[ix], &[creator]).expect("create_vault");
        VaultCtx {
            creator: creator.pubkey(),
            vault,
            authority_creator_link: wallet_link,
        }
    }

    pub fn deposit_vault(
        &mut self,
        depositor: &Keypair,
        vault: &VaultCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::DepositVault {
                depositor: depositor.pubkey(),
                vault: vault.vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::DepositVault { sol_amount }.data(),
        };
        self.send(&[ix], &[depositor])
    }

    pub fn withdraw_vault(
        &mut self,
        authority: &Keypair,
        vault: &VaultCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::WithdrawVault {
                authority: authority.pubkey(),
                vault: vault.vault,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::WithdrawVault { sol_amount }.data(),
        };
        self.send(&[ix], &[authority])
    }

    pub fn unlink_wallet(
        &mut self,
        authority: &Keypair,
        vault: &VaultCtx,
        wallet_to_unlink: Pubkey,
    ) -> Result<(), TransactionError> {
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, wallet_to_unlink.as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::UnlinkWallet {
                authority: authority.pubkey(),
                vault: vault.vault,
                wallet_to_unlink,
                wallet_link,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::UnlinkWallet {}.data(),
        };
        self.send(&[ix], &[authority])
    }

    pub fn transfer_vault_authority(
        &mut self,
        authority: &Keypair,
        vault: &VaultCtx,
        new_authority: Pubkey,
    ) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::TransferVaultAuthority {
                authority: authority.pubkey(),
                vault: vault.vault,
                new_authority,
            }
            .to_account_metas(None),
            data: torch_market::instruction::TransferAuthority {}.data(),
        };
        self.send(&[ix], &[authority])
    }

    pub fn link_wallet(
        &mut self,
        authority: &Keypair,
        vault: &VaultCtx,
        wallet_to_link: Pubkey,
    ) -> Result<(), TransactionError> {
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, wallet_to_link.as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LinkWallet {
                authority: authority.pubkey(),
                vault: vault.vault,
                wallet_to_link,
                wallet_link,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LinkWallet {}.data(),
        };
        self.send(&[ix], &[authority])
    }

    // -----------------------------------------------------------------------
    // Reclaim / clock
    // -----------------------------------------------------------------------

    pub fn reclaim_failed_token(
        &mut self,
        payer: &Keypair,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::ReclaimFailedToken {
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_treasury: t.treasury,
                protocol_treasury: self.protocol_treasury,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::ReclaimFailedToken {}.data(),
        };
        self.send(&[ix], &[payer])
    }

    pub fn contribute_revival(
        &mut self,
        contributor: &Keypair,
        t: &TokenCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::ContributeRevival {
                contributor: contributor.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::ContributeRevival { sol_amount }.data(),
        };
        self.send(&[ix], &[contributor])
    }

    /// Re-serialize an Anchor account at `addr` with `new` state, preserving
    /// the original lamports/owner/executable. For tests that need to put the
    /// chain in a specific state (e.g., poke treasury.sol_balance to trigger
    /// InsufficientMigrationFee). Test-only.
    pub fn poke_anchor<T: anchor_lang::AccountSerialize>(&mut self, addr: Pubkey, new: T) {
        let acct = self
            .svm
            .get_account(&addr)
            .unwrap_or_else(|| panic!("account {} not found", addr));
        let mut data = vec![0u8; acct.data().len()];
        let mut writer = std::io::Cursor::new(&mut data[..]);
        new.try_serialize(&mut writer).expect("anchor serialize");
        let new_acct = Account {
            lamports: acct.lamports(),
            data,
            owner: *acct.owner(),
            executable: acct.executable(),
            rent_epoch: acct.rent_epoch(),
        };
        self.svm.set_account(addr, new_acct).expect("set_account");
    }

    /// Advance the clock to a future slot (warp). Used for time-gated tests:
    /// reclaim (inactivity period), interest accrual, etc.
    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.svm.get_sysvar::<solana_sdk::clock::Clock>();
        clock.slot = slot;
        self.svm.set_sysvar::<solana_sdk::clock::Clock>(&clock);
    }

    pub fn current_slot(&self) -> u64 {
        self.svm.get_sysvar::<solana_sdk::clock::Clock>().slot
    }

    /// Advance both the slot and the unix_timestamp by `delta_seconds`. Slots
    /// move forward at the standard ~400ms/slot ratio.
    pub fn advance_time(&mut self, delta_seconds: i64) {
        let mut clock = self.svm.get_sysvar::<solana_sdk::clock::Clock>();
        clock.unix_timestamp += delta_seconds;
        clock.slot += (delta_seconds as u64) * 1000 / 400;
        self.svm.set_sysvar::<solana_sdk::clock::Clock>(&clock);
    }

    // -----------------------------------------------------------------------
    // Treasury / protocol_treasury / rewards
    // -----------------------------------------------------------------------

    /// harvest_fees with optional withholding sources passed as remaining_accounts.
    pub fn harvest_fees(
        &mut self,
        payer: &Keypair,
        t: &TokenCtx,
        sources: &[Pubkey],
    ) -> Result<(), TransactionError> {
        let mut metas = torch_market::accounts::HarvestFees {
            payer: payer.pubkey(),
            mint: t.mint,
            bonding_curve: t.bonding_curve,
            token_treasury: t.treasury,
            treasury_token_account: t.treasury_token_account,
            token_2022_program: TOKEN_2022_PROGRAM_ID,
            associated_token_program: spl_associated_token_account_id(),
        }
        .to_account_metas(None);
        for src in sources {
            metas.push(AccountMeta::new(*src, false));
        }
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: metas,
            data: torch_market::instruction::HarvestFees {}.data(),
        };
        self.send(&[ix], &[payer])
    }

    pub fn swap_fees_to_sol(
        &mut self,
        payer: &Keypair,
        t: &TokenCtx,
        minimum_amount_out: u64,
    ) -> Result<(), TransactionError> {
        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::SwapFeesToSol {
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                creator: t.creator,
                treasury: t.treasury,
                treasury_token_account: t.treasury_token_account,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: pool_validation::derive_deep_pool_event_authority(),
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::SwapFeesToSol { minimum_amount_out }.data(),
        };
        self.send(&[bump_cu, ix], &[payer])
    }

    pub fn advance_protocol_epoch(&mut self, payer: &Keypair) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::AdvanceProtocolEpoch {
                payer: payer.pubkey(),
                protocol_treasury: self.protocol_treasury,
            }
            .to_account_metas(None),
            data: torch_market::instruction::AdvanceProtocolEpoch {}.data(),
        };
        self.send(&[ix], &[payer])
    }

    pub fn claim_protocol_rewards(&mut self, user: &Keypair) -> Result<(), TransactionError> {
        let (user_stats, _) = Pubkey::find_program_address(
            &[USER_STATS_SEED, user.pubkey().as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::ClaimProtocolRewards {
                user: user.pubkey(),
                user_stats,
                protocol_treasury: self.protocol_treasury,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::ClaimProtocolRewards {}.data(),
        };
        self.send(&[ix], &[user])
    }

    pub fn star_token(&mut self, user: &Keypair, t: &TokenCtx) -> Result<(), TransactionError> {
        let (star_record, _) = Pubkey::find_program_address(
            &[STAR_RECORD_SEED, user.pubkey().as_ref(), t.mint.as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::StarToken {
                user: user.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_treasury: t.treasury,
                creator: t.creator,
                star_record,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::StarToken {}.data(),
        };
        self.send(&[ix], &[user])
    }

    /// Update a Token-2022 token account's `amount` field directly (bytes 64..72).
    /// Used to stage treasury_token_account state for swap_fees_to_sol tests.
    pub fn poke_token_amount(&mut self, addr: Pubkey, new_amount: u64) {
        let acct = self
            .svm
            .get_account(&addr)
            .unwrap_or_else(|| panic!("token account {} not found", addr));
        let mut data = acct.data().to_vec();
        data[64..72].copy_from_slice(&new_amount.to_le_bytes());
        let new = Account {
            lamports: acct.lamports(),
            data,
            owner: *acct.owner(),
            executable: acct.executable(),
            rent_epoch: acct.rent_epoch(),
        };
        self.svm.set_account(addr, new).expect("set_account");
    }

    // -----------------------------------------------------------------------
    // Bonding / migration
    // -----------------------------------------------------------------------

    /// Buy with a fresh wallet on each iteration until `bonding_curve.bonding_complete`.
    /// Each wallet buys 1 SOL chunks until it hits `MaxWalletExceeded`, then rotates.
    /// Returns the FIRST buyer — they bought when price was lowest and are near the
    /// wallet cap (~19M tokens). Useful as a test actor that holds collateral.
    pub fn bond_to_completion(&mut self, t: &TokenCtx) -> Keypair {
        const SOL_PER_BUY: u64 = LAMPORTS_PER_SOL; // 1 SOL
        const MAX_ITERS: usize = 400;
        let first_buyer = self.new_funded(3 * LAMPORTS_PER_SOL);
        let mut buyer = clone_keypair(&first_buyer);
        let mut iters = 0;
        loop {
            iters += 1;
            assert!(
                iters <= MAX_ITERS,
                "bond_to_completion exceeded {} iters",
                MAX_ITERS
            );
            let bc = self.get_bonding_curve(t);
            if bc.bonding_complete {
                return first_buyer;
            }
            let target = if bc.bonding_target == 0 {
                BONDING_TARGET_LAMPORTS
            } else {
                bc.bonding_target
            };
            let remaining = target.saturating_sub(bc.real_sol_reserves);
            let sol = remaining
                .saturating_mul(125)
                .saturating_div(100)
                .max(MIN_SOL_AMOUNT)
                .min(SOL_PER_BUY);
            match self.buy(&buyer, t, sol, 0) {
                Ok(()) => continue,
                Err(_) => {
                    buyer = self.new_funded(3 * LAMPORTS_PER_SOL);
                }
            }
        }
    }

    /// Single-tx migration matching the SDK shape (packages/sdk/src/transactions.ts:1689):
    /// ComputeBudget(400k) + create payer_token ATA + fund_migration_sol + migrate_to_dex.
    /// Payer signs + pays rent. Treasury must have >= MIN_MIGRATION_SOL.
    pub fn migrate(&mut self, t: &TokenCtx, payer: &Keypair) -> Result<(), TransactionError> {
        use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
        let payer_token = get_associated_token_address_2022(&payer.pubkey(), &t.mint);
        let payer_lp_account =
            get_associated_token_address_2022(&payer.pubkey(), &t.deep_pool_lp_mint);
        let deep_pool_lp_account =
            get_associated_token_address_2022(&t.deep_pool, &t.deep_pool_lp_mint);

        // SDK uses 400k. Bump to 600k in the harness — gives margin against the
        // CU-edge flake observed under parallel test scheduling. Doesn't diverge
        // from production's ix shape; only the explicit budget value.
        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(600_000);
        let create_ata_ix = build_create_associated_token_account_instruction(
            &payer.pubkey(),
            &payer.pubkey(),
            &t.mint,
        );
        let fund_ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::FundMigrationSol {
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
            }
            .to_account_metas(None),
            data: torch_market::instruction::FundMigrationSol {}.data(),
        };
        let migrate_ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::MigrateToDex {
                payer: payer.pubkey(),
                global_config: self.global_config,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                token_vault: t.token_vault,
                treasury_token_account: t.treasury_token_account,
                treasury_lock_token_account: t.treasury_lock_token_account,
                treasury_lock: t.treasury_lock,
                payer_token,
                deep_pool_program: deep_pool::ID,
                torch_config: self.torch_config,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_lp_mint: t.deep_pool_lp_mint,
                payer_lp_account,
                deep_pool_lp_account,
                deep_pool_event_authority: pool_validation::derive_deep_pool_event_authority(),
                token_program: TOKEN_2022_PROGRAM_ID,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::MigrateToDex {}.data(),
        };
        self.send(&[bump_cu, create_ata_ix, fund_ix, migrate_ix], &[payer])
    }

    // -----------------------------------------------------------------------
    // Lending (long): borrow / repay / liquidate
    // -----------------------------------------------------------------------

    pub fn borrow(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        collateral_amount: u64,
        sol_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let borrower_token_account = get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Borrow {
                borrower: borrower.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                collateral_vault,
                borrower_token_account,
                loan_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Borrow {
                args: torch_market::contexts::BorrowArgs {
                    collateral_amount,
                    sol_to_borrow,
                },
            }
            .data(),
        };
        self.send(&[ix], &[borrower])
    }

    pub fn repay(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let borrower_token_account = get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Repay {
                borrower: borrower.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                collateral_vault,
                borrower_token_account,
                loan_position,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Repay { sol_amount }.data(),
        };
        self.send(&[ix], &[borrower])
    }

    pub fn borrow_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        collateral_amount: u64,
        sol_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::BorrowViaVault {
                borrower: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                collateral_vault,
                loan_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::BorrowViaVault {
                args: torch_market::contexts::BorrowArgs {
                    collateral_amount,
                    sol_to_borrow,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn repay_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::RepayViaVault {
                borrower: signer.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                collateral_vault,
                loan_position,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::RepayViaVault { sol_amount }.data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn liquidate_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateViaVault {
                liquidator: signer.pubkey(),
                borrower,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                collateral_vault,
                loan_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateViaVault {}.data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn liquidate(
        &mut self,
        liquidator: &Keypair,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        let (collateral_vault, _) = Pubkey::find_program_address(
            &[COLLATERAL_VAULT_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Liquidate {
                liquidator: liquidator.pubkey(),
                borrower,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                collateral_vault,
                liquidator_token_account,
                loan_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Liquidate {}.data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // -----------------------------------------------------------------------
    // Shorts: open / close
    // -----------------------------------------------------------------------

    pub fn open_short(
        &mut self,
        shorter: &Keypair,
        t: &TokenCtx,
        sol_collateral: u64,
        tokens_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let shorter_token_account = get_associated_token_address_2022(&shorter.pubkey(), &t.mint);
        if !self.account_exists(&shorter_token_account) {
            use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
            let create_ata_ix = build_create_associated_token_account_instruction(
                &shorter.pubkey(),
                &shorter.pubkey(),
                &t.mint,
            );
            self.send(&[create_ata_ix], &[shorter])?;
        }
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), shorter.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenShort {
                shorter: shorter.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                shorter_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShort {
                args: torch_market::contexts::OpenShortArgs {
                    sol_collateral,
                    tokens_to_borrow,
                },
            }
            .data(),
        };
        self.send(&[ix], &[shorter])
    }

    pub fn close_short(
        &mut self,
        shorter: &Keypair,
        t: &TokenCtx,
        token_amount: u64,
    ) -> Result<(), TransactionError> {
        let shorter_token_account = get_associated_token_address_2022(&shorter.pubkey(), &t.mint);
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), shorter.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseShort {
                shorter: shorter.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                shorter_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShort { token_amount }.data(),
        };
        self.send(&[ix], &[shorter])
    }

    pub fn open_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_collateral: u64,
        tokens_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenShortViaVault {
                shorter: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShortViaVault {
                args: torch_market::contexts::OpenShortArgs {
                    sol_collateral,
                    tokens_to_borrow,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn close_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        token_amount: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseShortViaVault {
                shorter: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShortViaVault { token_amount }.data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn liquidate_short(
        &mut self,
        liquidator: &Keypair,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        self.ensure_token2022_ata(liquidator, &liquidator.pubkey(), &t.mint)?;
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateShort {
                liquidator: liquidator.pubkey(),
                borrower,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                liquidator_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateShort {}.data(),
        };
        self.send(&[ix], &[liquidator])
    }

    pub fn liquidate_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.ensure_token2022_ata(signer, &vault.vault, &t.mint)?;
        let (short_config, _) =
            Pubkey::find_program_address(&[SHORT_CONFIG_SEED, t.mint.as_ref()], &torch_market::ID);
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateShortViaVault {
                liquidator: signer.pubkey(),
                borrower,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                torch_vault: vault.vault,
                vault_wallet_link: wallet_link,
                vault_token_account,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateShortViaVault {}.data(),
        };
        self.send(&[ix], &[signer])
    }

    // -----------------------------------------------------------------------
    // Adversarial / testing-only helpers
    // -----------------------------------------------------------------------

    /// Force the deep_pool PDA's lamports to `target_lamports` (preserving its
    /// data and owner). Used to simulate pool drain for pool-thin tests.
    pub fn poke_pool_sol(&mut self, t: &TokenCtx, target_lamports: u64) {
        let acct = self
            .svm
            .get_account(&t.deep_pool)
            .expect("deep_pool not initialized — call migrate first");
        let new = Account {
            lamports: target_lamports,
            data: acct.data().to_vec(),
            owner: *acct.owner(),
            executable: acct.executable(),
            rent_epoch: acct.rent_epoch(),
        };
        self.svm.set_account(t.deep_pool, new).expect("set_account");
    }

    /// Create the Token-2022 ATA at `(owner, mint)` if it doesn't exist yet.
    /// The funding signer pays rent. Idempotent.
    pub fn ensure_token2022_ata(
        &mut self,
        funder: &Keypair,
        owner: &Pubkey,
        mint: &Pubkey,
    ) -> Result<(), TransactionError> {
        use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
        let ata = get_associated_token_address_2022(owner, mint);
        if self.account_exists(&ata) {
            return Ok(());
        }
        let ix = build_create_associated_token_account_instruction(&funder.pubkey(), owner, mint);
        self.send(&[ix], &[funder])
    }

    /// True if an account exists at `addr` and has data.
    pub fn account_exists(&self, addr: &Pubkey) -> bool {
        self.svm
            .get_account(addr)
            .map(|a| !a.data().is_empty())
            .unwrap_or(false)
    }
}

// ============================================================================
// TokenCtx
// ============================================================================

#[derive(Clone, Debug)]
pub struct TokenCtx {
    pub creator: Pubkey,
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub treasury: Pubkey,
    pub treasury_lock: Pubkey,
    pub token_vault: Pubkey,
    pub treasury_token_account: Pubkey,
    pub treasury_lock_token_account: Pubkey,
    pub deep_pool: Pubkey,
    pub deep_pool_token_vault: Pubkey,
    pub deep_pool_lp_mint: Pubkey,
}

#[derive(Clone, Debug)]
pub struct VaultCtx {
    pub creator: Pubkey,
    pub vault: Pubkey,
    pub authority_creator_link: Pubkey,
}

// ============================================================================
// Error assertion
// ============================================================================

/// Extract the Anchor error code (variant index + 6000) from a tx error.
/// Returns None if the failure isn't an InstructionError::Custom.
pub fn anchor_err_code(err: &TransactionError) -> Option<u32> {
    if let TransactionError::InstructionError(_, ix_err) = err {
        if let InstructionError::Custom(code) = ix_err {
            return Some(*code);
        }
    }
    None
}

/// Assert that `result` failed with the expected TorchMarketError variant.
/// `variant_index` is the discriminant of the error in the enum (0-based).
#[macro_export]
macro_rules! expect_err {
    ($result:expr, $variant:expr) => {{
        let res = $result;
        let err = res.expect_err("expected error, got Ok");
        let code = $crate::harness::anchor_err_code(&err)
            .unwrap_or_else(|| panic!("expected Anchor Custom error, got: {:?}", err));
        let expected = ($variant as u32) + 6000;
        assert_eq!(
            code, expected,
            "expected error code {} ({:?}), got {}",
            expected, $variant, code
        );
    }};
}

// ============================================================================
// Internal helpers
// ============================================================================

fn clone_keypair(k: &Keypair) -> Keypair {
    // Keypair doesn't impl Clone (intentional, but inconvenient in tests).
    // to_bytes() round-trip is safe for our test fixtures.
    #[allow(deprecated)]
    Keypair::from_bytes(&k.to_bytes()).unwrap()
}

fn spl_associated_token_account_id() -> Pubkey {
    // Re-export of the ATA program id constant from torch_market's token_2022_utils.
    use torch_market::token_2022_utils::ASSOCIATED_TOKEN_PROGRAM_ID;
    ASSOCIATED_TOKEN_PROGRAM_ID
}

fn deserialize_anchor<T: anchor_lang::AccountDeserialize>(svm: &LiteSVM, addr: &Pubkey) -> T {
    let account = svm
        .get_account(addr)
        .unwrap_or_else(|| panic!("account {} not found", addr));
    let mut data = account.data();
    T::try_deserialize(&mut data).expect("anchor deserialize failed")
}

fn try_deserialize_anchor<T: anchor_lang::AccountDeserialize>(
    svm: &LiteSVM,
    addr: &Pubkey,
) -> Option<T> {
    let account = svm.get_account(addr)?;
    let mut data = account.data();
    T::try_deserialize(&mut data).ok()
}
