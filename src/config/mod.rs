pub mod types;

pub use types::{AppConfig, RunMode, Secrets};

use crate::error::{Error, Result};

/// Load configuration from config.toml and environment variables.
pub fn load_config(path: &str) -> Result<AppConfig> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("failed to read {path}: {e}")))?;
    let config: AppConfig =
        toml::from_str(&contents).map_err(|e| Error::Config(format!("invalid config: {e}")))?;
    Ok(config)
}

/// Load secrets from environment variables (.env loaded via dotenvy).
/// Private key is optional (not needed for dry-run mode).
pub fn load_secrets() -> Secrets {
    // dotenvy::dotenv() is best-effort — missing .env is fine if env vars are set directly
    let _ = dotenvy::dotenv();

    let private_key = std::env::var("POLYMARKET_PRIVATE_KEY").unwrap_or_default();
    let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok();

    Secrets {
        private_key,
        telegram_bot_token,
    }
}
