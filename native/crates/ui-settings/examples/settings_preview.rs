//! Opens only the Settings page, on a given tab, for visual checks:
//! `cargo run -p ui-settings --example settings_preview -- models:engines`
use gpui_kit::component::{Theme, ThemeMode, TitleBar};
use gpui_kit::*;

struct Preview(AnyView);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn main() {
    let tab = std::env::args().nth(1).unwrap_or_else(|| "models".into());
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("MangaVox Settings preview".into()), ..TitleBar::title_bar_options() }),
                ..TitleBar::window_options()
            },
            cx,
            move |window, cx| {
                let route = mv_ui::Route::Settings { tab: Some(tab.clone()) };
                let view = ui_settings::open(&route, window, cx).expect("settings view");
                cx.new(|_| Preview(view))
            },
        )
        .expect("open window");
    });
}
