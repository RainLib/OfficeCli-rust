#[derive(Debug, thiserror::Error)]
pub enum HcdError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid HCD bundle: {0}")]
    InvalidBundle(String),
    #[error("invalid HCD patch: {0}")]
    InvalidPatch(String),
    #[error("revision conflict: {0}")]
    RevisionConflict(String),
    #[error("node not found: {0}")]
    NodeNotFound(String),
    #[error("node precondition failed: {0}")]
    PreconditionFailed(String),
    #[error("resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("source mismatch: {0}")]
    SourceMismatch(String),
    #[error("unsupported HCD operation: {0}")]
    Unsupported(String),
}
