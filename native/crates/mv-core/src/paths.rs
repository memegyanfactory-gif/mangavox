use std::path::PathBuf;

/// `~/.mangavox` — the same data folder the Electron app uses.
pub fn root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mangavox")
}

pub fn database() -> PathBuf {
    root().join("mangavox.db")
}

pub fn config() -> PathBuf {
    root().join("config.json")
}

pub fn covers() -> PathBuf {
    root().join("cache").join("covers")
}
