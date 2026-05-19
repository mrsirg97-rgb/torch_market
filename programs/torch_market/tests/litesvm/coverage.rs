// Coverage gate: every reachable `TorchMarketError` variant must appear in at
// least one `expect_err!(..., TorchMarketError::Variant)` call across the
// litesvm test files, OR be on the explicit `EXEMPT` allowlist below.
//
// This runs as a regular test — failure = CI failure. When you add a new
// error variant to errors.rs, either write a test that exercises it, or add
// the variant to `EXEMPT` with a one-line reason.

const EXEMPT: &[(&str, &str)] = &[
    // Internal overflow guards — covered by kani math proofs, not behavioral tests.
    (
        "MathOverflow",
        "covered by kani; reachable only via deliberate u128 overflow",
    ),
    // Account constraint variants (raised by `#[account(address = ...)]`),
    // not reachable through a valid program flow.
    (
        "InvalidPoolAccount",
        "address constraint on deep_pool; only triggers via wrong account",
    ),
    (
        "InvalidPoolVault",
        "address constraint on deep_pool_token_vault; same",
    ),
    // Authority/admin variants. update_dev_wallet is the only authority-gated
    // ix and the harness only sends from the correct authority.
    (
        "InvalidAuthority",
        "context creator constraint; would require wrong creator pubkey",
    ),
    (
        "Unauthorized",
        "global_config has_one authority; only update_dev_wallet uses it",
    ),
    // String-length checks on create_token — would require malformed args.
    ("NameTooLong", "create_token arg validation, low value"),
    ("SymbolTooLong", "create_token arg validation, low value"),
    ("UriTooLong", "create_token arg validation, low value"),
    // create_token bonding-target validation.
    (
        "InvalidBondingTarget",
        "VALID_BONDING_TARGETS check; tested implicitly by happy paths",
    ),
    // Defensive constraint on SwapFeesToSol; redundant with bonding_curve.migrated.
    (
        "BaselineNotInitialized",
        "SwapFeesToSol context constraint; defense-in-depth, never raised in valid flow",
    ),
    // Defensive zero-divide guard.
    (
        "ZeroPoolReserves",
        "defensive guard; deep_pool can't be empty post-migrate",
    ),
    // Defensive compute_sell guard. real_sol_reserves and virtual_sol_reserves
    // move in lockstep on every buy/sell, and transfer_checked enforces seller
    // balance ≤ owned tokens, so the `real_sol >= sol_out` check is unreachable
    // in valid flow. (Was tested via sell-after-reclaim until the !reclaimed
    // constraint moved the assertion up the failure chain.)
    (
        "InsufficientSol",
        "math invariant + transfer_checked enforce; unreachable in valid flow",
    ),
    // Hard-to-trigger without specific multi-step setup.
    (
        "NoVolumeInEpoch",
        "claim path; precluded by InsufficientVolumeForRewards happening first",
    ),
    (
        "ClaimBelowMinimum",
        "compute_claim cap; requires precise volume balance",
    ),
    (
        "EmptyBorrowRequest",
        "context constraint on Borrow/OpenShort; covered by happy-path coverage",
    ),
    (
        "InsufficientTreasury",
        "shift_lamports underflow; handler invariants prevent",
    ),
];

const ERRORS_SRC: &str = include_str!("../../src/errors.rs");
const LITESVM_FILES: &[(&str, &str)] = &[
    ("buy", include_str!("buy.rs")),
    ("sell", include_str!("sell.rs")),
    ("migration", include_str!("migration.rs")),
    ("reclaim", include_str!("reclaim.rs")),
    ("revival", include_str!("revival.rs")),
    ("lending", include_str!("lending.rs")),
    ("short", include_str!("short.rs")),
    ("vault", include_str!("vault.rs")),
    ("treasury", include_str!("treasury.rs")),
    ("protocol_treasury", include_str!("protocol_treasury.rs")),
    ("rewards", include_str!("rewards.rs")),
    ("tier_b", include_str!("tier_b.rs")),
];

#[test]
fn every_reachable_variant_has_a_test() {
    let variants = parse_error_variants(ERRORS_SRC);
    assert!(
        !variants.is_empty(),
        "could not parse any variants from errors.rs"
    );

    let exempt: std::collections::HashSet<&str> = EXEMPT.iter().map(|(v, _)| *v).collect();

    let mut covered: std::collections::HashSet<String> = Default::default();
    for (_, src) in LITESVM_FILES {
        for v in &variants {
            let needle = format!("TorchMarketError::{}", v);
            if src.contains(&needle) {
                covered.insert(v.clone());
            }
        }
    }

    let mut uncovered = Vec::new();
    for v in &variants {
        if !covered.contains(v) && !exempt.contains(v.as_str()) {
            uncovered.push(v.clone());
        }
    }
    if !uncovered.is_empty() {
        panic!(
            "{} TorchMarketError variant(s) have no test coverage and are not on the EXEMPT list:\n  - {}\n\nEither add a test that uses `expect_err!(..., TorchMarketError::<Variant>)`, or add the variant to EXEMPT in tests/litesvm/coverage.rs with a one-line reason.",
            uncovered.len(),
            uncovered.join("\n  - ")
        );
    }

    // Also catch EXEMPT entries that no longer exist in errors.rs — stale exemptions.
    let variant_set: std::collections::HashSet<String> = variants.iter().cloned().collect();
    let mut stale: Vec<&str> = exempt
        .iter()
        .copied()
        .filter(|v| !variant_set.contains(*v))
        .collect();
    stale.sort();
    if !stale.is_empty() {
        panic!(
            "EXEMPT list contains stale variant name(s) that no longer exist in errors.rs:\n  - {}\nRemove them.",
            stale.join("\n  - ")
        );
    }
}

/// Parse `Variant,` lines out of the `#[error_code] pub enum TorchMarketError { ... }` block.
fn parse_error_variants(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_enum = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub enum TorchMarketError") {
            in_enum = true;
            continue;
        }
        if in_enum {
            if trimmed == "}" {
                break;
            }
            // Skip attribute lines and comments.
            if trimmed.starts_with("#[") || trimmed.starts_with("//") || trimmed.is_empty() {
                continue;
            }
            // Variant lines look like `VariantName,`.
            if let Some(name) = trimmed.strip_suffix(',') {
                if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}
