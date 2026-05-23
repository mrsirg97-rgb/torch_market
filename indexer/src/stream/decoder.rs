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
    BondingCurveTrade, DeepPoolEvent, LiquidityAdded, LiquidityRemoved, LoanCreated,
    LoanLiquidated, LoanRepaid, MarketCreated, MigratedToDex, PoolCreated, RevivalContribution,
    ShortClosed, ShortLiquidated, ShortOpened, SwapExecuted, TokenRevived, TorchEvent,
    VaultSwapExecuted,
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
    pub loan_created: Discriminator,
    pub loan_repaid: Discriminator,
    pub loan_liquidated: Discriminator,
    pub short_opened: Discriminator,
    pub short_closed: Discriminator,
    pub short_liquidated: Discriminator,
    pub revival_contribution: Discriminator,
    pub token_revived: Discriminator,
}

impl TorchDiscriminators {
    pub fn compute() -> Self {
        Self {
            market_created: event_discriminator("MarketCreated"),
            bonding_curve_trade: event_discriminator("BondingCurveTrade"),
            migrated_to_dex: event_discriminator("MigratedToDex"),
            vault_swap_executed: event_discriminator("VaultSwapExecuted"),
            loan_created: event_discriminator("LoanCreated"),
            loan_repaid: event_discriminator("LoanRepaid"),
            loan_liquidated: event_discriminator("LoanLiquidated"),
            short_opened: event_discriminator("ShortOpened"),
            short_closed: event_discriminator("ShortClosed"),
            short_liquidated: event_discriminator("ShortLiquidated"),
            revival_contribution: event_discriminator("RevivalContribution"),
            token_revived: event_discriminator("TokenRevived"),
        }
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
    } else if event_disc == discs.loan_created {
        TorchEvent::LoanCreated(LoanCreated::deserialize(&mut payload)?)
    } else if event_disc == discs.loan_repaid {
        TorchEvent::LoanRepaid(LoanRepaid::deserialize(&mut payload)?)
    } else if event_disc == discs.loan_liquidated {
        TorchEvent::LoanLiquidated(LoanLiquidated::deserialize(&mut payload)?)
    } else if event_disc == discs.short_opened {
        TorchEvent::ShortOpened(ShortOpened::deserialize(&mut payload)?)
    } else if event_disc == discs.short_closed {
        TorchEvent::ShortClosed(ShortClosed::deserialize(&mut payload)?)
    } else if event_disc == discs.short_liquidated {
        TorchEvent::ShortLiquidated(ShortLiquidated::deserialize(&mut payload)?)
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
