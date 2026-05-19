// Harness for litesvm integration tests.
//
// Env::new bootstraps a fresh LiteSVM with torch_market loaded and
// global_config / protocol_treasury initialized. Helpers wrap each handler
// in a typed `Result<()>` so tests stay terse.

#![allow(dead_code)]

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
    state::{
        BondingCurve, GlobalConfig, LoanPosition, ProtocolTreasury, ShortPosition, TorchVault,
        Treasury,
    },
    token_2022_utils::{
        get_associated_token_address_2022, ASSOCIATED_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
    },
};

// ============================================================================
// File paths
// ============================================================================

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn torch_market_so() -> Vec<u8> {
    let path = workspace_root().join("target/deploy/torch_market.so");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "torch_market.so missing at {:?}: {}. Run `cargo build-sbf --manifest-path programs/torch_market/Cargo.toml` first.",
            path, e
        )
    })
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn raydium_cpmm_so() -> Vec<u8> {
    let path = fixtures_dir().join("raydium_cpmm.so");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "raydium_cpmm.so missing at {:?}: {}. Run `solana program dump -u mainnet-beta {} {}` to refresh.",
            path, e, RAYDIUM_CPMM_PROGRAM_ID_STR, path.display()
        )
    })
}

/// Read a `solana account --output json` fixture and reconstruct the on-chain
/// account. Used to plant Raydium's amm_config + fee_receiver at chain bootstrap.
fn read_account_fixture(name: &str) -> (Pubkey, Account) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let path = fixtures_dir().join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("fixture {:?} missing: {}", path, e));
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {:?}: {}", path, e));
    let pubkey: Pubkey = v["pubkey"]
        .as_str()
        .expect("fixture pubkey")
        .parse()
        .expect("parse pubkey");
    let acct = &v["account"];
    let data_b64 = acct["data"][0].as_str().expect("data[0]");
    let data = STANDARD.decode(data_b64).expect("base64 decode");
    let owner: Pubkey = acct["owner"].as_str().unwrap().parse().unwrap();
    let lamports = acct["lamports"].as_u64().unwrap();
    let executable = acct["executable"].as_bool().unwrap_or(false);
    (
        pubkey,
        Account {
            lamports,
            data,
            owner,
            executable,
            rent_epoch: u64::MAX,
        },
    )
}

// Raydium constants — strings parsed once at startup.
const RAYDIUM_CPMM_PROGRAM_ID_STR: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

fn raydium_cpmm_program_id() -> Pubkey {
    RAYDIUM_CPMM_PROGRAM_ID_STR.parse().unwrap()
}
fn wsol_mint() -> Pubkey {
    "So11111111111111111111111111111111111111112".parse().unwrap()
}
fn spl_token_program_id() -> Pubkey {
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".parse().unwrap()
}
// Re-export main's constant to guarantee we use the exact same ATA program id.
fn spl_ata_program_id() -> Pubkey {
    ASSOCIATED_TOKEN_PROGRAM_ID
}

// ============================================================================
// Env
// ============================================================================

pub struct Env {
    pub svm: LiteSVM,
    pub authority: Keypair,
    pub treasury_wallet: Keypair,
    pub dev_wallet: Keypair,
    pub global_config: Pubkey,
    pub protocol_treasury: Pubkey,
}

impl Env {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(torch_market::ID, &torch_market_so())
            .expect("add torch_market program");
        svm.add_program(raydium_cpmm_program_id(), &raydium_cpmm_so())
            .expect("add raydium cpmm program");

        // Plant Raydium's mainnet amm_config + fee_receiver — migrate_to_dex
        // validates against the hard-coded RAYDIUM_AMM_CONFIG address, so we
        // need the real on-chain account data, not a stub.
        for fixture in [
            "raydium_amm_config.json",
            "raydium_fee_receiver.json",
            "wsol_mint.json",
        ] {
            let (pk, acct) = read_account_fixture(fixture);
            svm.set_account(pk, acct).expect("set fixture account");
        }

        let authority = Keypair::new();
        let treasury_wallet = Keypair::new();
        let dev_wallet = Keypair::new();
        svm.airdrop(&authority.pubkey(), 100 * LAMPORTS_PER_SOL)
            .unwrap();

        let (global_config, _) =
            Pubkey::find_program_address(&[GLOBAL_CONFIG_SEED], &torch_market::ID);
        let (protocol_treasury, _) =
            Pubkey::find_program_address(&[PROTOCOL_TREASURY_SEED], &torch_market::ID);

        let mut env = Env {
            svm,
            authority,
            treasury_wallet,
            dev_wallet,
            global_config,
            protocol_treasury,
        };
        env.send_init_global_config();
        env.send_init_protocol_treasury();
        env
    }

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
    // Account accessors
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

    pub fn get_torch_vault(&self, vault: &Pubkey) -> TorchVault {
        deserialize_anchor(&self.svm, vault)
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

    pub fn account_exists(&self, addr: &Pubkey) -> bool {
        self.svm.get_account(addr).is_some()
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
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
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
        // create_token chains many CPIs; 200k CU is tight in tests.
        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
        self.send(&[bump_cu, ix], &[creator, &mint])
            .expect("create_token failed");

        TokenCtx {
            creator: creator.pubkey(),
            mint: mint_key,
            bonding_curve,
            treasury,
            treasury_lock,
            token_vault,
            treasury_token_account,
            treasury_lock_token_account,
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
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
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
    /// substitute one of the constraint-checked accounts. Pass None to use the default.
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
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
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

    /// Buy via a vault — uses main's unified Buy instruction with the optional
    /// torch_vault/wallet_link/vault_token_account fields set. Caller must
    /// ensure the vault's token ATA exists.
    pub fn buy_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_amount: u64,
        min_tokens_out: u64,
    ) -> Result<(), TransactionError> {
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        if !self.account_exists(&vault_token_account) {
            use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
            let create_ata = build_create_associated_token_account_instruction(
                &signer.pubkey(),
                &vault.vault,
                &t.mint,
            );
            self.send(&[create_ata], &[signer])?;
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
        let buyer_token_account = get_associated_token_address_2022(&signer.pubkey(), &t.mint);

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Buy {
                buyer: signer.pubkey(),
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
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
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
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
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
        let seller_token_account = get_associated_token_address_2022(&signer.pubkey(), &t.mint);

        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Sell {
                seller: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                token_vault: t.token_vault,
                seller_token_account,
                user_position: Some(user_position),
                token_treasury: t.treasury,
                user_stats: Some(user_stats),
                protocol_treasury: Some(self.protocol_treasury),
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
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
        self.send(&[ix], &[signer])
    }

    // -----------------------------------------------------------------------
    // Vaults
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Reclaim / revival
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

    // -----------------------------------------------------------------------
    // Test-only state helpers
    // -----------------------------------------------------------------------

    /// Re-serialize an Anchor account at `addr` with `new` state, preserving
    /// the original lamports/owner/executable. For tests that need to put the
    /// chain in a specific state.
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

    /// Set a Token-2022 token account's `amount` field directly (bytes 64..72).
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

    pub fn warp_to_slot(&mut self, slot: u64) {
        let mut clock = self.svm.get_sysvar::<solana_sdk::clock::Clock>();
        clock.slot = slot;
        self.svm.set_sysvar::<solana_sdk::clock::Clock>(&clock);
    }

    pub fn current_slot(&self) -> u64 {
        self.svm.get_sysvar::<solana_sdk::clock::Clock>().slot
    }

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
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
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
                torch_vault: None,
                vault_wallet_link: None,
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
                torch_vault: None,
                vault_wallet_link: None,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::StarToken {}.data(),
        };
        self.send(&[ix], &[user])
    }

    // -----------------------------------------------------------------------
    // Migration (Raydium CPMM)
    // -----------------------------------------------------------------------

    /// Run `fund_migration_wsol` + `migrate_to_dex` in a single tx, mirroring
    /// the SDK's buildMigrateTransaction. Payer fronts rent for Raydium pool
    /// accounts; treasury reimburses inside migrate_to_dex.
    pub fn migrate(&mut self, t: &TokenCtx, payer: &Keypair) -> Result<(), TransactionError> {
        let wsol = wsol_mint();
        let raydium = raydium_cpmm_program_id();
        let amm_config: Pubkey = "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2".parse().unwrap();
        let fee_receiver: Pubkey =
            "DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8".parse().unwrap();
        let spl_token = spl_token_program_id();
        let ata_program = spl_ata_program_id();

        // bc_wsol: bonding_curve's SPL-Token ATA for WSOL.
        let bc_wsol = derive_ata(&t.bonding_curve, &wsol, &spl_token);
        let payer_wsol = derive_ata(&payer.pubkey(), &wsol, &spl_token);
        let payer_token = derive_ata(&payer.pubkey(), &t.mint, &TOKEN_2022_PROGRAM_ID);

        // Order tokens for Raydium (smaller pubkey = token_0).
        let (token_0, token_1, _wsol_is_token_0) = if wsol < t.mint {
            (wsol, t.mint, true)
        } else {
            (t.mint, wsol, false)
        };

        // Raydium PDAs.
        let (raydium_authority, _) =
            Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &raydium);
        let (pool_state, _) = Pubkey::find_program_address(
            &[b"pool", amm_config.as_ref(), token_0.as_ref(), token_1.as_ref()],
            &raydium,
        );
        let (lp_mint, _) =
            Pubkey::find_program_address(&[b"pool_lp_mint", pool_state.as_ref()], &raydium);
        let (token_0_vault, _) = Pubkey::find_program_address(
            &[b"pool_vault", pool_state.as_ref(), token_0.as_ref()],
            &raydium,
        );
        let (token_1_vault, _) = Pubkey::find_program_address(
            &[b"pool_vault", pool_state.as_ref(), token_1.as_ref()],
            &raydium,
        );
        let (observation_state, _) =
            Pubkey::find_program_address(&[b"observation", pool_state.as_ref()], &raydium);
        let payer_lp_token = derive_ata(&payer.pubkey(), &lp_mint, &spl_token);

        // Pre-create the three ATAs the handler doesn't create itself.
        let create_bc_wsol = build_create_ata_idempotent_ix(
            &payer.pubkey(), &t.bonding_curve, &wsol, &spl_token,
        );
        let create_payer_wsol = build_create_ata_idempotent_ix(
            &payer.pubkey(), &payer.pubkey(), &wsol, &spl_token,
        );
        let create_payer_token = build_create_ata_idempotent_ix(
            &payer.pubkey(), &payer.pubkey(), &t.mint, &TOKEN_2022_PROGRAM_ID,
        );

        let fund_ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::FundMigrationWsol {
                payer: payer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                bc_wsol,
            }
            .to_account_metas(None),
            data: torch_market::instruction::FundMigrationWsol {}.data(),
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
                bc_wsol,
                payer_wsol,
                payer_token,
                raydium_program: raydium,
                amm_config,
                raydium_authority,
                pool_state,
                wsol_mint: wsol,
                token_0_vault,
                token_1_vault,
                lp_mint,
                payer_lp_token,
                observation_state,
                create_pool_fee: fee_receiver,
                token_program: spl_token,
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ata_program,
                system_program: system_program::ID,
                rent: solana_sdk::sysvar::rent::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::MigrateToDex {}.data(),
        };

        let bump_cu = ComputeBudgetInstruction::set_compute_unit_limit(400_000);
        self.send(
            &[bump_cu, create_bc_wsol, create_payer_wsol, create_payer_token, fund_ix, migrate_ix],
            &[payer],
        )
    }

    /// Derive the (pool_state, token_0_vault, token_1_vault) tuple for a
    /// migrated token, in the same order as Raydium expects.
    pub fn raydium_pool_accounts(&self, mint: &Pubkey) -> (Pubkey, Pubkey, Pubkey) {
        let wsol = wsol_mint();
        let raydium = raydium_cpmm_program_id();
        let amm_config: Pubkey =
            "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2".parse().unwrap();
        let (token_0, token_1) = if wsol < *mint { (wsol, *mint) } else { (*mint, wsol) };
        let (pool_state, _) = Pubkey::find_program_address(
            &[b"pool", amm_config.as_ref(), token_0.as_ref(), token_1.as_ref()],
            &raydium,
        );
        let (token_0_vault, _) = Pubkey::find_program_address(
            &[b"pool_vault", pool_state.as_ref(), token_0.as_ref()],
            &raydium,
        );
        let (token_1_vault, _) = Pubkey::find_program_address(
            &[b"pool_vault", pool_state.as_ref(), token_1.as_ref()],
            &raydium,
        );
        (pool_state, token_0_vault, token_1_vault)
    }

    // -----------------------------------------------------------------------
    // Lending
    // -----------------------------------------------------------------------

    pub fn borrow(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        collateral_amount: u64,
        sol_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let args = torch_market::contexts::BorrowArgs {
            collateral_amount,
            sol_to_borrow,
        };
        let borrower_token_account =
            get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Borrow { args }.data(),
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
        let args = torch_market::contexts::BorrowArgs {
            collateral_amount,
            sol_to_borrow,
        };
        let borrower_token_account = get_associated_token_address_2022(&signer.pubkey(), &t.mint);
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        for (wallet, ata) in [
            (signer.pubkey(), borrower_token_account),
            (vault.vault, vault_token_account),
        ] {
            if !self.account_exists(&ata) {
                use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
                let create = build_create_associated_token_account_instruction(
                    &signer.pubkey(), &wallet, &t.mint,
                );
                self.send(&[create], &[signer])?;
            }
        }
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::Borrow {
                borrower: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                collateral_vault,
                borrower_token_account,
                loan_position,
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Borrow { args }.data(),
        };
        self.send(&[ix], &[signer])
    }

    pub fn repay(
        &mut self,
        borrower: &Keypair,
        t: &TokenCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let borrower_token_account =
            get_associated_token_address_2022(&borrower.pubkey(), &t.mint);
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
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
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Repay { sol_amount }.data(),
        };
        self.send(&[ix], &[borrower])
    }

    pub fn repay_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_amount: u64,
    ) -> Result<(), TransactionError> {
        let borrower_token_account = get_associated_token_address_2022(&signer.pubkey(), &t.mint);
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
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
            accounts: torch_market::accounts::Repay {
                borrower: signer.pubkey(),
                mint: t.mint,
                treasury: t.treasury,
                collateral_vault,
                borrower_token_account,
                loan_position,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Repay { sol_amount }.data(),
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
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                system_program: system_program::ID,
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
            }
            .to_account_metas(None),
            data: torch_market::instruction::Liquidate {}.data(),
        };
        self.send(&[ix], &[liquidator])
    }

    pub fn liquidate_via_vault(
        &mut self,
        liquidator: &Keypair,
        vault: &VaultCtx,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        for (wallet, ata) in [
            (liquidator.pubkey(), liquidator_token_account),
            (vault.vault, vault_token_account),
        ] {
            if !self.account_exists(&ata) {
                use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
                let create = build_create_associated_token_account_instruction(
                    &liquidator.pubkey(), &wallet, &t.mint,
                );
                self.send(&[create], &[liquidator])?;
            }
        }
        let (collateral_vault, _) =
            Pubkey::find_program_address(&[COLLATERAL_VAULT_SEED, t.mint.as_ref()], &torch_market::ID);
        let (loan_position, _) = Pubkey::find_program_address(
            &[LOAN_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, liquidator.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                token_program: TOKEN_2022_PROGRAM_ID,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                system_program: system_program::ID,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
            }
            .to_account_metas(None),
            data: torch_market::instruction::Liquidate {}.data(),
        };
        self.send(&[ix], &[liquidator])
    }

    // -----------------------------------------------------------------------
    // Treasury fee-to-SOL swap (Raydium-routed)
    // -----------------------------------------------------------------------

    pub fn swap_fees_to_sol(
        &mut self,
        payer: &Keypair,
        t: &TokenCtx,
        minimum_amount_out: u64,
    ) -> Result<(), TransactionError> {
        let wsol = wsol_mint();
        let raydium = raydium_cpmm_program_id();
        let amm_config: Pubkey =
            "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2".parse().unwrap();
        let treasury_wsol = derive_ata(&t.treasury, &wsol, &spl_token_program_id());
        let (raydium_authority, _) =
            Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &raydium);
        let (pool_state, token_0_vault, token_1_vault) = self.raydium_pool_accounts(&t.mint);
        let (observation_state, _) =
            Pubkey::find_program_address(&[b"observation", pool_state.as_ref()], &raydium);
        // The mint's vault is token_0_vault if mint < WSOL, else token_1_vault.
        let (mint_vault, wsol_vault) = if wsol < t.mint {
            (token_1_vault, token_0_vault)
        } else {
            (token_0_vault, token_1_vault)
        };
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
                treasury_wsol,
                raydium_program: raydium,
                raydium_authority,
                amm_config,
                pool_state,
                token_vault: mint_vault,
                wsol_vault,
                wsol_mint: wsol,
                observation_state,
                token_program: spl_token_program_id(),
                token_2022_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::SwapFeesToSol { minimum_amount_out }.data(),
        };
        self.send(&[bump_cu, ix], &[payer])
    }

    // -----------------------------------------------------------------------
    // Shorts
    // -----------------------------------------------------------------------

    pub fn enable_short_selling(&mut self, t: &TokenCtx) -> Result<(), TransactionError> {
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::EnableShortSelling {
                authority: self.authority.pubkey(),
                global_config: self.global_config,
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                short_config,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::EnableShortSelling {}.data(),
        };
        let authority = clone_keypair(&self.authority);
        self.send(&[ix], &[&authority])
    }

    pub fn open_short(
        &mut self,
        shorter: &Keypair,
        t: &TokenCtx,
        sol_collateral: u64,
        tokens_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let args = torch_market::contexts::OpenShortArgs {
            sol_collateral,
            tokens_to_borrow,
        };
        let shorter_token_account = get_associated_token_address_2022(&shorter.pubkey(), &t.mint);
        // Ensure the ATA exists since OpenShort doesn't init it.
        if !self.account_exists(&shorter_token_account) {
            use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
            let create = build_create_associated_token_account_instruction(
                &shorter.pubkey(), &shorter.pubkey(), &t.mint,
            );
            self.send(&[create], &[shorter])?;
        }
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), shorter.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShort { args }.data(),
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
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
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
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShort { token_amount }.data(),
        };
        self.send(&[ix], &[shorter])
    }

    /// Override the WSOL vault balance on a migrated token's Raydium pool.
    /// Used to manipulate price for liquidation / LTV tests.
    pub fn poke_pool_sol(&mut self, t: &TokenCtx, new_sol_lamports: u64) {
        let wsol = wsol_mint();
        let (pool_state, token_0_vault, token_1_vault) = self.raydium_pool_accounts(&t.mint);
        let _ = pool_state;
        let wsol_vault = if wsol < t.mint { token_0_vault } else { token_1_vault };
        self.poke_token_amount(wsol_vault, new_sol_lamports);
    }

    /// Idempotent Token-2022 ATA creation for an arbitrary wallet.
    pub fn ensure_token2022_ata(&mut self, payer: &Keypair, wallet: &Pubkey, mint: &Pubkey) {
        let ata = get_associated_token_address_2022(wallet, mint);
        if self.account_exists(&ata) {
            return;
        }
        use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
        let create = build_create_associated_token_account_instruction(&payer.pubkey(), wallet, mint);
        self.send(&[create], &[payer]).expect("create ata");
    }

    pub fn close_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        token_amount: u64,
    ) -> Result<(), TransactionError> {
        let shorter_token_account =
            get_associated_token_address_2022(&signer.pubkey(), &t.mint);
        let vault_token_account =
            get_associated_token_address_2022(&vault.vault, &t.mint);
        for (wallet, ata) in [(signer.pubkey(), shorter_token_account), (vault.vault, vault_token_account)] {
            if !self.account_exists(&ata) {
                use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
                let create = build_create_associated_token_account_instruction(
                    &signer.pubkey(), &wallet, &t.mint,
                );
                self.send(&[create], &[signer])?;
            }
        }
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
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
            accounts: torch_market::accounts::CloseShort {
                shorter: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                shorter_token_account,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::CloseShort { token_amount }.data(),
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
        if !self.account_exists(&liquidator_token_account) {
            use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
            let create = build_create_associated_token_account_instruction(
                &liquidator.pubkey(),
                &liquidator.pubkey(),
                &t.mint,
            );
            self.send(&[create], &[liquidator])?;
        }
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: None,
                vault_wallet_link: None,
                vault_token_account: None,
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
        liquidator: &Keypair,
        vault: &VaultCtx,
        borrower: Pubkey,
        t: &TokenCtx,
    ) -> Result<(), TransactionError> {
        let liquidator_token_account =
            get_associated_token_address_2022(&liquidator.pubkey(), &t.mint);
        let vault_token_account = get_associated_token_address_2022(&vault.vault, &t.mint);
        for (wallet, ata) in [
            (liquidator.pubkey(), liquidator_token_account),
            (vault.vault, vault_token_account),
        ] {
            if !self.account_exists(&ata) {
                use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
                let create = build_create_associated_token_account_instruction(
                    &liquidator.pubkey(), &wallet, &t.mint,
                );
                self.send(&[create], &[liquidator])?;
            }
        }
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), borrower.as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, liquidator.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
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
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::LiquidateShort {}.data(),
        };
        self.send(&[ix], &[liquidator])
    }

    pub fn open_short_via_vault(
        &mut self,
        signer: &Keypair,
        vault: &VaultCtx,
        t: &TokenCtx,
        sol_collateral: u64,
        tokens_to_borrow: u64,
    ) -> Result<(), TransactionError> {
        let args = torch_market::contexts::OpenShortArgs {
            sol_collateral,
            tokens_to_borrow,
        };
        let shorter_token_account =
            get_associated_token_address_2022(&signer.pubkey(), &t.mint);
        let vault_token_account =
            get_associated_token_address_2022(&vault.vault, &t.mint);
        for (wallet, ata) in [(signer.pubkey(), shorter_token_account), (vault.vault, vault_token_account)] {
            if !self.account_exists(&ata) {
                use torch_market::token_2022_utils::build_create_associated_token_account_instruction;
                let create = build_create_associated_token_account_instruction(
                    &signer.pubkey(), &wallet, &t.mint,
                );
                self.send(&[create], &[signer])?;
            }
        }
        let (short_config, _) = Pubkey::find_program_address(
            &[SHORT_CONFIG_SEED, t.mint.as_ref()],
            &torch_market::ID,
        );
        let (short_position, _) = Pubkey::find_program_address(
            &[SHORT_SEED, t.mint.as_ref(), signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (wallet_link, _) = Pubkey::find_program_address(
            &[VAULT_WALLET_LINK_SEED, signer.pubkey().as_ref()],
            &torch_market::ID,
        );
        let (pool_state, token_vault_0, token_vault_1) = self.raydium_pool_accounts(&t.mint);
        let ix = Instruction {
            program_id: torch_market::ID,
            accounts: torch_market::accounts::OpenShort {
                shorter: signer.pubkey(),
                mint: t.mint,
                bonding_curve: t.bonding_curve,
                treasury: t.treasury,
                treasury_lock: t.treasury_lock,
                treasury_lock_token_account: t.treasury_lock_token_account,
                short_config,
                short_position,
                shorter_token_account,
                pool_state,
                token_vault_0,
                token_vault_1,
                torch_vault: Some(vault.vault),
                vault_wallet_link: Some(wallet_link),
                vault_token_account: Some(vault_token_account),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: torch_market::instruction::OpenShort { args }.data(),
        };
        self.send(&[ix], &[signer])
    }

    // -----------------------------------------------------------------------
    // Bonding flow
    // -----------------------------------------------------------------------

    /// Buy in 1-SOL chunks (capped so the final buy doesn't overshoot the target)
    /// until `bonding_complete`. Rotates to a fresh buyer on any error
    /// (MaxWalletExceeded, lamport underflow, etc.). Returns the first buyer.
    pub fn bond_to_completion(&mut self, t: &TokenCtx) -> Keypair {
        const SOL_PER_BUY: u64 = LAMPORTS_PER_SOL;
        const MAX_ITERS: usize = 400;
        let first_buyer = self.new_funded(3 * LAMPORTS_PER_SOL);
        let mut buyer = clone_keypair(&first_buyer);
        let mut iters = 0;
        loop {
            iters += 1;
            assert!(iters <= MAX_ITERS, "bond_to_completion exceeded {MAX_ITERS} iters");
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
}

// ============================================================================
// Token + vault contexts
// ============================================================================

#[derive(Clone, Debug)]
pub struct VaultCtx {
    pub creator: Pubkey,
    pub vault: Pubkey,
    pub authority_creator_link: Pubkey,
}

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
}

// ============================================================================
// Error helpers
// ============================================================================

pub fn anchor_err_code(err: &TransactionError) -> Option<u32> {
    if let TransactionError::InstructionError(_, ix_err) = err {
        if let InstructionError::Custom(code) = ix_err {
            return Some(*code);
        }
    }
    None
}

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

/// Derive an ATA address generically (works for both SPL Token and Token-2022).
fn derive_ata(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &spl_ata_program_id(),
    )
    .0
}

/// Build an ATA Program "CreateIdempotent" instruction (data = [1]). Works for
/// any token program by passing it in `token_program`.
fn build_create_ata_idempotent_ix(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let ata = derive_ata(wallet, mint, token_program);
    Instruction {
        program_id: spl_ata_program_id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1],
    }
}

fn clone_keypair(k: &Keypair) -> Keypair {
    #[allow(deprecated)]
    Keypair::from_bytes(&k.to_bytes()).unwrap()
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
