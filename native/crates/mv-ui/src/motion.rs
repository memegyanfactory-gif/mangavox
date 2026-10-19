//! Small looping animations for work in progress. Every frame re-renders the hosting view,
//! so show them only while something is actually running.
use std::time::Duration;

use gpui_kit::*;

use crate::components::icon;
use crate::theme::Tokens;

fn alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a: c.a * a.clamp(0., 1.), ..c }
}

fn smooth(k: f32) -> f32 {
    let k = k.clamp(0., 1.);
    k * k * (3. - 2. * k)
}

/// Rotating loader ring.
pub fn spinner(id: impl Into<ElementId>, color: Rgba, px_size: f32) -> impl IntoElement {
    icon("icons/loader-circle.svg", color, px_size)
        .with_animation(id, Animation::new(Duration::from_millis(800)).repeat(), |s, t| s.with_transformation(Transformation::rotate(percentage(t))))
}

/// A row of dots, `lit` ones solid accent. While `running` a glow travels along the row,
/// like a commit graph lighting up.
pub fn dot_trail(id: impl Into<ElementId>, lit: Vec<bool>, running: bool, dot: f32) -> AnyElement {
    let cell = move |size: f32, color: Rgba| div().size(px(dot + 2.)).flex().items_center().justify_center().child(div().size(px(size)).rounded_full().bg(color));
    let row = div().flex().items_center().gap(px(1.));
    if !running {
        return row.children(lit.iter().map(|&on| cell(dot, if on { Tokens::accent() } else { Tokens::active() }))).into_any_element();
    }
    let n = lit.len() as f32;
    row.with_animation(id, Animation::new(Duration::from_millis(1300)).repeat(), move |d, t| {
        // The wave head runs from before the first dot to past the last one.
        let head = t * (n + 2.) - 1.;
        d.children(lit.iter().enumerate().map(|(i, &on)| {
            let glow = smooth(1. - (i as f32 - head).abs() / 1.6);
            let base = if on { 0.85 } else { 0.16 };
            cell(dot + glow * 2., alpha(Tokens::accent(), base + (1. - base) * glow))
        }))
    })
    .into_any_element()
}

/// Thin progress bar. While `active` a comet of light sweeps the filled part (the whole track
/// before anything is done) so a long step never looks frozen.
pub fn progress_bar(id: impl Into<ElementId>, p: f32, active: bool) -> AnyElement {
    let p = p.clamp(0., 1.);
    let started = p > 0.02;
    let comet = |color: Rgba| {
        div().absolute().top_0().h_full().w(relative(0.35)).with_animation(
            id,
            Animation::new(Duration::from_millis(1700)).repeat().with_easing(ease_in_out),
            move |d, t| d.left(relative(-0.35 + t * 1.35)).bg(linear_gradient(90., linear_color_stop(alpha(color, 0.), 0.), linear_color_stop(color, 1.))),
        )
    };
    let fill = div().absolute().top_0().left_0().h_full().w(relative(p)).rounded_full().overflow_hidden().bg(Tokens::accent());
    let track = div().relative().w_full().h(px(4.)).rounded_full().overflow_hidden().bg(Tokens::active());
    match (active, started) {
        (false, _) => track.child(fill).into_any_element(),
        (true, true) => track.child(fill.child(comet(rgba(0xffffffb0)))).into_any_element(),
        (true, false) => track.child(comet(Tokens::accent())).into_any_element(),
    }
}

/// Overlay for an image being read: a soft accent band scans down it over a faint tint.
pub fn scan_overlay(id: impl Into<ElementId>) -> impl IntoElement {
    let accent = Tokens::accent();
    div().absolute().top_0().left_0().size_full().bg(alpha(accent, 0.05)).child(
        div().absolute().left_0().w_full().h(relative(0.16)).border_b_2().border_color(alpha(accent, 0.9)).with_animation(
            id,
            Animation::new(Duration::from_millis(2400)).repeat().with_easing(ease_in_out),
            move |d, t| d.top(relative(-0.16 + t * 1.16)).bg(linear_gradient(180., linear_color_stop(alpha(accent, 0.), 0.), linear_color_stop(alpha(accent, 0.32), 1.))),
        ),
    )
}

/// A dot that breathes, for "live" labels.
pub fn pulse_dot(id: impl Into<ElementId>, color: Rgba, px_size: f32) -> impl IntoElement {
    div()
        .size(px(px_size))
        .rounded_full()
        .bg(color)
        .with_animation(id, Animation::new(Duration::from_millis(1600)).repeat().with_easing(pulsating_between(0.35, 1.)), |d, a| d.opacity(a))
}
