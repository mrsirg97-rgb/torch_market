# Torch Market — V21 Architecture

Every token launches with its own closed-loop leveraged market. One Anchor program, 36 instructions, 11 account types, no external dependencies beyond DeepPool (also in-house) and the Token-2022 program.

**Program ID:** `E5b4rBqtS5jRvjHcYZ3ZSNo2sdSPJtauQKkEacKmmjqG` (V21, current)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          TORCH MARKET v21                               │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  PROTOCOL LAYER                                                         │
│  ┌──────────────┐  ┌────────────────────────────────────┐               │
│  │ GlobalConfig │  │ ProtocolTreasury                   │               │
│  │ (admin,      │  │ (0.5% fees, epoch rewards, dev      │              │
│  │  settings)   │  │  wallet 50% split)                 │               │
│  └──────────────┘  └────────────────────────────────────┘               │
│                                                                         │
│  PER-TOKEN LAYER                                                        │
│  ┌──────────────┐    ┌──────────────────┐    ┌──────────────────┐       │
│  │ Token-2022   │───▶│ BondingCurve     │───▶│ Treasury         │       │
│  │ Mint         │    │ (const product,  │    │ (long + short    │       │
│  │ + 0.07% fee  │    │  100 / 200 SOL   │    │  exposure +      │       │
│  │ + metadata   │    │  tier targets)   │    │  lending config) │       │
│  └──────────────┘    └────────┬─────────┘    └────────┬─────────┘       │
│                               │                       │                 │
│                               ▼                       │                 │
│                      ┌──────────────────┐             │                 │
│                      │ DeepPool CPMM    │◀────────────┘                 │
│                      │ (post-migration  │                               │
│                      │  liquidity +     │                               │
│                      │  keeperless TWAP)│                               │
│                      └────────┬─────────┘                               │
│                               │                                         │
│         ┌─────────────────────┼─────────────────────┐                   │
│         ▼                     ▼                     ▼                   │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐               │
│  │ Position     │    │ Position     │    │ TreasuryLock │               │
│  │ (unified;    │    │ vaults       │    │ (300M tokens │               │
│  │  Long/Short  │    │ (per-pos     │    │  locked =    │               │
│  │  side)       │    │  custody)    │    │  short pool) │               │
│  └──────────────┘    └──────────────┘    └──────────────┘               │
│                                                                         │
│  USER LAYER                                                             │
│  ┌──────────────┐  ┌──────────────┐                                     │
│  │ UserPosition │  │ UserStats    │                                     │
│  │ (spot buys)  │  │ (platform-   │                                     │
│  │              │  │  wide volume)│                                     │
│  └──────────────┘  └──────────────┘                                     │
│                                                                         │
│  VAULT LAYER (agent custody)                                            │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐                 │
│  │ TorchVault   │  │ VaultWallet- │  │ TorchVaultSol  │                 │
│  │ (state + SOL │  │ Link         │  │ (system-owned, │                 │
│  │  per creator)│  │ (reverse map)│  │  buy-path SOL  │                 │
│  │              │  │              │  │  hop, ephemeral│                 │
│  └──────────────┘  └──────────────┘  └────────────────┘                 │
│                                                                         │
├─────────────────────────────────────────────────────────────────────────┤
│  HANDLERS                                                               │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌─────────┐ ┌────────┐     │
│  │ admin  │ │ token  │ │ market │ │treasury│ │migration│ │reclaim │     │
│  └────────┘ └────────┘ └────────┘ └────────┘ └─────────┘ └────────┘     │
│  ┌────────┐ ┌────────┐ ┌──────────┐ ┌────────┐ ┌────────┐               │
│  │revival │ │protocol│ │ leverage │ │ vault  │ │  swap  │               │
│  │        │ │treasury│ │long+short│ │        │ │        │               │
│  └────────┘ └────────┘ └──────────┘ └────────┘ └────────┘               │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Overview

Torch turns every token into a self-contained financial system. A token launches with a bonding curve, graduates to a DeepPool CPMM, and immediately gets closed-loop leverage — leveraged longs and shorts, both backed by real on-chain reserves, no external oracle, no governance. The 300M-token treasury lock created at launch is the literal short pool; the SOL accumulated from transfer fees + bonding splits is the literal long-lending pool. Every leveraged position is protocol-custodied in a per-position vault until the user closes — borrowed capital never reaches a wallet (see [v21-closed-loop-leverage.md](./v21-closed-loop-leverage.md)).

Four phases per token: **Bonding → Migration → Trading → Leverage**. Each phase builds the next.

Torch's CPMM is DeepPool (in-house, formally verified) — no Raydium, no WSOL: DeepPool holds native SOL on the pool PDA, and the migration handler is ~100 lines. As of V21, DeepPool also self-publishes the keeperless TWAP mark torch's liquidations read (see [twap-oracle.md](./twap-oracle.md)). See [deeppool.md](./deeppool.md) for the integration detail.

---

## Phase Lifecycle

| Phase | Trigger | What activates |
|---|---|---|
| **Bonding** | `create_token` | Constant-product curve: 700M tokens sellable, 300M locked, 100 SOL (Flame) or 200 SOL (Torch) graduation target |
| **Migration** | `migrate_to_dex` (permissionless after target reached) | DeepPool pool created with bonded SOL + remaining tokens. 100% of LP burned to pool PDA. Mint/freeze/transfer-fee authorities revoked. |
| **Trading** | Post-migration | DeepPool swap as canonical price. 0.07% Token-2022 transfer fee on every transfer; permissionless `harvest_fees` + `swap_fees_to_sol` recycle into the treasury |
| **Leverage** | Auto-enabled at `create_token` (longs + shorts) | Closed-loop leveraged markets — every position is protocol-custodied (per-position vault), opening and closing atomically through DeepPool. Treasury SOL backs longs; the 300M `TreasuryLock` backs shorts. Depth-scaled rails: concave max-LTV curve (30-60%), 25%-of-pool size cap, 65% liquidation threshold, distress-scaled 32.5% bonus ceiling, 1.5%/epoch interest, keeperless DeepPool TWAP mark |

---

## Program Structure

```
programs/torch_market/src/
├── lib.rs               # 36 instruction entry points
├── handlers/            # Business logic per instruction domain
│   ├── admin.rs         # initialize, update_dev_wallet
│   ├── token.rs         # create_token (Token-2022 + treasury_lock + auto-enable shorts)
│   ├── market.rs        # buy, sell (curve trading, with vault routing)
│   ├── migration.rs     # migrate_to_dex (DeepPool create_pool CPI)
│   ├── treasury.rs      # harvest_fees, swap_fees_to_sol (DeepPool swap CPI)
│   ├── reclaim.rs       # reclaim_failed_token (7-day inactivity)
│   ├── revival.rs       # contribute_revival
│   ├── protocol_treasury.rs  # initialize / advance_epoch / claim_protocol_rewards
│   ├── leverage.rs     # open/close/liquidate × {long, short} + via_vault variants (unified Position)
│   ├── vault.rs         # create_vault, deposit, withdraw, link/unlink_wallet, transfer_authority, withdraw_tokens
│   └── swap.rs          # vault_swap (vault-routed DeepPool buy/sell)
├── contexts.rs          # Anchor #[derive(Accounts)] for every instruction
├── state.rs             # 10 #[account] types
├── constants.rs         # Protocol parameters and PDA seeds
├── errors.rs            # Custom error variants
├── math.rs              # Pure arithmetic (single source of truth for fees, curve, lending, shorts, accrual). Kani proofs import directly from here.
├── migration.rs         # Migration handler implementation (DeepPool CPI flow)
├── pool_validation.rs   # DeepPool PDA derivation + reserve reading + depth-band LTV helpers
├── token_2022_utils.rs  # Token-2022 transfer-fee + metadata extension helpers
└── kani_proofs.rs       # 97 formal verification harnesses (cfg(kani))
```

---

## On-Chain Accounts

14 `#[account]` types. One additional system-owned PDA (`TorchVaultSol`) has no data layout — it's referenced by seeds only.

### GlobalConfig

Protocol-wide configuration. Singleton.

| Field | Type | Description |
|---|---|---|
| authority | Pubkey | Admin (update_dev_wallet only — no pause; protocol is immutable post-launch) |
| treasury | Pubkey | Legacy fee wallet reference |
| dev_wallet | Pubkey | Receives 50% of protocol fee (`DEV_WALLET_SHARE_BPS = 5000`) |
| protocol_fee_bps | u16 | `PROTOCOL_FEE_BPS = 50` (0.5%) |
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

Per-token treasury: lending/short exposure counters, risk parameters, and the baseline for ratio gating. Holds **no** open-position assets — collateral for both sides lives in per-position vaults (D-2/D-8); the treasury tracks only aggregate exposure for the sticky lending gate + per-user caps. Treasury SOL is **derived** from the System-owned `treasury_sol_vault` lamports (no tracked `sol_balance` field — single source of truth, can't drift). The TWAP observation ring that used to live here is gone — the mark now lives in DeepPool (see [twap-oracle.md](./twap-oracle.md)).

| Field | Type | Description |
|---|---|---|
| bonding_curve | Pubkey | Back-reference |
| mint | Pubkey | Token mint |
| is_community_token | bool | `true` = 100% of fees to treasury (creator share = 0); `false` = 85/15 treasury/creator |
| last_buyback_slot | u64 | For sell-cycle cooldown |
| harvested_fees | u64 | Cumulative SOL from transfer-fee harvest |
| bump | u8 | |
| baseline_sol_reserves | u64 | Pool SOL at migration (ratio-gate baseline) |
| baseline_token_reserves | u64 | Pool tokens at migration |
| short_selling_enabled | bool | Set true by `create_token`. Always on. |
| min_buyback_interval_slots | u64 | Cooldown between `swap_fees_to_sol` calls |
| baseline_initialized | bool | Set true at migration |
| **Shorts (token debt)** | | |
| total_tokens_lent | u64 | Aggregate gross token debt across open shorts |
| active_shorts | u64 | Open short count |
| short_interest_collected | u64 | Accrued short interest revenue (token-denom) |
| **Longs (SOL debt)** | | |
| total_sol_lent_to_longs | u64 | Aggregate gross SOL debt across open longs |
| active_longs | u64 | Open long count |
| long_interest_collected | u64 | Accrued long interest revenue (SOL-denom) |
| total_token_collateral_locked | u64 | Aggregate token collateral across open longs |
| **Risk parameters** | | |
| lending_enabled | bool | Auto-enabled at creation |
| interest_rate_bps | u16 | Default 150 (1.5%/epoch) |
| max_ltv_bps | u16 | Default 6000 (60%) ceiling — effective LTV is `min(get_depth_max_ltv_bps(pool_sol), this)`; seeded to the curve asymptote so it rarely binds |
| liquidation_threshold_bps | u16 | Default 6500 (65%) |
| liquidation_bonus_bps | u16 | Default 3250 (32.5%) — derived `1.3·ρ_max`; the distress-ramp ceiling |
| liquidation_close_bps | u16 | Default 5000 (50% partial-close cap) |

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
| distributable_amount | u64 | Live spend-down ledger (decremented per claim) |
| epoch_distributable_snapshot | u64 | [F-7] Frozen share base for the epoch — pro-rata shares + the 10% cap compute against this, so payouts are order-independent |
| bump | u8 | |

**Seeds:** `["protocol_treasury_v11"]`

---

### Position

Unified leveraged position (D-3) — **one** account shape for both sides, distinguished by a `side` discriminant. Long and short share lifecycle, account layout, math, and liquidation; only the held/borrowed asset types mirror. The held asset is **not** a field — it IS the per-position vault balance (`position_sol_vault.lamports()` for shorts, `position_token_vault.amount` for longs), read from the vault, not tracked (derived state over tracked state makes the math unfalsifiable).

| Field | Type | Description |
|---|---|---|
| user | Pubkey | Position owner |
| mint | Pubkey | Token |
| side | PositionSide | `Long` (0) or `Short` (1) |
| position_index | u32 | Allows multiple positions per `(user, mint, side)` — DCA / scaled entries |
| collateral_amount | u64 | Initial deposit (record-keeping only) |
| debt_amount | u64 | Owed asset: tokens for short, SOL (gross) for long |
| accrued_interest | u64 | Interest since `last_slot`, in debt-asset denom |
| last_slot | u64 | Last accrual slot (advances on every accrual, including the zero-debt early-return path) |
| bump | u8 | |
| vault_bump | u8 | Bump of the position's SOL vault PDA |

**Seeds:** `["position", user, mint, side, position_index]` — vault-owned positions (via `TorchVault`) seed off `torch_vault` in place of `user`.

**Per-position vaults:** a short's `position_sol_vault` PDA (`["short_vault", user, mint, position_index]`) holds collateral + sale-proceeds SOL; a long's `position_token_vault` (program-owned Token-2022 ATA) holds collateral + bought tokens. The vault balance *is* the held asset — there is no tracked `held_amount`.

---

### UserRisk

Per-(owner, mint) aggregate leverage exposure — makes the per-user caps hold across `position_index` values (without it, each new index re-granted the full allowance). `owner` is the wallet for direct positions and the `torch_vault` PDA for via_vault positions. Created lazily on the first open (`init_if_needed`), never closed. **Zero-copy** (`AccountLoader`): the leverage contexts run within bytes of the SBF 4096-byte `try_accounts` frame, and a borsh `Account` here overflows the via_vault contexts.

| Field | Type | Description |
|---|---|---|
| owner | Pubkey | Wallet (direct) or TorchVault PDA (via_vault) |
| mint | Pubkey | Token |
| short_tokens_debt | u64 | Σ open short principal — capped at `MAX_WALLET_TOKENS` |
| long_sol_debt | u64 | Σ open long principal (gross SOL) — capped at `min(formula, 20%)` |
| long_collateral_tokens | u64 | Σ open long collateral (formula-cap basis) |
| bump | u8 | + 7 bytes explicit repr(C) padding |

**Seeds:** `["user_risk", owner, mint]`

**Invariant:** the debt fields equal the sums over the owner's open positions — opens add; closes/liquidations subtract principal + bad debt (litesvm `assert_user_risk` reconciles). Replaced `bonding_curve` in the leverage contexts (its migrated/!reclaimed checks moved to `treasury.baseline_initialized`), so net accounts-per-leverage-tx is unchanged.

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

**Balance invariant:** the DERIVED balance (`vault_sol` lamports − rent; there is no `sol_balance` field) `= total_deposited + total_received − total_withdrawn − total_spent` — executable as litesvm `vault_derived_balance_matches_lifetime_totals`

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

36 instructions across 10 domains. Every user-funded operation has two non-Optional context variants: a wallet path (`buy`, `open_long`, etc.) where the signer funds from their own SOL/ATA, and a `_via_vault` path (`buy_via_vault`, `open_long_via_vault`, etc.) where a linked `TorchVault` funds the operation. The split eliminates the V19 `Option<>` vault-triple pattern, removing all `as_ref().unwrap()` panic surface and moving all defense-in-depth + arg-validation checks to account-resolution time.

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

### Market (4)

| Instruction | Description |
|---|---|
| `buy` | Buy tokens from bonding curve. SOL split: 0.5% protocol fee (50/50 dev/protocol_treasury), then decaying treasury share (17.5% → 2.5%), creator share (0% → 1%; 0 for community tokens), remainder to curve. 100% of tokens to buyer. 2% wallet cap enforced. |
| `buy_via_vault` | Same as `buy`, funded from a linked `TorchVault`. Tokens delivered to vault ATA. |
| `sell` | Sell tokens back to curve. No sell fee. Per-buyer position tracked. |
| `sell_via_vault` | Same as `sell`, tokens sourced from vault ATA, SOL proceeds to vault. |

### Migration (1)

| Instruction | Description |
|---|---|
| `migrate_to_dex` | Permissionless. Moves bonded SOL out of the BondingCurve PDA, then CPI `deep_pool::create_pool` with `torch_config` PDA as signer. Burn 100% of LP. Revoke mint + freeze + transfer-fee-config authorities. Record `baseline_sol_reserves` + `baseline_token_reserves`. Reimburse payer migration cost from treasury. |

### Treasury (2)

| Instruction | Description |
|---|---|
| `harvest_fees` | Permissionless. Harvest accumulated Token-2022 withheld fees from arbitrary source accounts (passed via `remaining_accounts`) into the treasury's ATA. |
| `swap_fees_to_sol` | Permissionless. Ratio-gated: only sells when DeepPool price is ≥120% of migration baseline. Sells 15% of held tokens (or 100% if balance ≤ 1M tokens). DeepPool swap CPI (treasury signs as `sol_source`). Creator fee split (15%) carved off the SOL received for creator tokens. |

### Protocol Rewards (4)

| Instruction | Description |
|---|---|
| `initialize_protocol_treasury` | One-time setup of the ProtocolTreasury PDA |
| `advance_protocol_epoch` | Permissionless crank. Time-gated to one epoch (~7 days). Snapshots previous-epoch volume, opens new epoch, computes `distributable_amount` + freezes `epoch_distributable_snapshot` (the epoch's share base). |
| `claim_protocol_rewards` | User claims pro-rata share of the epoch SNAPSHOT based on `volume_previous_epoch` — order-independent: equal volume → equal payout, first or last to claim ([F-7]). Eligibility: ≥2 SOL volume in previous epoch. Capped at 10% of the snapshot per user; payout additionally bounded by the live `distributable_amount`. Min claim: 0.1 SOL. |
| `claim_protocol_rewards_via_vault` | Same as `claim_protocol_rewards`, SOL credited to vault. Controller wallet's volume is the basis. |

### Recovery (2)

| Instruction | Description |
|---|---|
| `reclaim_failed_token` | If bonding not complete and `last_activity_slot` is > 7 days old, anyone can reclaim. All curve SOL moves to protocol treasury (becomes epoch rewards). Marks token reclaimed. |
| `contribute_revival` | Permissionless deposit toward bringing a reclaimed token back. Threshold: `3 * bonding_target / 8` (37.5 SOL Flame / 75 SOL Torch). When met, trading resumes. Contributors receive no tokens. |

### Leverage — Longs (6)

A long borrows SOL from the treasury, atomically buys tokens through DeepPool into a per-position `position_token_vault`, and owes the gross SOL borrowed. Closed-loop: bought tokens never touch a wallet until close.

| Instruction | Description |
|---|---|
| `open_long` | Post token collateral, borrow SOL (sized by `effective_max_ltv = min(get_depth_max_ltv_bps(pool_sol), treasury.max_ltv_bps)`, clamped by the physical-float headroom, the per-user aggregate cap (`min(formula, 20%)` via `UserRisk`), and the Rail-2 size cap `debt_value ≤ 25% · pool_sol`), atomically `pool_buy` into the position vault. Charges `OPEN_FEE_BPS` (0.5%) on borrow value. |
| `open_long_via_vault` | Same as `open_long`, collateral + custody routed through a linked `TorchVault`. |
| `close_long` | Atomically sells all vault tokens through DeepPool, repays SOL debt + interest, returns surplus SOL to the user. Partial close via `repay_fraction_bps`. |
| `close_long_via_vault` | Same as `close_long`, surplus returned to the vault. |
| `liquidate_long` | Permissionless. `twap_ltv > 65%` against the DeepPool TWAP mark (spot veto only refuses a clearly-healthy position). Liquidator pays SOL, seizes vault tokens at the TWAP-priced seize + distress-scaled bonus (0 → 32.5% ceiling). Bad-debt write-off decrements `total_sol_lent_to_longs`. |
| `liquidate_long_via_vault` | Same as `liquidate_long`, funded/received via vault. |

### Leverage — Shorts (6)

A short borrows tokens from `TreasuryLock`, atomically sells them through DeepPool, and banks the SOL proceeds in a per-position `position_sol_vault`; it owes the token debt. Structural mirror of the long.

| Instruction | Description |
|---|---|
| `open_short` | Post SOL collateral, borrow tokens from `TreasuryLock`, atomically `pool_sell` so proceeds land in the position SOL vault. Same depth-band + per-user cap as longs (the SOL-side denominator is the treasury, since collateral is SOL). Charges `OPEN_FEE_BPS` (0.5%). |
| `open_short_via_vault` | Same as `open_short`, collateral + custody routed through a linked `TorchVault`. |
| `close_short` | Atomically `pool_buy`s tokens with vault SOL to repay the token debt + interest, returns surplus SOL to the user. Partial close via `repay_fraction_bps`. |
| `close_short_via_vault` | Same as `close_short`, surplus returned to the vault. |
| `liquidate_short` | Permissionless. Structural mirror of `liquidate_long`, asset-inverted: liquidator pays tokens, seizes vault SOL + bonus. Bad-debt write-off decrements `total_tokens_lent`. |
| `liquidate_short_via_vault` | Same as `liquidate_short`, funded/received via vault. |

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
migrate_to_dex
  ├─ move bonded SOL: BondingCurve PDA → payer (lamport debit)
  ├─ excess token burn (if any)
  ├─ transfer pool-side tokens BondingCurve → payer (with Token-2022 fee deduction)
  ├─ CPI deep_pool::create_pool (signed by torch_config PDA)
  ├─ burn 100% of LP received by payer
  ├─ revoke mint authority   → None  (permanent)
  ├─ revoke freeze authority → None  (permanent)
  ├─ revoke fee-config auth  → None  (transfer fee locked forever)
  ├─ treasury reimburses payer (rent + CPI cost, measured by lamport delta)
  └─ record baseline (pool SOL/token reserves at migration)

**Price-match caveat [F-11]:** the pool-seed tokens cross two Token-2022 transfer
legs (curve → payer → pool, 7 bps each), so the pool opens ~14 bps rich in SOL
terms vs the curve's final price. Documented, one-time, bounded by litesvm
`migration_price_within_double_transfer_fee_bound` (fails at 30 bps).
```

**Pool namespace:** every DeepPool pool created by torch lives at `[deep_pool, torch_config, mint]`. The `torch_config` PDA is signed by the program — cryptographically unfrontrunnable. Nobody outside torch_market can create a pool under torch's namespace.

**Pool reserve reading:** `pool_sol = pool_pda.lamports() - rent_exempt`, `pool_tokens = vault_ata.amount`. Two-line read, no raw byte parsing.

---

## Lending & Short Mechanics

Both sides share parameters, math, and lifecycle structure. The math is intentionally symmetric — the only asymmetry is asset roles (long borrows SOL against tokens; short borrows tokens against SOL).

### Depth-scaled risk rails

**[V21]** The old four-step LTV ladder is replaced by a continuous concave **curve** (Rail 1), a per-position **size cap** (Rail 2), and a **derived bonus** (Rail 3). Depth is priced once at open by two pure functions in `pool_validation`; the liquidation logic is unchanged. Full derivation: [depth-scaled-risk-rails.md](./depth-scaled-risk-rails.md); formal treatment: [risk.md](./risk.md) §2.

```rust
// Rail 1 — max LTV: concave in depth, 30% at the 100-SOL floor → 60% asymptote.
fn get_depth_max_ltv_bps(pool_sol: u64) -> u16 {
    if pool_sol < DEPTH_FLOOR_SOL { return 0; }           // < 100 SOL: no leverage
    let span = LTV_MAX_BPS - LTV_MIN_BPS;                 // 6000 − 3000 = 3000
    let drop = span * DEPTH_FLOOR_SOL / pool_sol;         // division-only (Kani-safe)
    (LTV_MAX_BPS - drop).clamp(LTV_MIN_BPS, LTV_MAX_BPS)  // 100→30% 200→45% 500→54% ∞→60%
}

// Rail 2 — size cap: a position's SOL-debt-value ≤ ρ_max of pool SOL, clamped at
// open. Makes worst-case liquidation-unwind slippage (≈ debt/pool_sol) depth-invariant.
fn max_debt_value_for_depth(pool_sol: u64) -> u64 {
    pool_sol as u128 * RHO_MAX_BPS as u128 / 10_000      // RHO_MAX_BPS = 2500 (25%)
}
```

Pool depth IS the manipulation-resistance signal: deeper pools = harder to move price = higher leverage permitted, on a smooth curve with no tier cliffs. Rail 2 then holds any single position's unwind slippage to `ρ_max` regardless of depth, so **Rail 3** — a flat liquidation bonus derived as `1.3 · ρ_max = 32.5%` (ramping 0→full on the TWAP LTV, full at `100/(1+bonus) = 75.5%`) — clears the unwind on every pool. No external oracle, no keeper, no stored baseline; the liquidation mark is a keeperless TWAP from the pool's own price cumulative.

### Per-user borrow cap (longs)

```rust
formula_cap   = max_lendable * (user_collateral / TOTAL_SUPPLY) * 23
absolute_cap  = max_lendable * MAX_USER_BORROW_SHARE_BPS / 10_000   // 20%
max_user_borrow = min(formula_cap, absolute_cap)
```

The formula cap scales with the user's share of total supply. The absolute cap clamps any single borrower at 20% of lendable SOL regardless of collateral size — without it, a user with > ~4.35% of supply (`TOTAL_SUPPLY / 23`) as collateral could take the entire lendable amount, defeating the per-user cap. Together they guarantee at least five simultaneous concentrated borrowers can be served. See [risk.md](./risk.md) §3.

### Per-user short cap

Flat `MAX_WALLET_TOKENS = 2% of TOTAL_SUPPLY` — the same anti-whale constant the bonding curve uses on buys, enforced as a per-USER aggregate across all of an owner's `position_index` values via the `UserRisk` account. Unified policy: "no wallet, by any means (buying / borrowing / shorting), can hold or short more than 2% of supply." There is **no aggregate short-utilization cap by design**: the lock may drain to zero (closes and liquidations only pay tokens *into* the lock, never need its inventory), and the real aggregate brake is pool depth — every short open drains pool SOL, shrinking Rail-2 and the depth-LTV curve until `PoolTooThin` stops new opens.

### Lending unlock gate

```rust
lending_assets = treasury_physical_sol + treasury.total_sol_lent_to_longs
require!(lending_assets >= MIN_TREASURY_SOL_FOR_LENDING)
```

The gate is **sticky** — it reads the treasury's *principal pool*, the V21 translation of V20's tracked `sol_balance` (see [lending-unlock.md](./lending-unlock.md)): the physical float plus outstanding receivables. Normal borrow/repay moves lamports between the two terms without changing the sum, so once the protocol has *earned* the threshold the gate stays open; only a bad-debt write-off (a real loss) re-locks it — the documented intent. The gate certifies meaningful lending scale ("nothing meaningful to lend against collateral at 10 SOL"); it is NOT a liquidity promise.

**Capacity is the physical float, first-come-first-serve.** No utilization cap — the float is fully lendable, mirroring the short side's full-drain lock. The float is already net of lent SOL (borrowed lamports physically leave `treasury_sol_vault` at open), so an exhausted float (`< MIN_BORROW_AMOUNT`) rejects new opens with `LendingCapExceeded` until repayments or transfer-fee growth restore it, while the gate stays open. **Frontends must display the physical float as borrow capacity, never the unlock flag.** Gate stickiness is proven division-free by Kani (`verify_lending_gate_sticky_under_borrow`).

The threshold is build-feature gated so test/dev environments work without organic activity:

| Build feature | `MIN_TREASURY_SOL_FOR_LENDING` |
|---|---|
| `simnet`  | 1 SOL (migration seeds well above this — unlocks naturally after bonding) |
| `devnet`  | 1 SOL (light e2e activity) |
| (default — mainnet) | 100 SOL |

`simnet` + `devnet` together is a `compile_error!`. The gate is volume-driven, not price-driven: it grows from buy fees + 4× transfer fees per short cycle + interest, not from oracle-pumpable signals. The short side has no equivalent gate because shorts borrow from the static 300M `TreasuryLock`, not from SOL treasury. See [docs/lending-unlock.md](./lending-unlock.md).

### Token-2022 lock conservation

The 300M `TreasuryLock` is preserved across short cycles by recording **gross** as the debt principal (not net received), and applying gross-up on close + liquidate so the lock always receives the full debt back.

```rust
// Open: lock sends gross, borrower receives net = gross − fee_open.
position.tokens_borrowed = args.tokens_to_borrow;  // gross

// Close: borrower pays gross_up(gross + interest); lock receives gross + interest.
gross_close = ceil((tokens_borrowed + interest) × 10_000 / (10_000 − TRANSFER_FEE_BPS))
```

Per-cycle lock change: `+interest`, regardless of hold duration. The borrower funds the open-leg fee gap at close time (acquiring `gross − net ≈ 0.07%` extra tokens from elsewhere), making round-trip transfer-fee cost (~0.14%) visible to the borrower rather than absorbed by the lock. Kani harnesses `verify_short_open_records_gross_amount` + `verify_short_full_close_lock_conservation` + `verify_gross_up_preserves_net_delivery` prove the invariant.

### Interest accrual

`interest = principal * rate_bps * slots / (10_000 * EPOCH_DURATION_SLOTS)`

Default rate: 1.5%/epoch (`DEFAULT_INTEREST_RATE_BPS = 150`; [V21] lowered from 200). Epoch = 7 days.

Accrual happens at the start of every position-touching instruction (`open_long`, `close_long`, `liquidate_long`, `open_short`, `close_short`, `liquidate_short`). The pure transition function `math::apply_interest_accrual` (and its short variant) ensures `last_update_slot` always advances to the current slot — including on the zero-debt early-return path. This prevents phantom interest on positions that are fully repaid and later re-borrowed without closing the account.

### Liquidation

`twap_ltv > 65%` (`DEFAULT_LIQUIDATION_THRESHOLD_BPS`), triggered on the hardened TWAP mark with a spot veto that can only refuse a *clearly-healthy* position (`LIQ_SPOT_VETO_MARGIN_BPS`). Liquidator covers up to 50% of total debt (`DEFAULT_LIQUIDATION_CLOSE_BPS`), receives collateral at the **TWAP-priced** seize plus a **distress-scaled bonus** — `effective_liq_bonus_bps` ramps 0 at the threshold to the `32.5%` ceiling at the full-bonus LTV (75.5%), so a manufactured barely-over liquidation earns ≈0. Bad-debt write-off: when collateral can't cover the slice, the shortfall reduces the position's `debt_amount` and `total_sol_lent_to_longs` together — proven equivalent to the simple form by Kani.

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
| Read pool reserves (`pool_pda.lamports() + vault.amount`) for leverage pricing | Holds reserves, computes swap outputs |
| Sign swaps as `user` from PDAs (`torch_vault`, `treasury`) via `invoke_signed` | Verifies `sol_source: Signer` constraint |
| Burn 100% of LP at migration → pool PDA's own LP ATA → permanently locked | Mints LP per `create_pool`; doesn't enforce burn |

DeepPool has its own audit and 16 separate Kani proofs covering swap math (K invariant, fee conservation, LP proportionality). Total verification across composed system: **100 proof harnesses** (84 torch + 16 deep_pool).

---

## Verification Surface

- **84 Kani proof harnesses** (`kani_proofs.rs`, gated by `cfg(kani)`). Cover all fee calculations, bonding curve pricing, lending math, short math, depth-band boundaries, migration arithmetic, interest accrual state transitions, treasury ratio gating, DeepPool CPI accounting, Token-2022 gross-up correctness, and the available-SOL lending-gate invariant. Math harnesses import directly from `math.rs` — every property is proven against the exact code that runs on-chain, not a replica.
- **55 proptest properties × 5,000 cases** (`tests/math_proptests.rs`). Random-input sweep across the full u64 space; complements Kani's bounded model checking.
- **110 litesvm integration tests** (`programs/torch_market/tests/litesvm/`). In-process BPF execution against the real torch_market and deep_pool `.so` binaries; ~7s for the full suite, with treasury-counter + per-user (`UserRisk`) reconciliation asserted across the leverage lifecycle. See [litesvm.md](./litesvm.md).
- **SDK e2e tests** (`packages/sdk/tests/test_e2e.ts`, `test_devnet_e2e.ts`). Run against Surfpool mainnet fork or devnet for SDK roundtrip and mainnet-state coverage.
- **Independent audit** (Claude Opus 4.7, see [audit.md](./audit.md)): 0 critical / 0 high / 0 medium / 0 low findings. 24 exploit classes covered in the adversarial redhat pass.

---

## Out of Scope

Honest about what V20 does not do:

- **Permissionless migration timing.** Anyone can call `migrate_to_dex` after bonding completes, but nobody is forced to. Economic incentive (treasury reimbursement) handles it in practice.
- **Opening new positions on shallow pools.** `[V21]` If pool depth is below the `DEPTH_FLOOR_SOL` = 100 SOL leverage floor, `open_long` and `open_short` reject new positions (`PoolTooThin` — the curve returns 0 max-LTV). Existing positions can still be liquidated — the depth gate was removed from the liquidate paths so trapped positions aren't stranded.
- **Upgrade authority revocation.** Live on mainnet during stabilization. Migrate to public timelock or multisig within the 30-90 day window post-launch via `solana program set-upgrade-authority --final`.
- **Cross-token margin.** Each `(user, mint, side, index)` has its own isolated `Position`. No portfolio margining. Failure of one position cannot affect another.
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
| V20 (interest accrual fix) | `apply_interest_accrual` post-condition strengthened: `last_update_slot` advances on every call including zero-debt. Prevents phantom interest on re-borrowed positions. +2 Kani harnesses + 2 proptest properties. |
| **V20 (current, deep_pool integration)** | (1) Lending unlock gate (`MIN_TREASURY_SOL_FOR_LENDING`) keyed on AVAILABLE SOL (`sol_balance − short_collateral_reserved`), not gross — short collateral cannot cosmetically unlock the gate (V20C-1 fix). (2) Absolute per-user borrow ceiling at 20% of lendable (`MAX_USER_BORROW_SHARE_BPS = 2000`) clamps the formula cap. (3) Per-user short cap unified to flat `MAX_WALLET_TOKENS` (2% supply), matching the bonding-curve anti-whale rule. (4) Token-2022 gross-up on every short close + liquidate so the 300M `TreasuryLock` never bleeds across cycles. (5) New error `LendingNotYetUnlocked`. +12 Kani harnesses (now 84) + 9 proptest properties (now 42). |
| **V21 (closed-loop leverage)** | Atomic-custodied long/short positions (vault-seeded, vault-routed via-vault variants); keeperless TWAP liquidation mark in DeepPool (D-10) with seize clamp, distress-scaled bonus, and asymmetric spot veto. **Depth-scaled risk rails:** the 4-step LTV ladder → continuous concave curve (30% floor → 60% asymptote, `DEPTH_FLOOR_SOL` = 100 SOL); new per-position **size cap** (`RHO_MAX_BPS = 2500`, debt ≤ 25% of pool); liquidation bonus + full-bonus-LTV **derived** from `ρ_max` (3250 bps / 7547 bps); interest 200 → 150 bps/epoch. New Kani proofs for the depth curve, size cap, and bonus ramp. See [depth-scaled-risk-rails.md](./depth-scaled-risk-rails.md), [v21-closed-loop-leverage.md](./v21-closed-loop-leverage.md). |

---

## Cross-References

- [whitepaper.md](./whitepaper.md) — protocol intent, parameters, economic design
- [risk.md](./risk.md) — formal analysis of the depth-anchored risk model
- [deeppool.md](./deeppool.md) — DeepPool integration detail
- [lending-unlock.md](./lending-unlock.md) — treasury-gated lending unlock design
- [verification.md](./verification.md) — Kani harness catalog (all 84)
- [properties.md](./properties.md) — proptest property catalog (all 42)
- [audit.md](./audit.md) — V20-current internal security audit + redhat findings
- [sdk.md](./sdk.md) — TypeScript SDK reference

---

*© 2026 Brightside Solutions.*
