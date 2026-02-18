pub mod arb;
pub mod fees;
pub mod freshness;
pub mod types;

pub use arb::detect_arb;
pub use freshness::validate_freshness;
pub use types::{ArbCheck, MarketSnapshot, Opportunity};
