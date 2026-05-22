// Filter types consumed by API handlers and forwarded to services.
// One module per indexed domain — torch side + deep_pool side.

pub mod loan;
pub mod market;
pub mod message;
pub mod migration;
pub mod short;
pub mod trade;

// deep_pool reused domains
pub mod liquidity;
pub mod pool;
pub mod reserves;
pub mod swap;

pub use loan::LoanFilter;
pub use market::MarketFilter;
pub use message::MessageFilter;
pub use migration::MigrationFilter;
pub use short::ShortFilter;
pub use trade::TradeFilter;

pub use liquidity::LiquidityFilter;
pub use pool::PoolFilter;
pub use reserves::ReservesFilter;
pub use swap::SwapFilter;
