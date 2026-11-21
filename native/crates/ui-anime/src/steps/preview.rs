//! The in-page animatic preview of the Cut step (design 9.11) [cut]: the current shot's still
//! moved by `mv_anime::animatic::camera_at` every frame (the same curve the export gives
//! ffmpeg, so the preview matches the MP4), cut on time, with the episode's voices played from
//! the mixed dialogue track (`ui_studio::audio`). W3-S5.
//!
//! Everything here is pure or draws from what it is given: which shot plays at a time, where
//! the still sits in the view, the subtitle at a time, the play clock, the strip's tile widths
//! and the tail of a WAV file (the audio player cannot seek, so playing from the middle plays a
//! copy that starts there).

use std::path::Path;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::animatic::{camera_at, CutShot};
use mv_anime::Move;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::thumbs::display_source;
use mv_ui::{ph, ph_icon, size, Tokens, Weight};

use crate::style::{HISTORY_THUMB, TILE_W};

/// Strip tiles grow with their shot's length: this many pixels a second...
const STRIP_PX_PER_S: f32 = 40.;
/// ...between a history thumbnail's width and a gallery tile's.
const STRIP_MIN_W: f32 = HISTORY_THUMB.0;
const STRIP_MAX_W: f32 = TILE_W;

// ---------------------------------------------------------------- timing

/// The shot playing at `t` seconds into the animatic and the time inside it. Shots follow each
/// other (`start_s` grows); a time before the first shot is its start, one after the last is its
/// end. `None` when there are no shots.
pub fn shot_at(shots: &[CutShot], t: f32) -> Option<(usize, f32)> {
    if shots.is_empty() {
        return None;
    }
    let t = if t.is_finite() { t.max(0.) } else { 0. };
    // A hair of slack so a time exactly on a cut belongs to the shot that starts there.
    let i = shots.iter().rposition(|s| s.start_s <= t + 1e-4).unwrap_or(0);
    let s = &shots[i];
    let len = if s.seconds.is_finite() { s.seconds.max(0.) } else { 0. };
    Some((i, (t - s.start_s).clamp(0., len)))
}

/// Where the still sits in the view for a camera of `(zoom, x, y)` (`camera_at`), as shares of the
/// view: (left, top, width, height). The still is enlarged by `zoom` and shifted so the window
/// whose top-left corner is (`x`, `y`) of the still fills the view.
pub fn still_box(zoom: f32, x: f32, y: f32) -> (f32, f32, f32, f32) {
    let z = if zoom.is_finite() && zoom >= 1. { zoom } else { 1. };
    let x = if x.is_finite() { x } else { 0. };
    let y = if y.is_finite() { y } else { 0. };
    (-x * z, -y * z, z, z)
}

/// [`still_box`] of `shot`'s camera `local` seconds into it.
pub fn box_at(shot: &CutShot, local: f32) -> (f32, f32, f32, f32) {
    let (z, x, y) = camera_at(shot, local);
    still_box(z, x, y)
}

/// The subtitle on screen `local` seconds into `shot`.
pub fn caption_at(shot: &CutShot, local: f32) -> Option<&str> {
    shot.subs.iter().find(|(a, b, _)| local >= *a && local < *b).map(|(_, _, t)| t.trim()).filter(|t| !t.is_empty())
}

/// The time under the pointer at `x` on a bar starting at `left`, `width` wide, of an animatic
/// `total` seconds long.
pub fn time_at(x: f32, left: f32, width: f32, total: f32) -> f32 {
    if !(width > 0. && total > 0. && x.is_finite()) {
        return 0.;
    }
    ((x - left) / width).clamp(0., 1.) * total
}

/// The share of the animatic played at `t` (for the bar).
pub fn share(t: f32, total: f32) -> f32 {
    if total > 0. && t.is_finite() {
        (t / total).clamp(0., 1.)
    } else {
        0.
    }
}

/// A camera move in plain words.
pub fn move_label(m: Move) -> &'static str {
    match m {
        Move::Hold => "Still",
        Move::PushIn => "Push in",
        Move::PullOut => "Pull out",
        Move::PanLeft => "Pan left",
        Move::PanRight => "Pan right",
        Move::TiltUp => "Tilt up",
        Move::TiltDown => "Tilt down",
        Move::TruckLeft => "Track left",
        Move::TruckRight => "Track right",
        Move::Shake => "Shake",
        Move::Whip => "Whip pan",
        Move::CrashZoom => "Crash zoom",
        Move::RackFocus => "Focus pull",
        Move::BgSlide => "Background slide",
        Move::Unknown => "Camera move",
    }
}

/// Width of a shot's tile in the strip: longer shots get wider tiles, within bounds.
pub fn tile_width(seconds: f32) -> f32 {
    let s = if seconds.is_finite() { seconds.max(0.) } else { 0. };
    (s * STRIP_PX_PER_S).clamp(STRIP_MIN_W, STRIP_MAX_W)
}

// ---------------------------------------------------------------- the clock

/// The preview's play clock: a position that runs while `since` is set.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    base: f32,
    since: Option<Instant>,
}

impl Clock {
    /// Seconds into the animatic now.
    pub fn now(&self) -> f32 {
        position(self.base, self.since.map(|s| s.elapsed().as_secs_f32()))
    }

    /// Jumps to `t` (keeps running if it was).
    pub fn set(&mut self, t: f32) {
        self.base = if t.is_finite() { t.max(0.) } else { 0. };
        if self.since.is_some() {
            self.since = Some(Instant::now());
        }
    }

    /// Starts counting from the position it holds.
    pub fn run(&mut self) {
        self.since = Some(Instant::now());
    }

    /// Stops counting and keeps the position.
    pub fn hold(&mut self) {
        self.base = self.now();
        self.since = None;
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }
}

/// The position of a clock held at `base` that has run for `elapsed` seconds.
pub fn position(base: f32, elapsed: Option<f32>) -> f32 {
    let e = elapsed.filter(|e| e.is_finite() && *e > 0.).unwrap_or(0.);
    (if base.is_finite() { base.max(0.) } else { 0. }) + e
}

// ---------------------------------------------------------------- audio from the middle

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// The part of a PCM WAV file from `from_s` seconds on, as a WAV file of the same format.
/// `None` when the bytes are not a WAV this can read.
pub fn wav_tail(bytes: &[u8], from_s: f32) -> Option<Vec<u8>> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut data: Option<(usize, usize)> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(bytes, at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            fmt = Some((u16_at(bytes, body)?, u16_at(bytes, body + 2)?, u32_at(bytes, body + 4)?, u16_at(bytes, body + 12)?, u16_at(bytes, body + 14)?));
        } else if id == b"data" {
            let end = body.saturating_add(len).min(bytes.len());
            data = Some((body, end));
            break;
        }
        // Chunks are padded to an even length.
        at = body.checked_add(len)?.checked_add(len % 2)?;
    }
    let (tag, channels, rate, align, bits) = fmt?;
    let (start, end) = data?;
    if align == 0 || rate == 0 {
        return None;
    }
    let align = align as usize;
    let from = if from_s.is_finite() { from_s.max(0.) } else { 0. };
    let skip = ((from as f64 * rate as f64).floor() as usize).saturating_mul(align);
    let whole = (end - start) / align * align;
    let body = &bytes[start + skip.min(whole)..start + whole];
    // An "extensible" header is written back as the plain one it stands for.
    let tag = if tag == 0xFFFE { 1 } else { tag };
    let mut out = Vec::with_capacity(44 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * align as u32).to_le_bytes());
    out.extend_from_slice(&(align as u16).to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    Some(out)
}

// ---------------------------------------------------------------- drawing

/// What the stage shows at one moment.
pub struct Moment<'a> {
    pub shot: &'a CutShot,
    /// Seconds into the shot.
    pub local: f32,
    /// The shot's still, when it has one.
    pub still: Option<&'a Path>,
    /// "3.4".
    pub number: &'a str,
    /// The next shot's still, loaded out of sight so the cut never waits for it.
    pub next: Option<&'a Path>,
    /// The subtitle to show, when subtitles are on.
    pub caption: Option<&'a str>,
}

/// The 16:9 picture of the preview: the still moved by its camera, or a card for a shot with no
/// picture yet, and the subtitle. Fills its parent; give it a 16:9 box.
pub fn stage(m: Option<Moment<'_>>) -> Div {
    let base = div().relative().size_full().overflow_hidden().bg(Tokens::well());
    let Some(m) = m else { return base };
    base.map(|d| match m.still {
        Some(p) => {
            let (l, t, w, h) = box_at(m.shot, m.local);
            d.child(img(display_source(p.to_path_buf())).absolute().left(relative(l)).top(relative(t)).w(relative(w)).h(relative(h)).object_fit(ObjectFit::Cover))
        }
        None => d.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::SM))
                .child(ph_icon(ph::IMAGE, Weight::Duotone, Tokens::text_3(), icon_px::HERO))
                .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(format!("Shot {} has no picture yet", m.number))),
        ),
    })
    .when_some(m.next, |d, p| d.child(img(display_source(p.to_path_buf())).absolute().top_0().left_0().size(px(1.)).opacity(0.)))
    .when_some(m.caption, |d, c| {
        d.child(
            div().absolute().left_0().right_0().bottom(px(space::XL)).flex().justify_center().px(px(space::XXL)).child(
                div()
                    .max_w(relative(0.8))
                    .px(px(space::MD))
                    .py(px(space::XS))
                    .rounded(px(radius::SM))
                    .bg(Tokens::canvas().alpha(0.78))
                    .text_size(px(size::LG))
                    .line_height(px(size::LG + space::SM))
                    .text_color(Tokens::text_1())
                    .text_center()
                    .whitespace_normal()
                    .child(c.to_string()),
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{caption_at, move_label, position, share, shot_at, still_box, tile_width, time_at, wav_tail, STRIP_MAX_W, STRIP_MIN_W};
    use mv_anime::animatic::{camera_at, CutShot};
    use mv_anime::Move;

    fn shot(start: f32, secs: f32, mv: Move) -> CutShot {
        CutShot { shot: format!("sh{start}"), start_s: start, seconds: secs, mv, zoom: 0.12, travel: 0.2, ..Default::default() }
    }

    fn reel() -> Vec<CutShot> {
        vec![shot(0., 2., Move::PushIn), shot(2., 3., Move::PanRight), shot(5., 1.5, Move::Hold)]
    }

    #[test]
    fn the_shot_playing_at_a_time() {
        let r = reel();
        assert_eq!(shot_at(&r, 0.), Some((0, 0.)));
        assert_eq!(shot_at(&r, 1.25), Some((0, 1.25)));
        // A time exactly on a cut belongs to the shot that starts there.
        assert_eq!(shot_at(&r, 2.), Some((1, 0.)));
        let (i, local) = shot_at(&r, 4.5).unwrap_or_default();
        assert_eq!(i, 1);
        assert!((local - 2.5).abs() < 1e-5);
        assert_eq!(shot_at(&r, 5.2).map(|x| x.0), Some(2));
        // Past the end: the last shot, held on its last moment.
        assert_eq!(shot_at(&r, 60.), Some((2, 1.5)));
        // Before the start or not a number: the very start.
        assert_eq!(shot_at(&r, -3.), Some((0, 0.)));
        assert_eq!(shot_at(&r, f32::NAN), Some((0, 0.)));
        assert_eq!(shot_at(&[], 1.), None);
    }

    #[test]
    fn the_still_box_follows_the_engine_camera() {
        // No zoom: the still fills the view exactly.
        assert_eq!(still_box(1., 0., 0.), (0., 0., 1., 1.));
        // Zoomed in on the middle: enlarged and shifted so the centre stays centred.
        let (l, t, w, h) = still_box(1.2, (1. - 1. / 1.2) / 2., (1. - 1. / 1.2) / 2.);
        assert!((w - 1.2).abs() < 1e-6 && (h - 1.2).abs() < 1e-6);
        assert!((l + 0.1).abs() < 1e-5 && (t + 0.1).abs() < 1e-5, "{l} {t}");
        // Nonsense never makes the still smaller than the view.
        assert_eq!(still_box(f32::NAN, f32::NAN, 0.), (0., 0., 1., 1.));
        assert_eq!(still_box(0.5, 0., 0.).2, 1.);
        // The view always stays covered: the box never leaves a gap at an edge.
        for s in reel() {
            for k in 0..=20 {
                let local = s.seconds * k as f32 / 20.;
                let (z, x, y) = camera_at(&s, local);
                let (l, t, w, h) = still_box(z, x, y);
                assert!(l <= 1e-5 && t <= 1e-5, "{:?} at {local}: starts inside the view", s.mv);
                assert!(l + w >= 1. - 1e-4 && t + h >= 1. - 1e-4, "{:?} at {local}: ends inside the view", s.mv);
            }
        }
    }

    #[test]
    fn a_push_grows_and_a_pan_slides() {
        let r = reel();
        let start = super::box_at(&r[0], 0.);
        let end = super::box_at(&r[0], 2.);
        assert!(end.2 > start.2, "a push in ends bigger than it starts");
        let a = super::box_at(&r[1], 0.);
        let b = super::box_at(&r[1], 3.);
        assert!(b.0 < a.0, "a pan right moves the picture left");
        assert!((a.2 - b.2).abs() < 1e-5, "a pan keeps its size");
    }

    #[test]
    fn captions_show_while_their_line_is_said() {
        let mut s = shot(0., 4., Move::Hold);
        s.subs = vec![(0.9, 2.1, "It moved.".into()), (2.4, 3.2, "  ".into())];
        assert_eq!(caption_at(&s, 0.5), None);
        assert_eq!(caption_at(&s, 0.9), Some("It moved."));
        assert_eq!(caption_at(&s, 2.1), None);
        assert_eq!(caption_at(&s, 2.5), None, "an empty line shows nothing");
    }

    #[test]
    fn the_bar_maps_the_pointer_to_a_time() {
        assert_eq!(time_at(100., 100., 400., 120.), 0.);
        assert_eq!(time_at(300., 100., 400., 120.), 60.);
        assert_eq!(time_at(900., 100., 400., 120.), 120.);
        assert_eq!(time_at(50., 100., 400., 120.), 0.);
        assert_eq!(time_at(300., 100., 0., 120.), 0.);
        assert_eq!(share(30., 120.), 0.25);
        assert_eq!(share(500., 120.), 1.);
        assert_eq!(share(5., 0.), 0.);
    }

    #[test]
    fn the_clock_adds_what_ran() {
        assert_eq!(position(3., None), 3.);
        assert_eq!(position(3., Some(1.5)), 4.5);
        assert_eq!(position(-2., Some(f32::NAN)), 0.);
    }

    #[test]
    fn strip_tiles_grow_with_the_shot() {
        assert_eq!(tile_width(0.5), STRIP_MIN_W);
        assert_eq!(tile_width(60.), STRIP_MAX_W);
        assert!(tile_width(4.) > tile_width(3.5));
        assert_eq!(tile_width(f32::NAN), STRIP_MIN_W);
        assert_eq!(move_label(Move::PushIn), "Push in");
        assert_eq!(move_label(Move::Hold), "Still");
    }

    /// A 16-bit mono WAV at `rate` holding `samples`.
    fn wav(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        // An extra chunk before the data, as some writers add.
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[1, 2, 3, 0]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    #[test]
    fn the_tail_of_a_wav_starts_at_the_asked_time() {
        let samples: Vec<i16> = (0..10).collect();
        let src = wav(4, &samples);
        // 1 s at 4 Hz = 4 samples skipped.
        let tail = wav_tail(&src, 1.).unwrap_or_default();
        assert_eq!(&tail[0..4], b"RIFF");
        assert_eq!(&tail[36..40], b"data");
        let body: Vec<i16> = tail[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(body, vec![4, 5, 6, 7, 8, 9]);
        assert_eq!(u32::from_le_bytes([tail[40], tail[41], tail[42], tail[43]]), 12);
        // From the start: the same sound; past the end: an empty but valid file.
        assert_eq!(wav_tail(&src, 0.).map(|t| t[44..].to_vec()), Some(src[src.len() - 20..].to_vec()));
        assert_eq!(wav_tail(&src, 99.).map(|t| t.len()), Some(44));
        assert_eq!(wav_tail(b"not a wav at all", 1.), None);
        assert_eq!(wav_tail(&src[..20], 1.), None, "a cut-off header is refused");
    }
}
