#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod activity;
mod keys;
mod palette;
mod panel;
mod perf;
mod prefs;
mod privacy;
mod routes;
mod shell;
#[cfg(windows)]
mod tray;
mod updates;

use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

/// Shared HTTP client + hidden WebView2 for Cloudflare-walled manga sources.
/// WebView2 comes up in the background (it took most of the time before the first frame);
/// reqwest uses the browser's User-Agent from the last run, saved in `webview/ua.txt`, so its
/// cookies replay. The first run (no saved User-Agent yet) still waits for WebView2, so its
/// cookies match from the start; a WebView2 that updated itself since switches reqwest to its
/// new User-Agent as soon as it is up.
fn init_sources() {
    use mv_sources::WallSolver;
    let profile_dir = mv_core::paths::root().join("webview");
    let ua_file = profile_dir.join("ua.txt");
    let saved_ua = std::fs::read_to_string(&ua_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let cfg = mv_webwall::WallConfig { profile_dir, ..Default::default() };
    let http_with = |ua: Option<&String>| {
        let mut cfg = mv_net::NetConfig::default();
        if let Some(ua) = ua {
            cfg.user_agent = ua.clone();
        }
        mv_net::Http::new(cfg)
    };
    let (wall, http) = if saved_ua.is_none() {
        let wall = mv_webwall::WebWall::start(cfg);
        perf::step("webview ready (first run, waited)");
        let ua = wall.as_ref().ok().and_then(|w| w.user_agent());
        if let Some(ua) = &ua {
            save_ua(&ua_file, ua);
        }
        (wall, http_with(ua.as_ref()))
    } else {
        let http = http_with(saved_ua.as_ref());
        let live = http.as_ref().ok().cloned();
        let wall = mv_webwall::WebWall::spawn(cfg, move |r| match r {
            Ok(w) => {
                perf::step("webview ready");
                if let Some(ua) = w.user_agent().filter(|ua| Some(ua) != saved_ua.as_ref()) {
                    if let Some(Err(e)) = live.as_ref().map(|h| h.set_user_agent(&ua)) {
                        eprintln!("[sources] could not switch to the browser's User-Agent: {e}");
                    }
                    save_ua(&ua_file, &ua);
                }
            }
            Err(e) => eprintln!("[sources] WebView2 unavailable, walls cannot be cleared: {e}"),
        });
        (wall, http)
    };
    let wall = match wall {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("[sources] WebView2 unavailable, walled sources disabled: {e}");
            None
        }
    };
    match http {
        Ok(http) => {
            let mut ctx = mv_sources::SourceCtx::new(http);
            if let Some(w) = wall {
                // Also for pages that drive a signed-in site in it (Creative's web image maker).
                mv_webwall::set_global(w.clone());
                ctx = ctx.with_solver(w);
            }
            ui_manga::set_source_ctx(ctx);
        }
        Err(e) => eprintln!("[sources] http client failed: {e}"),
    }
}

/// Saves the browser's User-Agent for the next start (through a temp file, so a crash mid-write
/// never leaves half of one).
fn save_ua(path: &std::path::Path, ua: &str) {
    let tmp = path.with_extension("txt.tmp");
    if std::fs::write(&tmp, ua).is_err() || std::fs::rename(&tmp, path).is_err() {
        eprintln!("[sources] could not save the browser User-Agent to {}", path.display());
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
    // Grok / Antigravity start `mangavox.exe --mcp-bridge` as a stdio MCP server once the user opted
    // in: forward to the running assistant turn, with no window, GPU or data-folder upkeep.
    if std::env::args().nth(1).as_deref() == Some(ui_assistant::mcp_bridge::FLAG) {
        std::process::exit(ui_assistant::mcp_bridge::run());
    }
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
    perf::step("paths ready");
    let welcome = mv_core::onboarding::init().shows_welcome();
    perf::step("onboarding");
    // Child processes (the Python voice and video scripts) save under the same data folder.
    // Not MANGAVOX_HOME: that would pin the folder for MangaVox itself when it restarts.
    std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());
    // Files an update moved aside; the previous process may still be exiting, so wait a bit.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(10));
        mv_update::cleanup();
        // Cover thumbnails: keep cache/thumbs under its size cap.
        mv_ui::thumbs::prune();
    });
    init_sources();
    perf::step("sources");
    // Remote posters/avatars load through GPUI's image cache, which needs an HTTP client.
    let http = reqwest_client::ReqwestClient::user_agent(mv_net::BROWSER_UA).expect("http client");
    gpui_kit::application()
        .with_assets(mv_ui::Assets)
        .with_http_client(std::sync::Arc::new(http))
        .run(move |cx| {
        perf::step("gpui running");
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        // Saved Appearance preset and reduce-motion, mapped onto Tokens and the component theme.
        mv_ui::theme::init(cx);
        // App-wide shortcuts (Ctrl+K palette, Ctrl+B sidebar, Alt+Left/Right...): see keys.rs.
        keys::bind(cx);
        perf::step("theme+keys");
        // The Anime and Movies catalogs are fetched ahead only when they are wanted: not while
        // the welcome setup is open (it starts this when Anime/Movies is chosen), and not after a
        // setup that left them out.
        if !welcome && mv_core::onboarding::wants_media(&mv_core::paths::root()) {
            ui_media::warm_up();
        }
        ui_settings::start_cli_update_checks(cx);
        // Channel refresh and the smart-upload dip automation, when YouTube is connected.
        if mv_ui::mode::studio_enabled(cx) {
            ui_youtube::start(cx);
        }
        perf::step("bg services");
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let main = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions { title: Some("MangaVox".into()), ..TitleBar::title_bar_options() }),
                ..TitleBar::window_options()
            },
            cx,
            |window, cx| {
                // After an update: what changed, once.
                mv_ui::whats_new::check_on_start(window, cx);
                // The app was closed while private mode was on: offer to clear that session.
                privacy::check_on_start(window, cx);
                mv_ui::nav::set_private_toggle(privacy::toggle);
                cx.new(|cx| shell::Shell::new(window, cx))
            },
        )
        .expect("open window");
        perf::step("window opened");
        // Phone companion: the UI bridge for its server, and the server when Phone access is on.
        ui_phone::start(main.0, cx);
        // The assistant's app-wide tools: each area registers its own; they reach GPUI state
        // through the bridge started here.
        ui_assistant::start(main.0, cx);
        ui_manga::register_assistant_tools();
        ui_media::register_assistant_tools();
        ui_studio::register_assistant_tools();
        // The manga section plays Studio's chapter videos: Studio checks first whether any
        // changed since it was rendered (the same check as its own Play and Export).
        ui_manga::recap::set_guard(ui_studio::data::fresh::guard_files);
        #[cfg(windows)]
        tray::install(main.0, cx);
        perf::step("post-window setup");
    });
}
