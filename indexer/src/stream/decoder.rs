// Per-program event decoder.
//
// Anchor's `emit_cpi!` macro emits events as a self-CPI whose data is:
//   [8-byte event-instruction tag: EVENT_IX_TAG_LE (fixed across Anchor)]
//   ++ [8-byte event discriminator: sha256("event:<EventName>")[..8]]
//   ++ [borsh-encoded payload]
//
// Two decoders here — torch_market and deep_pool — because they're separate
// Anchor programs with separate event name spaces and discriminator tables.
// Callers (stream/grpc.rs) dispatch to the right one based on the inner
// instruction's program_id_index.

use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};

use crate::constants::EVENT_IX_TAG_LE;
use crate::contracts::{
    BondingCompleted, BondingCurveTrade, CloseLongEvent, CloseShortEvent, DeepPoolEvent, LiquidateLongEvent,
    LiquidateShortEvent, LiquidityAdded, LiquidityRemoved, MarketCreated, MigratedToDex,
    OpenLongEvent, OpenShortEvent, PoolCreated, RevivalContribution, SwapExecuted, TokenReclaimed,
    TokenRevived, TorchEvent, VaultSwapExecuted,
};
use crate::error::DecodeError;

pub type Discriminator = [u8; 8];

pub fn event_discriminator(name: &str) -> Discriminator {
    let mut hasher = Sha256::new();
    hasher.update(format!("event:{name}").as_bytes());
    let hash = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&hash[..8]);
    out
}

// Anchor instruction discriminator = sha256("global:<snake_case_name>")[..8].
// Used to recognize the *outer* instruction that emitted a leverage event so
// we can tag vault-routed positions (`owner_is_vault`). The event payload
// alone can't distinguish `open_short` from `open_short_via_vault`.
pub fn ix_discriminator(name: &str) -> Discriminator {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{name}").as_bytes());
    let hash = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&hash[..8]);
    out
}

// The 6 `*_via_vault` leverage instruction discriminators. A leverage event
// whose emitting outer ix matches one of these belongs to a vault-owned
// position (`owner` is a TorchVault PDA, not a wallet).
pub const VIA_VAULT_IX_NAMES: [&str; 6] = [
    "open_short_via_vault",
    "close_short_via_vault",
    "liquidate_short_via_vault",
    "open_long_via_vault",
    "close_long_via_vault",
    "liquidate_long_via_vault",
];

#[derive(Debug, Clone, Copy)]
pub struct DeepPoolDiscriminators {
    pub pool_created: Discriminator,
    pub swap_executed: Discriminator,
    pub liquidity_added: Discriminator,
    pub liquidity_removed: Discriminator,
}

impl DeepPoolDiscriminators {
    pub fn compute() -> Self {
        Self {
            pool_created: event_discriminator("PoolCreated"),
            swap_executed: event_discriminator("SwapExecuted"),
            liquidity_added: event_discriminator("LiquidityAdded"),
            liquidity_removed: event_discriminator("LiquidityRemoved"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TorchDiscriminators {
    pub market_created: Discriminator,
    pub bonding_curve_trade: Discriminator,
    pub migrated_to_dex: Discriminator,
    pub vault_swap_executed: Discriminator,
    pub open_short: Discriminator,
    pub close_short: Discriminator,
    pub liquidate_short: Discriminator,
    pub open_long: Discriminator,
    pub close_long: Discriminator,
    pub liquidate_long: Discriminator,
    pub revival_contribution: Discriminator,
    pub token_revived: Discriminator,
    pub bonding_completed: Discriminator,
    pub token_reclaimed: Discriminator,
    // Instruction (not event) discriminators for the 6 `*_via_vault` variants.
    pub via_vault_ixs: [Discriminator; 6],
}

impl TorchDiscriminators {
    pub fn compute() -> Self {
        let mut via_vault_ixs = [[0u8; 8]; 6];
        for (slot, name) in VIA_VAULT_IX_NAMES.iter().enumerate() {
            via_vault_ixs[slot] = ix_discriminator(name);
        }
        Self {
            market_created: event_discriminator("MarketCreated"),
            bonding_curve_trade: event_discriminator("BondingCurveTrade"),
            migrated_to_dex: event_discriminator("MigratedToDex"),
            vault_swap_executed: event_discriminator("VaultSwapExecuted"),
            open_short: event_discriminator("OpenShortEvent"),
            close_short: event_discriminator("CloseShortEvent"),
            liquidate_short: event_discriminator("LiquidateShortEvent"),
            open_long: event_discriminator("OpenLongEvent"),
            close_long: event_discriminator("CloseLongEvent"),
            liquidate_long: event_discriminator("LiquidateLongEvent"),
            revival_contribution: event_discriminator("RevivalContribution"),
            token_revived: event_discriminator("TokenRevived"),
            bonding_completed: event_discriminator("BondingCompleted"),
            token_reclaimed: event_discriminator("TokenReclaimed"),
            via_vault_ixs,
        }
    }

    // True if `ix_data`'s leading 8-byte discriminator is one of the
    // `*_via_vault` leverage instructions. Caller should first confirm the
    // instruction targets the torch program.
    pub fn is_via_vault_ix(&self, ix_data: &[u8]) -> bool {
        if ix_data.len() < 8 {
            return false;
        }
        let disc = &ix_data[..8];
        self.via_vault_ixs.iter().any(|d| d == disc)
    }
}

// Shared header parse for both program decoders. Splits off the tag + disc
// and returns the trailing payload slice.
fn split_header(data: &[u8]) -> Result<([u8; 8], &[u8]), DecodeError> {
    if data.len() < 16 {
        return Err(DecodeError::TooShort);
    }
    if data[..8] != EVENT_IX_TAG_LE {
        // Not an Anchor emit_cpi! event — could be a regular CPI ix.
        // Distinct from UnknownDiscriminator so callers can silently skip
        // these instead of logging a misleading "did not decode" warning.
        return Err(DecodeError::NotAnEvent);
    }
    let mut event_disc = [0u8; 8];
    event_disc.copy_from_slice(&data[8..16]);
    Ok((event_disc, &data[16..]))
}

pub fn try_decode_deep_pool_event(
    data: &[u8],
    discs: &DeepPoolDiscriminators,
) -> Result<DeepPoolEvent, DecodeError> {
    let (event_disc, mut payload) = split_header(data)?;

    let event = if event_disc == discs.pool_created {
        DeepPoolEvent::PoolCreated(PoolCreated::deserialize(&mut payload)?)
    } else if event_disc == discs.swap_executed {
        DeepPoolEvent::SwapExecuted(SwapExecuted::deserialize(&mut payload)?)
    } else if event_disc == discs.liquidity_added {
        DeepPoolEvent::LiquidityAdded(LiquidityAdded::deserialize(&mut payload)?)
    } else if event_disc == discs.liquidity_removed {
        DeepPoolEvent::LiquidityRemoved(LiquidityRemoved::deserialize(&mut payload)?)
    } else {
        return Err(DecodeError::UnknownDiscriminator);
    };

    if !payload.is_empty() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(event)
}

pub fn try_decode_torch_event(
    data: &[u8],
    discs: &TorchDiscriminators,
) -> Result<TorchEvent, DecodeError> {
    let (event_disc, mut payload) = split_header(data)?;

    let event = if event_disc == discs.market_created {
        TorchEvent::MarketCreated(MarketCreated::deserialize(&mut payload)?)
    } else if event_disc == discs.bonding_curve_trade {
        TorchEvent::BondingCurveTrade(BondingCurveTrade::deserialize(&mut payload)?)
    } else if event_disc == discs.migrated_to_dex {
        TorchEvent::MigratedToDex(MigratedToDex::deserialize(&mut payload)?)
    } else if event_disc == discs.vault_swap_executed {
        TorchEvent::VaultSwapExecuted(VaultSwapExecuted::deserialize(&mut payload)?)
    } else if event_disc == discs.open_short {
        TorchEvent::OpenShort(OpenShortEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.close_short {
        TorchEvent::CloseShort(CloseShortEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.liquidate_short {
        TorchEvent::LiquidateShort(LiquidateShortEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.open_long {
        TorchEvent::OpenLong(OpenLongEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.close_long {
        TorchEvent::CloseLong(CloseLongEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.liquidate_long {
        TorchEvent::LiquidateLong(LiquidateLongEvent::deserialize(&mut payload)?)
    } else if event_disc == discs.bonding_completed {
        TorchEvent::BondingCompleted(BondingCompleted::deserialize(&mut payload)?)
    } else if event_disc == discs.token_reclaimed {
        TorchEvent::TokenReclaimed(TokenReclaimed::deserialize(&mut payload)?)
    } else if event_disc == discs.revival_contribution {
        TorchEvent::RevivalContribution(RevivalContribution::deserialize(&mut payload)?)
    } else if event_disc == discs.token_revived {
        TorchEvent::TokenRevived(TokenRevived::deserialize(&mut payload)?)
    } else {
        return Err(DecodeError::UnknownDiscriminator);
    };

    if !payload.is_empty() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(event)
}
