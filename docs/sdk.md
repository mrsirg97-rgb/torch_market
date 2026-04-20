# Torch SDK

TypeScript SDK for [torch.market](https://torch.market) — a protocol where every token launches with its own margin market.

Every token on Torch gets a bonding curve for price discovery, a treasury funded by a dynamic 17.5%→2.5% SOL rate, a 300M token lending reserve locked at creation, and a full margin system (lending + short selling) that activates after migration to Raydium. 100% of tokens go to buyers — no vote vault, no splits. The SDK builds transactions locally from the on-chain Anchor IDL, reads all state directly from Solana RPC, and handles routing between bonding curves and DEX pools automatically. No API server. No middleman.

## Install

```bash
pnpm add torchsdk
```

Peer dependency: `@solana/web3.js ^1.98.0`

## How It Works

```
1. Get a quote     →  getBuyQuote / getSellQuote
2. Build a tx      →  buildBuyTransaction / buildSellTransaction
3. Sign and send   →  your wallet / keypair
```

All transaction builders return VersionedTransactions (v0 message format).

Quotes work across both bonding curve and DEX — the `source` field tells you which. Pass the quote into the transaction builder and the SDK handles routing and slippage protection automatically.

```typescript
import { Connection } from "@solana/web3.js";
import { getBuyQuote, buildBuyTransaction } from "torchsdk";

const connection = new Connection("https://api.mainnet-beta.solana.com");

// Works on any token — bonding or migrated
const quote = await getBuyQuote(connection, mint, 100_000_000); // 0.1 SOL
console.log(`${quote.tokens_to_user / 1e6} tokens, source: ${quote.source}`);

const { transaction } = await buildBuyTransaction(connection, {
  mint,
  buyer: agentWallet,
  amount_sol: 100_000_000,
  slippage_bps: 500, // 5%
  vault: vaultCreator,
  quote, // drives routing + slippage protection
});
// sign and send
```

## Torch Vault

The vault is a full-custody on-chain escrow for AI agents. It holds all SOL and tokens. The agent wallet is a disposable controller that signs transactions but holds nothing of value.

```
Human (hardware wallet)           Agent (disposable, ~0.01 SOL for gas)
  ├── createVault()                 ├── buy(vault=creator)    → vault pays
  ├── depositVault(5 SOL)           ├── sell(vault=creator)   → SOL returns to vault
  ├── linkWallet(agentPubkey)       ├── borrow(vault=creator) → SOL to vault
  ├── withdrawVault()               ├── repay(vault=creator)  → collateral returns
  └── unlinkWallet(agent)           ├── openShort(vault=creator) → tokens to vault
                                    └── closeShort(vault=creator) → SOL returns to vault
```

Seven guarantees: full custody, closed economic loop, authority separation, one link per wallet, permissionless deposits, instant revocation, authority-only withdrawals.

## Operations

### Queries (no signing)

| Function | Description |
|----------|-------------|
| `getTokens(connection, params?)` | List tokens with filtering and sorting |
| `getToken(connection, mint)` | Full token details (price, treasury, status) |
| `getTokenMetadata(connection, mint)` | On-chain Token-2022 metadata |
| `getHolders(connection, mint)` | Token holder list |
| `getMessages(connection, mint, limit?, opts?)` | Trade-bundled memos. `{ enrich: true }` adds SAID verification |
| `getLendingInfo(connection, mint)` | Lending parameters for migrated tokens |
| `getLoanPosition(connection, mint, wallet)` | Single loan position |
| `getAllLoanPositions(connection, mint)` | All positions sorted by liquidation risk |
| `getShortPosition(connection, mint, wallet)` | Short position with health status and LTV |
| `getVault(connection, creator)` | Vault state |
| `getVaultForWallet(connection, wallet)` | Reverse lookup — find vault by linked wallet |
| `getVaultWalletLink(connection, wallet)` | Link state for a wallet |
| `getUserStats(connection, wallet)` | Per-user trading volume + rewards-claimed history |
| `getProtocolTreasuryState(connection)` | Protocol treasury epoch state + aggregate volumes + distributable amount |
| `getTreasuryState(connection, mint)` | Per-token treasury state — SOL balance, tokens held, baseline pool reserves at migration, stars |

### Quotes

| Function | Description |
|----------|-------------|
| `getBuyQuote(connection, mint, solAmount)` | Expected tokens, fees, price impact. Returns `source: 'bonding' \| 'dex'` |
| `getSellQuote(connection, mint, tokenAmount)` | Expected SOL, price impact. Returns `source: 'bonding' \| 'dex'` |
| `getBorrowQuote(connection, mint, collateralAmount)` | Max borrowable SOL with breakdown of constraints |

### Trading

All builders return `{ transaction: VersionedTransaction, message: string }`.

| Function | Description |
|----------|-------------|
| `buildBuyTransaction(connection, params)` | Buy tokens via vault. Auto-routes bonding curve or DEX based on quote |
| `buildDirectBuyTransaction(connection, params)` | Buy without vault (human wallets only) |
| `sendBuy(connection, wallet, params)` | Build + simulate + submit vault buy via `signAndSendTransaction` |
| `sendDirectBuy(connection, wallet, params)` | Build + simulate + submit direct buy via `signAndSendTransaction` |
| `buildSellTransaction(connection, params)` | Sell tokens via vault. Auto-routes bonding curve or DEX based on quote |
| `buildCreateTokenTransaction(connection, params)` | Launch a new token with bonding curve + treasury + 300M token lock |
| `sendCreateToken(connection, wallet, params)` | Build + simulate + submit token creation via `signAndSendTransaction` |
| `buildStarTransaction(connection, params)` | Star a token (0.02 SOL, sybil-resistant) |
| `buildMigrateTransaction(connection, params)` | Migrate bonding-complete token to Raydium (permissionless) |

### Vault Management

| Function | Signer | Description |
|----------|--------|-------------|
| `buildCreateVaultTransaction` | creator | Create vault + auto-link creator |
| `buildDepositVaultTransaction` | anyone | Deposit SOL (permissionless) |
| `buildWithdrawVaultTransaction` | authority | Withdraw SOL |
| `buildWithdrawTokensTransaction` | authority | Withdraw tokens from vault |
| `buildLinkWalletTransaction` | authority | Link a controller wallet |
| `buildUnlinkWalletTransaction` | authority | Revoke controller access |
| `buildTransferAuthorityTransaction` | authority | Transfer admin control |

### Lending (post-migration)

Treasury-backed margin lending. Borrow SOL against token collateral. Depth-adaptive max LTV (25-50% based on pool SOL depth), 65% liquidation threshold, 2% interest per epoch (simple-linear accrual). Deeper pools permit higher leverage — the pool itself is the risk engine. No oracles, no stored baseline.

Interest is only written on-chain when an instruction touches the position, but off-chain readers (`getLoanPosition`, `getShortPosition`, `getAllLoanPositions`) project it forward to the current slot using the exact on-chain formula. That means liquidation scanners see accurate `health` / `current_ltv_bps` immediately — no need to poke the loan first. Raw stored values are preserved in `accrued_interest_stored` and `last_update_slot` for callers who need the instant-of-signing amount.

| Function | Description |
|----------|-------------|
| `buildBorrowTransaction(connection, params)` | Borrow SOL against token collateral (vault-routed) |
| `buildRepayTransaction(connection, params)` | Repay debt + interest, unlock collateral (vault-routed) |
| `buildLiquidateTransaction(connection, params)` | Liquidate underwater position (>65% LTV, permissionless) |
| `buildClaimProtocolRewardsTransaction(connection, params)` | Claim epoch trading rewards (vault-routed) |

### Short Selling (post-migration)

Borrow real tokens from the 300M treasury lock, sell on the market, buy back to close. Same depth-adaptive LTV and parameters as lending — 25-50% max LTV (pool-depth dependent), 65% liquidation, 2% per epoch interest.

| Function | Description |
|----------|-------------|
| `buildOpenShortTransaction(connection, params)` | Post SOL collateral, borrow tokens from treasury lock (vault-routed) |
| `buildCloseShortTransaction(connection, params)` | Return tokens + interest, recover SOL collateral (vault-routed) |
| `buildLiquidateShortTransaction(connection, params)` | Liquidate underwater short (>65% LTV, permissionless) |
| `buildEnableShortSellingTransaction(connection, params)` | Enable shorts for pre-V5 tokens (admin only) |

### Treasury Cranks (permissionless)

| Function | Description |
|----------|-------------|
| `buildHarvestFeesTransaction(connection, params)` | Harvest Token-2022 transfer fees (0.07%) into treasury |
| `buildSwapFeesToSolTransaction(connection, params)` | Swap harvested tokens to SOL via Raydium |
| `buildAdvanceProtocolEpochTransaction(connection, params)` | Advance protocol epoch so previous-epoch trading rewards become claimable |
| `buildReclaimFailedTokenTransaction(connection, params)` | Reclaim tokens inactive 7+ days |

### SAID Protocol

| Function | Description |
|----------|-------------|
| `verifySaid(wallet)` | Check verification status and trust tier |
| `confirmTransaction(connection, sig, wallet)` | Report tx for reputation tracking |

## Network Configuration

```typescript
// Browser
(globalThis as any).__TORCH_NETWORK__ = 'devnet'

// Node.js
// TORCH_NETWORK=devnet npx tsx your-script.ts
```

## Testing

```bash
# Mainnet fork (Surfpool)
surfpool start --network mainnet --no-tui
npx tsx tests/test_e2e.ts

# Devnet
TORCH_NETWORK=devnet npx tsx tests/test_devnet_e2e.ts
```

## Links

- [torch.market](https://torch.market)
- [Whitepaper](https://torch.market/whitepaper)
- [SDK source](https://github.com/mrsirg97-rgb/torchsdk)
- [npm](https://www.npmjs.com/package/torchsdk)
- [Design doc](./design.md)
- [SDK audit](./audit.md)
- Program ID: `8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT`

## License

MIT
