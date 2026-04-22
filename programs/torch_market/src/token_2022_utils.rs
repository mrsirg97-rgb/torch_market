use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::constants::{
    EXTENSION_TLV_HEADER_SIZE, METADATA_POINTER_EXTENSION_SIZE, TOKEN_METADATA_FIXED_SIZE,
};

pub const TOKEN_2022_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 238, 117, 143, 222, 24, 66, 93, 188, 228, 108, 205, 218, 182, 26, 252, 77,
    131, 185, 13, 39, 254, 189, 249, 40, 216, 161, 139, 252,
]);

pub const EXTENSION_TYPE_TRANSFER_FEE_CONFIG: u16 = 1;
// Calculated using getMintLen([ExtensionType.TransferFeeConfig]) = 278
// This includes: base mint (82) + padding + account type + TLV headers + extension data
pub const MINT_WITH_TRANSFER_FEE_SIZE: usize = 278;

// Calculate the space needed for a mint with transfer fee extension
pub fn get_mint_with_transfer_fee_space() -> usize {
    MINT_WITH_TRANSFER_FEE_SIZE
}

// Build instruction to initialize transfer fee config extension
// Must be called before InitializeMint2
// Instruction format (from Solana docs):
// - Byte 0: 26 (0x1a) - Transfer fee extension discriminator
// - Byte 1: 0 (0x00) - InitializeTransferFeeConfig sub-instruction
// - Remaining bytes: instruction data
pub fn build_initialize_transfer_fee_config_instruction(
    mint: &Pubkey,
    transfer_fee_config_authority: Option<&Pubkey>,
    withdraw_withheld_authority: Option<&Pubkey>,
    transfer_fee_basis_points: u16,
    maximum_fee: u64,
) -> Instruction {
    // Two-byte discriminator: 26 (transfer fee extension) + 0 (InitializeTransferFeeConfig)
    let mut data = vec![26u8, 0u8];
    if let Some(authority) = transfer_fee_config_authority {
        data.push(1); // Some
        data.extend_from_slice(authority.as_ref());
    } else {
        data.push(0); // None
    }

    if let Some(authority) = withdraw_withheld_authority {
        data.push(1); // Some
        data.extend_from_slice(authority.as_ref());
    } else {
        data.push(0); // None
    }

    data.extend_from_slice(&transfer_fee_basis_points.to_le_bytes());
    data.extend_from_slice(&maximum_fee.to_le_bytes());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

// Build instruction to initialize mint (Token-2022 compatible)
pub fn build_initialize_mint2_instruction(
    mint: &Pubkey,
    mint_authority: &Pubkey,
    freeze_authority: Option<&Pubkey>,
    decimals: u8,
) -> Instruction {
    let mut data = vec![20u8];
    data.push(decimals);
    data.extend_from_slice(mint_authority.as_ref());
    if let Some(authority) = freeze_authority {
        data.push(1); // Some
        data.extend_from_slice(authority.as_ref());
    } else {
        data.push(0); // None
    }

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

// Build instruction to mint tokens (Token-2022 compatible)
pub fn build_mint_to_instruction(
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131, 11, 90, 19, 153, 218,
    255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
]);
// Token-2022 token account size (base + transfer fee extension for accounts)
pub const TOKEN_ACCOUNT_SIZE: usize = 165;
pub const ACCOUNT_TYPE_SIZE: usize = 1;
// Size of TLV header (type: u16 + length: u16)
pub const TLV_HEADER_SIZE: usize = 4;
pub const EXTENSION_TYPE_TRANSFER_FEE_AMOUNT: u16 = 2;
pub const TRANSFER_FEE_AMOUNT_SIZE: usize = 8;

// Calculate space for Token-2022 token account with transfer fee extension
pub fn get_token_account_with_transfer_fee_space() -> usize {
    TOKEN_ACCOUNT_SIZE + ACCOUNT_TYPE_SIZE + TLV_HEADER_SIZE + TRANSFER_FEE_AMOUNT_SIZE
}

// Derive the ATA address for Token-2022
pub fn get_associated_token_address_2022(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            TOKEN_2022_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

// Build instruction to create ATA for Token-2022
pub fn build_create_associated_token_account_instruction(
    payer: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> Instruction {
    let ata = get_associated_token_address_2022(wallet, mint);

    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(anchor_lang::solana_program::system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        ],
        data: vec![],
    }
}

// Build instruction to transfer tokens (Token-2022 with fee)
pub fn build_transfer_checked_instruction(
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*source, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

// Build instruction to burn tokens (Token-2022)
pub fn build_burn_instruction(
    account: &Pubkey,
    mint: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = vec![8u8];
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new(*mint, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

// Build instruction to harvest withheld tokens to mint
// This collects transfer fees from token accounts into the mint
// Transfer fee sub-instructions:
// - 0: InitializeTransferFeeConfig
// - 1: TransferCheckedWithFee
// - 2: WithdrawWithheldTokensFromMint
// - 3: WithdrawWithheldTokensFromAccounts
// - 4: HarvestWithheldTokensToMint
// - 5: SetTransferFee
pub fn build_harvest_withheld_tokens_to_mint_instruction(
    mint: &Pubkey,
    sources: &[Pubkey],
) -> Instruction {
    // Two-byte discriminator: 26 (transfer fee extension) + 4 (HarvestWithheldTokensToMint)
    let data = vec![26u8, 4u8];
    let mut accounts = vec![AccountMeta::new(*mint, false)];
    for source in sources {
        accounts.push(AccountMeta::new(*source, false));
    }

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts,
        data,
    }
}

// Build instruction to withdraw withheld tokens from mint to destination
// This moves harvested fees from mint to a token account
pub fn build_withdraw_withheld_tokens_from_mint_instruction(
    mint: &Pubkey,
    destination: &Pubkey,
    withdraw_withheld_authority: &Pubkey,
) -> Instruction {
    // Two-byte discriminator: 26 (transfer fee extension) + 2 (WithdrawWithheldTokensFromMint)
    let data = vec![26u8, 2u8];
    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*withdraw_withheld_authority, true),
        ],
        data,
    }
}

// Build instruction to initialize a Token-2022 token account
pub fn build_initialize_account3_instruction(
    account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Instruction {
    let mut data = vec![18u8];
    data.extend_from_slice(owner.as_ref());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new_readonly(*mint, false),
        ],
        data,
    }
}

// Initial mint space: TransferFeeConfig + MetadataPointer (no TokenMetadata yet).
// Token-2022 reallocs internally when InitializeTokenMetadata is called.
pub fn get_mint_with_pointer_space() -> usize {
    MINT_WITH_TRANSFER_FEE_SIZE + METADATA_POINTER_EXTENSION_SIZE
}

// Final mint space with TransferFeeConfig + MetadataPointer + TokenMetadata.
// Used to calculate additional rent needed before metadata init.
pub fn get_mint_with_metadata_space(name_len: usize, symbol_len: usize, uri_len: usize) -> usize {
    MINT_WITH_TRANSFER_FEE_SIZE           // 278 (base + TransferFeeConfig)
        + METADATA_POINTER_EXTENSION_SIZE // 68  (MetadataPointer TLV)
        + EXTENSION_TLV_HEADER_SIZE       // 4   (TokenMetadata TLV header)
        + TOKEN_METADATA_FIXED_SIZE       // 80  (fixed metadata fields)
        + name_len + symbol_len + uri_len // variable metadata
}

// Build InitializeMetadataPointer instruction.
// Discriminator: [39, 0] — TokenInstruction::MetadataPointerExtension + Initialize.
// Data uses OptionalNonZeroPubkey (raw 32 bytes, all-zeros = None). NOT COption.
// Must be called BEFORE InitializeMint2.
pub fn build_initialize_metadata_pointer_instruction(
    mint: &Pubkey,
    authority: Option<&Pubkey>,
    metadata_address: &Pubkey,
) -> Instruction {
    // Two-byte discriminator: 39 (MetadataPointer extension) + 0 (Initialize)
    let mut data = vec![39u8, 0u8];
    if let Some(auth) = authority {
        data.extend_from_slice(auth.as_ref());
    } else {
        data.extend_from_slice(&[0u8; 32]);
    }

    data.extend_from_slice(metadata_address.as_ref());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

// Build Initialize token metadata instruction (spl-token-metadata-interface format).
// Discriminator: first 8 bytes of SHA256("spl_token_metadata_interface:initialize_account").
// Must be called AFTER InitializeMint2. Requires mint_authority signer.
pub fn build_initialize_token_metadata_instruction(
    mint: &Pubkey,
    update_authority: &Pubkey,
    mint_authority: &Pubkey,
    name: &str,
    symbol: &str,
    uri: &str,
) -> Instruction {
    // SHA256("spl_token_metadata_interface:initialize_account")[:8]
    let mut data: Vec<u8> = vec![210, 225, 30, 162, 88, 184, 77, 141];
    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name.as_bytes());
    data.extend_from_slice(&(symbol.len() as u32).to_le_bytes());
    data.extend_from_slice(symbol.as_bytes());
    data.extend_from_slice(&(uri.len() as u32).to_le_bytes());
    data.extend_from_slice(uri.as_bytes());

    Instruction {
        program_id: TOKEN_2022_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false), // metadata account (mint itself)
            AccountMeta::new_readonly(*update_authority, false),
            AccountMeta::new_readonly(*mint, false), // mint
            AccountMeta::new_readonly(*mint_authority, true),
        ],
        data,
    }
}
