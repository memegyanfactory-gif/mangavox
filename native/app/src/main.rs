#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod perf;
mod shell;
#[cfg(windows)]
mod tray;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

/// Shared HTTP client + hidden WebView2 for Cloudflare-walled manga sources.
fn init_sources() {
    let wall = match mv_webwall::WebWall::start(mv_webwall::WallConfig::default()) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("[sources] WebView2 unavailable, walled sources disabled: {e}");
            None
        }
    };
    let mut cfg = mv_net::NetConfig::default();
    if let Some(w) = &wall {
        use mv_sources::WallSolver;
        if let Some(ua) = w.user_agent() {
            cfg.user_agent = ua;
        }
    }
    match mv_net::Http::new(cfg) {
        Ok(http) => {
            let mut ctx = mv_sources::SourceCtx::new(http);
            if let Some(w) = wall {
                ctx = ctx.with_solver(w);
            }
            ui_manga::set_source_ctx(ctx);
        }
        Err(e) => eprintln!("[sources] http client failed: {e}"),
    }
}

fn main() {
    // Settings → Apps → Uninstall runs `mangavox.exe --uninstall` (user data in ~/.mangavox stays).
    if std::env::args().any(|a| a == "--uninstall") {
        if let Err(e) = mv_update::uninstall() {
            eprintln!("uninstall: {e:#}");
        }
        return;
    }
    perf::mark_start();
    // Files an update moved aside; the previous process may still be exiting, so wait a bit.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(10));
        mv_update::cleanup();
    });
    init_sources();
    // Remote posters/avatars load through GPUI's image cache, which needs an HTTP client.
    let http = reqwest_client::ReqwestClient::user_agent(mv_net::BROWSER_UA).expect("http client");
    gpui_kit::application()
        .with_assets(mv_ui::Assets)
        .with_http_client(std::sync::Arc::new(http))
        .run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        ui_media::warm_up();
        ui_settings::start_cli_update_checks(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let main = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions { title: Some("MangaVox".into()), ..TitleBar::title_bar_options() }),
                ..TitleBar::window_options()
            },
            cx,
            |window, cx| cx.new(|cx| shell::Shell::new(window, cx)),
        )
        .expect("open window");
        #[cfg(windows)]
        tray::install(main.0, cx);
    });
}
