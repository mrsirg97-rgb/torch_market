# Torch Market — V20.0.0 Architecture

Every token launches with its own margin market. One Anchor program, 30 instructions, 13 account types, no external dependencies beyond DeepPool (also in-house) and the Token-2022 program.

**Program ID:** `4nwTCWyR6vapTQRkV39f32xJ3uQztdjBqfhubnR6wQQC` (V20 torch_next, current)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          TORCH MARKET v20.0.0                            │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  PROTOCOL LAYER                                                          │
│  ┌──────────────┐  ┌────────────────────────────────────┐               │
│  │ GlobalConfig │  │ ProtocolTreasury                   │               │
│  │ (admin,      │  │ (0.5% fees, epoch rewards, dev    │               │
│  │  settings)   │  │  wallet 50% split)                 │               │
│  └──────────────┘  └────────────────────────────────────┘               │
│                                                                          │
│  PER-TOKEN LAYER                                                         │
│                                                                          │
│  ┌──────────────┐    ┌──────────────────┐    ┌──────────────────┐       │
│  │ Token-2022   │───▶│ BondingCurve     │───▶│ Treasury         │       │
│  │ Mint         │    │ (const product,  │    │ (SOL + lending + │       │
│  │ + 0.07% fee  │    │  100 / 200 SOL   │    │  shorts config)  │       │
│  │ + metadata   │    │  tier targets)   │    └────────┬─────────┘       │
│  └──────────────┘    └────────┬─────────┘             │                 │
│                               │                       │                 │
│                               ▼                       │                 │
│                      ┌──────────────────┐             │                 │
│                      │ DeepPool CPMM    │◀────────────┘                 │
│                      │ (post-migration  │                                │
│                      │  liquidity, no   │                                │
│                      │  WSOL wrapping)  │                                │
│                      └────────┬─────────┘                                │
│                               │                                          │
│         ┌─────────────────────┼─────────────────────┐                    │
│         ▼                     ▼                     ▼                    │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐                │
│  │ LoanPosition │    │ ShortPosition│    │ TreasuryLock │                │
│  │ (borrow SOL  │    │ (borrow      │    │ (300M tokens │                │
│  │  vs tokens)  │    │  tokens vs   │    │  locked at   │                │
│  │              │    │  SOL)        │    │  creation)   │                │
│  └──────────────┘    └──────────────┘    └──────────────┘                │
│                                                                          │
│  USER LAYER                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                    │
│  │ UserPosition │  │ UserStats    │  │ StarRecord   │                    │
│  │ (per-token)  │  │ (platform-   │  │ (one star    │                    │
│  │              │  │  wide volume)│  │  per pair)   │                    │
│  └──────────────┘  └──────────────┘  └──────────────┘                    │
│                                                                          │
│  VAULT LAYER (agent custody)                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐                  │
│  │ TorchVault   │  │ VaultWallet- │  │ TorchVaultSol  │                  │
│  │ (state + SOL │  │ Link         │  │ (system-owned, │                  │
│  │  per creator)│  │ (reverse map)│  │  buy-path SOL  │                  │
│  │              │  │              │  │  hop, ephemeral│                  │
│  └──────────────┘  └──────────────┘  └────────────────┘                  │
│                                                                          │
├─────────────────────────────────────────────────────────────────────────┤
│  HANDLERS                                                                │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐      │
│  │ admin  │ │ token  │ │ market │ │treasury│ │migration│ │rewards │      │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘      │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐      │
│  │reclaim │ │revival │ │protocol│ │lending │ │ short  │ │ vault  │      │
│  │        │ │        │ │treasury│ │        │ │        │ │        │      │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘      │
│  ┌────────┐                                                              │
│  │  swap  │  (vault-routed DeepPool buys/sells)                          │
│  └────────┘                                                              │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Overview

Torch turns every token into a self-contained financial system. A token launches with a bonding curve, graduates to a DeepPool CPMM, and immediately gets margin lending + short selling — both backed by real on-chain reserves, no oracle, no governance. The 300M-token treasury lock created at launch is the literal short pool; the SOL accumulated from transfer fees + bonding splits is the literal lending pool.

Four phases per token: **Bonding → Migration → Trading → Margin**. Each phase builds the next.

V20 replaces the Raydium CPMM dependency with DeepPool (in-house, formally verified). All WSOL handling is gone — DeepPool holds native SOL on the pool PDA. The migration handler dropped from ~400 lines to ~100. See [deeppool.md](./deeppool.md) for the integration detail.

---

## Phase Lifecycle

| Phase | Trigger | What activates |
|---|---|---|
| **Bonding** | `create_token` | Constant-product curve: 700M tokens sellable, 300M locked, 100 SOL (Flame) or 200 SOL (Torch) graduation target |
| **Migration** | `fund_migration_sol` + `migrate_to_dex` (permissionless after target reached) | DeepPool pool created with bonded SOL + remaining tokens. 100% of LP burned to pool PDA. Mint/freeze/transfer-fee authorities revoked. |
| **Trading** | Post-migration | DeepPool swap as canonical price. 0.07% Token-2022 transfer fee on every transfer; permissionless `harvest_fees` + `swap_fees_to_sol` recycle into the treasury |
| **Margin** | Auto-enabled at `create_token` (longs; shorts via `enable_short_selling` if needed) | Treasury SOL is lending pool. 300M `TreasuryLock` tokens are short pool. Depth-anchored LTV (25-50%), 65% liquidation threshold, 2%/epoch interest, no oracle |

---

## Program Structure

```
programs/torch_market/src/
├── lib.rs               # 30 instruction entry points
├── handlers/            # Business logic per instruction domain
│   ├── admin.rs         # initialize, update_dev_wallet
│   ├── token.rs         # create_token (Token-2022 + treasury_lock + auto-enable shorts)
│   ├── market.rs        # buy, sell (curve trading, with vault routing)
│   ├── migration.rs     # fund_migration_sol, migrate_to_dex (DeepPool create_pool CPI)
│   ├── treasury.rs      # harvest_fees, swap_fees_to_sol (DeepPool swap CPI)
│   ├── rewards.rs       # star_token
│   ├── reclaim.rs       # reclaim_failed_token (7-day inactivity)
│   ├── revival.rs       # contribute_revival
│   ├── protocol_treasury.rs  # initialize / advance_epoch / claim_protocol_rewards
│   ├── lending.rs       # borrow, repay, liquidate
│   ├── short.rs         # enable_short_selling, open_short, close_short, liquidate_short
│   ├── vault.rs         # create_vault, deposit, withdraw, link/unlink_wallet, transfer_authority, withdraw_tokens
│   └── swap.rs          # vault_swap (vault-routed DeepPool buy/sell)
├── contexts.rs          # Anchor #[derive(Accounts)] for every instruction
├── state.rs             # 13 #[account] types
├── constants.rs         # Protocol parameters and PDA seeds
├── errors.rs            # Custom error variants
├── math.rs              # Pure arithmetic (single source of truth for fees, curve, lending, shorts, accrual). Kani proofs import directly from here.
├── migration.rs         # Migration handler implementation (DeepPool CPI flow)
├── pool_validation.rs   # DeepPool PDA derivation + reserve reading + depth-band LTV helpers
├── token_2022_utils.rs  # Token-2022 transfer-fee + metadata extension helpers
└── kani_proofs.rs       # 75 formal verification harnesses (cfg(kani))
```

---

## On-Chain Accounts

13 `#[account]` types. One additional system-owned PDA (`TorchVaultSol`) has no data layout — it's referenced by seeds only.

### GlobalConfig

Protocol-wide configuration. Singleton.

| Field | Type | Description |
|---|---|---|
| authority | Pubkey | Admin (pause, update settings) |
| treasury | Pubkey | Legacy fee wallet reference |
| dev_wallet | Pubkey | Receives 50% of protocol fee (`DEV_WALLET_SHARE_BPS = 5000`) |
| protocol_fee_bps | u16 | `PROTOCOL_FEE_BPS = 50` (0.5%) |
| paused | bool | Emergency pause flag |
| total_tokens_launched | u64 | Counter |
| total_volume_sol | u64 | Cumulative volume |
| bump | u8 | |

**Seeds:** `["global_config"]`

---

### BondingCurve

Per-token curve state. Created at `create_token`.

| Field | Type | Description |
|---|---|---|
| mint | Pubkey | Token mint |
| creator | Pubkey | Creator wallet |
| virtual_sol_reserves | u64 | For pricing — starts at `3 * bonding_target / 8` (37.5 SOL Flame / 75 SOL Torch) |
| virtual_token_reserves | u64 | For pricing — starts at 756.25M tokens |
| real_sol_reserves | u64 | Actual SOL accumulated |
| real_token_reserves | u64 | Actual tokens remaining |
| bonding_complete | bool | Reached graduation target |
| bonding_complete_slot | u64 | Slot of completion |
| migrated | bool | Migrated to DeepPool |
| last_activity_slot | u64 | For 7-day inactivity reclaim |
| reclaimed | bool | Failed token reclaimed |
| bump | u8 | |
| treasury_bump | u8 | Treasury PDA bump cache |
| bonding_target | u64 | Per-token graduation target (100 or 200 SOL in lamports; 0 = legacy default) |

**Seeds:** `["bonding_curve", mint]`

**Note:** Token metadata (`name`, `symbol`, `uri`) lives on the Token-2022 mint via the `TokenMetadata` extension — not on this account. Saves 243 bytes per curve vs the pre-V20 layout.

---

### Treasury

Per-token treasury: SOL balance, lending state, shorts state, baseline for ratio gating. The single account that holds per-token margin parameters.

| Field | Type | Description |
|---|---|---|
| bonding_curve | Pubkey | Back-reference |
| mint | Pubkey | Token mint |
| sol_balance | u64 | SOL available for lending + payouts |
| is_community_token | bool | `true` = 100% of fees to treasury (creator share = 0); `false` = 85/15 treasury/creator |
| short_collateral_reserved | u64 | SOL reserved by active shorts (excluded from available-to-lend) |
| last_buyback_slot | u64 | For sell-cycle cooldown |
| harvested_fees | u64 | Cumulative SOL from transfer-fee harvest |
| bump | u8 | |
| baseline_sol_reserves | u64 | Pool SOL at migration (ratio-gate baseline) |
| baseline_token_reserves | u64 | Pool tokens at migration |
| short_selling_enabled | bool | Set true by `enable_short_selling` (or `create_token` for new mints) |
| min_buyback_interval_slots | u64 | Cooldown between swap_fees_to_sol calls |
| baseline_initialized | bool | Set true at migration |
| total_stars | u64 | Stars received |
| star_sol_balance | u64 | SOL from stars |
| creator_paid_out | bool | One-time creator payout triggered |
| **Lending state** | | |
| total_sol_lent | u64 | SOL currently lent (longs) |
| total_collateral_locked | u64 | Tokens held as long collateral |
| active_loans | u64 | Open `LoanPosition` count |
| total_interest_collected | u64 | Cumulative interest paid by longs |
| lending_enabled | bool | Auto-enabled at creation |
| interest_rate_bps | u16 | Long interest, default 200 (2%/epoch) |
| max_ltv_bps | u16 | Default 5000 (50%) — clamped at borrow time by `get_depth_max_ltv_bps(pool_sol)` |
| liquidation_threshold_bps | u16 | Default 6500 (65%) |
| liquidation_bonus_bps | u16 | Default 1000 (10%) |
| liquidation_close_bps | u16 | Default 5000 (50% partial close cap) |
| lending_utilization_cap_bps | u16 | Default 8000 (80% of treasury SOL is lendable) |

**Seeds:** `["treasury", mint]`

---

### UserPosition

Per-user, per-token position on the bonding curve (pre-migration trading record).

| Field | Type | Description |
|---|---|---|
| user | Pubkey | Wallet |
| bonding_curve | Pubkey | Reference |
| total_purchased | u64 | Gross tokens received from buys |
| tokens_received | u64 | Net after any fees |
| tokens_burned | u64 | (Legacy field; always 0 in V20 — vote vault was removed) |
| total_sol_spent | u64 | SOL spent across all buys |
| bump | u8 | |

**Seeds:** `["user_position", bonding_curve, user]`

---

### UserStats

Per-user platform-wide volume tracking, drives epoch reward eligibility.

| Field | Type | Description |
|---|---|---|
| user | Pubkey | Wallet |
| total_volume | u64 | All-time SOL volume |
| volume_current_epoch | u64 | Current epoch volume |
| volume_previous_epoch | u64 | Previous epoch (claimable) |
| last_epoch_claimed | u64 | Last claimed epoch index |
| total_rewards_claimed | u64 | All-time rewards claimed |
| last_volume_epoch | u64 | Tracks lazy epoch transition |
| bump | u8 | |

**Seeds:** `["user_stats", user]`

---

### StarRecord

Idempotent star marker — one per (user, mint) pair.

| Field | Type | Description |
|---|---|---|
| user | Pubkey | User who starred |
| mint | Pubkey | Starred token |
| starred_at_slot | u64 | Slot of star |
| bump | u8 | |

**Seeds:** `["star_record", user, mint]`

---

### ProtocolTreasury

Singleton. Accumulates 0.5% protocol fees and reclaimed-token SOL; distributes via epoch claims.

| Field | Type | Description |
|---|---|---|
| authority | Pubkey | Protocol authority |
| current_balance | u64 | SOL held |
| reserve_floor | u64 | Minimum balance (currently 0) |
| total_fees_received | u64 | Lifetime fees |
| total_distributed | u64 | Lifetime distributions |
| current_epoch | u64 | Epoch index |
| last_epoch_ts | i64 | Unix timestamp of last `advance_protocol_epoch` |
| total_volume_current_epoch | u64 | Aggregate trading volume current epoch |
| total_volume_previous_epoch | u64 | Aggregate volume of just-closed epoch (claim denominator) |
| distributable_amount | u64 | Current claimable pool |
| bump | u8 | |

**Seeds:** `["protocol_treasury_v11"]`

---

### LoanPosition

Per-user, per-token long position. SOL borrowed against token collateral.

| Field | Type | Description |
|---|---|---|
| user | Pubkey | Borrower |
| mint | Pubkey | Token |
| collateral_amount | u64 | Tokens locked |
| borrowed_amount | u64 | SOL principal owed |
| accrued_interest | u64 | Interest since `last_update_slot` |
| last_update_slot | u64 | Last accrual slot (advances on every `accrue_interest` call, including zero-debt path — see `verify_interest_accrual_slot_advance`) |
| bump | u8 | |

**Seeds:** `["loan", mint, user]`

---

### ShortPosition

Per-user, per-token short position. Tokens borrowed against SOL collateral.

| Field | Type | Description |
|---|---|---|
| user | Pubkey | Shorter |
| mint | Pubkey | Token |
| sol_collateral | u64 | SOL posted (held in Treasury) |
| tokens_borrowed | u64 | Tokens owed |
| accrued_interest | u64 | Interest in token terms |
| last_update_slot | u64 | Last accrual slot (same invariant as LoanPosition) |
| bump | u8 | |

**Seeds:** `["short", mint, user]`

---

### ShortConfig

Per-token short market aggregate state. Holds no SOL; purely counters.

| Field | Type | Description |
|---|---|---|
| mint | Pubkey | Token |
| total_tokens_lent | u64 | Tokens currently borrowed by all shorts |
| active_positions | u64 | Open short count |
| total_interest_collected | u64 | Cumulative interest collected (tokens) |
| bump | u8 | |

**Seeds:** `["short_config", mint]`

---

### TreasuryLock

PDA that owns a Token-2022 ATA holding 300M locked tokens (30% of supply) — the short pool reserve. No instruction releases it.

| Field | Type | Description |
|---|---|---|
| mint | Pubkey | Token mint this lock belongs to |
| bump | u8 | |

**Seeds:** `["treasury_lock", mint]`

**Lock ATA:** `get_associated_token_address(treasury_lock_pda, mint, TOKEN_2022_PROGRAM)`

---

### TorchVault

Per-creator full-custody vault for agent interaction. Holds SOL and owns Token-2022 ATAs across any mint. Multi-wallet identity anchor — multiple wallets can be linked to act on the same vault, but only the `authority` can withdraw.

| Field | Type | Description |
|---|---|---|
| creator | Pubkey | Immutable — PDA seed |
| authority | Pubkey | Transferable; controls withdraw + link/unlink + transfer_authority |
| sol_balance | u64 | Available SOL |
| total_deposited | u64 | Lifetime deposits |
| total_withdrawn | u64 | Lifetime withdrawals |
| total_spent | u64 | Lifetime SOL spent (buys, repay, etc.) |
| total_received | u64 | Lifetime SOL received (sells, borrow proceeds) |
| linked_wallets | u8 | Current count |
| created_at | i64 | Unix timestamp |
| bump | u8 | |

**Seeds:** `["torch_vault", creator]`

**Balance invariant:** `sol_balance = total_deposited + total_received - total_withdrawn - total_spent`

---

### VaultWalletLink

Reverse pointer from a wallet to its vault. One per wallet.

| Field | Type | Description |
|---|---|---|
| vault | Pubkey | TorchVault this wallet acts on |
| wallet | Pubkey | The linked wallet |
| linked_at | i64 | Link creation timestamp |
| bump | u8 | |

**Seeds:** `["vault_wallet", wallet]`

---

### TorchVaultSol (system-owned PDA, no `#[account]`)

System-owned companion to TorchVault. 0 bytes of data. Used only during `vault_swap` buys as a lamport waypoint: the buy handler shuffles `amount_in` from `torch_vault` → `torch_vault_sol`, then DeepPool's swap CPI pulls it via `System.transfer` (which requires a system-owned source). Sits at 0 lamports between swaps. See **Vault Layer Mechanics** below and `audit.md` Deep Dive §7 for the redhat coverage.

**Seeds:** `["torch_vault_sol", creator]`

---

## Instructions

30 instructions across 7 domains. Vault-routed variants accept optional `torch_vault` + `vault_wallet_link` + `vault_token_account` accounts; without them, operations execute as wallet-funded.

### Admin (2)

| Instruction | Description |
|---|---|
| `initialize` | One-time protocol setup |
| `update_dev_wallet` | Authority-only update of the dev wallet address |

### Token Creation (1)

| Instruction | Description |
|---|---|
| `create_token` | Create Token-2022 mint with 0.07% transfer fee + metadata extension, init bonding curve (per-tier virtual reserves), mint 700M to curve vault + 300M to TreasuryLock ATA, auto-enable shorts |

`CreateTokenArgs`: `name: String`, `symbol: String`, `uri: String`, `sol_target: u64` (100 or 200 SOL, lamports), `community_token: bool` (default `true`).

### Market (2)

| Instruction | Description |
|---|---|
| `buy` | Buy tokens from bonding curve. SOL split: 0.5% protocol fee (50/50 dev/protocol_treasury), then decaying treasury share (17.5% → 2.5%), creator share (0% → 1%; 0 for community tokens), remainder to curve. 100% of tokens to buyer. 2% wallet cap enforced. |
| `sell` | Sell tokens back to curve. No sell fee. Per-buyer position tracked. |

### Migration (2)

| Instruction | Description |
|---|---|
| `fund_migration_sol` | Direct-lamport transfer of bonded SOL from BondingCurve PDA to payer. Separated from migrate_to_dex to isolate lamport manipulation from CPIs. |
| `migrate_to_dex` | Permissionless. CPI `deep_pool::create_pool` with `torch_config` PDA as signer. Burn 100% of LP. Revoke mint + freeze + transfer-fee-config authorities. Record `baseline_sol_reserves` + `baseline_token_reserves`. Reimburse payer migration cost from treasury. |

### Treasury (2)

| Instruction | Description |
|---|---|
| `harvest_fees` | Permissionless. Harvest accumulated Token-2022 withheld fees from arbitrary source accounts (passed via `remaining_accounts`) into the treasury's ATA. |
| `swap_fees_to_sol` | Permissionless. Ratio-gated: only sells when DeepPool price is ≥120% of migration baseline. Sells 15% of held tokens (or 100% if balance ≤ 1M tokens). DeepPool swap CPI (treasury signs as `sol_source`). Creator fee split (15%) carved off the SOL received for creator tokens. |

### Rewards (4)

| Instruction | Description |
|---|---|
| `star_token` | One-time star per (user, mint) for 0.02 SOL. Goes to `star_sol_balance`. |
| `initialize_protocol_treasury` | One-time setup of the ProtocolTreasury PDA |
| `advance_protocol_epoch` | Permissionless crank. Time-gated to one epoch (~7 days). Snapshots previous-epoch volume, opens new epoch, computes `distributable_amount`. |
| `claim_protocol_rewards` | User claims pro-rata share of `distributable_amount` based on `volume_previous_epoch`. Eligibility: ≥2 SOL volume in previous epoch. Capped at 10% of distributable per user. Min claim: 0.1 SOL. |

### Recovery (2)

| Instruction | Description |
|---|---|
| `reclaim_failed_token` | If bonding not complete and `last_activity_slot` is > 7 days old, anyone can reclaim. All curve SOL moves to protocol treasury (becomes epoch rewards). Marks token reclaimed. |
| `contribute_revival` | Permissionless deposit toward bringing a reclaimed token back. Threshold: `3 * bonding_target / 8` (37.5 SOL Flame / 75 SOL Torch). When met, trading resumes. Contributors receive no tokens. |

### Lending (3)

| Instruction | Description |
|---|---|
| `borrow` | Post token collateral, borrow SOL. Reads pool reserves from DeepPool. `effective_max_ltv = min(get_depth_max_ltv_bps(pool_sol), treasury.max_ltv_bps)`. Enforces utilization cap (80% of treasury) and per-user cap (`max_borrow = lendable * (collateral / TOTAL_SUPPLY) * 23`). |
| `repay` | Interest-first repayment. Full repay returns all collateral. Partial repay leaves position open. |
| `liquidate` | Permissionless. Re-checks LTV > 65% via current pool reads. Liquidator pays up to 50% of total debt, receives collateral tokens at current pool price + 10% bonus. Bad-debt write-off correctly decrements `total_sol_lent`. |

### Shorts (4)

| Instruction | Description |
|---|---|
| `enable_short_selling` | Admin (rare; auto-enabled at `create_token` for new mints). Creates ShortConfig, flips treasury flag. |
| `open_short` | Post SOL collateral, borrow tokens from TreasuryLock. Same depth-band + per-user cap as lending (denominator is `treasury.sol_balance`, not TOTAL_SUPPLY, since collateral is SOL). |
| `close_short` | Return tokens (+ interest in token terms). Interest-first. Full close releases SOL collateral. |
| `liquidate_short` | Permissionless. Same lifecycle as long liquidation, asset-inverted. Bad-debt write-off decrements `total_tokens_lent`. |

### Vault (8)

| Instruction | Description |
|---|---|
| `create_vault` | Create TorchVault for the signer (auto-links creator wallet) |
| `deposit_vault` | Permissionless SOL deposit |
| `withdraw_vault` | Authority-only SOL withdrawal |
| `link_wallet` | Authority links a controller wallet via VaultWalletLink |
| `unlink_wallet` | Authority closes a VaultWalletLink |
| `transfer_authority` | Transfer vault control to a new wallet |
| `withdraw_tokens` | Authority-only token withdrawal from vault ATA (composability escape hatch for external DeFi) |
| `vault_swap` | Vault-routed DeepPool buy or sell. Buy path uses `TorchVaultSol` as `sol_source`; sell path uses `torch_vault` directly. |

---

## Migration to DeepPool

The full DeepPool integration rationale, account-count reduction, and CPI shape lives in [deeppool.md](./deeppool.md). The architectural shape on torch_market's side:

```
fund_migration_sol     ────►  bonded SOL: BondingCurve PDA → payer (lamport debit)
migrate_to_dex
  ├─ excess token burn (if any)
  ├─ transfer pool-side tokens BondingCurve → payer (with Token-2022 fee deduction)
  ├─ CPI deep_pool::create_pool (signed by torch_config PDA)
  ├─ burn 100% of LP received by payer
  ├─ revoke mint authority   → None  (permanent)
  ├─ revoke freeze authority → None  (permanent)
  ├─ revoke fee-config auth  → None  (transfer fee locked forever)
  ├─ treasury reimburses payer (rent + CPI cost, measured by lamport delta)
  └─ record baseline (pool SOL/token reserves at migration)
```

**Pool namespace:** every DeepPool pool created by torch lives at `[deep_pool, torch_config, mint]`. The `torch_config` PDA is signed by the program — cryptographically unfrontrunnable. Nobody outside torch_market can create a pool under torch's namespace.

**Pool reserve reading:** `pool_sol = pool_pda.lamports() - rent_exempt`, `pool_tokens = vault_ata.amount`. Two-line read, no raw byte parsing.

---

## Lending & Short Mechanics

Both sides share parameters, math, and lifecycle structure. The math is intentionally symmetric — the only asymmetry is asset roles (long borrows SOL against tokens; short borrows tokens against SOL).

### Depth-anchored max LTV

```rust
fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < 5 SOL          { 0 }      // margin operations blocked
    else if pool_sol < 50 SOL    { 2500 }   // 25% max
    else if pool_sol < 200 SOL   { 3500 }   // 35% max
    else if pool_sol < 500 SOL   { 4500 }   // 45% max
    else                          { 5000 }  // 50% max
}
```

Pool depth IS the manipulation-resistance signal. Deeper pools = harder to move price = higher leverage permitted. No oracle, no keeper, no stored baseline.

### Per-user borrow cap

`max_user_borrow = lendable_pool * (user_collateral / divisor) * 23`

- Long divisor: `TOTAL_SUPPLY` (collateral is tokens, fixed denominator)
- Short divisor: `treasury.sol_balance` (collateral is SOL, dynamic denominator)

The `c` cancels — the cap-implied LTV depends only on pool ratio and treasury size, not individual position size. See [risk.md](./risk.md) §3.

### Interest accrual

`interest = principal * rate_bps * slots / (10_000 * EPOCH_DURATION_SLOTS)`

Default rate: 2%/epoch (`DEFAULT_INTEREST_RATE_BPS = 200`). Epoch = 7 days.

Accrual happens at the start of every position-touching instruction (`borrow`, `repay`, `liquidate`, `open_short`, `close_short`, `liquidate_short`). The pure transition function `math::apply_interest_accrual` (and its short variant) ensures `last_update_slot` always advances to the current slot — including on the zero-debt early-return path. This prevents phantom interest on positions that are fully repaid and later re-borrowed without closing the account.

### Liquidation

`current_ltv > 65%` (`DEFAULT_LIQUIDATION_THRESHOLD_BPS`). Liquidator covers up to 50% of total debt (`DEFAULT_LIQUIDATION_CLOSE_BPS`), receives collateral at current pool price + 10% bonus. Bad-debt write-off: when collateral can't cover the slice, the shortfall reduces `borrowed` and `total_sol_lent` together — proven equivalent to the simple form by Kani harnesses 63/64.

---

## Vault Layer Mechanics

`TorchVault` is the per-creator state-bearing account: SOL balance, lifetime totals, link count, plus token ATAs for any mint via `get_associated_token_address(vault_pda, mint, TOKEN_2022)`. Authority is transferable; linked wallets sign for trading but can't withdraw.

### Why the TorchVault + TorchVaultSol split

DeepPool v3.1 unified its swap path so all SOL flow goes through `System.transfer(from=sol_source, ...)`. The system program requires `from.owner == system_program`. TorchVault is program-owned (holds non-trivial state) and can't be a System.transfer source.

Solution: companion system-owned PDA `TorchVaultSol` at `["torch_vault_sol", creator]` (0 bytes, system-owned). Buy path:

1. Decrement `vault.sol_balance -= amount_in`
2. Direct lamport shuffle: `torch_vault.lamports -= amount_in; vault_sol.lamports += amount_in`
3. CPI `deep_pool::swap` with `user = torch_vault`, `sol_source = vault_sol`. DeepPool's `System.transfer(from=vault_sol, ...)` consumes the staged lamports.
4. After CPI, `vault_sol` returns to whatever it was before (typically 0).

All three steps execute atomically within one instruction. If the CPI fails, the full transaction reverts including the lamport shuffle.

Sell path: `sol_source = torch_vault` directly. DeepPool credits lamports via direct-add (owner-agnostic), which works for program-owned destinations.

### vault_sol dust trap (by design)

Anyone can `System.transfer` lamports to a `vault_sol` PDA. The donation lands and stays — no instruction reclaims arbitrary lamports from `vault_sol` (only `vault_swap` buy touches it, and only for exactly `amount_in`). Donor self-traps their SOL; creator and protocol are unaffected.

**Critical design constraint:** never add a permissionless reclaim instruction for `vault_sol` lamports. An attacker could pre-credit `vault_sol` and sweep it through that handler. Any future reclaim must require creator signature and cap at a safe amount. See `audit.md` Deep Dive §7 for the full redhat analysis.

---

## Treasury Harvest Cycle

Post-migration, the 0.07% Token-2022 transfer fee on every transfer is the perpetual treasury growth engine. Two permissionless cranks:

```
harvest_fees       ──► withheld balances from arbitrary Token-2022 accounts
                       → mint → treasury ATA (tokens)
swap_fees_to_sol   ──► DeepPool swap (treasury signs as sol_source)
                       → SOL received (delta-measured)
                       → creator fee split (15% for creator tokens, 0% community)
                       → treasury.sol_balance += rest
```

Ratio-gated: only sells when `(pool_sol/pool_tokens) >= 1.2 * baseline_ratio`. Sells 15% of held tokens per call (100% if balance ≤ 1M tokens). Cooldown via `min_buyback_interval_slots` (default ~18 min) to prevent rapid sell cycles.

---

## Composition With DeepPool

| torch_market does | DeepPool does |
|---|---|
| Create pools (CPI into `deep_pool::create_pool` with `torch_config` signer) | Owns pool PDA, enforces fee invariants, validates swap math |
| Validate Token-2022 extension allowlist on its own `create_token` (rejects `PermanentDelegate`, `NonTransferable`) | Stays permissionless for other integrators |
| Read pool reserves (`pool_pda.lamports() + vault.amount`) for margin pricing | Holds reserves, computes swap outputs |
| Sign swaps as `user` from PDAs (`torch_vault`, `treasury`) via `invoke_signed` | Verifies `sol_source: Signer` constraint |
| Burn 100% of LP at migration → pool PDA's own LP ATA → permanently locked | Mints LP per `create_pool`; doesn't enforce burn |

DeepPool has its own audit and 16 separate Kani proofs covering swap math (K invariant, fee conservation, LP proportionality). Total verification across composed system: **91 proof harnesses** (75 torch + 16 deep_pool).

---

## Verification Surface

- **75 Kani proof harnesses** (`kani_proofs.rs`, gated by `cfg(kani)`). Cover all fee calculations, bonding curve pricing, lending math, short math, depth-band boundaries, migration arithmetic, interest accrual state transitions, treasury ratio gating, DeepPool CPI accounting. Math harnesses import directly from `math.rs` — every property is proven against the exact code that runs on-chain, not a replica.
- **33 proptest properties × 5,000 cases** (`tests/math_proptests.rs`). Random-input sweep across the full u64 space; complements Kani's bounded model checking.
- **53 end-to-end tests** (`tests/`). Run with `anchor test` against a local validator.
- **Independent audit** (Claude Opus 4.7, see [audit.md](./audit.md)): 0 critical / 0 high / 0 medium / 0 low findings. 24 exploit classes covered in the adversarial redhat pass.

---

## Out of Scope (V20)

Honest about what V20 does not do:

- **Permissionless migration timing.** Anyone can call `migrate_to_dex` after bonding completes, but nobody is forced to. Economic incentive (treasury reimbursement) handles it in practice.
- **Shorts on drained pools.** If pool depth drops below 5 SOL, short liquidation is blocked. Accepted trade-off — the alternative enables sandwich attacks on the liquidation path.
- **Upgrade authority revocation.** Live on mainnet during stabilization. Migrate to public timelock or multisig within the 30-90 day window post-launch via `solana program set-upgrade-authority --final`.
- **Cross-token margin.** Each `(user, mint)` pair has its own isolated LoanPosition and ShortPosition. No portfolio margining. Failure of one position cannot affect another.
- **Governance.** None. All parameters are immutable at deploy. No vote, no proposal, no token-gated controls.

---

## Version Evolution (high-level)

| Era | Major changes |
|---|---|
| V3.x | Token-2022 transfer fee, on-chain metadata, vault system, vote vault, Raydium CPMM integration |
| V4.0 | Treasury rate rebalance (12.5% → 2.5%), protocol fee 1% → 0.5%, Spark tier removed from creation |
| V10.x | Oracle-free margin lending; depth bands + per-user caps; bad-debt accounting fix |
| V11 | Margin risk guards (depth-adaptive LTV, min pool liquidity floor); short selling |
| **V20.0.0** | **Raydium → DeepPool migration.** Removed all WSOL handling, byte-level Raydium pool parsing, vote vault. New Kani proofs (69, 72, 73) for DeepPool CPI accounting. |
| **V20 torch_next** | TorchVault + TorchVaultSol split for DeepPool v3.1 compatibility. BondingCurve shrink (243 bytes/curve). Dead-constraint cleanup. New program ID. 7 additional redhat exploit classes (#18-24, all mitigated). |
| V20 (interest accrual fix, current branch) | `apply_interest_accrual` post-condition strengthened: `last_update_slot` advances on every call including zero-debt. Prevents phantom interest on re-borrowed positions. +2 Kani harnesses (71-72) + 2 proptest properties. |

---

## Cross-References

- [whitepaper.md](./whitepaper.md) — protocol intent, parameters, economic design
- [risk.md](./risk.md) — formal analysis of the depth-anchored risk model
- [deeppool.md](./deeppool.md) — DeepPool integration detail
- [verification.md](./verification.md) — Kani harness catalog (all 75)
- [properties.md](./properties.md) — proptest property catalog (all 33)
- [audit.md](./audit.md) — V20.0.0 internal security audit + redhat findings
- [sdk.md](./sdk.md) — TypeScript SDK reference

---

*© 2026 Brightside Solutions.*
