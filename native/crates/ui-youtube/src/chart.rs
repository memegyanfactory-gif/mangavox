//! Small charts drawn with plain elements: a bar chart for daily numbers (hover shows the day
//! and value), a sparkline for table rows, KPI tiles and the trend chip.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{icon_ph, ph, size, tip, Ph, Tokens};
use mv_youtube::Trend;

/// Daily bars, oldest left. `labels[i]` and `values[i]` go into the hover tip with `unit`.
pub fn bars(id: &'static str, values: &[f64], labels: &[String], unit: &str, color: Rgba, height: f32) -> Div {
    let max = values.iter().copied().fold(0f64, f64::max).max(1.);
    let total: f64 = values.iter().sum();
    div()
        .flex()
        .flex_col()
        .gap(px(space::XS))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(2.))
                .h(px(height))
                .px(px(space::XS))
                .border_b_1()
                .border_color(Tokens::border())
                .children(values.iter().enumerate().map(|(i, v)| {
                    let h = ((v / max) as f32 * (height - 4.)).max(if *v > 0. { 2. } else { 1. });
                    let label = labels.get(i).cloned().unwrap_or_default();
                    let text = format!("{label}: {} {unit}", fmt_value(*v));
                    div()
                        .id((id, i))
                        .flex_1()
                        .min_w(px(2.))
                        .h(px(h))
                        .rounded_t(px(2.))
                        .bg(if *v > 0. { color } else { Tokens::track() })
                        .opacity(0.85)
                        .hover(|s| s.opacity(1.))
                        .tooltip(tip(text))
                })),
        )
        .when(total == 0., |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("No data for this period yet")))
}

/// Tiny bars for a table row (views per day since release).
pub fn sparkline(values: &[u64], color: Rgba) -> Div {
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f32;
    div().flex().items_end().gap(px(1.)).h(px(22.)).w(px(72.)).children(values.iter().rev().take(14).rev().map(|v| {
        let h = (*v as f32 / max * 20.).max(1.);
        div().flex_1().h(px(h)).rounded_t(px(1.)).bg(if *v > 0 { color } else { Tokens::track() })
    }))
}

/// A KPI tile: label, big number, and a small change line ("+12% vs previous 28 days").
pub fn kpi(icon: Ph, label: &str, value: String, change: Option<f64>) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(150.))
        .gap(px(space::XS))
        .p(px(space::LG))
        .rounded(px(radius::LG))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border())
        .child(div().flex().items_center().gap(px(space::XS)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::SM)).child(label.to_string()))
        .child(div().text_size(px(size::XXL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(value))
        .children(change.map(|c| {
            let (color, icon) = if c >= 0. { (Tokens::success(), ph::TREND_UP) } else { (Tokens::danger(), ph::TREND_DOWN) };
            div().flex().items_center().gap(px(space::XS)).text_size(px(size::XS)).text_color(color).child(icon_ph(icon, color, icon_px::XS)).child(format!("{:+.0}% vs previous period", c * 100.))
        }))
}

/// Rising / Steady / Dipping as a coloured chip with an icon (never colour alone).
pub fn trend_chip(t: Trend) -> Div {
    let (label, icon, fg, bg) = match t {
        Trend::TooNew => ("New".to_string(), ph::SPARKLE, Tokens::info(), Tokens::info_soft()),
        Trend::Rising => ("Rising".to_string(), ph::TREND_UP, Tokens::success(), Tokens::success_soft()),
        Trend::Steady => ("Steady".to_string(), ph::ARROW_RIGHT, Tokens::text_2(), Tokens::hover()),
        Trend::Dipping { drop } => (format!("Dipping {:.0}%", drop * 100.), ph::TREND_DOWN, Tokens::danger(), Tokens::danger_soft()),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(radius::XS))
        .bg(bg)
        .text_size(px(size::XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(icon_ph(icon, fg, icon_px::XS))
        .child(label)
}

/// Change from the previous period of the same length (`None` when there is no earlier data).
pub fn change(values: &[f64], days: usize) -> Option<f64> {
    if values.len() < days * 2 {
        return None;
    }
    let n = values.len();
    let cur: f64 = values[n - days..].iter().sum();
    let prev: f64 = values[n - 2 * days..n - days].iter().sum();
    (prev > 0.).then(|| (cur - prev) / prev)
}

fn fmt_value(v: f64) -> String {
    if v.fract() == 0. {
        crate::hub::fmt_num(v as u64)
    } else {
        format!("{v:.1}")
    }
}
