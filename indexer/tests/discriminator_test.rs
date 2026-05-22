// Known-good discriminators for every event the indexer decodes.
//
// Hard-coded values computed once via `printf "event:<Name>" | shasum -a 256`
// and pinned here so any future drift (rename, sha2 lib swap) trips a test
// instead of silently mis-decoding live events.
//
// If you intentionally rename an event in the program, recompute the
// matching constant — DON'T change the discriminator field on the program's
// #[event] struct, since Anchor derives it from the type name.

use torch_indexer::stream::decoder::{
    event_discriminator, DeepPoolDiscriminators, TorchDiscriminators,
};

fn hex(d: [u8; 8]) -> String {
    d.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn deep_pool_discriminators_match_ground_truth() {
    let d = DeepPoolDiscriminators::compute();
    assert_eq!(hex(d.pool_created), "ca2c295868dc9d52");
    assert_eq!(hex(d.swap_executed), "96a61ae11c59264f");
    assert_eq!(hex(d.liquidity_added), "9a1add6cee40d9a1");
    assert_eq!(hex(d.liquidity_removed), "e169d8277c74a9bd");
}

#[test]
fn torch_discriminators_match_ground_truth() {
    let d = TorchDiscriminators::compute();
    assert_eq!(hex(d.market_created), "58b882e7e254063a");
    assert_eq!(hex(d.bonding_curve_trade), "cdca357baf2e790c");
    assert_eq!(hex(d.migrated_to_dex), "eb88f0cb18c93648");
    assert_eq!(hex(d.vault_swap_executed), "4cd33dbdf72cc63c");
    assert_eq!(hex(d.loan_created), "8e941cd741b9f6c8");
    assert_eq!(hex(d.loan_repaid), "cab7583cd3368ef3");
    assert_eq!(hex(d.loan_liquidated), "011d1c60425f08cc");
    assert_eq!(hex(d.short_opened), "de05904213e263e9");
    assert_eq!(hex(d.short_closed), "8418b595e7328ace");
    assert_eq!(hex(d.short_liquidated), "33ffb87728870920");
    assert_eq!(hex(d.revival_contribution), "de7aa5a5957a50f0");
    assert_eq!(hex(d.token_revived), "70ddb46913da3465");
}

#[test]
fn all_discriminators_unique_across_both_programs() {
    let d = TorchDiscriminators::compute();
    let dp = DeepPoolDiscriminators::compute();
    let all = [
        d.market_created,
        d.bonding_curve_trade,
        d.migrated_to_dex,
        d.vault_swap_executed,
        d.loan_created,
        d.loan_repaid,
        d.loan_liquidated,
        d.short_opened,
        d.short_closed,
        d.short_liquidated,
        d.revival_contribution,
        d.token_revived,
        dp.pool_created,
        dp.swap_executed,
        dp.liquidity_added,
        dp.liquidity_removed,
    ];
    // Sort + dedup → length unchanged means no collisions.
    let mut sorted: Vec<[u8; 8]> = all.to_vec();
    sorted.sort();
    let original_len = sorted.len();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        original_len,
        "discriminator collision across the 16 events"
    );
}

#[test]
fn event_discriminator_helper_matches_hardcoded() {
    // Spot-check the helper directly so the discriminator structs above
    // aren't just testing themselves.
    assert_eq!(
        hex(event_discriminator("MarketCreated")),
        "58b882e7e254063a"
    );
    assert_eq!(hex(event_discriminator("PoolCreated")), "ca2c295868dc9d52");
}
