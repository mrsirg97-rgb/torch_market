// Service layer — DB read/write composed lazily off a per-request transaction.
// See deep_pool/docs/indexer.md §"Service DAG" for the design rationale.
//
// Each service wraps `&mut RequestCtx`; construction is free, work happens
// only when methods are invoked. Composition pattern: from inside a service,
// call `self.ctx.<other>()` — the borrow scope ends when the returned wrapper
// drops, freeing ctx for the next call.

pub mod context;

// Torch domains
pub mod loan;
pub mod market;
pub mod message;
pub mod migration;
pub mod pnl;
pub mod short;
pub mod trade;

// deep_pool domains
pub mod liquidity;
pub mod pool;
pub mod reserves;
pub mod swap;

pub use context::{Cache, RequestCtx};
pub use loan::LoanService;
pub use market::MarketService;
pub use message::MessageService;
pub use migration::MigrationService;
pub use pnl::PnlService;
pub use short::ShortService;
pub use trade::TradeService;

pub use liquidity::LiquidityService;
pub use pool::PoolService;
pub use reserves::ReservesService;
pub use swap::SwapService;

// Lazy service accessors on RequestCtx. Free construction, work on demand.
impl RequestCtx {
    pub fn markets(&mut self) -> MarketService<'_> {
        MarketService::new(self)
    }
    pub fn trades(&mut self) -> TradeService<'_> {
        TradeService::new(self)
    }
    pub fn messages(&mut self) -> MessageService<'_> {
        MessageService::new(self)
    }
    pub fn loans(&mut self) -> LoanService<'_> {
        LoanService::new(self)
    }
    pub fn shorts(&mut self) -> ShortService<'_> {
        ShortService::new(self)
    }
    pub fn migrations(&mut self) -> MigrationService<'_> {
        MigrationService::new(self)
    }
    pub fn pnl(&mut self) -> PnlService<'_> {
        PnlService::new(self)
    }
    pub fn pools(&mut self) -> PoolService<'_> {
        PoolService::new(self)
    }
    pub fn reserves(&mut self) -> ReservesService<'_> {
        ReservesService::new(self)
    }
    pub fn swaps(&mut self) -> SwapService<'_> {
        SwapService::new(self)
    }
    pub fn liquidity(&mut self) -> LiquidityService<'_> {
        LiquidityService::new(self)
    }
}
