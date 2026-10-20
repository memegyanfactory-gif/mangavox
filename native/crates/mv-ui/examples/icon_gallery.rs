//! Visual check for the icon set: every Phosphor icon in all four weights, the weight-by-size rule,
//! the on/off pairs and the brand mark. `cargo run -p mv-ui --example icon_gallery`
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use mv_ui::{icon, icon_ph, ph, ph_icon, size, Ph, Tokens, Weight};

struct Gallery;

fn label(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(text.into())
}

fn section(title: impl Into<SharedString>) -> Div {
    div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.into())
}

/// Icon name from its regular path: `ph/regular/caret-down.svg` is `caret-down`.
fn name(i: Ph) -> &'static str {
    i.regular().trim_start_matches("ph/regular/").trim_end_matches(".svg")
}

/// A pair of the same glyph off and on, as the app shows favourites and saved items.
fn state_pair(i: Ph, on_color: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(icon_ph(i, Tokens::text_3(), 14.))
        .child(icon(i.on(false), Tokens::text_3(), 20.))
        .child(icon(i.on(true), on_color, 20.))
        .child(ph_icon(i, Weight::Duotone, on_color, 20.))
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let weights = [(Weight::Regular, "Regular"), (Weight::Bold, "Bold"), (Weight::Fill, "Fill"), (Weight::Duotone, "Duotone")];

        let brand = div().flex().items_end().gap(px(20.)).children([16., 24., 32., 48., 96.].map(|s| icon("brand/mangavox.svg", Tokens::text_1(), s))).child(icon("brand/mangavox.svg", Tokens::accent(), 48.));

        let sizes = div().flex().items_center().gap(px(24.)).children([13., 14., 16., 20., 24.].map(|s| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(div().flex().gap(px(8.)).child(icon_ph(ph::PLAY, Tokens::text_1(), s)).child(icon_ph(ph::GEAR_SIX, Tokens::text_1(), s)).child(icon_ph(ph::MAGNIFYING_GLASS, Tokens::text_1(), s)))
                .child(label(format!("{s}px (auto weight)")))
        }));

        let states = div()
            .flex()
            .gap(px(32.))
            .child(state_pair(ph::STAR, Tokens::warning()))
            .child(state_pair(ph::HEART, Tokens::danger()))
            .child(state_pair(ph::BOOKMARK_SIMPLE, Tokens::accent()))
            .child(state_pair(ph::CHECK_CIRCLE, Tokens::accent()));

        let shadow = div().flex().items_center().gap(px(14.)).children(
            ["icons/chevron-down.svg", "icons/check.svg", "icons/close.svg", "icons/search.svg", "icons/eye-off.svg", "icons/loader-circle.svg", "icons/star-fill.svg", "icons/window-close.svg"]
                .map(|p| icon(p, Tokens::text_1(), 16.)),
        );

        let named = div().flex().flex_wrap().gap(px(8.)).children(ph::ALL.iter().map(|i| {
            div()
                .w(px(104.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .py(px(10.))
                .rounded(px(8.))
                .bg(Tokens::surface_2())
                .child(icon(i.regular(), Tokens::text_1(), 24.))
                .child(label(name(*i)))
        }));

        let mut root = div()
            .id("gallery")
            .size_full()
            .bg(Tokens::base())
            .overflow_y_scroll()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(section("Brand mark"))
            .child(brand)
            .child(section("Weight by size (icon_ph: bold at 14px and below)"))
            .child(sizes)
            .child(section("Off / on pairs: bold 14, regular 20, fill 20, duotone 20"))
            .child(states)
            .child(section("Widget paths served by Phosphor (window-close stays Lucide)"))
            .child(shadow)
            .child(section(format!("Regular, with names ({} icons)", ph::ALL.len())))
            .child(named);

        for (w, title) in weights {
            root = root.child(section(title)).child(div().flex().flex_wrap().gap(px(14.)).children(ph::ALL.iter().map(move |i| ph_icon(*i, w, Tokens::text_1(), 20.))));
        }
        root
    }
}

fn main() {
    gpui_kit::application().with_assets(mv_ui::Assets).run(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(860.)), cx);
        gpui_kit::open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(TitlebarOptions { title: Some("MangaVox icon gallery".into()), ..Default::default() }), ..Default::default() },
            cx,
            |_, cx| cx.new(|_| Gallery),
        )
        .expect("open window");
    });
}
