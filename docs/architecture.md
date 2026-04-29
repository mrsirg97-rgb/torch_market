# Torch Market Protocol Architecture

A Solana program for launching tokens with bonding curves, community voting, DEX migration, and oracle-free margin trading.

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                          TORCH MARKET PROTOCOL v11.0.0                               │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                      │
│  ┌─────────────────────────────────────────────────────────────────────────────┐    │
│  │                           PROTOCOL LAYER                                     │    │
│  │  ┌─────────────────┐  ┌──────────────────────────────────────┐              │    │
│  │  │  GlobalConfig   │  │        ProtocolTreasury               │              │    │
│  │  │  (authority,    │  │  (0.5% fees + reclaimed SOL,            │              │    │
│  │  │   settings)     │  │   no floor, epoch rewards)      │              │    │
│  │  └─────────────────┘  └──────────────────────────────────────┘              │    │
│  └─────────────────────────────────────────────────────────────────────────────┘    │
│                                       │                                              │
│                                       ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────┐    │
│  │                           PER-TOKEN LAYER                                    │    │
│  │                                                                              │    │
│  │  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐                   │    │
│  │  │    Token     │    │   Bonding    │    │   Treasury   │                   │    │
│  │  │   (Mint)     │───▶│    Curve     │───▶│  (fees,      │                   │    │
│  │  │  Token-2022  │    │  (pricing,   │    │   stars,     │                   │    │
│  │  │ 0.07% xfer   │    │  V36 novote) │    │   lending)   │                   │    │
│  │  └──────────────┘    └──────┬───────┘    └──────────────┘                   │    │
│  │                             │                                                │    │
│  │         ┌───────────────────┼───────────────────┐                           │    │
│  │         ▼                   ▼                   ▼                           │    │
│  │  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐                   │    │
│  │  │  Token Vault │    │  Treasury's  │    │  Raydium     │                   │    │
│  │  │  (tradeable  │    │  Token Acct  │    │  CPMM Pool   │                   │    │
│  │  │   supply)    │    │  (vote vault)│    │  (post-grad) │                   │    │
│  │  └──────────────┘    └──────────────┘    └──────────────┘                   │    │
│  └─────────────────────────────────────────────────────────────────────────────┘    │
│                                       │                                              │
│                                       ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────┐    │
│  │                            USER LAYER                                        │    │
│  │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐              │    │
│  │  │  UserPosition   │  │   UserStats     │  │   StarRecord    │              │    │
│  │  │  (per-token     │  │  (platform-wide │  │  (per-token     │              │    │
│  │  │   holdings,     │  │   volume,       │  │   appreciation) │              │    │
│  │  │   vote)         │  │   rewards)      │  │                 │              │    │
│  │  └─────────────────┘  └─────────────────┘  └─────────────────┘              │    │
│  └─────────────────────────────────────────────────────────────────────────────┘    │
│                                       │                                              │
│                                       ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────────────────┐    │
│  │                    VAULT LAYER (V3.1.0 — Full Custody)                       │    │
│  │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐              │    │
│  │  │   TorchVault    │  │VaultWalletLink  │  │  Vault ATAs     │              │    │
│  │  │  (per-creator   │◀─│  (per-wallet    │  │  (per-mint      │              │    │
│  │  │   SOL + token   │  │   reverse       │  │   token accts   │              │    │
│  │  │   full custody) │  │   pointer)      │  │   owned by PDA) │              │    │
│  │  └─────────────────┘  └─────────────────┘  └─────────────────┘              │    │
│  └─────────────────────────────────────────────────────────────────────────────┘    │
│                                                                                      │
├─────────────────────────────────────────────────────────────────────────────────────┤
│  INSTRUCTION HANDLERS                                                                │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐      │
│  │ admin  │ │ token  │ │ market │ │treasury│ │ dex    │ │rewards │ │reclaim │      │
│  │        │ │        │ │        │ │/lending│ │migrate │ │        │ │/revival│      │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘ └────────┘      │
│  ┌────────┐ ┌────────┐ ┌────────┐                                                   │
│  │ vault  │ │  swap  │ │ short  │                                                   │
│  │        │ │(V3.1.1)│ │ (V5)   │                                                   │
│  └────────┘ └────────┘ └────────┘                                                   │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

## Overview

Torch Market is a token launchpad protocol that provides:

1. **Fair Launch via Bonding Curves** - Tokens are launched with a mathematical pricing curve that ensures early supporters get better prices while preventing manipulation
2. **[V36] Vote System Removed** - New tokens initialize `vote_finalized = true` and never accumulate vote-vault tokens; the buy split goes 100% to the buyer. Vote fields remain on the `BondingCurve` struct for Borsh layout compatibility with pre-V36 tokens, but are never written for new tokens.
3. **Automatic DEX Migration** - When bonding completes (100/200 SOL per tier), tokens migrate to Raydium DEX with locked liquidity. Permissionless — anyone can call once the per-token target is reached.
4. **Treasury Yield** - Per-token treasuries accumulate SOL via fee harvesting and sell cycle, deployed into lending yield and epoch rewards
5. **Protocol Fee Distribution** - 0.5% protocol fees split 50/50 between protocol treasury (epoch rewards) and dev wallet ([V11], `DEV_WALLET_SHARE_BPS = 5000`)
6. **[V5] Oracle-Free Margin Trading** - Two-sided margin system: long leverage (borrow SOL, post tokens) and short selling (borrow tokens, post SOL). Same math, same liquidation, opposite direction. No external oracle — Raydium pool price is the canonical price source. All positions backed by real on-chain assets
7. **[V11] Margin Risk Guards** - New positions blocked when pool SOL < 5 SOL (`MIN_POOL_SOL_LENDING`) or pool price has drifted >50% from migration baseline (`MAX_PRICE_DEVIATION_BPS`). Max LTV scales with pool depth: 25% (<50 SOL) → 35% (50-200) → 45% (200-500) → 50% (500+).

---

## Program Structure

```
torch_market/programs/torch_market/src/
├── lib.rs              # Entry points and instruction routing (31 instructions)
├── handlers/           # Business logic for each instruction domain
│   ├── admin.rs        # Protocol initialization
│   ├── token.rs        # Token creation (Token-2022) + treasury lock + [V11] auto-enable shorts
│   ├── market.rs       # Buy/sell operations (with vault routing) — [V36] no vote arg
│   ├── treasury.rs     # Fee harvesting + swap fees to SOL
│   ├── migration.rs    # DEX migration wrapper (fund_migration_wsol + migrate_to_dex)
│   ├── rewards.rs      # Star system (with vault routing)
│   ├── reclaim.rs      # Failed token reclaim
│   ├── revival.rs      # Token revival contributions (V31 per-tier thresholds, single path)
│   ├── protocol_treasury.rs  # Protocol fee distribution
│   ├── lending.rs      # Treasury lending (borrow/repay/liquidate) — [V11] depth LTV + price band
│   ├── short.rs        # [V5] Short selling (open_short/close_short/liquidate_short) — [V11] same risk guards
│   ├── swap.rs         # [V3.1.1] Vault-routed DEX trading (fund_vault_wsol + vault_swap)
│   └── vault.rs        # [V3.0.0] Torch Vault lifecycle + multi-wallet identity + [V3.1.0] withdraw_tokens
├── contexts.rs         # Anchor account validation (#[derive(Accounts)]) + arg structs
├── state.rs            # On-chain data structures (14 account types)
├── constants.rs        # Protocol parameters and seeds
├── errors.rs           # Custom error codes
├── math.rs             # [V11] Pure integer arithmetic — single source of truth for fees, curve, lending, shorts. Imported by both handlers and Kani proofs.
├── migration.rs        # Raydium CPMM integration
├── pool_validation.rs  # Shared pool validation + PDA derivation + [V11] depth-tier LTV, price-band, min-liquidity helpers
├── kani_proofs.rs      # Formal verification proofs (cfg(kani), 71 proofs as of v11.0.1)
└── token_2022_utils.rs # Token-2022 extension utilities (transfer fee, metadata pointer, token metadata)
```

---

## On-Chain Accounts

### GlobalConfig
Protocol-wide configuration. Single PDA per deployment.

| Field | Type | Description |
|-------|------|-------------|
| authority | Pubkey | Protocol admin (can pause, update settings) |
| treasury | Pubkey | Legacy protocol fee wallet |
| dev_wallet | Pubkey | Dev wallet (receives 10% of protocol fee) |
| _deprecated_platform_treasury | Pubkey | Deprecated (V3.2 — merged into protocol treasury) |
| protocol_fee_bps | u16 | Protocol fee (50 = 0.5%) |
| paused | bool | Emergency pause flag |
| total_tokens_launched | u64 | Counter |
| total_volume_sol | u64 | Cumulative SOL volume |

**PDA Seeds:** `["global_config"]`

---

### BondingCurve
Per-token bonding curve state. Created when a token is launched.

| Field | Type | Description |
|-------|------|-------------|
| mint | Pubkey | Token mint address |
| creator | Pubkey | Token creator wallet |
| virtual_sol_reserves | u64 | Virtual SOL for pricing ([V27] starts at 3*bonding_target/8) |
| virtual_token_reserves | u64 | Virtual tokens for pricing ([V27] starts at 756.25M tokens) |
| real_sol_reserves | u64 | Actual SOL in curve |
| real_token_reserves | u64 | Actual tokens available |
| vote_vault_balance | u64 | [V36] Deprecated — never incremented for new tokens (kept for layout compat) |
| permanently_burned_tokens | u64 | [V33] Deprecated — historical buyback burns only |
| bonding_complete | bool | Reached bonding target (per-token, see `bonding_target`) |
| bonding_complete_slot | u64 | Slot when bonding completed |
| votes_return / votes_burn | u64 | [V36] Deprecated — always 0 for new tokens |
| total_voters | u64 | [V36] Deprecated — always 0 for new tokens |
| vote_finalized | bool | [V36] Initialized to `true` at creation so the migration gate passes without a vote round |
| vote_result_return | bool | [V36] Always `false` for new tokens (vote vault is always empty, so no migration-time branching matters) |
| migrated | bool | Migrated to Raydium DEX |
| is_token_2022 | bool | Token-2022 mint flag |
| last_activity_slot | u64 | For inactivity tracking |
| reclaimed | bool | Failed token was reclaimed |
| name / symbol / uri | bytes | Token metadata |
| **Tiered Bonding (V23)** | | |
| bonding_target | u64 | [V23] Per-token graduation target in lamports (0 = 200 SOL default) |

**PDA Seeds:** `["bonding_curve", mint.key()]`

---

### Treasury
Per-token treasury for fee accumulation, lending, and creator rewards.

| Field | Type | Description |
|-------|------|-------------|
| bonding_curve | Pubkey | Associated bonding curve |
| mint | Pubkey | Token mint |
| sol_balance | u64 | SOL available for lending and rewards |
| total_bought_back | u64 | [V33] Deprecated — always 0 for new tokens |
| total_burned_from_buyback | u64 | [V33] Deprecated — [V5] repurposed as `total_short_sol_collateral` when shorts enabled |
| tokens_held | u64 | Tokens held in treasury ATA (recycled via swap_fees_to_sol) |
| harvested_fees | u64 | [V20] Cumulative SOL earned from swapping harvested transfer fees |
| **Sell Cycle (V9, simplified V33)** | | |
| baseline_sol_reserves | u64 | Pool SOL at migration (sell cycle price gating) |
| baseline_token_reserves | u64 | Pool tokens at migration (sell cycle price gating) |
| ratio_threshold_bps | u16 | [V33] Deprecated — always 0 for new tokens |
| reserve_ratio_bps | u16 | [V33] Deprecated — always 0 for new tokens |
| buyback_percent_bps | u16 | [V33] Deprecated — [V5] repurposed as short-enabled sentinel (`u16::MAX` = enabled) |
| min_buyback_interval_slots | u64 | Sell cycle cooldown (~18 min) |
| **Star System (V10)** | | |
| total_stars | u64 | Stars received |
| star_sol_balance | u64 | SOL from stars |
| creator_paid_out | bool | One-time payout triggered |
| **Lending (V2.4, params live on Treasury)** | | |
| total_sol_lent | u64 | SOL currently lent to long borrowers |
| total_collateral_locked | u64 | Tokens held as long collateral |
| active_loans | u64 | Count of open `LoanPosition` accounts |
| total_interest_collected | u64 | Cumulative SOL interest paid by longs |
| lending_enabled | bool | Auto-enabled at creation; not currently togglable |
| interest_rate_bps | u16 | Long interest, default 200 (2%/epoch) |
| max_ltv_bps | u16 | Default 50% — [V11] superseded by depth-tier LTV at borrow time |
| liquidation_threshold_bps | u16 | Default 65% |
| liquidation_bonus_bps | u16 | Default 10% |
| liquidation_close_bps | u16 | Default 50% close factor |
| lending_utilization_cap_bps | u16 | Default 80% — fraction of treasury SOL borrowable |

**PDA Seeds:** `["treasury", mint.key()]`

---

### UserPosition
Per-user position for a specific token.

| Field | Type | Description |
|-------|------|-------------|
| user | Pubkey | User wallet |
| bonding_curve | Pubkey | Bonding curve reference |
| total_purchased | u64 | Gross tokens purchased |
| tokens_received | u64 | Net tokens after burns |
| tokens_burned | u64 | Tokens sent to burn |
| total_sol_spent | u64 | SOL spent |
| has_voted | bool | Has cast vote |
| vote_return | bool | Vote choice |

**PDA Seeds:** `["user_position", bonding_curve.key(), user.key()]`

---

### UserStats
Per-user platform-wide statistics for protocol rewards.

| Field | Type | Description |
|-------|------|-------------|
| user | Pubkey | User wallet |
| total_volume | u64 | All-time SOL volume |
| volume_current_epoch | u64 | Current epoch volume |
| volume_previous_epoch | u64 | Previous epoch volume |
| last_epoch_claimed | u64 | Last claimed epoch |
| total_rewards_claimed | u64 | All-time rewards |
| last_volume_epoch | u64 | Lazy epoch transition |

**PDA Seeds:** `["user_stats", user.key()]`

---

### ProtocolTreasury (V11)
Single protocol treasury funded by trading fees AND reclaimed token SOL. No reserve floor; epoch-based distribution.

| Field | Type | Description |
|-------|------|-------------|
| authority | Pubkey | Protocol authority |
| current_balance | u64 | SOL held |
| reserve_floor | u64 | [V32] Minimum balance (0 SOL — all fees distributed) |
| total_fees_received | u64 | All-time fees |
| total_distributed | u64 | All-time distributions |
| current_epoch | u64 | Current epoch |
| last_epoch_ts | i64 | Unix timestamp of the last `advance_protocol_epoch` |
| total_volume_current_epoch | u64 | Trading volume in the current epoch (for pro-rata claims) |
| total_volume_previous_epoch | u64 | Trading volume in the just-closed epoch (used by `claim_protocol_rewards`) |
| distributable_amount | u64 | [V32] Full available balance (floor = 0) |

**PDA Seeds:** `["protocol_treasury_v11"]`

---

### StarRecord
Prevents double-starring by tracking user-token star pairs.

| Field | Type | Description |
|-------|------|-------------|
| user | Pubkey | User who starred |
| mint | Pubkey | Token that received star |
| starred_at_slot | u64 | When star was given |

**PDA Seeds:** `["star_record", user.key(), mint.key()]`

---

### LoanPosition (V2.4)
Per-user, per-token loan position for treasury lending.

| Field | Type | Description |
|-------|------|-------------|
| user | Pubkey | Borrower wallet |
| mint | Pubkey | Token mint |
| collateral_amount | u64 | Tokens locked as collateral |
| borrowed_amount | u64 | SOL principal owed |
| accrued_interest | u64 | Interest since last update |
| last_update_slot | u64 | Last interest calculation slot |

**PDA Seeds:** `["loan", mint.key(), user.key()]`

---

### TorchVault (V3.0.0, updated V3.1.0)
Per-creator full-custody vault for safe agent interaction. Holds SOL and token ATAs. Multi-wallet identity anchor.

| Field | Type | Description |
|-------|------|-------------|
| creator | Pubkey | Immutable — PDA seed (never changes) |
| authority | Pubkey | Controls withdraw, link/unlink, transfer (transferable) |
| sol_balance | u64 | Available SOL in vault |
| total_deposited | u64 | Lifetime SOL deposited (manual deposits) |
| total_withdrawn | u64 | Lifetime SOL withdrawn (manual withdrawals) |
| total_spent | u64 | Lifetime SOL spent (buys, repay, star) |
| total_received | u64 | [V3.1.0] Lifetime SOL received (sells, borrow proceeds) |
| linked_wallets | u8 | Number of wallets currently linked |
| created_at | i64 | Creation timestamp |

**PDA Seeds:** `["torch_vault", creator.key()]`

**Size:** 122 bytes (+ 8 discriminator)

**Balance invariant:** `sol_balance = total_deposited + total_received - total_withdrawn - total_spent`

**Vault Token Accounts:** The vault PDA can own Associated Token Accounts for any Token-2022 mint. These are deterministic (`get_associated_token_address(vault_pda, mint, token_2022_program)`) and created permissionlessly via the SDK before first use.

---

### VaultWalletLink (V3.0.0)
Reverse pointer linking a wallet to a vault. One link per wallet.

| Field | Type | Description |
|-------|------|-------------|
| vault | Pubkey | The TorchVault this wallet belongs to |
| wallet | Pubkey | The linked wallet |
| linked_at | i64 | When the link was created |

**PDA Seeds:** `["vault_wallet", wallet.key()]`

**Size:** 81 bytes (+ 8 discriminator)

**Lookup pattern:** Given any wallet, derive its VaultWalletLink PDA → read `vault` field → find the TorchVault. No enumeration needed.

---

### TreasuryLock (V27, updated V31)
Per-token treasury lock PDA. Owns a Token-2022 ATA containing 300M locked tokens (30% of supply). [V31] Also receives vote-return tokens at migration (up to ~55M additional).

| Field | Type | Description |
|-------|------|-------------|
| mint | Pubkey | Token mint this lock belongs to |
| bump | u8 | PDA bump |

**PDA Seeds:** `["treasury_lock", mint.key()]`

**Size:** 33 bytes (+ 8 discriminator)

**Lock token ATA:** `get_associated_token_address(treasury_lock_pda, mint, TOKEN_2022)` — holds 300M tokens at creation, plus any vote-return tokens at migration. No instruction can withdraw — release logic deferred to a future governance mechanism.

---

### ShortPosition (V5)
Per-user, per-token short position for margin trading. Mirror of `LoanPosition` with inverted collateral/debt assets.

| Field | Type | Description |
|-------|------|-------------|
| user | Pubkey | Short seller wallet |
| mint | Pubkey | Token mint |
| sol_collateral | u64 | SOL posted as collateral (held in Treasury) |
| tokens_borrowed | u64 | Tokens owed to treasury |
| accrued_interest | u64 | Interest accumulated in token terms |
| last_update_slot | u64 | Last interest calculation slot |

**PDA Seeds:** `["short", mint.key(), user.key()]`

---

### ShortConfig (V5)
Per-token aggregate state for short positions. Holds no SOL — purely tracks token utilization and position counts. SOL collateral is held in Treasury, tracked via repurposed `total_burned_from_buyback` field.

| Field | Type | Description |
|-------|------|-------------|
| mint | Pubkey | Token mint |
| total_tokens_lent | u64 | Tokens currently borrowed by all shorts |
| active_positions | u64 | Count of open short positions |
| total_interest_collected | u64 | Cumulative interest collected (tokens) |

**PDA Seeds:** `["short_config", mint.key()]`

---

## Instructions

### Admin Instructions

#### `initialize`
Initialize the protocol. Must be called once before any tokens can be created.

**Accounts:** authority (signer), global_config, treasury, dev_wallet, system_program

---

#### `initialize_protocol_treasury` (V11)
Initialize the protocol fee treasury PDA.

**Accounts:** authority (signer), global_config, protocol_treasury, system_program

---

#### `update_dev_wallet` (V8)
Update the dev wallet address. Only authority can call.

**Accounts:** authority (signer), global_config, new_dev_wallet

> **Note (V3.7.0):** `update_authority` was removed. Authority transfer is now done at deployment time via multisig tooling rather than an on-chain instruction, reducing the protocol's admin attack surface.

---

### Token Creation

#### `create_token`
Create a new Token-2022 token with transfer fee and on-chain metadata extensions.

**Arguments:**
- `name: String` (max 32 chars)
- `symbol: String` (max 10 chars)
- `uri: String` (max 200 chars)
- `sol_target: u64` — [V23] Bonding target in lamports. 0 = default (200 SOL). Must be one of: 100 or 200 SOL. [V4.0] 50 SOL (Spark) removed from creation; existing Spark tokens still function.
- `community_token: bool` — [V35] `true` (default) = community token (0% creator fees, all to treasury). `false` = creator token (V34 fee structure).

**Accounts:** creator (signer), global_config, mint, bonding_curve, token_vault, treasury, treasury_token_account, treasury_lock, treasury_lock_token_account, token_2022_program, associated_token_program, system_program, rent

**Effects:**
- Creates Token-2022 mint with TransferFeeConfig ([V11] 0.07% flat fee, `TRANSFER_FEE_BPS = 7`) + MetadataPointer + TokenMetadata extensions
- [V29] Metadata (name, symbol, uri) stored directly on the mint via Token-2022 metadata extension — no Metaplex dependency. Mint is allocated in two phases: initial space for TransferFeeConfig + MetadataPointer, then Token-2022 reallocs internally when TokenMetadata is initialized after `InitializeMint2`
- [V27] Initializes bonding curve with per-tier virtual reserves (IVS = 3*bonding_target/8, IVT = 756.25M tokens)
- [V36] Sets `vote_finalized = true` and leaves all vote fields at 0 — no vote round runs
- [V31] Mints 700M tokens to curve vault + 300M to treasury lock ATA
- Creates treasury PDA and its token account (vote vault, kept for layout compat — never receives tokens for new mints)
- [V31] Creates TreasuryLock PDA and its token ATA (holds 300M locked tokens)
- [V23] Validates `sol_target` against allowed tiers and stores in `bonding_curve.bonding_target`
- [V35] Sets community token flag in `treasury.total_bought_back` (sentinel `u64::MAX` = community, `0` = creator)
- [V11] Auto-enables short selling for the new token: writes `treasury.buyback_percent_bps = SHORT_ENABLED_SENTINEL` and zeroes `total_burned_from_buyback` (the repurposed short-collateral counter). The `enable_short_selling` admin instruction remains available for tokens created before this auto-enable was added.

---

### Market Instructions

#### `buy`
Buy tokens from the bonding curve.

**Arguments:**
- `sol_amount: u64` - SOL to spend (min 0.001 SOL)
- `min_tokens_out: u64` - Slippage protection

**[V36]** The first-buy `vote: Option<bool>` argument was removed. `BuyArgs` now contains only `sol_amount` and `min_tokens_out`.

**Fee Breakdown ([V11] rates):**
```
0.5% Protocol Fee → 50% Protocol Treasury + 50% Dev Wallet (DEV_WALLET_SHARE_BPS = 5000)
0% Treasury Fee  → TREASURY_FEE_BPS = 0 (treasury is funded only via the dynamic split below)
Remaining 99.5%  → Curve + Treasury split:
                   ├── total_split = 17.5% → 2.5% (linear decay over bonding progress)
                   ├── creator_sol = 0.2% → 1% (linear growth, carved out of total_split)
                   └── treasury_sol = total_split − creator_sol
                  [V35] Community tokens: creator_sol = 0; full split goes to treasury
```

**Token Distribution ([V36]):**
```
tokens_out (from curve) → 100% to Buyer  (vote vault no longer receives a cut)
```

**Accounts:** buyer, global_config, dev_wallet, mint, bonding_curve, token_vault, token_treasury, treasury_token_account, buyer_token_account, user_position, protocol_treasury, creator, [user_stats], [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**Vault-Funded Buy (V3.0.0, updated V3.1.0):** When `torch_vault`, `vault_wallet_link`, and `vault_token_account` are provided:
1. Validates `vault_wallet_link.vault == torch_vault.key()` (buyer is linked to this vault)
2. Checks `vault.sol_balance >= sol_amount`
3. Token CPIs execute first: tokens go to `vault_token_account` instead of `buyer_token_account`
4. SOL distributed via lamport manipulation from vault to all destinations (curve, treasury, dev wallet, protocol treasury)
5. Updates `vault.sol_balance` and `vault.total_spent`

**[V3.1.0] Token Routing:** When `vault_token_account` is present, tokens are sent to the vault's ATA instead of the buyer's wallet. Max wallet cap (2%) is checked against the vault ATA balance.

When vault accounts are omitted, the buy works exactly as before (buyer pays from own wallet, tokens to buyer's ATA).

---

#### `sell`
Sell tokens back to the bonding curve.

**Arguments:**
- `token_amount: u64` - Tokens to sell
- `min_sol_out: u64` - Slippage protection

**Fee:** No sell fee (0%)

**Accounts:** seller, mint, bonding_curve, token_vault, seller_token_account, user_position, token_treasury, [user_stats], [protocol_treasury], [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**[V3.1.0] Vault-Routed Sell:** When vault accounts are provided:
1. Validates `vault_wallet_link.vault == torch_vault.key()` (seller is linked)
2. Token CPI: vault PDA signs transfer from `vault_token_account` → `token_vault`
3. SOL: lamport manipulation from `bonding_curve` → `torch_vault` (after token CPI)
4. Updates `vault.sol_balance` and `vault.total_received`

When vault accounts are omitted, the sell works as before (seller's tokens, SOL to seller).

---

### Treasury Instructions

#### `harvest_fees` (V3, fixed V3.2.1)
Harvest accumulated transfer fees from Token-2022 accounts.

**Accounts:** payer, mint, bonding_curve, token_treasury, treasury_token_account, token_2022_program, associated_token_program, [source accounts in remaining_accounts]

**[V3.2.1] Security Fix:** `treasury_token_account` is now constrained via Anchor's `associated_token` constraints (`mint`, `authority`, `token_program`) to ensure it matches the treasury PDA's actual ATA. Previously unconstrained — see Security Audit History below.

---

#### `swap_fees_to_sol` (V20, updated V30)
Swap treasury tokens to SOL via Raydium CPMM. Permissionless — anyone can call after migration.

[V30] **Ratio-gated selling:** Only sells when price is 20%+ above migration baseline. Sells 15% of held tokens per call, or 100% if balance <= 1M tokens. Uses cooldown (`last_buyback_slot` + `min_buyback_interval_slots`) to prevent rapid sell cycles. Pre-V9 tokens (no baseline) bypass ratio gating.

**Arguments:** `minimum_amount_out: u64` - Slippage protection

**Preconditions:** Token migrated, is Token-2022

**Accounts:** payer, mint, bonding_curve, creator, treasury, treasury_token_account, treasury_wsol, raydium_program, raydium_authority, amm_config, pool_state, token_vault, wsol_vault, wsol_mint, observation_state, token_program, token_2022_program, system_program

**Process:**
1. Validate pool accounts (defense in depth, vaults passed in pool order)
2. Read `treasury_token_account.amount` → `token_amount` (must be > 0)
3. [V30] Check shared cooldown (`last_buyback_slot` + `min_buyback_interval_slots`)
4. [V30] Read pool vault balances, compute ratio vs baseline — skip if price < 120% of baseline
5. [V30] Calculate sell amount: 15% of balance (or 100% if <= 1M tokens)
6. Read WSOL balance before swap (handles pre-existing WSOL)
7. Raydium `swap_base_input` CPI: Token-2022 → WSOL (treasury PDA signs)
8. Read WSOL balance after → `sol_received = after - before`
9. Slippage check: `sol_received >= minimum_amount_out`
10. Close WSOL ATA → treasury PDA (unwrap to SOL)
11. [V34] Creator fee split: `creator_amount = sol_received * 15%`, transferred via lamport manipulation
12. Update state: `treasury.sol_balance += treasury_amount`, `treasury.harvested_fees += treasury_amount`, `treasury.tokens_held -= sell_amount`, `treasury.last_buyback_slot = current_slot`

**SDK bundling:** `buildSwapFeesToSolTransaction` bundles `create_idempotent(treasury_wsol)` + `harvest_fees` + `swap_fees_to_sol` in one atomic transaction. Set `harvest=false` to skip harvest if already done separately.

---

### Migration Instructions

#### `fund_migration_wsol` (V26)
Fund the bonding curve's WSOL ATA with bonding curve SOL. Must be called before `migrate_to_dex` in the same transaction. Isolates direct lamport manipulation from CPIs.

**Preconditions:** Bonding complete, vote finalized, not migrated

**Accounts:** payer (signer), mint, bonding_curve, bc_wsol

**Effects:** Direct lamport transfer from bonding curve PDA to its WSOL ATA. No CPIs — same isolation pattern as `fund_vault_wsol`. SOL stays under bonding curve control at all times.

---

#### `migrate_to_dex` (V5, updated V26)
Migrate graduated token to Raydium CPMM DEX. **Permissionless** — anyone can call once bonding completes.

**Preconditions:**
- Bonding complete (per-token target reached: 100/200 SOL)
- Vote finalized
- Not already migrated
- Treasury has >= 1.5 SOL safety floor (`MIN_MIGRATION_SOL`)

**Process:**
1. [V26] Wrap bonding curve SOL to payer's WSOL (direct lamport debit + sync_native CPI)
2. Handle vote vault based on vote result ([V31] return → treasury lock, or burn)
3. Transfer tokens from vault to payer (temporary)
4. Create Raydium CPMM pool with tokens + WSOL
5. Burn LP tokens to lock liquidity forever
6. [V26] Revoke mint authority to `None` (permanent — supply capped forever)
7. [V26] Revoke freeze authority to `None` (permanent — free trading guaranteed)
8. [V29] Revoke transfer fee config authority to `None` (permanent — fee rate locked forever at the value the token was created with: [V11] 0.07% for new mints)
8. Initialize sell cycle baseline

**[V26] Key changes:** The program handles SOL→WSOL wrapping internally via direct lamport transfer from the bonding curve PDA to the payer's WSOL ATA, followed by `sync_native`. This eliminates the need for a separate `prepare_migration` step and makes migration callable by any wallet. Authority revocation uses Token-2022 `SetAuthority` CPI with `new_authority = None`, which is irreversible.

**[V28] Payer reimbursement:** The payer's lamport balance is snapshotted before and after Raydium CPIs. The exact cost (pool creation + account rent) is reimbursed from the treasury via direct lamport transfer. `treasury.sol_balance` is decremented by the measured cost, not a fixed constant.

**Accounts:** payer, global_config, mint, bonding_curve, treasury, token_vault, treasury_token_account, payer_wsol, payer_token, raydium_program, amm_config, pool_state, lp_mint, payer_lp_token, observation_state, create_pool_fee, ...

---

> **Note (V33):** `execute_auto_buyback` was removed. The auto buyback spent treasury SOL buying during dumps (providing exit liquidity to sellers), competed with lending and epoch rewards for treasury SOL, and had a fee-inflation bug in Raydium vault balance reads. Treasury SOL accumulation now relies solely on the sell cycle (`swap_fees_to_sol`), deployed into lending yield and epoch rewards.

### Rewards Instructions

#### `advance_protocol_epoch` (V11)
Advance the protocol treasury epoch. Permissionless crank.

**Calculates:** distributable_amount = full available balance (reserve floor = 0)

---

#### `claim_protocol_rewards` (V11)
Claim protocol fee rewards.

**Eligibility:** [V32] >= 2 SOL volume in previous epoch. Minimum claim: 0.1 SOL.

**Share:** `(user_volume / total_volume) * distributable_amount`

---

#### `star_token` (V10, updated V3.1.0)
Star a token to show appreciation. Costs 0.02 SOL.

**Auto-Payout:** When token reaches 2000 stars, accumulated star SOL (~40 SOL) is sent to creator.

**Accounts:** user (signer), mint, bonding_curve, token_treasury, creator, star_record, [torch_vault (optional)], [vault_wallet_link (optional)]

**[V3.1.0] Vault-Routed Star:** When vault accounts are provided, the 0.02 SOL star cost is paid from the vault via lamport manipulation (vault → token_treasury). Updates `vault.sol_balance` and `vault.total_spent`.

---

### Reclaim & Revival Instructions

#### `reclaim_failed_token` (V4/V12)
Reclaim SOL from an inactive, unbonded token.

**Preconditions:**
- Bonding NOT complete
- Inactive for >= 7 days (1 epoch)
- Not already reclaimed
- SOL balance >= 0.01 SOL

**Effect:** All SOL transferred to protocol treasury for redistribution

---

#### `contribute_revival` (V12)
Contribute SOL to revive a reclaimed token.

**Arguments:** `sol_amount: u64`

**Revival:** [V31] When cumulative contributions reach the token's initial virtual SOL, token is automatically revived. Uses `initial_virtual_reserves(bonding_target)` directly — single code path, no version branching. Thresholds: Spark: 18.75, Flame: 37.5, Torch: 75 SOL. Legacy tokens: 30 SOL.

---

### Vault Instructions (V3.0.0)

#### `create_vault`
Create a per-user SOL vault and auto-link the creator's wallet.

**Accounts:** creator (signer), vault (init), wallet_link (init), system_program

**Effects:**
- Creates TorchVault PDA with creator as both `creator` and `authority`
- Creates VaultWalletLink PDA for the creator (auto-linked)
- Sets `linked_wallets = 1`

---

#### `deposit_vault`
Deposit SOL into any vault. Permissionless — anyone can deposit.

**Arguments:** `sol_amount: u64`

**Accounts:** depositor (signer), vault, system_program

**Effects:** CPI `system_program::transfer` from depositor to vault PDA. Updates `sol_balance` and `total_deposited`.

---

#### `withdraw_vault`
Withdraw SOL from vault to authority wallet. Authority only.

**Arguments:** `sol_amount: u64`

**Accounts:** authority (signer), vault (has_one = authority), system_program

**Effects:** Lamport manipulation from vault PDA to authority. Updates `sol_balance` and `total_withdrawn`.

---

#### `link_wallet`
Link a wallet to the vault. Authority only. The linked wallet does NOT need to sign.

**Accounts:** authority (signer), vault (has_one = authority), wallet_to_link, wallet_link (init), system_program

**Effects:** Creates VaultWalletLink PDA for the target wallet. Increments `linked_wallets`. Anchor's `init` constraint prevents double-linking.

---

#### `unlink_wallet`
Unlink a wallet from the vault. Authority only. Closes the VaultWalletLink PDA.

**Accounts:** authority (signer), vault (has_one = authority), wallet_to_unlink, wallet_link (close = authority), system_program

**Effects:** Closes VaultWalletLink PDA (rent returns to authority). Decrements `linked_wallets`. Constraint validates `wallet_link.vault == vault.key()`.

---

#### `transfer_authority`
Transfer vault admin control to a new wallet. Does NOT affect existing wallet links.

**Accounts:** authority (signer), vault (has_one = authority), new_authority

**Effects:** Sets `vault.authority = new_authority`. The old authority retains their wallet link (can still use vault for buys).

---

#### `withdraw_tokens` (V3.1.0)
Withdraw tokens from a vault's ATA to any destination. Authority only. Composability escape hatch for external DeFi.

**Arguments:** `amount: u64`

**Accounts:** authority (signer), vault (has_one = authority), mint, vault_token_account, destination_token_account, token_program

**Effects:** Vault PDA signs `transfer_checked` from vault's ATA to destination. No on-chain token balance tracking needed (ATA balances are on-chain). Only the vault authority can call this — linked wallets cannot withdraw tokens.

---

### Vault DEX Swap Instructions (V3.1.1)

#### `fund_vault_wsol`
Fund the vault's WSOL ATA with lamports from the vault PDA. Must be called before `vault_swap` (buy) in the same transaction. Isolates direct lamport manipulation from CPIs to avoid Solana runtime balance errors.

**Arguments:** `amount: u64`

**Accounts:** signer, torch_vault, vault_wallet_link, vault_wsol_account

**Effects:** Direct lamport transfer from vault PDA to WSOL ATA. No CPIs — this is the key design constraint. The Solana runtime checks account balance sums at CPI boundaries; separating lamport manipulation into its own instruction avoids the mismatch.

---

#### `vault_swap`
Vault-routed Raydium CPMM swap for migrated Torch tokens. Handles both directions via `is_buy` flag.

**Arguments:**
- `amount_in: u64` - Input amount (SOL lamports for buy, token base units for sell)
- `minimum_amount_out: u64` - Slippage protection
- `is_buy: bool` - Direction (true = SOL→Token, false = Token→SOL)

**Accounts:** signer, torch_vault, vault_wallet_link, mint, bonding_curve, vault_token_account, vault_wsol_account, raydium_program, raydium_authority, amm_config, pool_state, pool_token_vault_0, pool_token_vault_1, observation_state, wsol_mint, token_program, token_2022_program, associated_token_program, system_program

**Buy flow (SDK builds 3 instructions in one transaction):**
1. `create_idempotent` — WSOL ATA (if needed)
2. `fund_vault_wsol(amount_in)` — lamports from vault to WSOL (no CPIs)
3. `vault_swap(amount_in, min_out, true)` — sync_native CPI → Raydium swap CPI → vault accounting

**Sell flow (SDK builds 2 instructions):**
1. `create_idempotent` — WSOL ATA (if needed)
2. `vault_swap(amount_in, min_out, false)` — Raydium swap CPI → close WSOL (unwraps to SOL) → vault accounting

---

### Lending Instructions (V2.4)

#### `borrow` (V2.4, updated V3.1.0)
Lock tokens as collateral and borrow SOL from the token's treasury.

**Arguments:**
- `collateral_amount: u64` - Tokens to lock (can be 0 to add debt to existing position)
- `sol_to_borrow: u64` - SOL to borrow (can be 0 to just add collateral)

**Preconditions:** Token migrated, not reclaimed, lending enabled, LTV within limit, utilization cap not exceeded, per-user borrow cap not exceeded

**Accounts:** borrower, mint, bonding_curve, treasury, collateral_vault, borrower_token_account, loan_position, pool_state, token_vault_0, token_vault_1, [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**[V3.1.0] Vault-Routed Borrow:** When vault accounts are provided:
1. Collateral: vault PDA signs transfer from `vault_token_account` → `collateral_vault`
2. SOL proceeds: lamport manipulation from `treasury` → `torch_vault` (after token CPI)
3. Updates `vault.sol_balance` and `vault.total_received`

---

#### `repay` (V2.4, updated V3.1.0)
Repay SOL debt. Full repay returns all collateral and closes the position.

**Arguments:** `sol_amount: u64`

**Accounts:** borrower, mint, treasury, collateral_vault, borrower_token_account, loan_position, [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**[V3.1.0] Vault-Routed Repay:** When vault accounts are provided:
1. Collateral return (full repay): treasury PDA signs transfer from `collateral_vault` → `vault_token_account` (CPI first)
2. SOL repayment: lamport manipulation from `torch_vault` → `treasury` (after token CPI)
3. Updates `vault.sol_balance` and `vault.total_spent`

---

#### `liquidate` (V2.4)
Liquidate an underwater position. Permissionless - anyone can call when LTV > 65%.

**Accounts:** liquidator, borrower, mint, bonding_curve, treasury, collateral_vault, liquidator_token_account, loan_position, pool_state, token_vault_0, token_vault_1

---

---

### Short Selling Instructions (V5)

#### `enable_short_selling` (V5, mostly redundant in [V11])
Enable short selling for a specific token. Admin only. Creates ShortConfig PDA and sets Treasury sentinel flags (repurposes deprecated `buyback_percent_bps` and `total_burned_from_buyback` fields).

**[V11] Note:** `create_token` now sets `SHORT_ENABLED_SENTINEL` automatically and zeroes the short-collateral counter, so newly-created tokens never need this instruction. It is retained so the protocol authority can opt in legacy tokens (created before the auto-enable change) without redeploying them.

**Preconditions:** Authority only, token migrated, lending enabled, shorts not already enabled

**Accounts:** authority (signer), global_config, mint, bonding_curve, treasury, short_config (init), system_program

---

#### `open_short` (V5)
Post SOL collateral and borrow tokens from treasury. Creates ShortPosition on first call.

**Arguments:**
- `sol_collateral: u64` - SOL to post as collateral (can be 0 to add debt to existing position)
- `tokens_to_borrow: u64` - Tokens to borrow (can be 0 to just add collateral, min 1,000 tokens)

**Preconditions:** Token migrated, not reclaimed, shorts enabled (sentinel check), LTV within limit, token utilization cap not exceeded, per-user short cap not exceeded

**Accounts:** shorter (signer), mint, bonding_curve, treasury, treasury_token_account, short_config, short_position, shorter_token_account, pool_state, token_vault_0, token_vault_1, [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**Flow:** SOL collateral → Treasury PDA (sol_balance + total_burned_from_buyback). Tokens → shorter (treasury PDA signs transfer_checked from treasury ATA). LTV calculated as `debt_value_in_sol / sol_collateral`.

**Vault-Routed Open Short:** When vault accounts are provided, SOL comes from vault (lamport manipulation), tokens go to vault ATA (treasury PDA signs).

---

#### `close_short` (V5)
Return tokens to close or partially repay a short position. Full close returns SOL collateral.

**Arguments:** `token_amount: u64` - Tokens to return

**Accounts:** shorter (signer), mint, bonding_curve, treasury, treasury_token_account, short_config, short_position, shorter_token_account, [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**Flow:** Tokens returned to treasury ATA (CPI first). Interest paid in tokens before principal. Full close: SOL collateral returned from treasury to shorter (lamport manipulation after CPI). Updates treasury `sol_balance`, `total_burned_from_buyback`, `tokens_held`.

---

#### `liquidate_short` (V5)
Liquidate an underwater short position. Permissionless — anyone can call when LTV exceeds 65%.

**Accounts:** liquidator (signer), borrower, mint, bonding_curve, treasury, treasury_token_account, short_config, short_position, liquidator_token_account, pool_state, token_vault_0, token_vault_1, [torch_vault (optional)], [vault_wallet_link (optional)], [vault_token_account (optional)]

**Flow:** Liquidator sends tokens to treasury ATA (CPI first). Receives SOL from treasury (lamport manipulation after CPI). SOL seized = `debt_value * (1 + 10% bonus)`. Close factor: max 50% per call. Bad debt: if collateral < debt, protocol absorbs loss.

---

> **Note (V2.4.1):** `configure_buyback` and `configure_lending` were removed from the program. Lending uses compile-time defaults (2% interest/epoch, 50% max LTV, 65% liquidation, 10% bonus, [V4.0] 80% utilization cap). Parameters are immutable on-chain.

> **Note (V3.7.0):** Pre-migration `buyback` (on bonding curve) was removed. `update_authority` was removed — authority is set at deployment and cannot be changed on-chain. **[V33]** Post-migration `execute_auto_buyback` (on Raydium DEX) also removed — treasury relies on sell cycle + lending yield.

> **Note (V3.1.0 — Vault Full Custody):** The Torch Vault is a **full-custody** mechanism. Buy, sell, star, borrow, and repay all support optional vault routing. When vault accounts are provided, all value (SOL and tokens) stays within the vault. The agent wallet only needs gas SOL (~0.01 SOL) for transaction fees. Liquidate does not route through the vault (it is permissionless — anyone can call it, and the liquidator receives collateral to their own wallet).

---

## Protocol Constants

### Token Economics
| Constant | Value | Description |
|----------|-------|-------------|
| TOTAL_SUPPLY | 1,000,000,000 (1B) | Total token supply (6 decimals) |
| MAX_WALLET_TOKENS | 20,000,000 (2%) | Maximum per-wallet holding |
| BONDING_TARGET_LAMPORTS | 200 SOL | Default SOL required to complete bonding |
| INITIAL_VIRTUAL_SOL | 30 SOL | Starting virtual SOL reserves (legacy, pre-V25) |
| INITIAL_VIRTUAL_TOKENS_V27 | 756,250,000 (756.25M) | [V27] Starting virtual token reserves |
| CURVE_SUPPLY | 700,000,000 (700M) | [V31] Tokens minted to bonding curve vault (70%) |
| TREASURY_LOCK_TOKENS | 300,000,000 (300M) | [V31] Tokens locked in treasury lock PDA (30%) |
| V27 IVS | 3 * bonding_target / 8 | [V27] Per-tier initial virtual SOL (18.75 / 37.5 / 75 SOL) |

### [V23/V27] Tiered Bonding Curves
Creators choose a graduation target at token creation. [V27] Each tier has per-tier virtual reserves (IVS = 3BT/8, IVT = 756.25M tokens) producing a consistent ~13.44x multiplier across all tiers. [V31] 300M tokens are locked in a treasury lock PDA at creation; 700M go to the bonding curve. Zero tokens burned at migration.

| Tier | Target | IVS | IVT | Multiplier | Constant |
|------|--------|-----|-----|------------|----------|
| **Spark** (legacy) | 50 SOL | 18.75 SOL | 756.25M tokens | ~13.44x | `BONDING_TARGET_SPARK` |
| **Flame** | 100 SOL | 37.5 SOL | 756.25M tokens | ~13.44x | `BONDING_TARGET_FLAME` |
| **Torch** (default) | 200 SOL | 75 SOL | 756.25M tokens | ~13.44x | `BONDING_TARGET_TORCH` |

[V4.0] Only Flame and Torch targets are accepted for new tokens. Existing Spark tokens continue to function — `initial_virtual_reserves()` still handles the Spark case. Invalid values are rejected with `InvalidBondingTarget`. Existing tokens have `bonding_target = 0`, treated as 200 SOL (Torch tier) with legacy virtual reserves (30 SOL / 107.3T tokens).

### Fee Structure
| Fee | Rate | Destination |
|-----|------|-------------|
| Protocol Fee | 0.5% (`PROTOCOL_FEE_BPS = 50`) | [V11] 50% Protocol Treasury + 50% Dev Wallet (`DEV_WALLET_SHARE_BPS = 5000`) |
| Treasury Fee | 0% (`TREASURY_FEE_BPS = 0`) | Treasury is funded by the dynamic split below, not a flat per-buy fee |
| Treasury SOL Split | [V11] 17.5% → 2.5% (`TREASURY_SOL_MAX_BPS / MIN_BPS` = 1750 / 250) | Linear inverse decay over bonding progress; `creator_sol` is carved from this total |
| Transfer Fee (Token-2022) | [V11] 0.07% (7 bps) | Flat fee on all transfers. 85% harvested to treasury, 15% to creator. [V35] Community tokens: 100% to treasury, 0% to creator. Fee config authority revoked at migration (rate permanently locked at the value the token was created with — older mints retain 3/4 bps). |
| Sell Fee | 0% (`SELL_FEE_BPS = 0`) | No sell fee |

### Dynamic Treasury SOL Rate ([V11] rates)
The treasury SOL split uses inverse decay based on bonding progress:
- **At start (0 SOL):** 17.5% of net buy SOL goes to the treasury split
- **At completion (target SOL):** 2.5% of net buy SOL goes to the treasury split
- **Formula (`math::calc_treasury_rate_bps`):** `total_rate = 17.5% − (17.5% − 2.5%) × (real_sol_reserves / bonding_target)`, floored at 2.5%

**[V34] Creator carve-out:** The total rate is split: creator gets 0.2% → 1% (linear growth, `math::calc_creator_rate_bps`), treasury gets `total_rate − creator`. Buyer's portion is unchanged. **[V35] Community tokens (default at creation) skip the creator carve-out — full split goes to treasury.**
- **Creator rate:** `creator = 0.2% + (1% − 0.2%) × (real_sol_reserves / bonding_target)` (0% for community tokens)
- **Treasury rate:** `treasury = total_rate − creator` (= `total_rate` for community tokens)

This creates stronger treasury funding early when tokens need momentum, tapering off as the curve matures. The decay uses the per-token `bonding_target`, so all tiers reach the 2.5% floor at their respective graduation points.

### Token Distribution (V2.2, updated V31)

**At Creation (V31):**
| Destination | Amount | Description |
|-------------|--------|-------------|
| Bonding Curve Vault | 700M (70%) | Available for trading on the bonding curve |
| Treasury Lock PDA | 300M (30%) | Locked in a TreasuryLock PDA. No withdrawal instruction exists — release deferred to future governance |

**Per Buy (during bonding) — [V36]:**
| Destination | Rate | Description |
|-------------|------|-------------|
| Buyer | 100% of tokens_out | Vote vault no longer receives a cut |

**At Migration (full supply breakdown for new V36 tokens):**
| Destination | Typical % | Description |
|-------------|-----------|-------------|
| Treasury Lock | 30% | Locked at creation — not affected by migration |
| Wallets (buyers) | ~55% | 100% of tokens sold during bonding (from 700M curve supply) |
| Raydium Pool | ~15% | Unsold tokens from vault, seeded into DEX pool with matching SOL |

**[V36] No vote at migration:** `vote_finalized` is set to `true` at creation, so the migration gate passes immediately once `bonding_complete` is set. The vote vault token account on the treasury PDA still exists (Anchor account layout) but is never funded for new tokens. Pre-V36 tokens with non-zero `vote_vault_balance` still hit the original branch in `migrate_to_dex` (return → treasury lock, burn → mint burn).

**[V31] Zero-burn migration:** With CURVE_SUPPLY = 700M and IVT = 756.25M, the unsold vault tokens exactly equal the price-matched pool allocation at bonding completion. No excess tokens to burn.

### Timing
| Duration | Value | Description |
|----------|-------|-------------|
| Epoch Duration | 7 days | Reward distribution cycle |
| Inactivity Period | 7 days | Before token can be reclaimed |
| Sell Cycle Interval | ~18 minutes | [V33] Sell cycle cooldown (`last_buyback_slot` field, name kept for layout compat) |

### Star System
| Constant | Value | Description |
|----------|-------|-------------|
| STAR_COST | 0.02 SOL | [V34] Cost per star (sybil protection, was 0.05 SOL) |
| STAR_THRESHOLD | 2,000 | Stars needed for creator payout |
| Creator Payout | ~40 SOL | [V34] Accumulated star SOL at threshold (was ~100 SOL) |

### Protocol Treasury (V11, updated V32)
| Constant | Value | Description |
|----------|-------|-------------|
| Reserve Floor | 0 SOL | [V32] No floor — all fees distributed each epoch |
| Min Volume Eligibility | 2 SOL | [V32] Minimum epoch volume to claim rewards |
| Min Claim Amount | 0.1 SOL | [V32] Minimum payout per claim (rejects dust) |

### Creator Revenue (V34)
| Constant | Value | Description |
|----------|-------|-------------|
| CREATOR_SOL_MIN_BPS | 20 (0.2%) | Creator SOL share at start of bonding (carved from treasury rate) |
| CREATOR_SOL_MAX_BPS | 100 (1%) | Creator SOL share at bonding completion |
| CREATOR_FEE_SHARE_BPS | 1500 (15%) | Creator's share of post-migration `swap_fees_to_sol` proceeds |
| COMMUNITY_TOKEN_SENTINEL | `u64::MAX` | [V35] Stored in `Treasury.total_bought_back` — when set, all creator fees route to treasury instead |
| Creator Rate Formula | `0.2% + (1% - 0.2%) × (reserves / target)` | Linear growth, inverse of treasury decay (0% for community tokens) |

### Revival (V12, updated V31)
| Constant | Value | Description |
|----------|-------|-------------|
| Revival Threshold | IVS per tier | [V31] SOL needed to revive = `initial_virtual_reserves(bonding_target).0`. Spark: 18.75, Flame: 37.5, Torch: 75 SOL. Legacy (bonding_target=0): 30 SOL. Single code path, no version branching. |

### Torch Vault (V3.0.0, updated V3.1.0)
| Constant | Value | Description |
|----------|-------|-------------|
| TORCH_VAULT_SEED | `"torch_vault"` | Vault PDA seed |
| VAULT_WALLET_LINK_SEED | `"vault_wallet"` | Wallet link PDA seed |
| Max linked wallets | 255 (u8) | Per-vault wallet link limit |
| Vault Token ATAs | Derived | `get_associated_token_address(vault_pda, mint, TOKEN_2022)` |

### Treasury Lending (V2.4) & Short Selling (V5) — [V11] depth-aware
| Constant | Value | Description |
|----------|-------|-------------|
| Max LTV (depth-tier) | [V11] 25% / 35% / 45% / 50% | `get_depth_max_ltv_bps(pool_sol)`: <50 SOL → 25%, 50–200 → 35%, 200–500 → 45%, 500+ → 50%. Replaces the flat 50% cap for new positions; `Treasury.max_ltv_bps` (default 50%) is still the stored ceiling. |
| Min pool SOL for new positions | [V11] 5 SOL (`MIN_POOL_SOL_LENDING`) | Below this, `borrow` and `open_short` reject; liquidations are exempt |
| Max price deviation | [V11] 50% (`MAX_PRICE_DEVIATION_BPS = 5000`) | New positions blocked when current pool ratio drifts more than ±50% from migration baseline (`Treasury.baseline_*`). Liquidations exempt. |
| Liquidation Threshold | 65% | LTV that triggers liquidation (shared: longs + shorts) |
| Liquidation Bonus | 10% | Incentive for liquidators (shared: longs + shorts) |
| Liquidation Close Factor | 50% | Max position closed per liquidation call (shared) |
| Interest Rate | 2% per epoch (`DEFAULT_INTEREST_RATE_BPS = 200`) | ~10.4% annualized; SOL for longs, tokens for shorts |
| Utilization Cap | 80% (`DEFAULT_LENDING_UTILIZATION_CAP_BPS = 8000`) | Max fraction of treasury SOL (longs) or treasury tokens (shorts) that can be lent |
| Per-User Borrow Cap | [V11] 23× collateral share (`BORROW_SHARE_MULTIPLIER = 23`) | Longs: `max_lendable * collateral * 23 / TOTAL_SUPPLY`. Shorts: `max_lendable_tokens * sol_collateral * 23 / treasury_sol`. Bumped from 5× to give realistic leverage to small-collateral users now that the depth-tier LTV does the heavy lifting on risk. |
| Min Borrow | 0.1 SOL (`MIN_BORROW_AMOUNT`) | Minimum borrow amount (longs) |
| Min Short | 1,000 tokens (`MIN_SHORT_TOKENS = 1_000_000_000` base units, 6 decimals) | [V5] Minimum short position size |
| RAYDIUM_AMM_CONFIG | hardcoded `Pubkey` | [V11] Validated against `pool_state.amm_config` to lock the fee tier — prevents an attacker from substituting a higher-fee or differently-parameterized pool |
| SHORT_ENABLED_SENTINEL | `u16::MAX` | [V5] Stored in `Treasury.buyback_percent_bps`. [V11] `create_token` sets this on creation, so new tokens are short-enabled by default. The `enable_short_selling` admin instruction remains for legacy tokens. |
| SHORT_CONFIG_SEED | `"short_config"` | [V5] Per-token short aggregate state PDA |
| SHORT_SEED | `"short"` | [V5] Per-user short position PDA |

**[V5] Margin System:** The lending system (V2.4) handles leveraged longs (borrow SOL, post tokens). Short selling (V5) adds the mirror: borrow tokens, post SOL. Same LTV, same liquidation, same interest model, same vault routing. The treasury is the counterparty to both sides. SOL collateral from shorts is held in the Treasury PDA, tracked via repurposed `total_burned_from_buyback` field. The lending handler subtracts reserved short collateral from available SOL before calculating max lendable. No external oracle — Raydium pool reserves provide deterministic on-chain pricing for both directions.

---

## Bonding Curve Formula

The protocol uses a constant product bonding curve:

**Buy:**
```
tokens_out = (virtual_token_reserves * sol_in) / (virtual_sol_reserves + sol_in)
```

**Sell:**
```
sol_out = (virtual_sol_reserves * token_in) / (virtual_token_reserves + token_in)
```

**Price at any point:**
```
price = virtual_sol_reserves / virtual_token_reserves
```

**[V31] Treasury Lock Token Distribution:** Each tier has per-tier virtual reserves tuned for a consistent ~13.44x multiplier:
- IVS = `3 * bonding_target / 8` (Spark: 18.75, Flame: 37.5, Torch: 75 SOL)
- IVT = 756.25M tokens (all tiers)
- CURVE_SUPPLY = 700M (minted to bonding curve vault)
- TREASURY_LOCK_TOKENS = 300M (minted to treasury lock PDA at creation)
- [V36] At bonding completion, ~55% of supply in wallets, ~15% goes to the Raydium pool, 30% stays locked. The vote-vault leg is gone (always 0 for new tokens).
- Zero tokens burned at migration — vault remainder exactly equals price-matched pool allocation

Legacy tokens (bonding_target = 0) use the original 30 SOL / 107.3T virtual reserves.

---

## Token Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           TOKEN LIFECYCLE                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  V3.1.0: All operations (buy, sell, star, borrow, repay) can be routed     │
│  through a TorchVault. Vault is optional — both paths work everywhere.     │
│                                                                              │
│  ┌─────────────┐     ┌─────────────┐                                        │
│  │   CREATE    │────▶│   BONDING   │                                        │
│  │   TOKEN     │     │   PHASE     │                                        │
│  │ (pick tier) │     │ 0-target*   │                                        │
│                      └──────┬──────┘                                        │
│                             │                                               │
│                       ┌─────┴─────┐                                         │
│                       │           │                                         │
│                       ▼           │                                         │
│               ┌─────────────┐    │      ┌─────────────┐                    │
│               │  INACTIVE   │    │      │  MIGRATE    │                    │
│               │  7+ DAYS    │    │      │  TO DEX     │                    │
│               └──────┬──────┘    │      │ (anyone)*   │                    │
│                      │           │      └──────┬──────┘                    │
│                      ▼           │             │                           │
│               ┌─────────────┐    │             ▼                           │
│               │  RECLAIM    │    │      ┌─────────────┐                    │
│               │  (to plat-  │    │      │   TRADING   │                    │
│               │   form)     │    │      │   ON DEX    │                    │
│               └──────┬──────┘    │      └──────┬──────┘                    │
│                      │           │             │                           │
│                      ▼           │             ▼                           │
│               ┌─────────────┐    │      ┌────────────────────────┐         │
│               │  REVIVAL    │    │      │  TREASURY RECYCLING    │         │
│               │ (per-tier   │────┘      │  harvest_fees →        │         │
│               │  threshold) │           │  swap_fees_to_sol      │         │
│               └─────────────┘           │  (sell ≥120% baseline, │         │
│                                         │  15% per call)         │         │
│                                         └────────────────────────┘         │
│                                                                              │
│  * [V23] Target = bonding_target (Flame 100, Torch 200 SOL; Spark 50 legacy)│
│  * [V27] ~13.44x multiplier across all tiers (IVS=3BT/8, IVT=756.25M)       │
│  * [V31] 300M tokens locked in TreasuryLock PDA at creation, 0 burn         │
│  * [V36] No vote step — `vote_finalized=true` at creation, vault stays empty│
│  * [V26] migrate_to_dex is permissionless — anyone can call it              │
│  * [V33] Auto-buyback removed; only the sell-cycle leg remains              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Fee Flow Diagram (v11)

```
                              BUYER'S SOL
                                  │
                                  ▼
               ┌──────────────────────────────────────┐
               │           TOTAL SOL INPUT            │
               └──────────────────────────────────────┘
                                  │
                ┌─────────────────┴─────────────────┐
                │                                   │
                ▼                                   ▼
        ┌────────────────┐                  ┌────────────────┐
        │ 0.5% Protocol  │                  │     99.5%      │
        │      Fee       │                  │   Remaining    │
        └───────┬────────┘                  └───────┬────────┘
                │                                   │
         ┌──────┴──────┐                ┌───────────┼───────────┐
         │             │                │           │           │
         ▼             ▼                ▼           ▼           ▼
   ┌──────────┐  ┌──────────┐     ┌─────────┐ ┌──────────┐ ┌──────────┐
   │ Protocol │  │   Dev    │     │ Token   │ │ Creator  │ │ Bonding  │
   │ Treasury │  │  Wallet  │     │Treasury │ │  Wallet  │ │  Curve   │
   │  (50%)   │  │  (50%)   │     │(17.3→1.5│ │(0.2→1%)* │ │(82.5%→97%│
   └──────────┘  └──────────┘     │  %)*    │ │          │ │   )*     │
                                  └────┬────┘ └──────────┘ └─────┬────┘
                                       │                          │
                                       │  *Treasury split 3 ways: ▼
                                       │   Total 17.5→2.5%  ┌─────────────┐
                                       │   Creator 0.2→1%   │   TOKENS    │
                                       │   Treasury = total │    OUT      │
                                       │       − creator    └──────┬──────┘
                                       │                            │
                                       ▼                            ▼
                                ┌──────────────┐             ┌───────────┐
                                │ Sell cycle / │             │   BUYER   │
                                │ lending yield│             │  (100%,   │
                                │ epoch rewards│             │   V36)    │
                                └──────────────┘             └───────────┘
```

[V36] No vote-vault leg: 100% of `tokens_out` goes to the buyer. The community-treasury / vote-result branches that previously sat under the buyer column are gone for new tokens.

---

## Security Considerations

### Access Control
- **Protocol authority-only:** initialize, update_dev_wallet, enable_short_selling
- **Vault authority-only:** withdraw_vault, link_wallet, unlink_wallet, transfer_authority, withdraw_tokens
- **Permissionless cranks:** advance_protocol_epoch, harvest_fees, swap_fees_to_sol, fund_migration_wsol, migrate_to_dex, reclaim_failed_token, liquidate, liquidate_short
- **Permissionless deposits:** deposit_vault (anyone can deposit into any vault)

### Economic Security
- **2% wallet cap:** Prevents whale concentration
- **Slippage protection:** All trades require min output
- **[V32] No reserve floor:** Protocol treasury distributes all fees each epoch (min claim 0.1 SOL prevents dust drain)
- **Lending cap:** 80% utilization cap — 20% visible reserve in per-token treasury (applies to both SOL lending and token lending for shorts)
- **[V11] Per-user borrow cap:** Max borrow = 23× collateral's share of the lendable pool (e.g. 1% of supply as collateral → ~23% of lendable SOL). Prevents single-whale pool monopolization while keeping the cap useful at small collateral sizes
- **[V11] Depth-tier LTV:** Max LTV scales with pool SOL depth (25% / 35% / 45% / 50%); thin pools force smaller positions
- **[V11] Price-deviation gate:** `borrow` and `open_short` reject when the current pool ratio has moved more than ±50% from the migration baseline; liquidations are exempt so underwater positions can still be unwound
- **[V11] Min pool liquidity:** New margin positions require ≥5 SOL in the Raydium pool; below that the oracle is too thin to trust
- **[V5] Short collateral isolation:** Short sellers' SOL collateral is tracked via repurposed Treasury field (`total_burned_from_buyback`). The lending handler subtracts this reserved amount before calculating max lendable SOL, preventing short collateral from being lent to long positions
- **Shorting enablement:** [V11] new tokens auto-enable shorting at creation (`SHORT_ENABLED_SENTINEL` written by `create_token`). The `enable_short_selling` admin instruction is retained so legacy tokens can be opted in.
- **Cooldowns:** Minimum interval between sell cycles (~18 min)

### Vault Security (V3.1.0 — Full Custody)
- **One vault per creator:** PDA seed `["torch_vault", creator.key()]` enforces uniqueness
- **One link per wallet:** PDA seed `["vault_wallet", wallet.key()]` prevents wallet belonging to multiple vaults
- **Authority separation:** `creator` (immutable seed) vs `authority` (transferable admin). Linked wallets can use the vault for buys/sells/stars/borrow/repay, but only authority can withdraw SOL or tokens
- **Vault-routed operations:** Buy, sell, star, borrow, repay, open_short, close_short, and liquidate_short all support optional vault routing. All value stays in the vault — the agent wallet never holds tokens or significant SOL
- **Handler-level authorization:** Every vault-routed instruction validates that `vault_wallet_link` is provided when `torch_vault` is present. This prevents unauthorized vault usage via independently optional Anchor accounts (V3.0.0 audit C-1 fix)
- **CPI ordering:** All token `transfer_checked` CPIs execute before direct lamport manipulation to avoid Solana runtime stale balance issues. This is enforced in sell (vault_ata → token_vault before SOL routing), borrow (vault_ata → collateral_vault before SOL routing), and repay (collateral_vault → vault_ata before SOL routing)
- **Closed economic loop:** Buy spends vault SOL, receives tokens into vault ATA. Sell sends vault tokens, receives SOL into vault. Borrow locks vault tokens, receives SOL into vault. Repay spends vault SOL, receives tokens into vault. Open short spends vault SOL (collateral), receives borrowed tokens into vault. Close short returns vault tokens, receives SOL collateral into vault. All value circulates inside the vault
- **Withdraw escape hatch:** Authority can withdraw SOL (`withdraw_vault`) or tokens (`withdraw_tokens`) to use in external DeFi. Linked wallets cannot withdraw
- **Compromised key safety:** If an agent wallet is compromised, the attacker can sign transactions but all value routes back to the vault. Authority unlinks the key and links a new one. Zero funds at risk

### Sybil Resistance
- **Star cost:** 0.02 SOL per star prevents fake appreciation
- **Volume tracking:** Integrated into buy/sell to prevent inflation

### Token-2022 Security
- **Transfer fee config authority:** Bonding curve PDA pre-migration → `None` post-migration (revoked permanently — fee rate locked at the value the token was created with: [V11] 0.07% for new mints, 0.04% for V34-era mints, 0.03% for V31-era mints)
- **Withdraw authority:** Token treasury PDA (can harvest accumulated fees)
- **[V26] Mint authority:** Bonding curve PDA pre-migration → `None` post-migration (revoked permanently at migration)
- **[V26] Freeze authority:** Bonding curve PDA pre-migration → `None` post-migration (revoked permanently at migration)
- **[V29] Metadata pointer authority:** `None` (permanently immutable — pointer always references the mint itself)
- **[V29] Token metadata:** Stored on-chain via Token-2022 MetadataPointer + TokenMetadata extensions. No Metaplex dependency. The `add_metadata` instruction (Metaplex backfill for legacy tokens) was temporary and has been removed
- **[V3.2.1] harvest_fees hardened:** `treasury_token_account` constrained to treasury's exact ATA via Anchor's `associated_token` constraints. Prevents fee theft via substituted destination accounts.

### Raydium Pool Validation (V27 PDA-based, [V11] AMM config pin)
The `borrow`, `liquidate`, `open_short`, `liquidate_short`, `swap_fees_to_sol`, and `vault_swap` instructions use Raydium pool reserves as a price oracle. Pool accounts are validated via **PDA derivation constraints** in Anchor contexts, with `validate_pool_accounts()` running as defense-in-depth:

- **Pool state:** `address = derive_pool_state(&mint.key())` — deterministic PDA from `[b"pool", RAYDIUM_AMM_CONFIG, token0, token1]`
- **Pool vaults:** `address = derive_pool_vault(&pool_state.key(), &vault_mint)` — PDA from `[b"pool_vault", pool_state, vault_mint]`
- **Observation state:** `address = derive_observation_state(&pool_state.key())` — PDA from `[b"observation", pool_state]`
- **AMM config pin:** `RAYDIUM_AMM_CONFIG` is a hardcoded `Pubkey`. [V11] `validate_pool_accounts()` reads `pool_state.amm_config` (offset 8) and rejects the call unless it matches — closing the substitution path where an attacker could find a different AMM config that happens to derive to a valid-looking pool PDA.

Callers must pass vaults in pool order (vault_0, vault_1 by mint pubkey ordering), not by swap direction. PDA constraints + the AMM-config pin together make oracle manipulation infeasible: there is exactly one canonical pool per token, and the program will only read prices from it.

---

## Security Audit History

### V3.2.1 — `harvest_fees` Unconstrained Destination (CRITICAL, Fixed)
**Date:** February 15, 2026 | **Severity:** Critical | **Status:** Fixed and deployed

**Finding:** The `harvest_fees` instruction did not validate that `treasury_token_account` matched the treasury PDA's ATA. An attacker could substitute their own Token-2022 ATA, call `harvest_fees`, and receive all accumulated transfer fees (1% of all transfer volume, platform-wide). Confirmed via mainnet transaction simulation.

**Root cause:** Token-2022's `WithdrawWithheldTokensFromMint` CPI accepts any valid token account for the correct mint as destination. The program signed the CPI with treasury PDA seeds (valid authority) but did not constrain the destination account.

**Fix:** Added Anchor `associated_token` constraints to `treasury_token_account` in the `HarvestFees` context:
```rust
#[account(
    mut,
    associated_token::mint = mint,
    associated_token::authority = token_treasury,
    associated_token::token_program = token_2022_program,
)]
pub treasury_token_account: Box<InterfaceAccount<'info, TokenAccountInterface>>,
```
Also upgraded `mint` to `InterfaceAccount<MintInterface>` and `token_2022_program` to `Interface<TokenInterface>` for full Anchor type safety.

### V3.2.1 — Oracle Manipulation via Unconstrained Raydium Pool (Reported, Non-Issue)
**Date:** February 15, 2026 | **Severity:** Reported as Critical | **Status:** Non-issue (validation already exists)

**Finding reported:** `borrow`, `liquidate`, and `execute_auto_buyback` pool accounts were reported as unconstrained, allowing fake Raydium pools to manipulate price oracles.

**Assessment:** The report was incorrect. `validate_pool_accounts()` in `pool_validation.rs` already validates pool ownership, vault addresses, and mint composition. No code changes required.

### Auditor Sign-Off
Both findings were reviewed by an independent human security auditor. The `harvest_fees` fix was verified as correct, and the Raydium pool validation was confirmed as sufficient. The auditor gave a green flag on both the on-chain program and the frontend.

---

## Events

### RevivalContribution (V12)
Emitted when someone contributes to a token revival.

```rust
RevivalContribution {
    mint: Pubkey,
    contributor: Pubkey,
    amount: u64,
    total_contributed: u64,
    threshold: u64,
    revived: bool,
}
```

### TokenRevived (V12)
Emitted when a token is successfully revived.

```rust
TokenRevived {
    mint: Pubkey,
    total_contributed: u64,
    revival_slot: u64,
}
```

### ShortSellingEnabled (V5)
Emitted when admin enables short selling for a token.

### ShortOpened (V5)
Emitted when a short position is opened or added to. Includes `sol_collateral`, `tokens_borrowed`, `ltv_bps`.

### ShortClosed (V5)
Emitted when a short position is partially or fully closed. Includes `tokens_returned`, `interest_paid_tokens`, `sol_returned`, `fully_closed`.

### ShortLiquidated (V5)
Emitted when a short position is liquidated. Includes `tokens_covered`, `sol_seized`, `bad_debt_tokens`.

---

## Appendix: Market Dynamics Observed in Simulations

The following emergent market dynamics were observed during E2E testing on Surfpool (mainnet fork):

### Self-Reinforcing Fee Market

The protocol creates a self-sustaining fee market where:

1. **Trading activity generates fees** → Protocol treasury accumulates SOL
2. **Epoch advances** → All fees distributed to active traders (no floor)
3. **Traders receive rewards** → Incentivized to trade more
4. **More trading** → More fees → cycle continues

In simulation: 1,270 SOL volume generated 12.7 SOL in protocol fees (1% verified).

### Treasury Accumulation Loop (V30, simplified V33, [V11] rates)

The treasury accumulates SOL via a unidirectional fee-to-SOL pipeline, deployed into lending yield and epoch rewards:

1. **Transfer fees accumulate** → [V11] 0.07% on every transfer, held in recipient accounts
2. **`harvest_fees` collects tokens** → Permissionless crank harvests fee tokens to treasury ATA
3. **`swap_fees_to_sol` sells tokens** → Ratio-gated (120%+ of baseline), sells on Raydium → [V34] 85% SOL to treasury, 15% to creator ([V35] community tokens: 100% treasury)
4. **SOL deployed** → Lending yield (2%/epoch interest from borrowers, 80% utilization cap) + epoch rewards

The sell cycle uses ~18 min cooldown and 15% partial sell amounts to prevent large single-transaction market impact. Permissionless crank.

In simulation: 103.375 SOL accumulated in token treasury post-migration via fee harvesting and sell cycle.

### Community Governance Alignment

The vote-on-buy mechanism aligns incentives:

1. **First buy requires vote** → Every participant has skin in the game
2. **Vote recorded at purchase time** → Prevents vote manipulation after the fact
3. **Final buyer's vote counts** → No last-minute vote changes possible
4. **Instant finalization** → Vote result determined when bonding completes

In simulation: 24 RETURN vs 17 BURN votes - community chose to return tokens to circulation.

### Creator Incentive Structure

The star system creates sustainable creator incentives:

1. **Stars cost 0.02 SOL** → Sybil-resistant appreciation (reduced from 0.05 in V34)
2. **2000 stars = ~40 SOL payout** → Meaningful creator reward
3. **[V34] Bonding SOL share** → 0.2%→1% of buy SOL carved from treasury rate (creator tokens only)
4. **[V34] Post-migration fee share** → 15% of `swap_fees_to_sol` proceeds (creator tokens only)
5. **[V35] Community tokens** → 0% creator fees (default). All SOL stays in treasury. Opt-in via `community_token: false` at creation.
6. **Post-payout stars still tracked** → Ongoing social signal
7. **No self-starring** → Prevents gaming

In simulation: Creator received 100 SOL at exactly 2000 stars (at old 0.05 SOL rate).

### Oracle-Free Margin Trading (V5)

The protocol is a fully on-chain derivatives engine with no external oracle dependency:

1. **Raydium pool IS the oracle** — constant-product AMM pricing, deterministic, manipulation-resistant (quadratic slippage)
2. **Treasury IS the counterparty** — lends SOL to longs, lends tokens to shorts, collects interest from both sides
3. **Same math, opposite direction** — long leverage and short selling share identical LTV, liquidation, and interest mechanics
4. **Self-funding** — fees from trading fund the treasury, which funds the margin system. Interest from both longs and shorts flows back to treasury
5. **All positions backed by real assets** — no synthetic exposure, no virtual balances, no promises that exceed reserves

```
LONG:  Post tokens → borrow SOL → profit when price rises → liquidate when price drops
SHORT: Post SOL → borrow tokens → profit when price drops → liquidate when price rises
```

The utilization cap (80%) ensures the treasury always retains reserves. [V11] On top of utilization, every new position has to clear three additional gates: pool SOL ≥ 5 SOL (`MIN_POOL_SOL_LENDING`), current pool ratio within ±50% of migration baseline (`MAX_PRICE_DEVIATION_BPS`), and depth-tier max LTV (25/35/45/50% by pool SOL). Short collateral is ringfenced from long lending via the repurposed `total_burned_from_buyback` field. Liquidation cascades on one side return assets to treasury, increasing availability for the other side.

### Failed Token Recycling

The reclaim/revival system efficiently recycles capital:

1. **Inactive tokens (7+ days)** → SOL reclaimed to protocol treasury
2. **Protocol treasury** → Redistributed as epoch rewards (no floor, min claim 0.1 SOL)
3. **Revival option** → Community can resurrect promising tokens
4. **Per-tier revival threshold** → Same as initial virtual SOL for token version

This creates a "second chance" mechanism where capital isn't permanently locked in abandoned tokens.

---

## Version History

| Version | Features |
|---------|----------|
| V1 | Basic bonding curve, buy/sell |
| V2 | Treasury, buybacks, permanent burn split |
| V3 | Token-2022 with transfer fees |
| V4 | Failed token reclaim, platform rewards |
| V5 | Raydium DEX migration |
| V6 | Migration timelock (removed) |
| V7 | Creator star system (simplified in V10) |
| V8 | Dev wallet split (10% of protocol fee, updated V32 from 25%) |
| V9 | Auto-buyback on DEX price dips (removed in V33) |
| V10 | Simplified star system with auto-payout |
| V11 | Protocol treasury with epoch rewards (reserve floor removed in V32) |
| V12 | Token revival (30 SOL threshold) |
| V13 | Treasury unification: burn_vault → treasury_token_account, 200 SOL bonding target |
| V2.1 | Fee structure update: 20% SOL + 20% tokens to community treasury (symmetrical), 80% to buyer |
| V2.2 | Reduced token burn rate: 10% to community treasury, 90% to buyer |
| V2.3 | Dynamic treasury SOL rate: inverse decay from 20% to 5% as bonding progresses |
| V2.4 | Treasury lending: borrow SOL against token collateral (post-migration) |
| V2.4.1 | Lean program: removed configure_buyback, configure_lending, migrate_treasury. All params use compile-time defaults. BondingCurve accounts now 410 bytes (down from 482). Discriminator-based account filtering. |
| V3.0.0 | **Torch Vault — Multi-Wallet Identity.** Per-creator SOL escrow (TorchVault PDA) with multi-wallet support (VaultWalletLink reverse pointers). 6 new instructions: create_vault, deposit_vault, withdraw_vault, link_wallet, unlink_wallet, transfer_authority. Buy instruction accepts optional vault accounts for vault-funded buys. Authority separation (creator vs authority). Pre-fund lamport pattern for Solana system transfer compatibility. |
| V3.1.0 | **Vault Full Custody.** The vault becomes the fund holder; the agent wallet becomes a disposable controller. Buy, sell, star, borrow, and repay all support optional vault routing — tokens go to vault ATAs, SOL returns to vault. New `withdraw_tokens` instruction (authority-only escape hatch). New `total_received` field on TorchVault (122 bytes). Handler-level authorization guards on all vault-routed instructions (V3.0.0 audit C-1 fix). CPI ordering enforced: token CPIs before lamport manipulation in all vault paths. |
| V3.1.1 | **Vault DEX Swap.** Two new instructions: `fund_vault_wsol` (lamport-only, no CPIs) and `vault_swap` (Raydium CPMM swap via vault PDA). Buy and sell migrated tokens on Raydium DEX while preserving full vault custody. Lamport manipulation isolated into separate instruction to avoid Solana runtime "sum of account balances" error at CPI boundaries. WSOL ATA persistent on buy (no close), closed on sell (to unwrap proceeds). |
| V3.2.0 | **Merge Platform Treasury into Protocol Treasury.** Removed `PlatformTreasury` struct and 3 instructions (`initialize_platform_treasury`, `advance_epoch`, `claim_epoch_rewards`). Protocol treasury is now the single reward treasury — funded by trading fees AND reclaimed token SOL. Single epoch cycle via `advance_protocol_epoch`, single claim flow via `claim_protocol_rewards`. Reclaim SOL routes to protocol treasury. Buy/Sell contexts no longer accept `platform_treasury` account. ~300 lines removed, zero added. |
| V3.2.1 | **Security: `harvest_fees` hardened.** Fixed critical vulnerability where `treasury_token_account` was unconstrained — an attacker could substitute their own ATA and steal all accumulated transfer fees. Added Anchor `associated_token` constraints (`mint`, `authority`, `token_program`). Upgraded account types to `InterfaceAccount` for full type safety. Independent auditor verified the fix and confirmed Raydium pool validation (reported separately) was already sufficient. |
| V3.3.0 | **Tiered Bonding Curves (V23).** Creators choose a graduation target at token creation: Spark (50 SOL, ~7x), Flame (100 SOL, ~19x), or Torch (200 SOL, ~59x, default). Same constant-product formula, same virtual reserves, different graduation points. New `bonding_target: u64` field appended to `BondingCurve` state (+8 bytes). New `sol_target: u64` in `CreateTokenArgs`. Existing tokens zero-initialize to 0, treated as 200 SOL. Treasury SOL decay rate scales by per-token target. No new instructions, no new accounts, no migration needed. |
| V3.4.x | **Verification & Migration Fixes.** Price-matched migration ensuring pool pricing matches curve at graduation. Post-migration verification proofs. Devnet support. |
| V3.5.0 | **V25 Pump-Style Token Distribution.** Per-tier virtual reserves (IVS = bonding_target/8, IVT = 900M tokens) producing a consistent ~81x multiplier across all tiers. Flat 20%→5% treasury SOL rate across all tiers (reverted from V24 per-tier rates). Per-tier revival threshold (IVS instead of fixed 30 SOL). Excess unsold tokens burned at migration. 35 kani formal verification proofs all passing. |
| V3.6.0 | **V26 Permissionless Migration + Authority Revocation.** `migrate_to_dex` is now fully permissionless — the program wraps bonding curve SOL to WSOL internally via direct lamport transfer + `sync_native` CPI, eliminating the authority-only `prepare_migration` step. After LP burn, mint authority and freeze authority are revoked to `None` (permanent, irreversible) — supply is capped forever and no accounts can be frozen. Removed `prepare_migration` instruction and `PrepareMigration` context. New `fund_migration_wsol` instruction isolates lamport manipulation. 33 kani proofs (replaced 3 prepare_migration proofs with 1 SOL wrapping conservation proof). |
| V3.7.0 | **V27 Treasury Lock + PDA Pool Validation.** 250M tokens (25%) locked in TreasuryLock PDA at creation; 750M (75%) for bonding curve (updated to 300M/700M in V31). IVS = 3BT/8, IVT = 756.25M tokens — 13.44x multiplier. PDA-based Raydium pool validation replaces runtime validation in `Borrow`, `Liquidate`, `TreasuryBuybackDex`, and `VaultSwap` contexts (`derive_pool_state`, `derive_pool_vault`, `derive_observation_state`). Revival handler checks TreasuryLock existence (`data_len > 0`) for V27 vs legacy thresholds. V28 `update_authority` instruction added then removed. Removed pre-migration `buyback` handler + `Buyback` context (~216 lines). Fixed `treasury.sol_balance` decrement in `borrow` handler. New `TreasuryLock` account type (12 total). Minimal admin surface: only `initialize` and `update_dev_wallet` require authority. |
| V3.7.1 | **V28 Migration Payer Reimbursement.** Treasury reimburses the migration payer for exact Raydium costs (pool creation + account rent) via direct lamport transfer, measured by snapshotting payer balance before/after CPIs. Replaced fixed `RAYDIUM_POOL_CREATION_FEE` (0.15 SOL) with `MIN_MIGRATION_SOL` (1.5 SOL) safety floor constraint. `treasury.sol_balance` decremented by actual measured cost. 36 kani proofs all passing (no new proofs needed — change is operational, not arithmetic). |
| V3.7.2 | **V20 Harvest Fees + Swap to SOL.** New `swap_fees_to_sol` instruction sells harvested Token-2022 transfer fee tokens back to SOL via Raydium CPMM. Treasury PDA signs the swap, closes WSOL ATA to unwrap proceeds, increments `treasury.sol_balance` and `treasury.harvested_fees` (repurposed from unused field). SDK bundles `create_idempotent(treasury_wsol)` + `harvest_fees` + `swap_fees_to_sol` in one atomic transaction via `buildSwapFeesToSolTransaction`. Fixed `validate_pool_accounts` vault ordering bug in both `swap_fees_to_sol` and `execute_auto_buyback` — vaults now passed in pool order (by mint pubkey) instead of swap direction, preventing false validation failures for tokens where `mint < WSOL` (~2.6% of tokens). 28 instructions total. No new state fields, no migration needed. |
| V3.7.3 | **V29 On-Chain Token Metadata + Simplified Transfer Fee.** Transfer fee reduced from 1% to 0.1% (10 bps) flat on all transfers — simpler model, same treasury funding via harvest + swap. Transfer fee config authority revoked to `None` at migration (fee rate permanently locked alongside mint/freeze authority). New tokens have metadata (name, symbol, uri) stored directly on the mint via Token-2022 MetadataPointer + TokenMetadata extensions — no Metaplex dependency. Mint allocation uses a two-phase approach: initial space for TransferFeeConfig + MetadataPointer, then Token-2022 reallocs internally when TokenMetadata is initialized after `InitializeMint2`. `add_metadata` (Metaplex backfill for legacy pre-V29 tokens) was temporary — 13/24 legacy tokens succeeded, remaining 11 have old account layouts. All Metaplex code removed (constant, instruction builder, handler, context, error variant). SDK test coverage: `getTokenMetadata()` verification after token creation. 28 instructions total. |
| V3.7.4 | **V30 Simplified Auto Buyback + Ratio-Gated Sell.** Buyback no longer burns tokens — bought-back tokens are held in treasury ATA (`tokens_held` tracks accumulation). `swap_fees_to_sol` is now ratio-gated: only sells when price is 20%+ above migration baseline (inverse of buyback's 20% dip trigger). Sells 15% of held tokens per call (or 100% if <= 1M tokens). Shared cooldown with buyback via `last_buyback_slot` — prevents rapid buy/sell cycles. Pre-V9 tokens (no baseline) bypass ratio gating. Removed `SUPPLY_FLOOR` constant (500M) — no supply floor, no burn. New constants: `DEFAULT_SELL_THRESHOLD_BPS` (12000 = 120%), `DEFAULT_SELL_PERCENT_BPS` (1500 = 15%), `SELL_ALL_TOKEN_THRESHOLD` (1M tokens). Treasury self-sustains: buy low on dips, hold, sell high on pumps, refill from transfer fee income. No new accounts, no new instructions, no IDL changes, no state struct changes. 37 kani proofs all passing (new: `verify_sell_threshold_fits_u64`). |
| V3.7.5 | **V31 Zero-Burn Migration + Transfer Fee Reduction.** CURVE_SUPPLY reduced from 750M to 700M, TREASURY_LOCK_TOKENS increased from 250M to 300M — vault remainder exactly equals price-matched pool allocation at bonding completion, eliminating migration burn entirely. Post-migration supply stays at 1B (vote=return) or ~945M (vote=burn). Transfer fee reduced from 0.1% to 0.03% (TRANSFER_FEE_BPS 10→3) for lower friction on transfers. Vote RETURN now sends tokens to treasury lock PDA (community reserve) instead of Raydium LP — preserved for future governance release. Revival handler simplified to single code path using `initial_virtual_reserves()` directly — removed legacy V25 branching and TreasuryLock existence checks. Removed `initial_virtual_reserves_v25()` and `INITIAL_VIRTUAL_TOKENS_V25`. All three tiers (Spark/Flame/Torch) preserved — Spark at 50 SOL gives pump.fun-style ~$3,700 starting MC. `treasury_lock` and `treasury_lock_token_account` added to `MigrateToDex` context. Kani proofs updated: `excess_burned == 0` assertion replaces `< CURVE_SUPPLY / 5`. No new accounts, no new instructions, no state struct changes. |
| V3.7.6 | **V32 Protocol Treasury Rebalance.** Reserve floor removed (PROTOCOL_TREASURY_RESERVE_FLOOR 1,500 SOL → 0) — all accumulated fees distributed each epoch. Volume eligibility lowered from 10 SOL to 2 SOL (MIN_EPOCH_VOLUME_ELIGIBILITY). New MIN_CLAIM_AMOUNT constant (0.1 SOL) prevents dust claims — `claim_protocol_rewards` rejects payouts below threshold. Protocol fee split changed from 75/25 to 90/10 (DEV_WALLET_SHARE_BPS 2500 → 1000) — more fees flow to community. New `ClaimBelowMinimum` error variant. New kani proof: `verify_min_claim_enforcement`. No new accounts, no new instructions, no state struct changes. 39 kani proofs all passing. |
| V3.7.7 | **V33 Remove Auto Buyback + Extend Lending.** Removed `execute_auto_buyback` instruction, `TreasuryBuybackDex` context (~100 lines), and `execute_auto_buyback_handler` (~230 lines). The buyback spent treasury SOL buying during dumps (exit liquidity to sellers), competed with lending for treasury SOL, and had a fee-inflation bug in Raydium vault balance reads. Removed 4 buyback-only constants (`DEFAULT_RATIO_THRESHOLD_BPS`, `DEFAULT_RESERVE_RATIO_BPS`, `DEFAULT_BUYBACK_PERCENT_BPS`, `MIN_BUYBACK_AMOUNT`). Treasury now simplified to: fee harvest → sell high → SOL → lending yield + epoch rewards. Lending utilization cap increased from 50% to 70% (`DEFAULT_LENDING_UTILIZATION_CAP_BPS` 5000 → 7000) — 30% visible reserve for confidence, more SOL available for community borrowing. Buyback config fields zeroed at token creation (struct layout unchanged for backward compat). Binary size ~804 KB (down from ~850 KB, ~6% reduction). 27 instructions total. 39 kani proofs all passing. |
| V3.7.8 | **V34 Creator Revenue + Star Cost Reduction + Transfer Fee Bump.** Three creator income streams: (1) bonding SOL share 0.2%→1% carved from treasury rate during bonding, (2) 15% of post-migration `swap_fees_to_sol` proceeds, (3) star payout at 2000 stars. Star cost reduced from 0.05 to 0.02 SOL (`STAR_COST_LAMPORTS` 50M→20M). Transfer fee increased from 3 to 4 bps (`TRANSFER_FEE_BPS` 3→4, new tokens only — old tokens immutable at 3 bps). New constants: `CREATOR_FEE_SHARE_BPS` (1500 = 15%), `CREATOR_SOL_MIN_BPS` (20 = 0.2%), `CREATOR_SOL_MAX_BPS` (100 = 1%). `creator` account added to `Buy` and `SwapFeesToSol` contexts (validated against `bonding_curve.creator`). No new instructions, no state struct changes, no new PDAs. 27 instructions total. 43 kani proofs all passing. |
| V3.7.9 | **Per-User Borrow Cap.** New `BORROW_SHARE_MULTIPLIER = 3` limits each borrower to 3x their collateral's proportional share of the lendable pool (e.g., 1% of supply as collateral → max 3% of lendable SOL). Prevents single-whale pool monopolization. New `UserBorrowCapExceeded` error. New Kani proof `verify_per_user_borrow_cap_bounded` verifies no overflow, upper bound, and boundary correctness at all three tier lendable caps. No new instructions, no state struct changes, no new PDAs. 27 instructions total. 44 kani proofs all passing. |
| V3.7.10 | **V35 Community Token Option.** New `community_token: bool` in `CreateTokenArgs` (default `true`). Community tokens route 0% to creator — all bonding SOL share and post-migration `swap_fees_to_sol` proceeds go entirely to treasury. Creator tokens (opt-in `community_token: false`) retain V34 behavior: 0.2%→1% bonding SOL share + 15% fee swap share. Implementation uses sentinel value (`u64::MAX`) in deprecated `Treasury.total_bought_back` field — no struct layout changes, full backward compatibility (old tokens have `total_bought_back` at 0 or historical values, always treated as creator tokens). New constant `COMMUNITY_TOKEN_SENTINEL`. Stars system unchanged (user-funded appreciation, not protocol fees). No new instructions, no new accounts, no new PDAs. 27 instructions total. 48 kani proofs all passing (new: `verify_community_token_buy_conservation`, `verify_community_token_swap_fees_conservation`). |
| V4.0.0 | **V4.0 Simplified Tiers & Reduced Fees.** Removed 50 SOL (Spark) tier from `VALID_BONDING_TARGETS` — existing Spark tokens unaffected, only new creation blocked. Treasury SOL rate reduced from 20%→5% to 12.5%→4% (`TREASURY_SOL_MAX_BPS` 2000→1250, `TREASURY_SOL_MIN_BPS` 500→400). Protocol fee reduced from 1% to 0.5% (`PROTOCOL_FEE_BPS` 100→50). Per-user borrow cap increased from 3x to 5x (`BORROW_SHARE_MULTIPLIER` 3→5). Lending utilization cap increased from 70% to 80% (`DEFAULT_LENDING_UTILIZATION_CAP_BPS` 7000→8000). Constants-only change — no new instructions, no state migration, no new accounts. 27 instructions total. 48 Kani proofs all passing. |
| V5.0.0 | **V5 Oracle-Free Margin Trading (Short Selling).** Completes the two-sided margin system by adding short positions — the mirror of existing long leverage. 4 new instructions: `enable_short_selling` (admin), `open_short`, `close_short`, `liquidate_short`. 2 new account types: `ShortPosition` (per-user, per-token) and `ShortConfig` (per-token stats, holds no SOL). SOL collateral deposited to Treasury, tracked via repurposed deprecated `total_burned_from_buyback` field. Short-enabled flag uses `buyback_percent_bps` sentinel (`u16::MAX`), following V35 sentinel pattern. Same math as long lending: 50% max LTV, 65% liquidation threshold, 10% bonus, 50% close factor, 2%/epoch interest (in token terms), 80% utilization cap. One change to existing code: `borrow()` subtracts reserved short collateral from available SOL before calculating max lendable. All 4 instructions support vault routing (V18 pattern). No external oracle — Raydium pool price is the canonical price source for both directions. No state layout changes to existing accounts. 31 instructions total. 56 Kani proofs all passing (8 new: debt value bounds, LTV edge cases, interest non-overflow, liquidation bonus, lifecycle conservation, partial close accounting, lifecycle with interest, collateral reservation). |
| v11.0.0 | **V11 Margin Risk Refresh + Vote Removal + Fee Rebalance.** Voting (V36): new tokens initialize `vote_finalized = true`, the vote vault is never funded, the buy split goes 100% to the buyer, and `BuyArgs.vote` is removed. Vote fields stay on `BondingCurve` for Borsh layout compatibility but are dead code for new mints. Margin guards: depth-tier max LTV (25%/35%/45%/50% by pool SOL via `get_depth_max_ltv_bps`), `MIN_POOL_SOL_LENDING = 5 SOL` floor for new positions, ±50% price-band gate (`MAX_PRICE_DEVIATION_BPS = 5000`) on `borrow`/`open_short`, AMM-config pin in `validate_pool_accounts` (reads `pool_state.amm_config` and compares to hardcoded `RAYDIUM_AMM_CONFIG`). Per-user borrow cap raised from 5× to 23× (`BORROW_SHARE_MULTIPLIER = 23`) since the depth-tier LTV is now the primary risk lever. Fee rebalance: protocol fee split flipped to 50/50 protocol-treasury/dev (`DEV_WALLET_SHARE_BPS = 5000`); treasury SOL decay widened to 17.5% → 2.5% (`TREASURY_SOL_MAX_BPS = 1750`, `TREASURY_SOL_MIN_BPS = 250`); transfer fee bumped to 0.07% (`TRANSFER_FEE_BPS = 7`); flat per-buy treasury fee zeroed (`TREASURY_FEE_BPS = 0`). Short selling auto-enabled on `create_token` (writes `SHORT_ENABLED_SENTINEL`); `enable_short_selling` retained for legacy tokens. New `math.rs` module hosts every fee/curve/lending/short formula as `Option<u64>`-returning helpers; Kani proofs and handlers both import from it (single source of truth, no replica). 31 instructions total. 71 Kani proofs (v11.0.0 + v11.0.1 math refactor). |
