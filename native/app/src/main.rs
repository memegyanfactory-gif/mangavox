#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod keys;
mod palette;
mod perf;
mod prefs;
mod routes;
mod shell;
#[cfg(windows)]
mod tray;
mod updates;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

/// Shared HTTP client + hidden WebView2 for Cloudflare-walled manga sources.
fn init_sources() {
    let cfg = mv_webwall::WallConfig { profile_dir: mv_core::paths::root().join("webview"), ..Default::default() };
    let wall = match mv_webwall::WebWall::start(cfg) {
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

/// Blocks until process `pid` has exited (at most 60 s). False when it is still running.
fn wait_for_exit(pid: u32) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true;
        }
        let r = WaitForSingleObject(h, 60_000);
        CloseHandle(h);
        r != WAIT_TIMEOUT
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        std::thread::sleep(std::time::Duration::from_secs(2));
        true
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
    // Settings restarts the app with `--after-pid <old pid>`: let the old instance let go of the files first.
    let mut old_running = false;
    if let Some(pid) = std::env::args().skip_while(|a| a != "--after-pid").nth(1).and_then(|p| p.parse::<u32>().ok()) {
        old_running = !wait_for_exit(pid);
    }
    // Before anything opens the data folder: finish a queued move or reset, update old layouts.
    // A move waits while the old instance still holds the files.
    for line in mv_core::paths::prepare(old_running) {
        eprintln!("[data] {line}");
    }
    // First run or not, decided once before anything below creates files in the data folder
    // (`MANGAVOX_WELCOME=1|0` or `--welcome` force it; see mv_core::onboarding).
    let welcome = mv_core::onboarding::init().shows_welcome();
    // Child processes (the Python voice and video scripts) save under the same data folder.
    // Not MANGAVOX_HOME: that would pin the folder for MangaVox itself when it restarts.
    std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());
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
        .run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        // Saved Appearance preset and reduce-motion, mapped onto Tokens and the component theme.
        mv_ui::theme::init(cx);
        // App-wide shortcuts (Ctrl+K palette, Ctrl+B sidebar, Alt+Left/Right...): see keys.rs.
        keys::bind(cx);
        // The Anime and Movies catalogs are fetched ahead only when they are wanted: not while
        // the welcome setup is open (it starts this when Anime/Movies is chosen), and not after a
        // setup that left them out.
        if !welcome && mv_core::onboarding::wants_media(&mv_core::paths::root()) {
            ui_media::warm_up();
        }
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
        // Phone companion: the UI bridge for its server, and the server when Phone access is on.
        ui_phone::start(main.0, cx);
        #[cfg(windows)]
        tray::install(main.0, cx);
    });
}
