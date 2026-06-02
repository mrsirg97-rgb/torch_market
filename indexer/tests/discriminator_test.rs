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
    // [V21] leverage events (renamed + restructured from the V20 Loan*/Short*).
    assert_eq!(hex(d.open_short), "33cf5551bbcf8ec3");
    assert_eq!(hex(d.close_short), "045fccd790c76c50");
    assert_eq!(hex(d.liquidate_short), "20fb22fbd7959b51");
    assert_eq!(hex(d.open_long), "0ee30d2421c0980e");
    assert_eq!(hex(d.close_long), "e037d02b41ec21df");
    assert_eq!(hex(d.liquidate_long), "285639d9db273c46");
    assert_eq!(hex(d.revival_contribution), "de7aa5a5957a50f0");
    assert_eq!(hex(d.token_revived), "70ddb46913da3465");
    // [V21] `*_via_vault` INSTRUCTION discriminators (global:, not event:).
    assert_eq!(hex(d.via_vault_ixs[0]), "180eb597c9dbd6e8"); // open_short_via_vault
    assert_eq!(hex(d.via_vault_ixs[1]), "30e21d7b50590ba0"); // close_short_via_vault
    assert_eq!(hex(d.via_vault_ixs[2]), "2a683bd19f5449ce"); // liquidate_short_via_vault
    assert_eq!(hex(d.via_vault_ixs[3]), "8d04868529df27f2"); // open_long_via_vault
    assert_eq!(hex(d.via_vault_ixs[4]), "d9a312686c493a5b"); // close_long_via_vault
    assert_eq!(hex(d.via_vault_ixs[5]), "8a52e2b05231d709"); // liquidate_long_via_vault
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
        d.open_short,
        d.close_short,
        d.liquidate_short,
        d.open_long,
        d.close_long,
        d.liquidate_long,
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
