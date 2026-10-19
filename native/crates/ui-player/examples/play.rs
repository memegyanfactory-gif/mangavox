//! Dev harness: opens the player directly in a window shaped like the app's.
//! `cargo run -p ui-player --example play -- [url|magnet|file] [title] [imdb id]`
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;
use mv_ui::nav::{PlayRequest, Route};

const SAMPLE: &str = "https://download.blender.org/durian/trailer/sintel_trailer-1080p.mp4";

struct Host {
    page: AnyView,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().bg(mv_ui::Tokens::base()).child(TitleBar::new().child("MangaVox")).child(div().flex_1().min_h_0().child(self.page.clone()))
    }
}

fn main() {
    let arg = |i: usize, default: &str| std::env::args().nth(i).unwrap_or_else(|| default.to_string());
    let (source, title, imdb) = (arg(1, SAMPLE), arg(2, "Sintel"), arg(3, "tt1727587"));
    let http = reqwest_client::ReqwestClient::user_agent("Mozilla/5.0 MangaVox-dev").expect("http client");
    gpui_kit::application().with_assets(mv_ui::Assets).with_http_client(std::sync::Arc::new(http)).run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        mv_ui::theme::init(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("MangaVox player (dev)".into()), ..TitleBar::title_bar_options() }),
                ..TitleBar::window_options()
            },
            cx,
            move |window, cx| {
                let request = PlayRequest {
                    title: title.clone(),
                    source: source.clone(),
                    file_idx: None,
                    imdb_id: Some(imdb.clone()).filter(|s| !s.is_empty()),
                    mal_id: None,
                    season: None,
                    episode: None,
                };
                let page = ui_player::open(&Route::Player { request }, window, cx).expect("player route");
                cx.new(|_| Host { page })
            },
        )
        .expect("open window");
    });
}
