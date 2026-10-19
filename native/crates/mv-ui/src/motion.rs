//! Small looping animations for work in progress. Every frame re-renders the hosting view,
//! so show them only while something is actually running. All of them hold still (or slow
//! right down, for the spinner) when [`reduced`] is true.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::*;

use crate::components::icon;
use crate::icons::ph;
use crate::interact::IconSrc;
use crate::theme::Tokens;

static USER_REDUCED: AtomicBool = AtomicBool::new(false);
static SYSTEM_REDUCED: AtomicBool = AtomicBool::new(false);

/// True when animation should be kept to a minimum: Settings → Appearance "Reduce motion" is
/// on, or Windows "Animation effects" is off.
pub fn reduced() -> bool {
    USER_REDUCED.load(Ordering::Relaxed) || SYSTEM_REDUCED.load(Ordering::Relaxed)
}

/// Sets the user's choice, re-reads the Windows setting and redraws every window. Also sets
/// GPUI's own flag, so gpui-component dialogs, tooltips and shimmer text hold still too.
pub fn set_reduced(on: bool, cx: &mut App) {
    USER_REDUCED.store(on, Ordering::Relaxed);
    SYSTEM_REDUCED.store(system_reduced(), Ordering::Relaxed);
    cx.set_reduce_motion(reduced());
    cx.refresh_windows();
}

/// Windows Settings → Accessibility → Visual effects → "Animation effects" is off
/// (`SPI_GETCLIENTAREAANIMATION`). Always false on other platforms.
pub fn system_reduced() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
        let mut on: i32 = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer we pass.
        if unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) } != 0 {
            return on == 0;
        }
    }
    false
}

fn alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a: c.a * a.clamp(0., 1.), ..c }
}

fn smooth(k: f32) -> f32 {
    let k = k.clamp(0., 1.);
    k * k * (3. - 2. * k)
}

/// Rotating loader ring. Under reduced motion it turns slowly (2.4 s a turn) so work still
/// reads as ongoing without a fast spin.
pub fn spinner(id: impl Into<ElementId>, color: Rgba, px_size: f32) -> impl IntoElement {
    let period = if reduced() { 2400 } else { 800 };
    icon(IconSrc::Ph(ph::CIRCLE_NOTCH).for_size(px_size), color, px_size)
        .with_animation(id, Animation::new(Duration::from_millis(period)).repeat(), |s, t| s.with_transformation(Transformation::rotate(percentage(t))))
}

/// A row of dots, `lit` ones solid accent. While `running` a glow travels along the row,
/// like a commit graph lighting up (static under reduced motion).
pub fn dot_trail(id: impl Into<ElementId>, lit: Vec<bool>, running: bool, dot: f32) -> AnyElement {
    let cell = move |size: f32, color: Rgba| div().size(px(dot + 2.)).flex().items_center().justify_center().child(div().size(px(size)).rounded_full().bg(color));
    let row = div().flex().items_center().gap(px(1.));
    if !running || reduced() {
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
/// before anything is done) so a long step never looks frozen. Under reduced motion there is no
/// comet; an active bar with nothing done yet shows a soft accent track instead.
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
    match (active, started, reduced()) {
        (true, false, true) => track.bg(Tokens::accent_soft()).into_any_element(),
        (false, _, _) | (true, true, true) => track.child(fill).into_any_element(),
        (true, true, false) => track.child(fill.child(comet(alpha(Tokens::text_1(), 0.7)))).into_any_element(),
        (true, false, false) => track.child(comet(Tokens::accent())).into_any_element(),
    }
}

/// Overlay for an image being read: a soft accent band scans down it over a faint tint
/// (just the tint under reduced motion).
pub fn scan_overlay(id: impl Into<ElementId>) -> AnyElement {
    let accent = Tokens::accent();
    let tint = div().absolute().top_0().left_0().size_full().bg(alpha(accent, 0.05));
    if reduced() {
        return tint.into_any_element();
    }
    tint.child(
        div().absolute().left_0().w_full().h(relative(0.16)).border_b_2().border_color(alpha(accent, 0.9)).with_animation(
            id,
            Animation::new(Duration::from_millis(2400)).repeat().with_easing(ease_in_out),
            move |d, t| d.top(relative(-0.16 + t * 1.16)).bg(linear_gradient(180., linear_color_stop(alpha(accent, 0.), 0.), linear_color_stop(alpha(accent, 0.32), 1.))),
        ),
    )
    .into_any_element()
}

/// Loading shimmer: a soft band of light sweeping left to right across its parent, which must be
/// `relative()` and `overflow_hidden()`. Every shimmer shares one clock, so a skeleton screen
/// sweeps in step. Nothing under reduced motion (the placeholder stays a calm solid block).
/// NN/g, skeleton screens: <https://www.nngroup.com/articles/skeleton-screens/>
pub fn shimmer() -> AnyElement {
    if reduced() {
        return div().into_any_element();
    }
    let light = Tokens::active();
    let half = |from: Rgba, to: Rgba| div().h_full().flex_1().bg(linear_gradient(90., linear_color_stop(from, 0.), linear_color_stop(to, 1.)));
    div()
        .absolute()
        .top_0()
        .h_full()
        .w(relative(0.6))
        .flex()
        .child(half(alpha(light, 0.), light))
        .child(half(light, alpha(light, 0.)))
        .with_animation("mv-shimmer", Animation::new(Duration::from_millis(1600)).repeat().with_easing(ease_in_out), |d, t| d.left(relative(-0.6 + t * 1.6)))
        .into_any_element()
}

/// Fades an element in over `duration::BASE` with the enter curve; shown at once under reduced
/// motion. For popovers, menus and panels that appear in place.
/// Material 3 motion: <https://m3.material.io/styles/motion/overview>
pub fn appear<E: IntoElement + Styled + 'static>(id: impl Into<ElementId>, el: E) -> AnyElement {
    if reduced() {
        return el.into_any_element();
    }
    el.with_animation(id, Animation::new(crate::theme::duration::BASE).with_easing(crate::theme::easing::enter), |e, t| e.opacity(t)).into_any_element()
}

/// A dot that breathes, for "live" labels (solid under reduced motion).
pub fn pulse_dot(id: impl Into<ElementId>, color: Rgba, px_size: f32) -> AnyElement {
    let dot = div().size(px(px_size)).rounded_full().bg(color);
    if reduced() {
        return dot.into_any_element();
    }
    dot.with_animation(id, Animation::new(Duration::from_millis(1600)).repeat().with_easing(pulsating_between(0.35, 1.)), |d, a| d.opacity(a))
        .into_any_element()
}
