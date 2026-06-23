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

    /// Attempted to mutate a store that was opened in read-only mode
    /// (issue #263). Carries the operation name so the caller can tell
    /// the user what they tried to do.
    #[error("store is read-only; cannot perform: {0}")]
    ReadOnly(String),

    /// The active storage backend does not implement this operation
    /// (issue #301). The default SQLite backend supports everything;
    /// opt-in remote backends (e.g. PostgreSQL) may only cover the core
    /// memory surface and return this for the heavier subsystems
    /// (memoir graph, transcripts, structured facts, feedback). Carries
    /// the operation name so the caller can tell the user what is
    /// unavailable on this backend.
    #[error("operation not supported on this storage backend: {0}")]
    Unsupported(String),
}

pub type IcmResult<T> = Result<T, IcmError>;
