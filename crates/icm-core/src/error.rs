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

    #[error("embedding error: {0}")]
    Embedding(String),

    /// Caller-supplied input violates a domain invariant (e.g. would
    /// introduce a cycle in the concept graph, empty topic, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type IcmResult<T> = Result<T, IcmError>;
