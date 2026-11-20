//! Pacing as measured, cut by cut, on two episodes of a TV anime against the manga chapters they
//! adapt (`native/docs/design/anime.md`): how long a manga panel becomes on screen, how long cuts
//! run, how much of a scene may be continuous animation, how fast the camera moves and how lines
//! sit inside a cut. The director is given these numbers and the checks hold its board to them.
use crate::project::Line;

/// The kinds of scene, each paced and animated its own way.
pub const SCENE_KINDS: [&str; 6] = ["dialogue", "action", "dread", "narration", "flashback", "montage"];

pub struct Pace {
    pub kind: &'static str,
    /// Seconds of screen time one manga panel of such a scene becomes.
    pub secs_per_panel: f32,
    /// The range the mean cut length of such a scene stays in.
    pub mean_cut: (f32, f32),
    /// A cut longer than this is a deliberate long take.
    pub long_take: f32,
    /// The least and the most of the scene's screen time that is continuous animation ("full").
    pub full: (f32, f32),
    /// The least of its screen time that needs no character animation at all (hold, camera).
    pub still_min: f32,
    /// How fast the camera travels over a still drawing, in frame widths per second.
    pub travel: f32,
    /// How fast it pushes in or pulls out, as a share of the frame per second.
    pub zoom: f32,
}

/// Dialogue: 4.1-5.5 s cuts, a quarter of the time a still under a slow camera, 15% full
/// animation. Action: 1.9-3.1 s, more than half full animation. Dread 3.7-4.5 s, narration 5-8 s.
/// Slow camera moves travel about 2% of the frame a second and barely zoom.
pub const PACES: [Pace; 6] = [
    Pace { kind: "dialogue", secs_per_panel: 4.0, mean_cut: (3.4, 6.0), long_take: 16., full: (0., 0.22), still_min: 0.22, travel: 0.02, zoom: 0.008 },
    Pace { kind: "action", secs_per_panel: 2.0, mean_cut: (1.4, 3.2), long_take: 10., full: (0.35, 0.85), still_min: 0., travel: 0.12, zoom: 0.05 },
    Pace { kind: "dread", secs_per_panel: 3.6, mean_cut: (2.8, 5.0), long_take: 16., full: (0., 0.4), still_min: 0.12, travel: 0.025, zoom: 0.01 },
    Pace { kind: "narration", secs_per_panel: 5.0, mean_cut: (4.0, 8.5), long_take: 14., full: (0., 0.2), still_min: 0.4, travel: 0.025, zoom: 0.008 },
    Pace { kind: "flashback", secs_per_panel: 3.6, mean_cut: (3.4, 6.0), long_take: 10., full: (0., 0.3), still_min: 0.3, travel: 0.03, zoom: 0.01 },
    Pace { kind: "montage", secs_per_panel: 2.5, mean_cut: (2.0, 4.5), long_take: 8., full: (0., 0.4), still_min: 0.3, travel: 0.04, zoom: 0.015 },
];

pub fn pace(kind: &str) -> &'static Pace {
    PACES.iter().find(|p| p.kind == kind.trim()).unwrap_or(&PACES[0])
}

/// Words spoken per second (2.5 subtitle words a second measured; an English voice is about this).
pub const WORDS_PER_SEC: f32 = 2.6;
/// A line starts this long after its cut began (measured median 0.5-0.6 s).
pub const LINE_LEAD: f32 = 0.5;
/// The breath between two lines inside one cut.
pub const LINE_GAP: f32 = 0.35;
/// The shortest cut the compositor makes (an impact cut), and the longest.
pub const CUT_MIN: f32 = 0.4;
pub const CUT_MAX: f32 = 20.;

pub fn words(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

/// How long one line takes to say.
pub fn line_seconds(text: &str) -> f32 {
    (words(text) as f32 / WORDS_PER_SEC).max(0.6)
}

/// How long a cut must be to carry `lines`: the lead, each line, the breaths between them.
pub fn spoken_seconds(lines: &[Line]) -> f32 {
    let said: Vec<f32> = lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| line_seconds(&l.text)).collect();
    if said.is_empty() {
        return 0.;
    }
    LINE_LEAD + said.iter().sum::<f32>() + LINE_GAP * (said.len() - 1) as f32
}

/// What a scene of `kind` adapting `panels` story panels with `spoken` seconds of lines should
/// run to: by the panels, but never shorter than its lines need (with a little air).
pub fn scene_seconds(kind: &str, panels: usize, spoken: f32) -> f32 {
    let by_panels = panels.max(1) as f32 * pace(kind).secs_per_panel;
    by_panels.max(spoken * 1.15).clamp(2., 900.)
}

/// How many cuts a scene of that length usually has (low, high).
pub fn scene_cuts(kind: &str, seconds: f32) -> (usize, usize) {
    let (a, b) = pace(kind).mean_cut;
    (((seconds / b).floor() as usize).max(1), ((seconds / a).ceil() as usize).max(1))
}

/// What a camera move covers over one cut, as `(travel, zoom)`: the fraction of the frame the
/// picture slides, and the fraction it grows or shrinks. `amount` scales the usual speed (1).
pub fn camera_extent(kind: &str, camera: &str, seconds: f32, amount: f32) -> (f32, f32) {
    let p = pace(kind);
    let a = if amount.is_finite() && amount > 0. { amount.clamp(0.25, 6.) } else { 1. };
    let s = seconds.clamp(CUT_MIN, CUT_MAX);
    match camera {
        "pan_left" | "pan_right" | "tilt_up" | "tilt_down" => ((p.travel * s * a).clamp(0.03, 0.9), 0.),
        "push_in" | "pull_out" => (0., (p.zoom * s * a).clamp(0.015, 0.35)),
        // A fast move toward the subject: most of it in the cut's first second.
        "track_in" => (0., (0.14 * a).clamp(0.08, 0.4)),
        // A whip covers half a frame or more and settles.
        "whip" => ((0.5 * a).clamp(0.3, 1.2), 0.),
        _ => (0., 0.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line { text: text.into(), ..Default::default() }
    }

    #[test]
    fn lines_set_how_long_a_cut_must_be() {
        assert_eq!(words("Too bad I already caught it all."), 7);
        assert_eq!(words("... !?"), 0);
        // 7 words at 2.6 a second, after half a second.
        let one = spoken_seconds(&[line("Too bad I already caught it all.")]);
        assert!((one - (0.5 + 7. / 2.6)).abs() < 0.01, "{one}");
        let two = spoken_seconds(&[line("No."), line("Let's check it out.")]);
        assert!((two - (0.5 + 0.6 + 0.35 + 4. / 2.6)).abs() < 0.01, "{two}");
        assert_eq!(spoken_seconds(&[line("  ")]), 0.);
    }

    #[test]
    fn scenes_run_as_long_as_the_reference() {
        // Forty panels of talk: about 160 s in 26 to 48 cuts.
        let s = scene_seconds("dialogue", 40, 60.);
        assert_eq!(s, 160.);
        assert_eq!(scene_cuts("dialogue", s), (26, 48));
        // Action is twice as dense in panels.
        assert_eq!(scene_seconds("action", 40, 5.), 80.);
        // A wordy scene of few panels is as long as its lines.
        assert!((scene_seconds("dialogue", 3, 40.) - 46.).abs() < 0.01);
        assert_eq!(pace("nonsense").kind, "dialogue");
    }

    #[test]
    fn talk_moves_the_camera_slowly_and_action_fast() {
        let (t, z) = camera_extent("dialogue", "pan_right", 6., 1.);
        assert!((t - 0.12).abs() < 0.001 && z == 0.);
        let (t, z) = camera_extent("dialogue", "push_in", 5., 1.);
        assert!(t == 0. && (z - 0.04).abs() < 0.001);
        assert!(camera_extent("action", "pan_left", 2., 1.).0 > 0.2);
        assert_eq!(camera_extent("dread", "fix", 5., 1.), (0., 0.));
        // A director's "three times as far" is kept, within what a plate can give.
        assert!((camera_extent("dialogue", "pan_left", 6., 3.).0 - 0.36).abs() < 0.001);
        assert_eq!(camera_extent("dialogue", "pan_left", 20., 6.).0, 0.9);
    }
}
