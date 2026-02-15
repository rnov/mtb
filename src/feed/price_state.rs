use std::collections::HashMap;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::feed::crypto_price::asset_to_binance_symbol;

/// Latest price snapshot for a single symbol.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PriceEntry {
    /// Mid price (bid + ask) / 2 from Binance bookTicker.
    pub price: Decimal,
    /// When this entry was last updated (monotonic clock).
    pub updated_at: Instant,
    /// Binance timestamp in milliseconds (wall clock, for logging).
    pub timestamp_ms: i64,
}

/// In-memory store for the latest Binance price per symbol.
///
/// Keyed by Binance symbol (e.g. "btcusdt"). Provides lookup by
/// either Binance symbol or config asset name (e.g. "BTC").
pub struct PriceState {
    prices: HashMap<String, PriceEntry>,
}

impl Default for PriceState {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl PriceState {
    pub fn new() -> Self {
        Self {
            prices: HashMap::new(),
        }
    }

    /// Update the stored price for a symbol.
    /// Called from the main loop on every `FeedEvent::CryptoPrice`.
    pub fn update(&mut self, symbol: &str, price: Decimal, timestamp_ms: i64) {
        self.prices.insert(
            symbol.to_string(),
            PriceEntry {
                price,
                updated_at: Instant::now(),
                timestamp_ms,
            },
        );
    }

    /// Get the latest price entry by Binance symbol (e.g. "btcusdt").
    pub fn get(&self, symbol: &str) -> Option<&PriceEntry> {
        self.prices.get(symbol)
    }

    /// Get the latest price entry by config asset name (e.g. "BTC").
    /// Converts to Binance symbol internally.
    pub fn get_by_asset(&self, asset: &str) -> Option<&PriceEntry> {
        let symbol = asset_to_binance_symbol(asset);
        self.prices.get(&symbol)
    }

    /// How stale is the latest price for a given asset.
    /// Returns None if we've never received a price for this asset.
    pub fn staleness(&self, asset: &str) -> Option<Duration> {
        self.get_by_asset(asset).map(|e| e.updated_at.elapsed())
    }

    /// Is the price fresh enough to act on?
    pub fn is_fresh(&self, asset: &str, max_age: Duration) -> bool {
        self.staleness(asset)
            .map(|age| age <= max_age)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::thread;

    #[test]
    fn update_and_get() {
        let mut state = PriceState::new();

        assert!(state.get("btcusdt").is_none());
        assert!(state.get_by_asset("BTC").is_none());

        state.update("btcusdt", dec!(69000.50), 1700000000000);

        let entry = state.get("btcusdt").unwrap();
        assert_eq!(entry.price, dec!(69000.50));
        assert_eq!(entry.timestamp_ms, 1700000000000);

        // Also accessible by asset name
        let entry = state.get_by_asset("BTC").unwrap();
        assert_eq!(entry.price, dec!(69000.50));
    }

    #[test]
    fn update_overwrites_previous() {
        let mut state = PriceState::new();
        state.update("ethusdt", dec!(3500), 1000);
        state.update("ethusdt", dec!(3600), 2000);

        let entry = state.get("ethusdt").unwrap();
        assert_eq!(entry.price, dec!(3600));
        assert_eq!(entry.timestamp_ms, 2000);
    }

    #[test]
    fn staleness_and_freshness() {
        let mut state = PriceState::new();

        // No data = not fresh
        assert!(state.staleness("BTC").is_none());
        assert!(!state.is_fresh("BTC", Duration::from_secs(1)));

        state.update("btcusdt", dec!(69000), 1000);

        // Just updated = fresh
        assert!(state.is_fresh("BTC", Duration::from_secs(1)));
        assert!(state.staleness("BTC").unwrap() < Duration::from_millis(100));

        // Wait and check staleness grows
        thread::sleep(Duration::from_millis(50));
        assert!(state.staleness("BTC").unwrap() >= Duration::from_millis(50));
    }

    #[test]
    fn multiple_symbols() {
        let mut state = PriceState::new();
        state.update("btcusdt", dec!(69000), 1000);
        state.update("ethusdt", dec!(3500), 1000);
        state.update("solusdt", dec!(180), 1000);

        assert_eq!(state.get_by_asset("BTC").unwrap().price, dec!(69000));
        assert_eq!(state.get_by_asset("ETH").unwrap().price, dec!(3500));
        assert_eq!(state.get_by_asset("SOL").unwrap().price, dec!(180));
        assert!(state.get_by_asset("DOGE").is_none());
    }
}
