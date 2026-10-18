use thiserror::Error;

pub type Result<T, E = MediaError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
    #[error("invalid JSON from {url}: {source}")]
    Json {
        url: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("[jikan] unavailable (circuit open)")]
    CircuitOpen,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid Stremio addon manifest: {0}")]
    InvalidManifest(String),
}
