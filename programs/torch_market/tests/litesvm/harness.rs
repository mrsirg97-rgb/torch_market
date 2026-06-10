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
    transaction::{Transaction, TransactionError},
};
use solana_sdk::system_program;

use torch_market::{
    constants::*,
    pool_validation,
    state::{
        BondingCurve, GlobalConfig, Position, PositionSide, ProtocolTreasury, TorchVault,
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
    // Metadata of the last successful tx — event-assertion tests decode
    // emit_cpi! payloads from its inner instructions (see extract_event).
    pub last_meta: Option<litesvm::types::TransactionMetadata>,
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
            last_meta: None,
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
            Ok(meta) => {
                self.last_meta = Some(meta);
                Ok(())
            }
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

    /// Derived treasury SOL = treasury_sol_vault lamports − rent (mirrors the
    /// program's `treasury_physical_sol`; there is no tracked `sol_balance` field).
    pub fn treasury_sol(&self, t: &TokenCtx) -> u64 {
        let lamports = self
            .svm
            .get_account(&t.treasury_sol_vault)
            .map(|a| a.lamports)
            .unwrap_or(0);
        let rent = self
            .svm
            .get_sysvar::<solana_sdk::rent::Rent>()
            .minimum_balance(0);
        lamports.saturating_sub(rent)
    }

    /// Force an account's lamports to an exact value (preserving data/owner) —
    /// used to set the treasury_sol_vault below/above a gate in tests.
    pub fn poke_lamports(&mut self, addr: &Pubkey, lamports: u64) {
        let acct = self.svm.get_account(addr).unwrap_or_default();
        let new = Account {
            lamports,
            data: acct.data().to_vec(),
            owner: *acct.owner(),
            executable: acct.executable(),
            rent_epoch: acct.rent_epoch(),
        };
        self.svm.set_account(*addr, new).expect("set_account");
    }

    // [V21] Resolve a unified Position PDA for (user, mint, side, index). `side`
    // is POSITION_SIDE_LONG / POSITION_SIDE_SHORT; the byte disambiguates a long
    // and a short at the same index.
    pub fn get_position(
        &self,
        t: &TokenCtx,
        user: &Pubkey,
        side: u8,
        index: u32,
    ) -> Option<Position> {
        let (addr, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                user.as_ref(),
                t.mint.as_ref(),
                &[side],
                &index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        try_deserialize_anchor(&self.svm, &addr)
    }

    pub fn get_torch_vault(&self, vault: &Pubkey) -> TorchVault {
        deserialize_anchor(&self.svm, vault)
    }

    // [F-8] Counter-reconciliation invariant: the treasury's tracked aggregates
    // must equal the sums re-derived from the live Position accounts at all
    // times. The conservation is emergent across 12 handler paths and every
    // decrement is a saturating_sub, so drift would otherwise be silent — call
    // this after EVERY state-mutating leverage op in lifecycle tests.
    //
    // `positions` lists every position the test ever opened as
    // (owner, side, index) — owner is the wallet for direct positions and the
    // torch_vault for via_vault positions. Closed positions deserialize to
    // None and contribute zero, so the full history can be passed unchanged.
    pub fn assert_treasury_counters(&self, t: &TokenCtx, positions: &[(Pubkey, u8, u32)]) {
        let mut tokens_lent = 0u64; // Σ open-short debt_amount (principal)
        let mut sol_lent = 0u64; // Σ open-long debt_amount (gross principal)
        let mut shorts = 0u64;
        let mut longs = 0u64;
        let mut collateral_locked = 0u64; // Σ open-long collateral_amount
        for (owner, side, index) in positions {
            if let Some(p) = self.get_position(t, owner, *side, *index) {
                match p.side {
                    PositionSide::Short => {
                        shorts += 1;
                        tokens_lent += p.debt_amount;
                    }
                    PositionSide::Long => {
                        longs += 1;
                        sol_lent += p.debt_amount;
                        collateral_locked += p.collateral_amount;
                    }
                }
            }
        }
        let tr = self.get_treasury(t);
        assert_eq!(tr.total_tokens_lent, tokens_lent, "total_tokens_lent drift");
        assert_eq!(
            tr.total_sol_lent_to_longs, sol_lent,
            "total_sol_lent_to_longs drift"
        );
        assert_eq!(tr.active_shorts, shorts, "active_shorts drift");
        assert_eq!(tr.active_longs, longs, "active_longs drift");
        assert_eq!(
            tr.total_token_collateral_locked, collateral_locked,
            "total_token_collateral_locked drift"
        );
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
        let (bonding_curve_sol, _) = Pubkey::find_program_address(
            &[BONDING_CURVE_SOL_SEED, mint_key.as_ref()],
            &torch_market::ID,
        );
        let (treasury, _) =
            Pubkey::find_program_address(&[TREASURY_SEED, mint_key.as_ref()], &torch_market::ID);
        let (treasury_sol_vault, _) = Pubkey::find_program_address(
            &[TREASURY_SOL_VAULT_SEED, mint_key.as_ref()],
            &torch_market::ID,
        );
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                creator: creator.pubkey(),
                global_config: self.global_config,
                mint: mint_key,
                bonding_curve,
                token_vault,
                treasury,
                treasury_sol_vault,
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
            bonding_curve_sol,
            treasury,
            treasury_sol_vault,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                buyer: buyer.pubkey(),
                global_config: self.global_config,
                dev_wallet: self.dev_wallet.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                buyer: buyer.pubkey(),
                global_config: self.global_config,
                dev_wallet: dev_wallet_override.unwrap_or(self.dev_wallet.pubkey()),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                buyer: signer.pubkey(),
                global_config: self.global_config,
                dev_wallet: self.dev_wallet.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                treasury_token_account: t.treasury_token_account,
                user_position,
                user_stats: Some(user_stats),
                protocol_treasury: self.protocol_treasury,
                creator: t.creator,
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                seller: seller.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                seller_token_account,
                user_position: Some(user_position),
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                seller: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                user_position: Some(user_position),
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                user_stats: Some(user_stats),
                protocol_treasury: Some(self.protocol_treasury),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
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
        let (vault_sol, _) = Pubkey::find_program_address(
            &[TORCH_VAULT_SOL_SEED, creator.pubkey().as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CreateVault {
                creator: creator.pubkey(),
                vault,
                vault_sol,
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
            vault_sol,
            authority_creator_link: wallet_link,
        }
    }

    /// Derived TorchVault SOL balance = vault_sol lamports − rent (mirrors
    /// vault_physical_sol). There is no tracked sol_balance field.
    pub fn vault_sol(&self, vault: &VaultCtx) -> u64 {
        let lamports = self
            .svm
            .get_account(&vault.vault_sol)
            .map(|a| a.lamports)
            .unwrap_or(0);
        let rent = self
            .svm
            .get_sysvar::<solana_sdk::rent::Rent>()
            .minimum_balance(0);
        lamports.saturating_sub(rent)
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
                vault_sol: vault.vault_sol,
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
                vault_sol: vault.vault_sol,
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
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
                token_treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
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
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                contributor: contributor.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bonding_curve_sol: t.bonding_curve_sol,
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
                treasury_sol_vault: t.treasury_sol_vault,
                treasury_token_account: t.treasury_token_account,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: pool_validation::derive_deep_pool_event_authority(),
                token_2022_program: TOKEN_2022_PROGRAM_ID,
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

    /// Single-tx migration: ComputeBudget + create payer_token ATA + migrate_to_dex.
    /// The bonded SOL is sourced directly from bonding_curve_sol (seed-signed) inside
    /// migrate_to_dex — no separate fund step. Payer signs + pays rent (reimbursed).
    /// Treasury must have >= MIN_MIGRATION_SOL.
    pub fn migrate(&mut self, t: &TokenCtx, payer: &Keypair) -> Result<(), TransactionError> {
        use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
        let payer_token = get_associated_token_address_2022(&payer.pubkey(), &t.mint);
        let payer_lp_account =
            get_associated_token_address_2022(&payer.pubkey(), &t.deep_pool_lp_mint);
        let deep_pool_lp_account =
            get_associated_token_address_2022(&t.deep_pool, &t.deep_pool_lp_mint);

        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(600_000);
        let create_ata_ix = build_create_associated_token_account_instruction(
            &payer.pubkey(),
            &payer.pubkey(),
            &t.mint,
        );
        let migrate_ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::MigrateToDex {
                event_authority: anchor_lang::solana_program::pubkey::Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                bonding_curve_sol: t.bonding_curve_sol,
                token_vault: t.token_vault,
                payer_token,
                deep_pool_program: deep_pool::ID,
                torch_config: self.torch_config,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_lp_mint: t.deep_pool_lp_mint,
                payer_lp_account,
                deep_pool_lp_account,
                deep_pool_event_authority: pool_validation::derive_deep_pool_event_authority(),
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::MigrateToDex {}.data(),
        };
        self.send(&[bump_cu, create_ata_ix, migrate_ix], &[payer])
    }

    // -----------------------------------------------------------------------
    // Lending (long): borrow / repay / liquidate
    // -----------------------------------------------------------------------

    // [V21] Atomic-custodied long open. Token collateral → position token vault;
    // treasury funds a SOL borrow that atomically buys tokens into the same
    // vault. `min_out` = min tokens out from the buy.
    pub fn open_long(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        position_index: u32,
        collateral: u64,
        min_out: u64,
    ) -> Result<(), TransactionError> {
        let borrower_token_account = get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                borrower.pubkey().as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        // The position token vault is the canonical ATA of the Position PDA.
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);
        let (long_sol_vault, _) = Pubkey::find_program_address(
            &[
                LONG_SOL_VAULT_SEED,
                borrower.pubkey().as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenLongPosition {
                user_risk: user_risk_pda(&borrower.pubkey(), &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                borrower: borrower.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                borrower_token_account,
                position,
                position_token_vault,
                long_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenLong {
                args: torch_market::contexts::OpenPositionArgs {
                    position_index,
                    collateral,
                    min_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[borrower])
    }

    // [V21] Atomic long close. Sells the vault tokens, splits SOL output:
    // debt → treasury, surplus → user. `repay_fraction_bps` = 10000 for full.
    pub fn close_long(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        position_index: u32,
        repay_fraction_bps: u16,
        min_surplus_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                borrower.pubkey().as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);
        let (long_sol_vault, _) = Pubkey::find_program_address(
            &[
                LONG_SOL_VAULT_SEED,
                borrower.pubkey().as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseLongPosition {
                user_risk: user_risk_pda(&borrower.pubkey(), &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                borrower: borrower.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                position,
                position_token_vault,
                long_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseLong {
                args: torch_market::contexts::ClosePositionArgs {
                    position_index,
                    repay_fraction_bps,
                    min_surplus_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[borrower])
    }

    // [V21] Long liquidation: liquidator pays SOL debt → treasury, seizes vault
    // tokens + bonus. No swap CPI; deep_pool read-only for the mark. Residual
    // equity tokens (full liq) return to the borrower's ATA. Hardened TWAP (D-10).
    pub fn liquidate_long(
        &mut self,
        liquidator: &Keypair,
        borrower: Pubkey,
        t: &TokenCtx,
        position_index: u32,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        let borrower_token_account = get_associated_token_address_2022(&borrower, &t.mint);
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                borrower.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateLongPosition {
                user_risk: user_risk_pda(&borrower, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                liquidator: liquidator.pubkey(),
                borrower,
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                position,
                borrower_token_account,
                position_token_vault,
                liquidator_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateLong {
                args: torch_market::contexts::LiquidatePositionArgs { position_index },
            }
            .data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // Test helper: materialize the vault's token ATA and seed it with `amount`
    // raw token units (mirrors poke_token_amount on other token accounts). Used
    // to stock the vault with long-collateral tokens.
    pub fn fund_vault_tokens(&mut self, funder: &Keypair, vault: &VaultCtx, t: &TokenCtx, amount: u64) {
        self.ensure_token2022_ata(funder, &vault.vault, &t.mint)
            .expect("create vault ATA");
        let ata = get_associated_token_address_2022(&vault.vault, &t.mint);
        self.poke_token_amount(ata, amount);
    }

    // [V21] Vault-routed open_long. Token collateral comes from the vault's token
    // ATA (seed-signed); borrow is from the treasury; position is vault-seeded.
    // `signer` is a linked wallet (pays position + token-vault rent).
    pub fn open_long_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
        collateral: u64,
        min_out: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);
        let (long_sol_vault, _) = Pubkey::find_program_address(
            &[
                LONG_SOL_VAULT_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (vault_wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenLongViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                signer: signer.pubkey(),
                torch_vault: vault.vault,
                vault_wallet_link,
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                vault_token_account,
                position,
                position_token_vault,
                long_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenLongViaVault {
                args: torch_market::contexts::OpenPositionArgs {
                    position_index,
                    collateral,
                    min_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    // [V21] Vault-routed close_long. Surplus P&L → vault_sol; signer reclaims rent.
    pub fn close_long_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
        repay_fraction_bps: u16,
        min_surplus_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);
        let (long_sol_vault, _) = Pubkey::find_program_address(
            &[
                LONG_SOL_VAULT_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (vault_wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseLongViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                signer: signer.pubkey(),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
                vault_wallet_link,
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                position,
                position_token_vault,
                long_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseLongViaVault {
                args: torch_market::contexts::ClosePositionArgs {
                    position_index,
                    repay_fraction_bps,
                    min_surplus_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    // [V21] Vault-routed long liquidation: external liquidator repays SOL debt →
    // treasury, seizes vault tokens. Residual tokens → vault ATA, rent → vault_sol.
    pub fn liquidate_long_via_vault(
        &mut self,
        liquidator: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_LONG],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let position_token_vault = get_associated_token_address_2022(&position, &t.mint);

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateLongViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                liquidator: liquidator.pubkey(),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                position,
                position_token_vault,
                vault_token_account,
                liquidator_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: spl_associated_token_account_id(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateLongViaVault {
                args: torch_market::contexts::LiquidatePositionArgs { position_index },
            }
            .data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // -----------------------------------------------------------------------
    // Shorts: open / close
    // -----------------------------------------------------------------------

    // [V21] Atomic-custodied short open. Collateral SOL → fee + net to the
    // per-position SOL vault; borrowed tokens atomically sold on deep_pool, SOL
    // proceeds land in that vault. `min_out` = min SOL out from the sale.
    pub fn open_short(
        &mut self,
        shorter: &Keypair,
        t: &TokenCtx,
        position_index: u32,
        collateral: u64,
        min_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                shorter.pubkey().as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_SHORT],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[
                SHORT_VAULT_SEED,
                shorter.pubkey().as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenShortPosition {
                user_risk: user_risk_pda(&shorter.pubkey(), &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                shorter: shorter.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShort {
                args: torch_market::contexts::OpenPositionArgs {
                    position_index,
                    collateral,
                    min_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[shorter])
    }

    /// Vault-routed open_short. `signer` is a linked wallet; collateral comes from
    /// the vault's vault_sol; the position is VAULT-SEEDED (keyed by vault.vault).
    pub fn open_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
        collateral: u64,
        min_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_SHORT],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[
                SHORT_VAULT_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (vault_wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenShortViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                signer: signer.pubkey(),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
                vault_wallet_link,
                mint: t.mint,
                treasury: t.treasury,
                treasury_sol_vault: t.treasury_sol_vault,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShortViaVault {
                args: torch_market::contexts::OpenPositionArgs {
                    position_index,
                    collateral,
                    min_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    // [V21] Atomic short close. Vault SOL pool-buys tokens to repay the lock;
    // surplus SOL → user. `repay_fraction_bps` = 10000 for full close.
    pub fn close_short(
        &mut self,
        shorter: &Keypair,
        t: &TokenCtx,
        position_index: u32,
        repay_fraction_bps: u16,
        min_surplus_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                shorter.pubkey().as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_SHORT],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[
                SHORT_VAULT_SEED,
                shorter.pubkey().as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseShortPosition {
                user_risk: user_risk_pda(&shorter.pubkey(), &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                shorter: shorter.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShort {
                args: torch_market::contexts::ClosePositionArgs {
                    position_index,
                    repay_fraction_bps,
                    min_surplus_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[shorter])
    }

    /// Vault-routed close_short. `signer` is a linked wallet; surplus → vault_sol.
    pub fn close_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
        repay_fraction_bps: u16,
        min_surplus_sol_out: u64,
    ) -> Result<(), TransactionError> {
        let (position, _) = Pubkey::find_program_address(
            &[POSITION_SEED, vault.vault.as_ref(), t.mint.as_ref(), &[POSITION_SIDE_SHORT], &position_index.to_le_bytes()],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[SHORT_VAULT_SEED, vault.vault.as_ref(), t.mint.as_ref(), &position_index.to_le_bytes()],
            &torch_market::ID,
        );
        let (vault_wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::CloseShortViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                signer: signer.pubkey(),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
                vault_wallet_link,
                mint: t.mint,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                deep_pool_program: deep_pool::ID,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                deep_pool_event_authority: Pubkey::find_program_address(&[b"__event_authority"], &deep_pool::ID).0,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShortViaVault {
                args: torch_market::contexts::ClosePositionArgs {
                    position_index,
                    repay_fraction_bps,
                    min_surplus_sol_out,
                },
            }
            .data(),
        };
        self.send(&[ix], &[signer])
    }

    // [V21] Short liquidation: liquidator pays cover tokens → lock, seizes vault
    // SOL + bonus. No swap CPI; deep_pool read-only for the mark. Hardened TWAP (D-10).
    pub fn liquidate_short(
        &mut self,
        liquidator: &Keypair,
        borrower: Pubkey,
        t: &TokenCtx,
        position_index: u32,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        self.ensure_token2022_ata(liquidator, &liquidator.pubkey(), &t.mint)?;
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                borrower.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_SHORT],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[
                SHORT_VAULT_SEED,
                borrower.as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateShortPosition {
                user_risk: user_risk_pda(&borrower, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                liquidator: liquidator.pubkey(),
                borrower,
                mint: t.mint,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                liquidator_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateShort {
                args: torch_market::contexts::LiquidatePositionArgs { position_index },
            }
            .data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // [V21] Vault-routed short liquidation: external liquidator covers a
    // vault-owned (vault-seeded) short; seized SOL → liquidator, residual + rent
    // → the vault's vault_sol. No vault_wallet_link (anyone can liquidate).
    pub fn liquidate_short_via_vault(
        &mut self,
        liquidator: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        position_index: u32,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        self.ensure_token2022_ata(liquidator, &liquidator.pubkey(), &t.mint)?;
        let (position, _) = Pubkey::find_program_address(
            &[
                POSITION_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &[POSITION_SIDE_SHORT],
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );
        let (position_sol_vault, _) = Pubkey::find_program_address(
            &[
                SHORT_VAULT_SEED,
                vault.vault.as_ref(),
                t.mint.as_ref(),
                &position_index.to_le_bytes(),
            ],
            &torch_market::ID,
        );

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::LiquidateShortViaVault {
                user_risk: user_risk_pda(&vault.vault, &t.mint),
                event_authority: Pubkey::find_program_address(&[b"__event_authority"], &torch_market::ID).0,
                program: torch_market::ID,
                liquidator: liquidator.pubkey(),
                torch_vault: vault.vault,
                vault_sol: vault.vault_sol,
                mint: t.mint,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                position,
                position_sol_vault,
                liquidator_token_account,
                deep_pool: t.deep_pool,
                deep_pool_token_vault: t.deep_pool_token_vault,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateShortViaVault {
                args: torch_market::contexts::LiquidatePositionArgs { position_index },
            }
            .data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // [V21] Direct deep_pool swap — a tiny buy (SOL→token) used to stamp the
    // keeperless TWAP oracle, which advances ONLY on swaps now. `buyer` pays
    // `sol_in` lamports and receives tokens into their Token-2022 ATA (created on
    // demand). Same Swap accounts torch's leverage handlers CPI into.
    pub fn deep_pool_buy(
        &mut self,
        buyer: &Keypair,
        t: &TokenCtx,
        sol_in: u64,
    ) -> Result<(), TransactionError> {
        self.ensure_token2022_ata(buyer, &buyer.pubkey(), &t.mint)?;
        let buyer_ata = get_associated_token_address_2022(&buyer.pubkey(), &t.mint);
        let ix = Instruction {
            program_id: deep_pool::ID,
            accounts: deep_pool::accounts::Swap {
                user: buyer.pubkey(),
                sol_source: buyer.pubkey(),
                pool: t.deep_pool,
                token_mint: t.mint,
                token_vault: t.deep_pool_token_vault,
                user_token_account: buyer_ata,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
                event_authority: pool_validation::derive_deep_pool_event_authority(),
                program: deep_pool::ID,
            }
            .to_account_metas(None),
            data: deep_pool::instruction::Swap {
                args: deep_pool::SwapArgs {
                    amount_in: sol_in,
                    minimum_out: 0,
                    buy: true,
                },
            }
            .data(),
        };
        self.send(&[ix], &[buyer])
    }

    // [V21][D-10] Warm the keeperless TWAP so the liquidation mark is readable.
    // The oracle lives in deep_pool and advances ONLY on swaps, so stamp its ring
    // with a series of tiny buys spaced > MIN_OBS_SPACING_SLOTS apart, spanning
    // the consumer lookback. After this, `read_twap_sol_per_tok` is anchored at a
    // snapshot ≥ LIQ_TWAP_LOOKBACK_SLOTS old (no longer warmup). Warps THEN buys
    // so every buy lands a fresh snapshot. Records at the CURRENT pool price —
    // call BEFORE moving price to mark healthy, or AFTER to track a moved price.
    // `cranker` pays the tiny SOL + one ATA rent. Returns the slot landed on.
    pub fn warm_twap(&mut self, cranker: &Keypair, t: &TokenCtx) -> u64 {
        let steps = (LIQ_TWAP_LOOKBACK_SLOTS / deep_pool::MIN_OBS_SPACING_SLOTS) + 3;
        for _ in 0..steps {
            let next = self.current_slot() + deep_pool::MIN_OBS_SPACING_SLOTS + 1;
            self.warp_to_slot(next);
            self.deep_pool_buy(cranker, t, 100_000).expect("warm swap");
        }
        self.current_slot()
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


// [F-1] Per-(owner, mint) aggregate-exposure PDA (UserRisk).
pub fn user_risk_pda(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[USER_RISK_SEED, owner.as_ref(), mint.as_ref()],
        &torch_market::ID,
    )
    .0
}

impl Env {
    // [F-1] Reconcile an owner's UserRisk aggregate against their live
    // positions: short_tokens_debt == Σ open short debt, long_sol_debt == Σ
    // open long debt, long_collateral_tokens == Σ open long collateral.
    // `positions` lists every (side, index) the owner ever opened; closed
    // positions deserialize to None and contribute zero.
    pub fn assert_user_risk(&self, t: &TokenCtx, owner: &Pubkey, positions: &[(u8, u32)]) {
        let mut short_debt = 0u64;
        let mut long_debt = 0u64;
        let mut long_coll = 0u64;
        for (side, idx) in positions {
            if let Some(p) = self.get_position(t, owner, *side, *idx) {
                match p.side {
                    PositionSide::Short => short_debt += p.debt_amount,
                    PositionSide::Long => {
                        long_debt += p.debt_amount;
                        long_coll += p.collateral_amount;
                    }
                }
            }
        }
        let risk: torch_market::state::UserRisk =
            deserialize_anchor(&self.svm, &user_risk_pda(owner, &t.mint));
        assert_eq!(risk.short_tokens_debt, short_debt, "user_risk short debt drift");
        assert_eq!(risk.long_sol_debt, long_debt, "user_risk long debt drift");
        assert_eq!(
            risk.long_collateral_tokens, long_coll,
            "user_risk long collateral drift"
        );
    }
}

/// Decode the first emitted event of type `T` from the last tx's inner
/// instructions (same decode path as the indexer). Port of the deep_pool
/// harness helper.
pub fn extract_event<T: anchor_lang::Discriminator + anchor_lang::AnchorDeserialize>(
    meta: &litesvm::types::TransactionMetadata,
) -> Option<T> {
    for group in &meta.inner_instructions {
        for inner in group {
            let data: &[u8] = &inner.instruction.data;
            if data.len() >= 16 && &data[8..16] == T::DISCRIMINATOR {
                if let Ok(ev) = T::deserialize(&mut &data[16..]) {
                    return Some(ev);
                }
            }
        }
    }
    None
}

/// Compare an event-payload Pubkey with a context Pubkey.
pub fn b58_eq(a: &anchor_lang::prelude::Pubkey, b: &Pubkey) -> bool {
    a == b
}

// ============================================================================
// TokenCtx
// ============================================================================

#[derive(Clone, Debug)]
pub struct TokenCtx {
    pub creator: Pubkey,
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub bonding_curve_sol: Pubkey,
    pub treasury: Pubkey,
    pub treasury_sol_vault: Pubkey,
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
    pub vault_sol: Pubkey,
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

/// Anchor framework's `AccountNotInitialized` error (not a TorchMarketError).
pub const ANCHOR_ACCOUNT_NOT_INITIALIZED: u32 = 3012;

/// Assert that `result` failed with a raw Anchor/framework error code (e.g.
/// `AccountNotInitialized` = 3012, raised by account resolution before the
/// handler body runs — used where a closed PDA can't be re-loaded).
#[macro_export]
macro_rules! expect_anchor_err {
    ($result:expr, $code:expr) => {{
        let res = $result;
        let err = res.expect_err("expected error, got Ok");
        let code = $crate::harness::anchor_err_code(&err)
            .unwrap_or_else(|| panic!("expected Anchor Custom error, got: {:?}", err));
        assert_eq!(code, $code, "expected anchor error code {}, got {}", $code, code);
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
