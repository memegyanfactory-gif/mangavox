//! The AI switches on the Studio dashboard, both off by default (see `mv_recap::auto`):
//! Light read (pages read by a cheaper model that can see pictures, the story still by the picked
//! model) and Auto (a failing model is replaced by one that passes a one-page test).
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{icon_ph, ph, size, switch, tip, Ph, Tokens};

use crate::page::StudioPage;

const LIGHT_WHAT: &str = "Pages are read by a cheaper model that can see pictures (a free OpenCode vision model first, then Antigravity's Gemini Flash, then Claude Haiku), which writes down every line, who says it, the scene and the panels: no looks or face boxes. The story is still written by the model you picked. The first model that reads a test page well is used.";
const AUTO_WHAT: &str = "When Read or Story fails (an error, an empty answer, a usage limit, a model that can't see pictures), other models are tested on one page of the chapter: the same AI's other models first, then your other signed-in CLIs (Claude Code, Antigravity, OpenCode...). Read only tries models that can see pictures. The first one that passes carries on where the step stopped, and is used for the rest of the session.";

impl StudioPage {
    /// The Light read and Auto switches.
    pub(crate) fn ai_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let light = self.prefs.light_read;
        let auto = self.prefs.auto_switch;
        let light_toggle = switch("light-read", light).on_click(cx.listener(|this, on: &bool, _, cx| {
            this.prefs.light_read = *on;
            mv_recap::auto::set_light(*on);
            cx.notify();
        }));
        let auto_toggle = switch("auto-switch", auto).on_click(cx.listener(|this, on: &bool, _, cx| {
            this.prefs.auto_switch = *on;
            mv_recap::auto::set_auto(*on);
            cx.notify();
        }));
        let any = light || auto;
        div()
            .id("ai-card")
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::MD))
            .border_1()
            .border_color(if any { Tokens::accent_border() } else { Tokens::border() })
            .bg(if any { Tokens::accent_soft() } else { Tokens::surface() })
            .child(row(
                "light",
                ph::TEXT_AA,
                "Light read",
                light,
                if light { "On · a cheaper vision model reads the text, your model writes the story" } else { "Read pages with a cheaper vision model (text and basics only)" },
                LIGHT_WHAT,
                light_toggle.into_any_element(),
            ))
            .child(row(
                "auto",
                ph::SHUFFLE,
                "Auto switch",
                auto,
                if auto { "On · a failing model is replaced by one that passes a test page" } else { "Switch model or AI by itself when a step fails" },
                AUTO_WHAT,
                auto_toggle.into_any_element(),
            ))
            .into_any_element()
    }
}

fn row(key: &'static str, icon: Ph, title: &'static str, on: bool, line: &'static str, what: &'static str, toggle: AnyElement) -> impl IntoElement {
    let head = div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .child(icon_ph(icon, if on { Tokens::accent() } else { Tokens::text_2() }, icon_px::SM))
        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title));
    div()
        .flex()
        .items_center()
        .gap(px(space::MD))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(head)
                .child(div().id(SharedString::from(format!("ai-{key}-line"))).truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).tooltip(tip(what)).child(line)),
        )
        .child(toggle)
}
