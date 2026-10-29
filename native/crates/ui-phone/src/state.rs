//! Shared state of the phone companion: the saved settings (`phone` in config.json), the server
//! status the Mobile page shows, and a change counter the page watches.
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Port tried first; the next free port is used when it is taken.
pub const DEFAULT_PORT: u16 = 8790;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Phone access is on: the server runs (also after a restart).
    pub enabled: bool,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self { enabled: false, port: DEFAULT_PORT }
    }
}

pub fn settings() -> Settings {
    mv_core::config::Config::open_default().get("phone").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    let _ = mv_core::config::Config::open_default().set("phone", serde_json::to_value(s).unwrap_or_default());
    changed();
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub running: bool,
    /// Where the server listens (0.0.0.0:port).
    pub addr: Option<SocketAddr>,
    /// This PC's address on the local network, for the QR code.
    pub lan_ip: Option<IpAddr>,
    /// Why the server is not running, when it failed to start.
    pub error: Option<String>,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn status() -> Status {
    STATUS.lock().map(|s| s.clone().unwrap_or_default()).unwrap_or_default()
}

pub fn set_status(s: Status) {
    if let Ok(mut cur) = STATUS.lock() {
        *cur = Some(s);
    }
    changed();
}

/// Bumped on every change the Mobile page shows (server status, a phone paired or revoked).
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Relaxed)
}

pub fn changed() {
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// The address a phone opens for `code` (what the QR code holds).
pub fn pair_url(code: &str) -> Option<String> {
    let s = status();
    let (ip, port) = (s.lan_ip?, s.addr?.port());
    Some(format!("http://{ip}:{port}/pair?code={code}"))
}
