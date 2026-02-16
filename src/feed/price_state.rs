use std::collections::HashMap;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde::Serialize;

/// Latest price snapshot for a single exchange+symbol pair.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PriceEntry {
    /// Exchange name (e.g. "binance", "coinbase", "okx").
    pub exchange: String,
    /// Mid price (bid + ask) / 2.
    pub price: Decimal,
    /// When this entry was last updated (monotonic clock).
    pub updated_at: Instant,
    /// Exchange timestamp in milliseconds (wall clock, for logging).
    pub timestamp_ms: i64,
}

/// A single exchange's price at snapshot time (for logging).
#[derive(Debug, Clone, Serialize)]
pub struct ExchangePrice {
    pub exchange: String,
    pub price: Decimal,
    pub age_ms: u128,
}

/// Snapshot of all exchange prices for one asset at a point in time.
/// Captures the full context needed to understand why an arb was accepted or rejected.
#[derive(Debug, Clone, Serialize)]
pub struct ExchangeSnapshot {
    pub asset: String,
    pub exchanges: Vec<ExchangePrice>,
    pub fresh_count: usize,
    pub divergence_pct: Option<Decimal>,
    pub chainlink_price: Option<Decimal>,
    pub chainlink_age_ms: Option<u128>,
}

/// Sources that are NOT direct exchange feeds (excluded from freshness validation).
const NON_EXCHANGE_SOURCES: &[&str] = &["chainlink"];

/// In-memory store for the latest price per exchange+symbol.
///
/// Keyed by `"exchange:symbol"` (e.g. `"binance:btcusdt"`, `"coinbase:btc-usd"`).
/// Supports querying all exchanges for a given asset.
///
/// Sources are split into two tiers:
/// - **Exchanges** (Binance, Coinbase, OKX, Bybit): fast, direct WS feeds used for
///   leg-risk assessment (exchange agreement = stable book = safe to fill both sides)
/// - **Resolution** (Chainlink): slower relay, used separately for oracle tracking
///   (danger-zone detection near strike, abnormal oracle behavior diagnosis)
pub struct PriceState {
    /// Key: "exchange:symbol", Value: PriceEntry
    prices: HashMap<String, PriceEntry>,
    /// Maps config asset name to its symbols per exchange.
    /// e.g. "BTC" -> [("binance", "btcusdt"), ("coinbase", "btc-usd"), ("okx", "btc-usdt")]
    asset_map: HashMap<String, Vec<(String, String)>>,
}

impl Default for PriceState {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceState {
    pub fn new() -> Self {
        // Pre-populate the asset-to-exchange-symbol mapping
        let mut asset_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for asset in &["BTC", "ETH", "SOL", "XRP"] {
            asset_map.insert(
                asset.to_string(),
                vec![
                    ("binance".into(), format!("{}usdt", asset.to_lowercase())),
                    ("coinbase".into(), format!("{}-USD", asset.to_uppercase())),
                    ("okx".into(), format!("{}-USDT", asset.to_uppercase())),
                    ("bybit".into(), format!("{}USDT", asset.to_uppercase())),
                    ("chainlink".into(), format!("{}/usd", asset.to_lowercase())),
                ],
            );
        }

        Self {
            prices: HashMap::new(),
            asset_map,
        }
    }

    fn key(exchange: &str, symbol: &str) -> String {
        format!("{exchange}:{symbol}")
    }

    /// Update the stored price for an exchange+symbol pair.
    pub fn update(&mut self, exchange: &str, symbol: &str, price: Decimal, timestamp_ms: i64) {
        self.prices.insert(
            Self::key(exchange, symbol),
            PriceEntry {
                exchange: exchange.to_string(),
                price,
                updated_at: Instant::now(),
                timestamp_ms,
            },
        );
    }

    /// Get the latest price entry by exchange and symbol.
    #[allow(dead_code)]
    pub fn get(&self, exchange: &str, symbol: &str) -> Option<&PriceEntry> {
        self.prices.get(&Self::key(exchange, symbol))
    }

    /// Get all available price entries for a config asset (e.g. "BTC").
    /// Returns entries from all exchanges that have data for this asset.
    #[allow(dead_code)]
    pub fn get_all_for_asset(&self, asset: &str) -> Vec<&PriceEntry> {
        let Some(mappings) = self.asset_map.get(asset) else {
            return Vec::new();
        };
        mappings
            .iter()
            .filter_map(|(exchange, symbol)| self.prices.get(&Self::key(exchange, symbol)))
            .collect()
    }

    /// Get the latest price from the best (most recently updated) exchange for an asset.
    #[allow(dead_code)]
    pub fn get_freshest_for_asset(&self, asset: &str) -> Option<&PriceEntry> {
        self.get_all_for_asset(asset)
            .into_iter()
            .min_by_key(|e| e.updated_at.elapsed())
    }

    /// How stale is the freshest price for a given asset (across all exchanges).
    #[allow(dead_code)]
    pub fn staleness(&self, asset: &str) -> Option<Duration> {
        self.get_freshest_for_asset(asset)
            .map(|e| e.updated_at.elapsed())
    }

    /// Is the freshest price fresh enough to act on?
    #[allow(dead_code)]
    pub fn is_fresh(&self, asset: &str, max_age: Duration) -> bool {
        self.staleness(asset)
            .map(|age| age <= max_age)
            .unwrap_or(false)
    }

    /// Count how many exchanges have fresh (within max_age) price data for an asset.
    #[allow(dead_code)]
    pub fn count_fresh_sources(&self, asset: &str, max_age: Duration) -> usize {
        self.get_all_for_asset(asset)
            .iter()
            .filter(|e| e.updated_at.elapsed() <= max_age)
            .count()
    }

    /// Get the max divergence percentage between any two exchanges for an asset.
    /// Returns None if fewer than 2 sources have data.
    #[allow(dead_code)]
    pub fn max_divergence_pct(&self, asset: &str) -> Option<Decimal> {
        let entries = self.get_all_for_asset(asset);
        if entries.len() < 2 {
            return None;
        }

        let prices: Vec<Decimal> = entries.iter().map(|e| e.price).collect();
        let min_price = prices.iter().min().unwrap();
        let max_price = prices.iter().max().unwrap();

        if *min_price == Decimal::ZERO {
            return None;
        }

        Some((*max_price - *min_price) / *min_price * Decimal::ONE_HUNDRED)
    }

    /// Check if all exchanges agree on price within a tolerance (for arb validation).
    /// Returns true if all available prices are within `max_divergence_pct` of each other.
    /// Returns false if fewer than `min_sources` have data.
    #[allow(dead_code)]
    pub fn prices_agree(
        &self,
        asset: &str,
        max_divergence_pct: Decimal,
        min_sources: usize,
    ) -> bool {
        let entries = self.get_all_for_asset(asset);
        if entries.len() < min_sources {
            return false;
        }

        let prices: Vec<Decimal> = entries.iter().map(|e| e.price).collect();
        let min_price = prices.iter().min().unwrap();
        let max_price = prices.iter().max().unwrap();

        if *min_price == Decimal::ZERO {
            return false;
        }

        let divergence = (*max_price - *min_price) / *min_price * Decimal::ONE_HUNDRED;
        divergence <= max_divergence_pct
    }

    // --- Exchange-only methods (exclude Chainlink and other non-exchange sources) ---

    fn is_exchange(exchange: &str) -> bool {
        !NON_EXCHANGE_SOURCES.contains(&exchange)
    }

    /// Get price entries from direct exchange feeds only (excludes Chainlink).
    pub fn get_exchange_entries(&self, asset: &str) -> Vec<&PriceEntry> {
        let Some(mappings) = self.asset_map.get(asset) else {
            return Vec::new();
        };
        mappings
            .iter()
            .filter(|(exchange, _)| Self::is_exchange(exchange))
            .filter_map(|(exchange, symbol)| self.prices.get(&Self::key(exchange, symbol)))
            .collect()
    }

    /// Count fresh direct exchange sources (excludes Chainlink).
    pub fn count_fresh_exchange_sources(&self, asset: &str, max_age: Duration) -> usize {
        self.get_exchange_entries(asset)
            .iter()
            .filter(|e| e.updated_at.elapsed() <= max_age)
            .count()
    }

    /// Max divergence among direct exchange feeds only (excludes Chainlink).
    pub fn exchange_max_divergence_pct(&self, asset: &str) -> Option<Decimal> {
        let entries = self.get_exchange_entries(asset);
        if entries.len() < 2 {
            return None;
        }

        let prices: Vec<Decimal> = entries.iter().map(|e| e.price).collect();
        let min_price = prices.iter().min().unwrap();
        let max_price = prices.iter().max().unwrap();

        if *min_price == Decimal::ZERO {
            return None;
        }

        Some((*max_price - *min_price) / *min_price * Decimal::ONE_HUNDRED)
    }

    /// Check if direct exchange feeds agree within tolerance (excludes Chainlink).
    pub fn exchanges_agree(
        &self,
        asset: &str,
        max_divergence_pct: Decimal,
        min_sources: usize,
    ) -> bool {
        let entries = self.get_exchange_entries(asset);
        if entries.len() < min_sources {
            return false;
        }

        let prices: Vec<Decimal> = entries.iter().map(|e| e.price).collect();
        let min_price = prices.iter().min().unwrap();
        let max_price = prices.iter().max().unwrap();

        if *min_price == Decimal::ZERO {
            return false;
        }

        let divergence = (*max_price - *min_price) / *min_price * Decimal::ONE_HUNDRED;
        divergence <= max_divergence_pct
    }

    /// Staleness of the freshest direct exchange price (excludes Chainlink).
    pub fn exchange_staleness(&self, asset: &str) -> Option<Duration> {
        self.get_exchange_entries(asset)
            .into_iter()
            .min_by_key(|e| e.updated_at.elapsed())
            .map(|e| e.updated_at.elapsed())
    }

    /// Capture a serializable snapshot of all exchange prices for an asset.
    /// Used for enriching trade log records with full context at decision time.
    pub fn snapshot_asset(&self, asset: &str, max_age: Duration) -> ExchangeSnapshot {
        let exchanges: Vec<ExchangePrice> = self
            .get_exchange_entries(asset)
            .iter()
            .map(|e| ExchangePrice {
                exchange: e.exchange.clone(),
                price: e.price,
                age_ms: e.updated_at.elapsed().as_millis(),
            })
            .collect();

        let fresh_count = exchanges
            .iter()
            .filter(|e| e.age_ms <= max_age.as_millis())
            .count();
        let divergence_pct = self.exchange_max_divergence_pct(asset);

        let (chainlink_price, chainlink_age_ms) = match self.get_chainlink(asset) {
            Some(entry) => (
                Some(entry.price),
                Some(entry.updated_at.elapsed().as_millis()),
            ),
            None => (None, None),
        };

        ExchangeSnapshot {
            asset: asset.to_string(),
            exchanges,
            fresh_count,
            divergence_pct,
            chainlink_price,
            chainlink_age_ms,
        }
    }

    /// Get the Chainlink resolution price for an asset, if available.
    #[allow(dead_code)]
    pub fn get_chainlink(&self, asset: &str) -> Option<&PriceEntry> {
        let mappings = self.asset_map.get(asset)?;
        mappings
            .iter()
            .find(|(exchange, _)| exchange == "chainlink")
            .and_then(|(exchange, symbol)| self.prices.get(&Self::key(exchange, symbol)))
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

        assert!(state.get("binance", "btcusdt").is_none());

        state.update("binance", "btcusdt", dec!(69000.50), 1700000000000);

        let entry = state.get("binance", "btcusdt").unwrap();
        assert_eq!(entry.price, dec!(69000.50));
        assert_eq!(entry.exchange, "binance");
        assert_eq!(entry.timestamp_ms, 1700000000000);
    }

    #[test]
    fn update_overwrites_previous() {
        let mut state = PriceState::new();
        state.update("binance", "ethusdt", dec!(3500), 1000);
        state.update("binance", "ethusdt", dec!(3600), 2000);

        let entry = state.get("binance", "ethusdt").unwrap();
        assert_eq!(entry.price, dec!(3600));
        assert_eq!(entry.timestamp_ms, 2000);
    }

    #[test]
    fn multi_exchange_for_asset() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);
        state.update("okx", "BTC-USDT", dec!(68990), 1000);

        let entries = state.get_all_for_asset("BTC");
        assert_eq!(entries.len(), 3);

        let freshest = state.get_freshest_for_asset("BTC").unwrap();
        assert!(freshest.price > Decimal::ZERO);
    }

    #[test]
    fn staleness_and_freshness() {
        let mut state = PriceState::new();

        // No data = not fresh
        assert!(state.staleness("BTC").is_none());
        assert!(!state.is_fresh("BTC", Duration::from_secs(1)));

        state.update("binance", "btcusdt", dec!(69000), 1000);

        // Just updated = fresh
        assert!(state.is_fresh("BTC", Duration::from_secs(1)));
        assert!(state.staleness("BTC").unwrap() < Duration::from_millis(100));

        // Wait and check staleness grows
        thread::sleep(Duration::from_millis(50));
        assert!(state.staleness("BTC").unwrap() >= Duration::from_millis(50));
    }

    #[test]
    fn prices_agree_within_tolerance() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);
        state.update("okx", "BTC-USDT", dec!(68990), 1000);

        // 0.03% divergence, should agree within 0.1%
        assert!(state.prices_agree("BTC", dec!(0.1), 2));

        // Not enough sources
        assert!(!state.prices_agree("BTC", dec!(0.1), 5));
    }

    #[test]
    fn prices_disagree_on_spike() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(70000), 1000); // 1.4% higher

        // 0.1% tolerance — should disagree
        assert!(!state.prices_agree("BTC", dec!(0.1), 2));

        // 2% tolerance — should agree
        assert!(state.prices_agree("BTC", dec!(2.0), 2));
    }

    #[test]
    fn unknown_asset_returns_empty() {
        let state = PriceState::new();
        assert!(state.get_all_for_asset("DOGE").is_empty());
        assert!(state.get_freshest_for_asset("DOGE").is_none());
        assert!(!state.prices_agree("DOGE", dec!(1.0), 1));
    }

    #[test]
    fn exchange_entries_exclude_chainlink() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);
        state.update("chainlink", "btc/usd", dec!(69005), 1000);

        // get_all includes chainlink
        let all = state.get_all_for_asset("BTC");
        assert_eq!(all.len(), 3);

        // exchange-only excludes chainlink
        let exchanges = state.get_exchange_entries("BTC");
        assert_eq!(exchanges.len(), 2);
        assert!(exchanges.iter().all(|e| e.exchange != "chainlink"));
    }

    #[test]
    fn exchange_fresh_count_excludes_chainlink() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("chainlink", "btc/usd", dec!(69005), 1000);

        // All sources: 2 fresh
        assert_eq!(state.count_fresh_sources("BTC", Duration::from_secs(10)), 2);
        // Exchange-only: 1 fresh (chainlink excluded)
        assert_eq!(
            state.count_fresh_exchange_sources("BTC", Duration::from_secs(10)),
            1
        );
    }

    #[test]
    fn exchanges_agree_excludes_chainlink() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);
        // Chainlink is far behind — would cause divergence if included
        state.update("chainlink", "btc/usd", dec!(68500), 1000);

        // prices_agree (all sources) should fail — chainlink diverges 0.7%
        assert!(!state.prices_agree("BTC", dec!(0.5), 2));

        // exchanges_agree should pass — only binance+coinbase, 0.01% divergence
        assert!(state.exchanges_agree("BTC", dec!(0.5), 2));
    }

    #[test]
    fn snapshot_asset_captures_context() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);
        state.update("coinbase", "BTC-USD", dec!(69010), 1000);
        state.update("chainlink", "btc/usd", dec!(69005), 1000);

        let snap = state.snapshot_asset("BTC", Duration::from_secs(10));
        assert_eq!(snap.asset, "BTC");
        assert_eq!(snap.exchanges.len(), 2); // excludes chainlink
        assert_eq!(snap.fresh_count, 2);
        assert!(snap.divergence_pct.is_some());
        assert_eq!(snap.chainlink_price, Some(dec!(69005)));
        assert!(snap.chainlink_age_ms.is_some());

        // Verify exchange names
        let names: Vec<&str> = snap.exchanges.iter().map(|e| e.exchange.as_str()).collect();
        assert!(names.contains(&"binance"));
        assert!(names.contains(&"coinbase"));
        assert!(!names.contains(&"chainlink"));
    }

    #[test]
    fn snapshot_asset_no_chainlink() {
        let mut state = PriceState::new();
        state.update("binance", "btcusdt", dec!(69000), 1000);

        let snap = state.snapshot_asset("BTC", Duration::from_secs(10));
        assert_eq!(snap.exchanges.len(), 1);
        assert_eq!(snap.fresh_count, 1);
        assert!(snap.chainlink_price.is_none());
        assert!(snap.chainlink_age_ms.is_none());
    }

    #[test]
    fn get_chainlink_price() {
        let mut state = PriceState::new();
        assert!(state.get_chainlink("BTC").is_none());

        state.update("chainlink", "btc/usd", dec!(69005), 1000);
        let entry = state.get_chainlink("BTC").unwrap();
        assert_eq!(entry.exchange, "chainlink");
        assert_eq!(entry.price, dec!(69005));
    }
}
