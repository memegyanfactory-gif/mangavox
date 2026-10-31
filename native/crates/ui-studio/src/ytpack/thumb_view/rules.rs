//! The generator's "Thumbnail rules" tab: what the click checklist learned from real recap
//! channels (do, what to write, avoid, titles), each rule with its evidence and example videos,
//! how far to trust it, and every current candidate (AI ideas and creations) ranked by score.
use super::click::{Example, Rule};
use super::*;

const EX_W: f32 = 128.;

/// "152x", "2.3x", "0.04x": views over the channel's usual.
fn times(x: f32) -> String {
    if x >= 10. {
        format!("{x:.0}x")
    } else if x >= 1. {
        format!("{x:.1}x")
    } else {
        format!("{x:.2}x")
    }
}

/// A small YouTube thumbnail with its "×2.3" over expected; opens the video.
fn example_tile(id: impl Into<ElementId>, e: &Example) -> impl IntoElement {
    let url = e.url();
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(px(3.))
        .w(px(EX_W))
        .cursor_pointer()
        .tooltip(tip(format!("{}\n{} · {} views · {} its channel's usual{}", e.title, e.channel, short_count(e.views), times(e.x), if e.text.is_empty() { String::new() } else { format!("\nText: {}", e.text) })))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .child(
            div()
                .relative()
                .w(px(EX_W))
                .h(px(EX_W * 9. / 16.))
                .rounded(px(radius::MD))
                .overflow_hidden()
                .bg(Tokens::surface_2())
                .child(img(SharedString::from(e.thumb())).size_full().object_fit(ObjectFit::Cover))
                .child(div().absolute().right(px(4.)).bottom(px(4.)).child(badge(times(e.x), if e.x >= 1. { Tone::Success } else { Tone::Danger }))),
        )
        .child(div().text_size(px(10.)).text_color(Tokens::text_3()).truncate().child(e.channel.clone()))
}

fn chip(text: impl Into<SharedString>) -> Div {
    div().flex_none().px(px(6.)).h(px(20.)).flex().items_center().rounded(px(radius::SM)).bg(Tokens::surface_3()).text_size(px(size::XS)).text_color(Tokens::text_2()).child(text.into())
}

fn rule_card(id: &'static str, k: usize, r: &Rule, icon: Ph, color: Rgba) -> Div {
    let mut chips = div().flex().flex_wrap().gap(px(4.));
    if r.big_share > 0. || r.small_share > 0. {
        chips = chips.child(chip(format!("big channels {:.0}% · small {:.0}%", r.big_share * 100., r.small_share * 100.)));
    }
    if r.channels >= 3 {
        chips = chips.child(chip(format!("held in {} of {} channels", r.held, r.channels)));
    }
    if r.lift > 0. {
        chips = chips.child(chip(format!("{:.2}x views", r.lift)));
    }
    chips = chips.child(chip(format!("{} examples", r.n)));
    div()
        .flex()
        .gap(px(space::MD))
        .p(px(space::MD))
        .rounded(px(radius::LG))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border())
        .child(div().flex_none().pt(px(2.)).child(ph_icon(icon, Weight::Fill, color, icon_px::SM)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(200.))
                .gap(px(6.))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).whitespace_normal().child(r.text.clone()))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(r.why.clone()))
                .child(chips),
        )
        .child(div().flex().flex_none().gap(px(space::SM)).children(r.examples.iter().take(2).enumerate().map(|(i, e)| example_tile(SharedString::from(format!("{id}-{k}-{i}")), e))))
}

fn section(icon: Ph, color: Rgba, title: &str, sub: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(space::SM))
        .child(div().flex().items_center().gap(px(space::SM)).child(ph_icon(icon, Weight::Duotone, color, icon_px::MD)).child(div().text_size(px(size::MD)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(title.to_string())))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(sub.to_string()))
}

impl ThumbGen {
    pub(super) fn rules_tab(&self, cx: &mut Context<Self>) -> Div {
        guide(Some(self.candidates(cx)))
    }
}

/// The rules guide, with `candidates` (this manga's ranked ideas and creations) after the intro.
/// The Thumbnails page shows it without candidates before there is any manga.
pub(in crate::ytpack) fn guide(candidates: Option<Div>) -> Div {
    let m = click::model();
    let e = &m.eval;
    let intro = div()
        .flex()
        .flex_col()
        .gap(px(space::SM))
        .p(px(space::MD))
        .rounded(px(radius::LG))
        .bg(Tokens::accent_soft())
        .child(div().flex().items_center().gap(px(space::SM)).child(ph_icon(ph::CHART_LINE_UP, Weight::Duotone, Tokens::accent(), icon_px::SM)).child(div().text_size(px(size::SM)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child("What wins clicks on recap thumbnails")))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_1()).whitespace_normal().child(format!(
            "Measured on {} labelled thumbnails ({} videos) from {} manhwa / manga recap channels, trained {}. Each video is compared with what its own channel usually gets at that age, and the biggest channels ({}) are compared with the smallest. The AI's ideas and prompts follow these rules, and every candidate gets a score: the share of the studied thumbnails it beats on this checklist.",
            m.data.labelled,
            m.data.videos,
            m.data.channels,
            m.trained,
            m.big_channels.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
        )))
        .child(div().flex().items_start().gap(px(6.)).child(icon_ph(ph::INFO, Tokens::text_2(), icon_px::XS)).child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(format!(
            "How far to trust it: tested on channels it never saw, the score lines up with how big a channel is (rank correlation {:+.2} over {} channels), but it does not tell which of one channel's own videos will win ({:+.2}, the same as chance {:+.2} ± {:.2}). Within a channel the title, the premise and the length matter more: titles + length predict it ({:+.2} vs chance {:+.2} ± {:.2}). So use the checklist to look like the channels that grow, and spend the most care on the title and the story's hook.",
            e.between, e.between_channels, e.within, e.within_shuffled, e.within_sd, e.titles, e.titles_shuffled, e.titles_sd
        ))));

    let mut out = div().flex().flex_col().gap(px(space::MD)).child(intro).children(candidates);

    out = out.child(section(ph::CHECK_CIRCLE, Tokens::success(), "Do", "What the growing channels do more, and what lifted views within channels."));
    for (k, r) in m.rules_of("do").enumerate() {
        out = out.child(rule_card("thr-do", k, r, ph::CHECK_CIRCLE, Tokens::success()));
    }

    out = out.child(section(ph::TEXT_T, Tokens::accent(), "What to write", "The words on the thumbnail: style and patterns, then the words on the top outperformers."));
    for (k, r) in m.rules_of("write").enumerate() {
        out = out.child(rule_card("thr-write", k, r, ph::TEXT_T, Tokens::accent()));
    }
    let words = |list: &[Example], good: bool, id: &'static str| {
        div().flex().flex_wrap().gap(px(6.)).children(list.iter().filter(|e| !e.text.trim().is_empty()).take(18).enumerate().map(move |(i, e)| {
            let url = e.url();
            div()
                .id((id, i))
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .h(px(26.))
                .rounded(px(radius::MD))
                .bg(if good { Tokens::success_soft() } else { Tokens::danger_soft() })
                .cursor_pointer()
                .tooltip(tip(format!("{} · {} · {} its channel's usual", e.title, e.channel, times(e.x))))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(e.text.chars().take(48).collect::<String>()))
                .child(div().text_size(px(10.)).text_color(Tokens::text_3()).child(times(e.x)))
        }))
    };
    out = out
        .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("Words on the biggest outperformers"))
        .child(words(&m.text_wins, true, "thr-win"))
        .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("Words on the weakest videos"))
        .child(words(&m.text_losses, false, "thr-loss"));

    out = out.child(section(ph::PROHIBIT, Tokens::danger(), "Avoid", "What small channels lean on, or what lost views within channels. Use one only when it is the story's hook."));
    for (k, r) in m.rules_of("avoid").enumerate() {
        out = out.child(rule_card("thr-avoid", k, r, ph::PROHIBIT, Tokens::danger()));
    }

    out = out.child(section(ph::YOUTUBE_LOGO, Tokens::danger(), "Titles", "Measured on long recaps (1 hour and more): views against the channel's usual, with the pattern and without."));
    for (k, r) in m.title_rules.iter().enumerate() {
        let (icon, color) = if r.kind == "do" { (ph::CHECK_CIRCLE, Tokens::success()) } else { (ph::PROHIBIT, Tokens::danger()) };
        out = out.child(rule_card("thr-title", k, r, icon, color));
    }

    out = out.child(section(ph::TROPHY, Tokens::warning(), "Proven hits", "The biggest outperformers in the data, all-time and from the last 45 days. Click one to open it on YouTube."));
    let tiles = |list: &[Example], id: &'static str| div().flex().flex_wrap().gap(px(space::MD)).children(list.iter().enumerate().map(move |(i, e)| example_tile(SharedString::from(format!("{id}-{i}")), e)));
    out.child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("All-time")).child(tiles(&m.hits, "thr-hit")).child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child("Recent")).child(tiles(&m.recent, "thr-recent"))
}

impl ThumbGen {
    /// Every current candidate, best score first: AI ideas (from the prompt) and creations
    /// (prompt + image).
    fn candidates(&self, cx: &mut Context<Self>) -> Div {
        let m = click::model();
        enum Cand {
            Idea(usize),
            Made(usize),
        }
        let mut list: Vec<(Cand, String, Score)> = vec![];
        for (i, c) in self.th.gallery.concepts.iter().enumerate() {
            list.push((Cand::Idea(i), c.name.clone(), m.score_idea(&c.prompt, &c.text)));
        }
        for (i, t) in self.th.gallery.thumbs.iter().enumerate() {
            if let Some(s) = self.th.scores.get(&t.id) {
                list.push((Cand::Made(i), t.label.clone(), s.clone()));
            }
        }
        list.sort_by_key(|(_, _, s)| std::cmp::Reverse(s.value));
        let head = section(ph::SORT_DESCENDING, Tokens::accent(), "Your candidates", "This manga's AI ideas and creations, best score first, with what helps and what hurts.");
        if list.is_empty() {
            return div().flex().flex_col().gap(px(space::SM)).child(head).child(muted("No candidates yet: press Design with AI, or generate a thumbnail."));
        }
        let mut col = div().flex().flex_col().gap(px(space::SM)).child(head);
        for (k, (cand, name, s)) in list.into_iter().take(10).enumerate() {
            let (preview, kind, action): (AnyElement, &str, AnyElement) = match cand {
                Cand::Idea(i) => {
                    let prompt = self.th.gallery.concepts[i].prompt.clone();
                    (
                        div().flex().items_center().justify_center().size_full().child(ph_icon(ph::LIGHTBULB, Weight::Duotone, Tokens::text_3(), 22.)).into_any_element(),
                        "AI idea",
                        btn(("thr-use", k), "Use prompt", ButtonKind::Ghost).small().icon(ph::TEXT_T).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_prompt(prompt.clone(), window, cx))).into_any_element(),
                    )
                }
                Cand::Made(i) => {
                    let id = self.th.gallery.thumbs[i].id.clone();
                    (
                        img(Gallery::image(&self.manga_id, &id)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                        "Creation",
                        btn(("thr-open", k), "Open", ButtonKind::Ghost)
                            .small()
                            .icon(ph::ARROW_SQUARE_OUT)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.th.open = Some(id.clone());
                                this.th.tab = Tab::Creations;
                                cx.notify();
                            }))
                            .into_any_element(),
                    )
                }
            };
            col = col.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::MD))
                    .p(px(space::SM))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(Tokens::border())
                    .child(div().flex_none().w(px(EX_W)).h(px(EX_W * 9. / 16.)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::surface_2()).child(preview))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(div().flex().items_center().gap(px(space::SM)).child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(name)).child(badge(kind, Tone::Neutral)))
                            .child(score_detail(SharedString::from(format!("thr-cand-score-{k}")), &s)),
                    )
                    .child(div().flex_none().child(action)),
            );
        }
        col
    }
}
