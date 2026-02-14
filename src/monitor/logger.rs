use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::info;

use crate::error::{Error, Result};
use crate::strategy::Opportunity;

/// Append-only JSON lines trade/opportunity log.
pub struct TradeLogger {
    #[allow(dead_code)]
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl TradeLogger {
    pub fn new(path: &str) -> Result<Self> {
        let path = PathBuf::from(path);

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        info!(path = %path.display(), "trade logger initialized");

        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Log an opportunity as a JSON line.
    pub fn log_opportunity(&self, opportunity: &Opportunity) -> Result<()> {
        let json = serde_json::to_string(opportunity)?;
        let mut file = self
            .file
            .lock()
            .map_err(|_| Error::Monitor("logger lock poisoned".into()))?;
        writeln!(file, "{json}")?;
        Ok(())
    }
}
