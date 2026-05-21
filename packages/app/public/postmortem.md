# Post-Mortem: Migration Price Mismatch

## Summary

The `migrate_to_dex` instruction dumped **all** vault tokens into the Raydium pool paired against only the real SOL reserves. This created a pool price far below the bonding curve's final price, causing an immediate price cliff at migration. Holders who bought at the bonding curve price saw their tokens lose value the moment the Raydium pool went live.

**Root cause:** The migration used `token_vault.amount` (all remaining tokens) as the pool's token side, instead of calculating the price-matched amount from the bonding curve's virtual reserves.

**Fix:** Calculate `tokens_for_pool = real_sol * virtual_tokens / virtual_sol` to match the bonding curve price, and burn excess tokens.

---

## The Bug

### How the bonding curve prices tokens

The bonding curve uses **virtual reserves** for pricing:

```
price = virtual_sol / virtual_tokens
```

At bonding completion (e.g., Spark tier = 50 SOL raised):
- `virtual_sol` = 30 SOL (initial) + 50 SOL (raised) = **80 SOL**
- `virtual_tokens` = ~40T (reduced from 107.3T as buyers purchased)
- Final price = 80 SOL / 40T tokens

### What the old code did

```rust
// OLD (buggy)
let sol_amount = bonding_curve.real_sol_reserves;  // 50 SOL
let token_amount = ctx.accounts.token_vault.amount; // ALL tokens in vault
```

The token vault holds all unsold tokens plus any vote-vault returns. At bonding completion, the vault might contain ~60T tokens (unsold curve tokens + treasury tokens returned by vote).

The Raydium pool was initialized with:
- **SOL side:** 50 SOL (correct)
- **Token side:** ~60T tokens (wrong -- far too many)

This creates a pool price of `50 SOL / 60T tokens`, which is drastically lower than the bonding curve's final price of `80 SOL / 40T tokens`. The pool opens at roughly **40% of the correct price**.

### The price cliff

```
Bonding curve final price:  80 / 40T  = 0.000000002 SOL/token
Raydium pool opening price: 50 / 60T  = 0.000000000833 SOL/token
                                         ~2.4x lower
```

Anyone holding tokens purchased at the bonding curve price immediately saw their position worth ~40% of what they paid. Arbitrageurs could buy cheap on Raydium and the damage was done.

## The Fix

### Price-matched migration

```rust
// NEW (fixed)
let sol_amount = bonding_curve.real_sol_reserves;
let vault_token_amount = ctx.accounts.token_vault.amount;

// Match the bonding curve price on Raydium:
//   tokens_for_pool = real_sol * virtual_tokens / virtual_sol
let tokens_for_pool = (sol_amount as u128)
    .checked_mul(bonding_curve.virtual_token_reserves as u128)?
    .checked_div(bonding_curve.virtual_sol_reserves as u128)? as u64;

let token_amount = tokens_for_pool.min(vault_token_amount);

// Burn excess tokens
let excess_tokens = vault_token_amount - token_amount;
if excess_tokens > 0 {
    burn(/* excess_tokens from vault */);
}
```

The formula `real_sol * virtual_tokens / virtual_sol` produces a token amount that, when paired with `real_sol` in a constant-product AMM, yields the same price as the bonding curve's virtual reserves.

**Excess tokens are burned** rather than left in the vault, which would create a phantom supply overhang.

### Why this works

The bonding curve price at any point is:

```
price = virtual_sol / virtual_tokens
```

For the Raydium pool to open at the same price:

```
pool_price = pool_sol / pool_tokens = price
pool_tokens = pool_sol / price = pool_sol * virtual_tokens / virtual_sol
```

This is exactly what `tokens_for_pool` computes. The only loss is integer truncation (floor division), which is bounded to less than 1 unit of the divisor -- verified by Kani harnesses `verify_price_matched_pool_spark` and `verify_price_matched_pool_torch`.

## Formal Verification

Six Kani proof harnesses were added to verify the fix:

| Harness | What it proves |
|---------|---------------|
| `verify_price_matched_pool_spark` | Pool ratio matches curve ratio for Spark tier (truncation < 1 unit) |
| `verify_price_matched_pool_torch` | Pool ratio matches curve ratio for Torch tier (truncation < 1 unit) |
| `verify_excess_token_burn_conservation` | `pool_tokens + burned_tokens == vault_total` (no tokens created or lost) |
| `verify_prepare_migration_conservation` | SOL withdrawal from bonding curve is exact |
| `verify_refund_skip_after_prepare_migration` | Conditional refund correctly skips after prepare_migration |
| `verify_normal_refund_path` | Conditional refund correctly transfers in the normal path |

All 26 harnesses pass. See [VERIFICATION.md](./VERIFICATION.md) for full details.

## Timeline

1. **Bug identified** -- migration creates pool at wrong price, dumping liquidity
2. **Fix implemented** -- price-matched calculation with excess token burn
3. **Formal verification added** -- 6 new Kani harnesses prove the fix is correct

## Lessons Learned

**Formal verification catches what tests miss.** The original code would pass any test that simply checks "did migration succeed?" The bug is in the *economics* of the pool initialization, not in whether the transaction lands. The Kani harnesses verify the mathematical relationship between bonding curve price and pool price.

## Future Consideration: Treasury-Boosted Liquidity

The current fix matches the bonding curve price correctly, but the Raydium pool has less liquidity depth than the bonding curve did. This is because the bonding curve subsidized depth with 30 SOL of virtual reserves that don't exist as real assets.

A future release could route a portion of treasury SOL into the pool alongside `real_sol` at migration time. This would increase the pool's `k` value (constant product), reducing slippage for post-migration trades. The price-matched token amount would need to scale proportionally: `tokens_for_pool = (real_sol + treasury_boost) * virtual_tokens / virtual_sol`, with matching treasury SOL added to the pool's SOL side. This would require new Kani harnesses to verify the adjusted formula and treasury accounting.
