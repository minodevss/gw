#[derive(Debug, thiserror::Error)]
pub(crate) enum ProviderError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("upstream returned status {status}: {body_excerpt}")]
    Status { status: u16, body_excerpt: String },
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid response: {0}")]
    Invalid(String),
    #[error("credentials expired")]
    Expired,
    #[error("login cancelled")]
    Cancelled,
}
