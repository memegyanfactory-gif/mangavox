//! Shell layout the user chose (sidebar collapsed, assistant width), kept in config.json under
//! `shell` so the window reopens the way it was left (Fluent NavigationView: the pane remembers
//! its state: <https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview>).
use std::sync::Mutex;

use mv_core::config::Config;
use serde_json::{json, Value};

pub const DOCK_MIN: f32 = 320.;
pub const DOCK_MAX: f32 = 560.;
pub const DOCK_DEFAULT: f32 = 400.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellPrefs {
    pub collapsed: bool,
    pub dock_w: f32,
}

impl Default for ShellPrefs {
    fn default() -> Self {
        ShellPrefs { collapsed: false, dock_w: DOCK_DEFAULT }
    }
}

impl ShellPrefs {
    fn from_value(v: Option<Value>) -> Self {
        let d = ShellPrefs::default();
        let Some(v) = v else { return d };
        ShellPrefs {
            collapsed: v.get("sidebarCollapsed").and_then(Value::as_bool).unwrap_or(d.collapsed),
            dock_w: v.get("dockWidth").and_then(Value::as_f64).map(|w| (w as f32).clamp(DOCK_MIN, DOCK_MAX)).unwrap_or(d.dock_w),
        }
    }

    fn to_value(self) -> Value {
        json!({ "sidebarCollapsed": self.collapsed, "dockWidth": self.dock_w.round() })
    }
}

/// Reads the saved layout (defaults when missing or malformed).
pub fn load() -> ShellPrefs {
    ShellPrefs::from_value(Config::open_default().get("shell"))
}

/// The newest layout not yet written, and the lock that keeps writes in order.
static LATEST: Mutex<Option<ShellPrefs>> = Mutex::new(None);
static WRITING: Mutex<()> = Mutex::new(());

/// Saves off the UI thread; a failed write only loses the layout. Writers take turns and each
/// writes the newest layout, so two quick toggles (Ctrl+B twice) can never leave the older
/// one on disk.
pub fn save(p: ShellPrefs) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    std::thread::spawn(|| {
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(p) = LATEST.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        if let Err(e) = Config::open_default().set("shell", p.to_value()) {
            eprintln!("[shell] could not save layout: {e:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_bad_values_fall_back() {
        assert_eq!(ShellPrefs::from_value(None), ShellPrefs::default());
        let p = ShellPrefs::from_value(Some(json!({ "sidebarCollapsed": "yes", "dockWidth": 9000 })));
        assert!(!p.collapsed);
        assert_eq!(p.dock_w, DOCK_MAX);
    }

    #[test]
    fn round_trips() {
        let p = ShellPrefs { collapsed: true, dock_w: 452. };
        assert_eq!(ShellPrefs::from_value(Some(p.to_value())), p);
    }
}
