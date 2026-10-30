//! Phone companion. While "Phone access" is on, a small web server on the local network serves a
//! mobile web app (`web/`) to read the manga library, listen to and download Studio narration and
//! recap videos, start Studio generation, browse Movies and Anime and play them on the PC or
//! stream them to the phone, and mirror and control this window. A phone gets in only by
//! scanning the single-use QR code shown on the Mobile page; every other device on the same
//! Wi-Fi gets the locked page.
//!
//! Layout:
//! - `state`: the shared state (settings, pairing codes, paired devices, server status).
//! - `auth`: pairing, device sessions and the request guard (LAN-only peers, Host check, CSRF).
//! - `server`: binding, the router and start / stop.
//! - `bridge`: runs closures on the GPUI thread for the API modules (navigate, play, ...).
//! - `api_manga`, `api_studio`, `api_media`, `api_anime`, `api_watch`, `api_img`, `screen`: the `/api` routes.
//! - `page`, `qr`: the desktop Mobile page with the QR code.
use gpui_kit::*;
use mv_ui::Route;

pub mod api_anime;
pub mod api_img;
pub mod api_manga;
pub mod api_media;
pub mod api_studio;
pub mod api_watch;
pub mod auth;
pub mod bridge;
pub mod firewall;
pub mod page;
pub mod qr;
pub mod screen;
pub mod server;
pub mod state;

/// Called once at launch with the main window: installs the UI bridge and, when Phone access was
/// left on, starts the server.
pub fn start(window: AnyWindowHandle, cx: &mut App) {
    bridge::install(window, cx);
    if state::settings().enabled {
        server::start();
    }
}

/// The Mobile page for `Route::Mobile`.
pub fn open(route: &Route, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    match route {
        Route::Mobile => Some(cx.new(|cx| page::MobilePage::new(window, cx)).into()),
        _ => None,
    }
}
