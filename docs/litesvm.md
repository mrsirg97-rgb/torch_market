# litesvm integration test suite

## goal

Cover the gap between pure-math verification (kani + proptest) and SDK e2e (Surfpool / devnet). Catches handler-layer bugs: account constraints, lamport flow correctness, error variant mapping, CPI shape. Tier B work blocks until v1 of this is in place.

## test surface map

| Layer | Tool | What it catches |
|---|---|---|
| Pure math | kani (72) + proptest (33×5k) | Arithmetic invariants |
| Handler integration | litesvm (this doc) | `#[account(...)]` constraints, lamport flow, error variants, PDA derivation, CPI behavior |
| Full system | SDK e2e (Surfpool fork, devnet) | SDK serialization, RPC roundtrip, mainnet-state interactions |

The three layers are complementary, not redundant. A bug like "liquidation blocked when pool drops below 5 SOL" is invisible to kani (it's about handler control flow, not math), expensive to set up in Surfpool (need real swaps to drain a pool), and trivial in litesvm (poke the pool PDA's lamports directly).

## crate layout

```
programs/torch_market/tests/
├── math_proptests.rs            (existing)
├── litesvm.rs                   (entry — declares modules below)
└── litesvm/
    ├── harness.rs               (Env, fixtures, lamport poking helpers)
    ├── buy.rs
    ├── sell.rs
    ├── migration.rs
    ├── reclaim.rs
    ├── revival.rs
    ├── lending.rs               (borrow / repay / liquidate + vault variants)
    ├── short.rs                 (open / close / liquidate + vault variants)
    ├── vault.rs                 (create / deposit / withdraw / link / transfer)
    ├── treasury.rs              (harvest_fees, swap_fees_to_sol gates)
    ├── protocol_treasury.rs     (epoch advance, claim)
    ├── rewards.rs               (star + creator payout)
    └── tier_b.rs                (the three Tier B items, isolated for clarity)
```

Cargo convention: `tests/litesvm.rs` is the test binary; `tests/litesvm/*.rs` are its modules (declared via `mod x;` in `litesvm.rs`). Single `cargo test --test litesvm` runs everything.

## dependencies

Added under `[dev-dependencies]`:

- `litesvm` — in-process Solana runtime
- `solana-sdk`, `solana-program` — already-transitive deps surfaced for tests
- `borsh` — direct account deserialization in assertions
- `anchor-lang` (already present) — instruction builders

deep_pool is loaded as an external program: `Env::new()` reads `deep_pool.so` from `$DEEP_POOL_SO_PATH` (default `../../deep_pool/target/deploy/deep_pool.so`) and registers it via `svm.add_program(deep_pool::ID, &bytes)`. If the file is missing, `Env::new()` panics with a clear message pointing at the deep_pool build command.

Token-2022 and System are loaded by `litesvm`'s built-in default program set.

## harness contract

```rust
pub struct Env {
    pub svm: LiteSVM,
    pub authority: Keypair,        // protocol authority
    pub dev_wallet: Keypair,
    pub global_config: Pubkey,
    pub protocol_treasury: Pubkey,
}

impl Env {
    pub fn new() -> Self;          // bootstrap: load programs, init global_config + protocol_treasury

    // creation
    pub fn create_token(&mut self, creator: &Keypair, target: u64, community: bool) -> TokenCtx;

    // trading
    pub fn buy(&mut self, buyer: &Keypair, t: &TokenCtx, sol: u64, min_out: u64) -> Result<()>;
    pub fn sell(&mut self, seller: &Keypair, t: &TokenCtx, tokens: u64, min_out: u64) -> Result<()>;
    pub fn bond_to_completion(&mut self, t: &TokenCtx, buyer: &Keypair);         // helper
    pub fn migrate(&mut self, t: &TokenCtx, payer: &Keypair) -> Result<()>;

    // lending
    pub fn borrow(&mut self, borrower: &Keypair, t: &TokenCtx, collateral: u64, sol_to_borrow: u64) -> Result<()>;
    pub fn repay(&mut self, borrower: &Keypair, t: &TokenCtx, sol: u64) -> Result<()>;
    pub fn liquidate(&mut self, liquidator: &Keypair, borrower: Pubkey, t: &TokenCtx) -> Result<()>;

    // shorts (similar)
    pub fn open_short(&mut self, ...) -> Result<()>;
    pub fn close_short(&mut self, ...) -> Result<()>;
    pub fn liquidate_short(&mut self, ...) -> Result<()>;

    // vaults
    pub fn create_vault(&mut self, creator: &Keypair) -> VaultCtx;
    pub fn link_wallet(&mut self, auth: &Keypair, v: &VaultCtx, wallet: Pubkey) -> Result<()>;
    // ... full vault api

    // accessors (typed deserialization)
    pub fn get_bonding_curve(&self, t: &TokenCtx) -> BondingCurve;
    pub fn get_treasury(&self, t: &TokenCtx) -> Treasury;
    pub fn get_loan(&self, t: &TokenCtx, borrower: Pubkey) -> Option<LoanPosition>;
    pub fn get_short(&self, t: &TokenCtx, shorter: Pubkey) -> Option<ShortPosition>;
    pub fn get_protocol_treasury(&self) -> ProtocolTreasury;
    pub fn get_vault(&self, v: &VaultCtx) -> TorchVault;

    // adversarial / testing-only helpers
    pub fn airdrop(&mut self, to: Pubkey, lamports: u64);
    pub fn poke_pool_sol(&mut self, t: &TokenCtx, target_lamports: u64);   // for pool-thin tests
    pub fn advance_slots(&mut self, n: u64);                                // for interest accrual / reclaim
    pub fn warp_to_slot(&mut self, slot: u64);
}

pub struct TokenCtx {
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub treasury: Pubkey,
    pub treasury_lock: Pubkey,
    pub deep_pool: Pubkey,           // populated after migrate()
}
```

Error assertions use a `expect_err!(result, TorchMarketError::Variant)` macro that decodes the Anchor error code from the `TransactionError`. Defined in `harness.rs`.

## coverage plan

~85 tests, organized per handler domain. Every reachable variant in `TorchMarketError` must be asserted at least once. Numbers are floors.

### tier_b.rs (3) — write first to validate the harness
- `liquidation_pool_thin_currently_blocks` — open loan with healthy pool, drain pool below `MIN_POOL_SOL_LENDING`, attempt `liquidate`, assert `PoolTooThin` (this is the current behavior; the test pins it before we decide whether to change)
- `loan_position_leaked_after_full_repay` — open loan, full repay, assert PDA still exists with zeroed fields (pins current behavior)
- `short_position_leaked_after_full_close` — same shape for shorts

### buy.rs (10)
happy / `MaxWalletExceeded` / `SlippageExceeded` / `InsufficientTokens` (curve out of tokens) / `BondingComplete` / `AmountTooSmall` / community-token (creator gets 0) / first-buy account init / vault variant happy / `InvalidDevWallet`

### sell.rs (6)
happy / `ZeroAmount` / `BondingComplete` / `InsufficientTokens` / vault variant / sell-after-reclaim path

### migration.rs (8)
happy / `BondingNotComplete` / `AlreadyMigrated` / `InsufficientMigrationFee` / baseline-recorded-correctly / LP-fully-burned / mint/freeze/fee authorities revoked / payer reimbursement = rent only (no double-count of sol_amount)

### reclaim.rs (5)
happy / `TokenStillActive` / `AlreadyReclaimed` / `BelowReclaimThreshold` / `BondingComplete`

### revival.rs (4)
happy / `TokenNotReclaimed` / `AmountTooSmall` / threshold-triggers-revival-event

### lending.rs (22)
borrow (9): happy / `LendingNotEnabled` / `LendingRequiresMigration` / `LtvExceeded` / depth-tier-correct (each of the 4 tiers) / `LendingCapExceeded` / `UserBorrowCapExceeded` / partial-deposit-no-borrow / vault variant
repay (6): partial / full-returns-collateral / interest-paid-first / `NoActiveLoan` / `ZeroAmount` / vault variant
liquidate (7): happy / `NotLiquidatable` / partial-with-close-bps / full-liquidation-clears-active-loans / bad-debt-handled / `PoolTooThin` (covered in tier_b.rs but asserted here too) / vault variant

### short.rs (20)
open (8): happy / `ShortNotEnabled` / `LtvExceeded` / `ShortCapExceeded` / `UserShortCapExceeded` / `ShortTooSmall` / `NotMigrated` / vault variant
close (6): partial / full-returns-sol / interest-paid-first / `NoActiveShort` / `ZeroAmount` / vault variant
liquidate_short (6): happy / `ShortNotLiquidatable` / partial / bad-debt / `PoolTooThin` / vault variant

### vault.rs (5)
create / deposit / withdraw `VaultUnauthorized` / link + unlink + `VaultWalletLinkMismatch` / transfer authority

### treasury.rs (4)
harvest_fees happy / swap_fees_to_sol below-threshold returns Ok(no-op) / cooldown enforced / creator-fee split correct

### protocol_treasury.rs (5)
init / advance_epoch `EpochNotComplete` / advance_epoch happy / claim happy / `AlreadyClaimed` / `InsufficientVolumeForRewards`

### rewards.rs (3)
star happy / `CannotStarSelf` / creator-payout-at-threshold

## ordering

Each step independently reviewable:

1. **Harness scaffolding** — `Env::new` + `create_token` + `buy` + `bond_to_completion` + `migrate` + 2 sanity tests. Validates the entire stack works before we write 80 more tests on top.
2. **tier_b.rs** — pins current Tier B behavior. These tests will *intentionally fail* once we implement Tier B fixes — that's the contract.
3. **buy.rs + sell.rs**
4. **migration.rs + reclaim.rs + revival.rs**
5. **lending.rs**
6. **short.rs**
7. **vault.rs + treasury.rs + protocol_treasury.rs + rewards.rs**

After step 2 the harness is proven; the rest is volume.

## verification

- `cargo test --test litesvm` green (after all steps)
- Add a CI step: build deep_pool first, then run litesvm tests. Document in readme.md.
- Coverage assertion: a small CI grep that every `TorchMarketError` variant that's `pub fn`-reachable appears in at least one `expect_err!` call. Catches a forgotten error path on PRs that add error variants.

## non-goals

- Replacing the Surfpool / devnet e2e suite. They still cover SDK roundtrip and real-network state.
- Replacing kani / proptest. They cover math at a depth litesvm can't reach (exhaustive symbolic vs. concrete-input fuzzing).
- Loadtest. AGENTS.md treats this as a separate concern (spec-driven CU + p99 budgets).
- Mainnet replay. Future work if a real incident motivates it.

## honest caveat

A test suite — any test suite — catches the bugs you thought to test for. Litesvm closes a real gap, but it does not make the program bug-free. Pair it with continued audits, kani harnesses for new state machines, and treating any mainnet incident as a permanent regression test.
