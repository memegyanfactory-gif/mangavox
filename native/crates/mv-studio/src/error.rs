use std::path::PathBuf;

/// Errors from interpreter discovery, worker runs and job handling.
#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    #[error("Python not available: {0}")]
    PythonNotFound(String),

    #[error("worker script not found: {}", .0.display())]
    ScriptNotFound(PathBuf),

    #[error("failed to start `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    /// A worker reported an error message (`type: "error"`).
    #[error("{message}")]
    Worker {
        message: String,
        stage: Option<String>,
    },

    /// A worker exited unsuccessfully without a terminal message.
    #[error("worker exited with code {}", code.map(|c| c.to_string()).unwrap_or_else(|| "null".into()))]
    Exit { code: Option<i32>, stderr: String },

    #[error("cancelled")]
    Cancelled,

    #[error("{0}")]
    Preview(String),

    /// A voice pack download failed (message is shown to the user).
    #[error("{0}")]
    Download(String),

    #[error("requirements check failed: {0}")]
    Requirements(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
