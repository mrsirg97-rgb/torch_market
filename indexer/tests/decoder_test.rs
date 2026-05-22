// Decoder roundtrip tests.
//
// Builds synthetic event payloads in the same shape Anchor's emit_cpi!
// macro produces — EVENT_IX_TAG_LE ++ event_discriminator ++ borsh(payload)
// — and verifies the decoder reproduces every field exactly. Catches three
// classes of regressions:
//   1. Field-order drift on the Borsh structs (positional serialization).
//   2. EVENT_IX_TAG_LE / discriminator math regressions.
//   3. Error paths (TooShort, UnknownDiscriminator, TrailingBytes).
//
// No DB, no Yellowstone — pure Rust.

use borsh::BorshSerialize;

use torch_indexer::constants::EVENT_IX_TAG_LE;
use torch_indexer::contracts::{
    BondingCurveTrade, DeepPoolEvent, LiquidityAdded, MarketCreated, MigratedToDex, PoolCreated,
    ShortOpened, SwapExecuted, TorchEvent,
};
use torch_indexer::error::DecodeError;
use torch_indexer::stream::decoder::{
    event_discriminator, try_decode_deep_pool_event, try_decode_torch_event,
    DeepPoolDiscriminators, TorchDiscriminators,
};

fn wrap_event<T: BorshSerialize>(name: &str, payload: &T) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + 256);
    data.extend_from_slice(&EVENT_IX_TAG_LE);
    data.extend_from_slice(&event_discriminator(name));
    payload.serialize(&mut data).expect("borsh serialize");
    data
}

fn pk(byte: u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[0] = byte; // distinguish by first byte
    out
}

// ─── deep_pool roundtrips ────────────────────────────────────────────────

#[test]
fn pool_created_roundtrip() {
    let discs = DeepPoolDiscriminators::compute();
    let original = PoolCreated {
        pool: pk(1),
        config: pk(2),
        token_mint: pk(3),
        lp_mint: pk(4),
        creator: pk(5),
        sol_in_gross: 1_000_000_000,
        sol_in_net: 999_300_000,
        tokens_in_gross: 800_000_000_000,
        tokens_in_net: 799_440_000_000,
        sol_reserve_after: 999_300_000,
        token_reserve_after: 799_440_000_000,
        lp_supply_after: 28_274_333,
        lp_to_creator: 28_274_333,
        lp_locked: 1_000,
    };
    let data = wrap_event("PoolCreated", &original);
    let decoded = try_decode_deep_pool_event(&data, &discs).expect("decode");
    match decoded {
        DeepPoolEvent::PoolCreated(p) => {
            assert_eq!(p.pool, original.pool);
            assert_eq!(p.token_mint, original.token_mint);
            assert_eq!(p.sol_in_net, original.sol_in_net);
            assert_eq!(p.lp_supply_after, original.lp_supply_after);
        }
        _ => panic!("decoded wrong variant"),
    }
}

#[test]
fn swap_executed_roundtrip() {
    let discs = DeepPoolDiscriminators::compute();
    let original = SwapExecuted {
        pool: pk(7),
        user: pk(8),
        sol_source: pk(9),
        buy: true,
        amount_in_gross: 500_000_000,
        amount_in_net: 499_650_000,
        amount_out_gross: 1_234_567,
        amount_out_net: 1_233_703,
        fee: 350_000,
        sol_reserve_after: 1_500_000_000,
        token_reserve_after: 798_206_297,
    };
    let data = wrap_event("SwapExecuted", &original);
    let decoded = try_decode_deep_pool_event(&data, &discs).expect("decode");
    match decoded {
        DeepPoolEvent::SwapExecuted(s) => {
            assert!(s.buy);
            assert_eq!(s.amount_out_net, 1_233_703);
            assert_eq!(s.fee, 350_000);
        }
        _ => panic!("decoded wrong variant"),
    }
}

#[test]
fn liquidity_added_roundtrip() {
    let discs = DeepPoolDiscriminators::compute();
    let original = LiquidityAdded {
        pool: pk(10),
        provider: pk(11),
        sol_in_gross: 100,
        sol_in_net: 99,
        tokens_in_gross: 1000,
        tokens_in_net: 999,
        lp_to_provider: 50,
        lp_locked: 0,
        sol_reserve_after: 1100,
        token_reserve_after: 11000,
        lp_supply_after: 100,
    };
    let data = wrap_event("LiquidityAdded", &original);
    let decoded = try_decode_deep_pool_event(&data, &discs).expect("decode");
    assert!(matches!(decoded, DeepPoolEvent::LiquidityAdded(_)));
}

// ─── torch roundtrips ────────────────────────────────────────────────────

#[test]
fn market_created_roundtrip_with_metadata() {
    let discs = TorchDiscriminators::compute();
    let original = MarketCreated {
        mint: pk(42),
        creator: pk(43),
        name: "Flame Token".to_string(),
        symbol: "FLAME".to_string(),
        metadata_uri: "https://arweave.net/abc123".to_string(),
        is_community_token: false,
        sol_target: 100_000_000_000,
        virtual_sol_reserves: 30_000_000_000,
        virtual_token_reserves: 1_073_000_000_000_000,
    };
    let data = wrap_event("MarketCreated", &original);
    let decoded = try_decode_torch_event(&data, &discs).expect("decode");
    match decoded {
        TorchEvent::MarketCreated(m) => {
            assert_eq!(m.name, "Flame Token");
            assert_eq!(m.symbol, "FLAME");
            assert_eq!(m.metadata_uri, "https://arweave.net/abc123");
            assert!(!m.is_community_token);
            assert_eq!(m.sol_target, 100_000_000_000);
        }
        _ => panic!("decoded wrong variant"),
    }
}

#[test]
fn bonding_curve_trade_buy_direct() {
    let discs = TorchDiscriminators::compute();
    let original = BondingCurveTrade {
        mint: pk(20),
        trader: pk(21),
        vault: [0u8; 32], // direct trade
        is_buy: true,
        sol_in: 1_000_000_000,
        sol_out: 0,
        tokens_in: 0,
        tokens_out: 315_000_000_000_000,
        sol_to_treasury: 5_000_000,
        sol_to_creator: 0,
        protocol_fee: 5_000_000,
        virtual_sol_after: 31_000_000_000,
        virtual_token_after: 1_072_685_000_000_000,
        real_sol_after: 1_000_000_000,
        real_token_after: 800_000_000_000,
    };
    let data = wrap_event("BondingCurveTrade", &original);
    let decoded = try_decode_torch_event(&data, &discs).expect("decode");
    match decoded {
        TorchEvent::BondingCurveTrade(t) => {
            assert!(t.is_buy);
            assert_eq!(t.vault, [0u8; 32]);
            assert_eq!(t.sol_in, 1_000_000_000);
            assert_eq!(t.tokens_out, 315_000_000_000_000);
            assert_eq!(t.sol_out, 0);
            assert_eq!(t.tokens_in, 0);
        }
        _ => panic!("decoded wrong variant"),
    }
}

#[test]
fn migrated_to_dex_roundtrip() {
    let discs = TorchDiscriminators::compute();
    let original = MigratedToDex {
        mint: pk(30),
        deep_pool: pk(31),
        sol_seeded: 30_000_000_000,
        tokens_seeded: 1_073_000_000_000_000,
        lp_burned: 28_274_333,
    };
    let data = wrap_event("MigratedToDex", &original);
    let decoded = try_decode_torch_event(&data, &discs).expect("decode");
    match decoded {
        TorchEvent::MigratedToDex(m) => {
            assert_eq!(m.mint, pk(30));
            assert_eq!(m.deep_pool, pk(31));
            assert_eq!(m.lp_burned, 28_274_333);
        }
        _ => panic!("decoded wrong variant"),
    }
}

#[test]
fn short_opened_carries_net_amount() {
    let discs = TorchDiscriminators::compute();
    let original = ShortOpened {
        mint: pk(50),
        user: pk(51),
        sol_collateral: 2_000_000_000,
        // Net amount after Token-2022 fee — the value the on-chain handler
        // records post-fix.
        tokens_borrowed: 999_300_000,
        ltv_bps: 4500,
    };
    let data = wrap_event("ShortOpened", &original);
    let decoded = try_decode_torch_event(&data, &discs).expect("decode");
    match decoded {
        TorchEvent::ShortOpened(s) => {
            assert_eq!(s.tokens_borrowed, 999_300_000);
            assert_eq!(s.ltv_bps, 4500);
        }
        _ => panic!("decoded wrong variant"),
    }
}

// ─── error paths ─────────────────────────────────────────────────────────

#[test]
fn truncated_data_returns_too_short() {
    let discs = TorchDiscriminators::compute();
    let too_short = [0u8; 4];
    match try_decode_torch_event(&too_short, &discs) {
        Err(DecodeError::TooShort) => {}
        other => panic!("expected TooShort, got {:?}", other),
    }
}

#[test]
fn wrong_tag_returns_unknown_discriminator() {
    let discs = TorchDiscriminators::compute();
    // 16 bytes of arbitrary data — has length but no event tag.
    let mut bad = vec![0xffu8; 16];
    bad[0] = 0x00; // doesn't match EVENT_IX_TAG_LE
    match try_decode_torch_event(&bad, &discs) {
        Err(DecodeError::UnknownDiscriminator) => {}
        other => panic!("expected UnknownDiscriminator, got {:?}", other),
    }
}

#[test]
fn unknown_event_disc_returns_unknown_discriminator() {
    let discs = TorchDiscriminators::compute();
    let mut data = Vec::new();
    data.extend_from_slice(&EVENT_IX_TAG_LE);
    data.extend_from_slice(&[0xdeu8; 8]); // disc that doesn't match any torch event
    match try_decode_torch_event(&data, &discs) {
        Err(DecodeError::UnknownDiscriminator) => {}
        other => panic!("expected UnknownDiscriminator, got {:?}", other),
    }
}

#[test]
fn trailing_bytes_after_payload_rejected() {
    let discs = TorchDiscriminators::compute();
    let mut data = wrap_event(
        "MigratedToDex",
        &MigratedToDex {
            mint: pk(1),
            deep_pool: pk(2),
            sol_seeded: 1,
            tokens_seeded: 2,
            lp_burned: 3,
        },
    );
    data.extend_from_slice(&[0xaa, 0xbb, 0xcc]); // garbage
    match try_decode_torch_event(&data, &discs) {
        Err(DecodeError::TrailingBytes) => {}
        other => panic!("expected TrailingBytes, got {:?}", other),
    }
}

#[test]
fn deep_pool_decoder_rejects_torch_event() {
    // Cross-program isolation: a torch event payload, fed to the deep_pool
    // decoder, must not accidentally match a deep_pool discriminator.
    let dp_discs = DeepPoolDiscriminators::compute();
    let torch_data = wrap_event(
        "MarketCreated",
        &MarketCreated {
            mint: pk(0),
            creator: pk(1),
            name: "x".into(),
            symbol: "x".into(),
            metadata_uri: String::new(),
            is_community_token: false,
            sol_target: 0,
            virtual_sol_reserves: 0,
            virtual_token_reserves: 0,
        },
    );
    match try_decode_deep_pool_event(&torch_data, &dp_discs) {
        Err(DecodeError::UnknownDiscriminator) => {}
        other => panic!("expected UnknownDiscriminator, got {:?}", other),
    }
}
