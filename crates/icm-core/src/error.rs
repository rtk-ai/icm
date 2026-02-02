use thiserror::Error;

#[derive(Debug, Error)]
pub enum IcmError {
    #[error("memory not found: {0}")]
    NotFound(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("config error: {0}")]
    Config(String),
}

pub type IcmResult<T> = Result<T, IcmError>;
