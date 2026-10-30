//! Timeline arithmetic for the Video mode, kept pure so it can be tested: ruler tick steps, zoom
//! anchored at the pointer, snapping, the bottom navigator, ripple moves and what an edit changed.
//! Ported from bhippi-Video-editor (MIT) `Timeline.tsx` / `editor.ts`.
use crate::vedit::{Clip, Layer, FPS};

/// Zoom bounds and default, in pixels per second.
pub const MIN_PPS: f32 = 0.02;
pub const MAX_PPS: f32 = 2400.;
/// Snapping reach on screen, in pixels.
pub const SNAP_PX: f32 = 10.;
/// Minor ticks between two labelled ticks (and the one on the labelled tick).
pub const MINOR: u32 = 4;
/// The navigator thumb is never narrower than this.
pub const NAV_MIN_THUMB: f32 = 24.;
/// The shortest span the navigator's zoom handles may show, in seconds.
pub const NAV_MIN_SPAN: f32 = 0.05;

/// Seconds between labelled ruler ticks: the first step at least 80 px apart at `pps`.
pub fn tick_step(pps: f32) -> f64 {
    let f = 1. / FPS as f64;
    let steps = [f, 2. * f, 5. * f, 10. * f, 0.5, 1., 2., 5., 10., 15., 30., 60., 120., 300., 600., 900., 1800., 3600., 7200.];
    steps.iter().copied().find(|s| *s * pps as f64 >= 80. - 1e-6).unwrap_or(7200.)
}

/// A ruler label: `m:ss` on whole-second steps, else `m:ss:ff` (frames); hours when needed.
pub fn tick_label(ms: u64, step_s: f64) -> String {
    let s = ms / 1000;
    let base = if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) };
    if step_s < 1. {
        format!("{base}:{:02}", (ms % 1000) * FPS / 1000)
    } else {
        base
    }
}

/// Zoom factor for a wheel turn (`dy` pixels, positive = wheel up = closer), as bhippi:
/// `1.002^dy`, at most 2x a step.
pub fn wheel_factor(dy: f32) -> f32 {
    1.002f32.powf(dy).clamp(0.5, 2.)
}

/// New `(pps, scroll)` after zooming by `k`, keeping the time under `anchor_x` (pixels from the
/// lanes' left edge) where it is.
pub fn zoom_at(pps: f32, scroll: f32, anchor_x: f32, k: f32, min: f32, max: f32) -> (f32, f32) {
    let next = (pps * k).clamp(min, max);
    let t = (scroll + anchor_x) / pps.max(1e-6);
    (next, (t * next - anchor_x).max(0.))
}

/// Reach of a snap, in ms, at `pps`.
pub fn snap_reach(pps: f32) -> u64 {
    (SNAP_PX / pps.max(1e-6) * 1000.) as u64
}

/// Snapping applies: the toggle, inverted while Shift is held.
pub fn snap_active(enabled: bool, shift: bool) -> bool {
    enabled != shift
}

/// `t` pulled onto the nearest target within `reach`: the time and the target hit.
pub fn snap_point(t: i64, targets: &[u64], reach: u64) -> Option<u64> {
    targets.iter().copied().map(|p| (p, (p as i64 - t).unsigned_abs())).filter(|(_, d)| *d <= reach).min_by_key(|(_, d)| *d).map(|(p, _)| p)
}

/// A span `dur` long starting at `start` moved so its nearer edge lands on a target: the new
/// start and the target (for the snap line). Both edges are tried; the closer wins.
pub fn snap_span(start: i64, dur: u64, targets: &[u64], reach: u64) -> Option<(i64, u64)> {
    let head = snap_point(start, targets, reach).map(|p| (p as i64, p, (p as i64 - start).unsigned_abs()));
    let tail = snap_point(start + dur as i64, targets, reach).map(|p| (p as i64 - dur as i64, p, (p as i64 - start - dur as i64).unsigned_abs()));
    match (head, tail) {
        (Some(h), Some(t)) => Some(if t.2 < h.2 { (t.0, t.1) } else { (h.0, h.1) }),
        (Some(h), None) => Some((h.0, h.1)),
        (None, Some(t)) => Some((t.0, t.1)),
        (None, None) => None,
    }
}

/// The navigator's thumb on a `bar_w` bar: (left, width) in pixels.
pub fn nav_thumb(bar_w: f32, content_px: f32, view_w: f32, scroll: f32) -> (f32, f32) {
    let ratio = bar_w / content_px.max(view_w).max(1.);
    let w = (view_w * ratio).clamp(NAV_MIN_THUMB.min(bar_w), bar_w.max(1.));
    ((scroll * ratio).clamp(0., (bar_w - w).max(0.)), w)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavGrip {
    Move,
    Left,
    Right,
}

/// A navigator drag `dx` pixels from where it began: the new `(pps, scroll)`. Moving pans;
/// a handle widens or narrows the shown span (zooming), the other edge staying put.
#[allow(clippy::too_many_arguments)]
pub fn nav_drag(grip: NavGrip, dx: f32, bar_w: f32, total_s: f32, view_w: f32, pps0: f32, scroll0: f32, min: f32) -> (f32, f32) {
    let content = (total_s * pps0).max(view_w).max(1.);
    let ratio = bar_w.max(1.) / content;
    if grip == NavGrip::Move {
        return (pps0, (scroll0 + dx / ratio).clamp(0., (content - view_w).max(0.)));
    }
    let span0 = view_w / pps0;
    let secs_per_px = total_s.max(span0) / bar_w.max(1.);
    let d = dx * secs_per_px;
    let span = if grip == NavGrip::Right { span0 + d } else { span0 - d };
    let pps = (view_w / span.max(NAV_MIN_SPAN)).clamp(min, MAX_PPS);
    let start_s = scroll0 / pps0 + if grip == NavGrip::Left { d } else { 0. };
    (pps, (start_s * pps).max(0.))
}

/// Ripple: layers starting at or after `from_ms` move `by_ms` earlier (what a deleted clip of
/// that length leaves behind).
pub fn ripple_layers(layers: &mut [Layer], from_ms: u64, by_ms: u64) {
    for l in layers.iter_mut().filter(|l| l.start_ms >= from_ms) {
        l.start_ms = l.start_ms.saturating_sub(by_ms);
        l.end_ms = l.end_ms.saturating_sub(by_ms);
    }
}

/// Clips of `after` with no identical clip in `before` (each `before` clip matched once): what an
/// edit added or changed, for the AI highlight.
pub fn changed_clips(before: &[Clip], after: &[Clip]) -> Vec<usize> {
    let mut used = vec![false; before.len()];
    let mut out = vec![];
    for (i, c) in after.iter().enumerate() {
        match before.iter().enumerate().position(|(k, b)| !used[k] && b == c) {
            Some(k) => used[k] = true,
            None => out.push(i),
        }
    }
    out
}

/// Ids of layers that are new or differ.
pub fn changed_layers(before: &[Layer], after: &[Layer]) -> Vec<u64> {
    after.iter().filter(|l| !before.iter().any(|b| b == *l)).map(|l| l.id).collect()
}

/// `AI: "tighten the intro"` (shortened).
pub fn ai_label(prompt: &str) -> String {
    let p = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = p.chars().take(48).collect();
    let more = if p.chars().count() > 48 { "…" } else { "" };
    format!("{}\"{short}{more}\"", crate::vedit_history::AI_PREFIX)
}

/// Seconds left of a job `p` (0..1) done after `elapsed_s`, once there is enough to tell.
pub fn eta(elapsed_s: f32, p: f32) -> Option<f32> {
    (p >= 0.02 && p < 1. && elapsed_s >= 1.).then(|| elapsed_s * (1. - p) / p)
}

/// One decibel step of a volume multiplier (`[` / `]`), kept within 0..2.
pub fn nudge_db(volume: f32, db: f32) -> f32 {
    let v = if volume <= 0.001 && db > 0. { 0.05 } else { volume * 10f32.powf(db / 20.) };
    (v.clamp(0., 2.) * 1000.).round() / 1000.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vedit::LayerKind;

    #[test]
    fn tick_steps_are_at_least_80px() {
        assert!((tick_step(2400.) - 1. / 30.).abs() < 1e-9);
        assert!((tick_step(1200.) - 2. / 30.).abs() < 1e-9);
        assert_eq!(tick_step(160.), 0.5);
        assert_eq!(tick_step(80.), 1.);
        assert_eq!(tick_step(60.), 2.);
        assert_eq!(tick_step(1.), 120.);
        assert_eq!(tick_step(0.001), 7200.);
        for pps in [0.05f32, 0.5, 3., 24., 60., 333., 2400.] {
            let s = tick_step(pps);
            assert!(s * pps as f64 >= 80. - 1e-6 || s == 7200.);
        }
        assert_eq!(tick_label(48_134, 0.5), "0:48:04");
        assert_eq!(tick_label(65_000, 5.), "1:05");
        assert_eq!(tick_label(3_723_000, 60.), "1:02:03");
    }

    #[test]
    fn zoom_keeps_the_time_under_the_pointer() {
        let (pps, scroll) = (60., 300.);
        let anchor = 200.;
        let t = (scroll + anchor) / pps;
        let (p2, s2) = zoom_at(pps, scroll, anchor, 2., MIN_PPS, MAX_PPS);
        assert_eq!(p2, 120.);
        assert!(((s2 + anchor) / p2 - t).abs() < 1e-4);
        // Clamped at the bounds; never scrolls before 0.
        assert_eq!(zoom_at(2000., 0., 10., 4., MIN_PPS, MAX_PPS).0, MAX_PPS);
        assert_eq!(zoom_at(60., 0., 0., 0.5, MIN_PPS, MAX_PPS).1, 0.);
        assert!(wheel_factor(100.) > 1. && wheel_factor(-100.) < 1.);
        assert_eq!(wheel_factor(10_000.), 2.);
    }

    #[test]
    fn snapping_tries_both_edges() {
        let targets = [0, 5000, 9000];
        assert_eq!(snap_point(5080, &targets, 100), Some(5000));
        assert_eq!(snap_point(5200, &targets, 100), None);
        // The tail is closer than the head.
        assert_eq!(snap_span(3950, 5000, &targets, 100), Some((4000, 9000)));
        assert_eq!(snap_span(4990, 1000, &targets, 100), Some((5000, 5000)));
        assert_eq!(snap_span(2000, 1000, &targets, 100), None);
        assert_eq!(snap_reach(100.), 100);
        assert!(snap_active(true, false) && !snap_active(true, true) && snap_active(false, true));
    }

    #[test]
    fn navigator_pans_and_zooms() {
        let (l, w) = nav_thumb(500., 2000., 500., 750.);
        assert_eq!((l, w), (187.5, 125.));
        assert_eq!(nav_thumb(500., 100., 500., 0.), (0., 500.));
        // 100 s at 20 px/s in a 500 px view on a 500 px bar: one bar px is 4 content px.
        let (p, s) = nav_drag(NavGrip::Move, 10., 500., 100., 500., 20., 0., MIN_PPS);
        assert_eq!((p, s), (20., 40.));
        // Dragging the right handle right shows more: zooms out.
        let (p, s) = nav_drag(NavGrip::Right, 50., 500., 100., 500., 20., 0., MIN_PPS);
        assert!(p < 20. && s == 0.);
        // The left handle keeps the right edge.
        let (p, s) = nav_drag(NavGrip::Left, 25., 500., 100., 500., 20., 0., MIN_PPS);
        let right_s = s / p + 500. / p;
        assert!((right_s - 25.).abs() < 1e-3, "{right_s}");
    }

    #[test]
    fn ripple_and_changes() {
        let l = |id, s, e| Layer { id, kind: LayerKind::Text { text: "t".into(), color: "#ffffff".into(), boxed: false }, start_ms: s, end_ms: e, x: 50., y: 50., scale: 0.1, opacity: 1. };
        let mut ls = vec![l(1, 0, 1000), l(2, 5000, 6000)];
        ripple_layers(&mut ls, 4000, 2000);
        assert_eq!((ls[0].start_ms, ls[1].start_ms, ls[1].end_ms), (0, 3000, 4000));

        let before: Vec<Clip> = (0..3).map(Clip::of).collect();
        let mut after = before.clone();
        after[1].motion = crate::vedit::Motion::Shake;
        after.push(Clip::of(0));
        assert_eq!(changed_clips(&before, &after), vec![1, 3]);
        assert!(changed_clips(&before, &before[..2]).is_empty());
        let mut ls2 = ls.clone();
        ls2[1].x = 10.;
        assert_eq!(changed_layers(&ls, &ls2), vec![2]);
    }

    #[test]
    fn labels_eta_and_volume() {
        assert_eq!(ai_label("  tighten   the intro "), "AI: \"tighten the intro\"");
        assert!(ai_label(&"x".repeat(80)).ends_with("…\""));
        assert_eq!(eta(10., 0.5), Some(10.));
        assert_eq!(eta(10., 0.01), None);
        assert_eq!(nudge_db(1., 0.), 1.);
        assert!((nudge_db(1., 1.) - 1.122).abs() < 1e-3);
        assert_eq!(nudge_db(0., 1.), 0.05);
        assert_eq!(nudge_db(2., 1.), 2.);
    }
}
