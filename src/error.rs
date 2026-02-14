use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum Error {
    #[error("config error: {0}")]
    Config(String),

    #[error("market discovery error: {0}")]
    Discovery(String),

    #[error("feed error: {0}")]
    Feed(String),

    #[error("strategy error: {0}")]
    Strategy(String),

    #[error("execution error: {0}")]
    Execution(String),

    #[error("risk check failed: {0}")]
    Risk(String),

    #[error("monitor error: {0}")]
    Monitor(String),

    #[error("SDK error: {0}")]
    Sdk(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
