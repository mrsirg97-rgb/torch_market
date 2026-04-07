# DeepPool Integration: Remove Raydium Dependency

## Overview

Replace Raydium CPMM as Torch's liquidity layer with DeepPool. This removes the only external program dependency, eliminates WSOL handling, and simplifies the codebase by ~25%.

## Why

- **Raydium blocked Anchor 1.0 migration** — their CPI crate isn't updated for Solana 3.0
- **WSOL complexity** — wrapping, unwrapping, syncing, closing ATA accounts. Half the migration handler is WSOL ceremony
- **Fee extraction** — Raydium takes 16% of swap fees. DeepPool takes 0%
- **Pool validation** — 200 lines of raw byte parsing to read Raydium pool state. DeepPool is a single PDA read
- **External trust** — Raydium is an external program. DeepPool is ours, formally verified

## What Changes

### Deleted Entirely

| File/Instruction | Reason |
|------------------|--------|
| `fund_migration_wsol` instruction | No WSOL needed — DeepPool uses native SOL |
| `RAYDIUM_CPMM_PROGRAM_ID` constant | No Raydium |
| `RAYDIUM_AMM_CONFIG` constant | No Raydium |
| `WSOL_MINT` constant | No WSOL |
| `raydium-cpmm-cpi` Cargo dependency | No Raydium |
| `order_mints()` | Raydium-specific mint ordering |
| `derive_pool_state()` | Raydium PDA derivation |
| `derive_pool_vault()` | Raydium vault derivation |
| `derive_observation_state()` | Raydium observation PDA |
| `validate_pool_accounts()` | Raydium raw byte parsing |
| `read_pool_accumulated_fees()` | Raydium fee offset parsing |
| `is_wsol_vault_0()` | Raydium vault ordering detection |
| All WSOL ATA creation/close logic | No WSOL |

### Rewritten

| File | Before | After |
|------|--------|-------|
| `migration.rs` | 400+ lines: WSOL wrap, Raydium pool create CPI, LP mint, LP burn, authority revoke, baseline capture | ~100 lines: DeepPool `create_pool` CPI, burn LP tokens, revoke authorities, record baseline |
| `handlers/treasury.rs` (`swap_fees_to_sol`) | Raydium swap CPI with WSOL unwrap, fee subtraction, ratio gating | DeepPool `swap` CPI (sell tokens for SOL), ratio gating |
| `handlers/swap.rs` (vault swap) | Raydium swap CPI with WSOL wrap/unwrap | DeepPool `swap` CPI — native SOL, no wrapping |
| `pool_validation.rs` | 210 lines: Raydium PDA derivation, raw byte parsing, pool validation | ~40 lines: DeepPool PDA derivation, `get_depth_max_ltv_bps`, `require_min_pool_liquidity` |
| `contexts.rs` | Raydium accounts in 6+ contexts (migrate, swap, borrow, liquidate, short, treasury) | DeepPool pool PDA + vault in same contexts, far fewer accounts |

### Simplified

| File | Change |
|------|--------|
| `handlers/lending.rs` | Pool price read: was raw vault byte parse → now `pool_pda.lamports()` + vault balance |
| `handlers/short.rs` | Same pool price simplification |
| `constants.rs` | Remove 8 Raydium constants, add DeepPool program ID + seeds |
| `errors.rs` | Remove `InvalidPoolAccount` (was Raydium validation), keep `PoolTooThin` |

### Unchanged

| File | Why |
|------|-----|
| `handlers/market.rs` | Bonding curve — no Raydium involvement |
| `handlers/token.rs` | Token creation — no Raydium involvement |
| `handlers/vault.rs` | Vault management — no Raydium involvement |
| `handlers/rewards.rs` | Rewards — no Raydium involvement |
| `handlers/reclaim.rs` | Reclaim — no Raydium involvement |
| `handlers/admin.rs` | Admin — no Raydium involvement |
| `state.rs` | Account layout unchanged — baseline fields remain for `swap_fees_to_sol` ratio gating |
| `kani_proofs.rs` | Arithmetic proofs don't depend on Raydium |

## Migration Flow: Before vs After

### Before (Raydium)

```
1. fund_migration_wsol() — wrap SOL to WSOL
2. migrate_to_dex():
   a. Create Raydium pool (CPI with 13 accounts)
   b. Deposit WSOL + tokens into pool vaults
   c. Mint LP tokens
   d. Burn LP tokens
   e. Revoke mint/freeze/fee authority
   f. Close WSOL ATA
   g. Record baseline
```

### After (DeepPool)

```
1. migrate_to_dex():
   a. CPI to DeepPool create_pool (native SOL + tokens)
   b. Burn received LP tokens
   c. Revoke mint/freeze authority
   d. Record baseline from pool PDA
```

One instruction instead of two. ~5 accounts instead of ~15. No WSOL.

## Pool Price Reading: Before vs After

### Before

```rust
// Read raw Raydium pool state bytes
let data = pool_state.try_borrow_data()?;
let stored_vault_0 = read_pubkey_at(&data, 72)?;
let stored_vault_1 = read_pubkey_at(&data, 104)?;
let mint_0 = read_pubkey_at(&data, 168)?;
let mint_1 = read_pubkey_at(&data, 200)?;
// Figure out which vault is SOL
let wsol_is_0 = mint_0 == WSOL_MINT;
let (pool_sol, pool_tokens) = if wsol_is_0 { ... } else { ... };
// Subtract accumulated Raydium fees from vault balances
let (sol_fees, token_fees) = read_pool_accumulated_fees(&pool_state, wsol_is_0)?;
let pool_sol = pool_sol.saturating_sub(sol_fees);
```

### After

```rust
// DeepPool: SOL reserve = pool PDA lamports - rent
let pool_sol = pool_pda.lamports() - rent_exempt;
// Token reserve = vault balance
let pool_tokens = token_vault.amount;
```

Two lines instead of twenty. No raw byte parsing. No vault ordering. No fee subtraction.

## Depth-Anchored Risk Model

No changes needed. `get_depth_max_ltv_bps(pool_sol)` works identically — it reads pool SOL regardless of source. With DeepPool, `pool_sol = pool_pda.lamports() - rent`. Same number, simpler path.

## Cargo.toml

### Before

```toml
anchor-lang = { version = "0.32.1", features = ["init-if-needed"] }
anchor-spl = { version = "0.32.1", features = ["token", "associated_token"] }
raydium-cpmm-cpi = { git = "https://github.com/raydium-io/raydium-cpi", package = "raydium-cpmm-cpi" }
```

### After

```toml
anchor-lang = { version = "0.32.1", features = ["init-if-needed"] }
anchor-spl = { version = "0.32.1", features = ["token", "associated_token"] }
```

One dependency removed. Zero external program crates.

## Account Count Reduction

| Context | Before (Raydium) | After (DeepPool) |
|---------|------------------|-------------------|
| MigrateToDex | ~18 accounts | ~8 accounts |
| SwapFeesToSol | ~15 accounts | ~8 accounts |
| VaultSwap (buy/sell) | ~15 accounts | ~8 accounts |
| Borrow | ~12 accounts | ~8 accounts |
| Liquidate | ~12 accounts | ~8 accounts |
| OpenShort | ~12 accounts | ~8 accounts |
| LiquidateShort | ~12 accounts | ~8 accounts |

Fewer accounts = smaller transactions = more headroom = less failure.

## Risks

- **DeepPool must be deployed and verified before Torch migration** — can't create pools on a program that doesn't exist
- **Existing Raydium pools** — tokens already migrated to Raydium stay there. This integration only affects new migrations
- **CPI compute budget** — DeepPool CPI should be cheaper than Raydium CPI (simpler program), but verify
- **LP token handling** — Torch must burn DeepPool LP tokens at migration. Verify the burn flow works with Token-2022 LP mints
