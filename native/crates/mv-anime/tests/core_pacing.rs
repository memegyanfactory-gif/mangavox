//! Pacing numbers (design 6.3 table, 8.1 shot timing).

use mv_anime::pacing::{camera_extent, effective_s, line_seconds, pace, scene_seconds, spoken_seconds, words};
use mv_anime::{Line, LineKind, Move, SceneKind, Shot};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn line(text: &str) -> Line {
    Line { spoken: text.into(), ..Line::default() }
}

#[test]
fn the_table_holds_the_measured_numbers() {
    let d = pace(SceneKind::Dialogue);
    assert_eq!((d.median_cut_s, d.mean_range_s, d.secs_per_panel, d.long_take_s, d.line_lead_s, d.full_animation), (3.8, (3.0, 6.0), 4.0, 16., 0.9, (0., 0.22)));
    let a = pace(SceneKind::Action);
    assert_eq!((a.median_cut_s, a.mean_range_s, a.secs_per_panel, a.long_take_s, a.line_lead_s, a.full_animation), (1.6, (1.0, 3.2), 2.0, 10., 0.5, (0.35, 0.85)));
    // The dread correction: shorter than the salvage had it.
    let r = pace(SceneKind::Dread);
    assert_eq!((r.median_cut_s, r.mean_range_s, r.secs_per_panel, r.long_take_s, r.line_lead_s, r.full_animation), (2.8, (2.0, 5.0), 3.6, 10., 1.2, (0., 0.4)));
    let n = pace(SceneKind::Narration);
    assert_eq!((n.median_cut_s, n.mean_range_s, n.secs_per_panel, n.long_take_s, n.line_lead_s, n.full_animation), (4.7, (4.0, 8.5), 5.0, 14., 0.6, (0., 0.2)));
    let f = pace(SceneKind::Flashback);
    assert_eq!((f.median_cut_s, f.mean_range_s, f.secs_per_panel, f.long_take_s, f.line_lead_s, f.full_animation), (3.6, (3.0, 6.0), 3.6, 10., 0.9, (0., 0.3)));
    let m = pace(SceneKind::Montage);
    assert_eq!((m.median_cut_s, m.mean_range_s, m.secs_per_panel, m.long_take_s, m.line_lead_s, m.full_animation), (2.5, (2.0, 4.5), 2.5, 8., 0.6, (0., 0.4)));
    // A kind from a newer MangaVox paces like dialogue.
    assert_eq!(pace(SceneKind::Unknown), d);
}

#[test]
fn a_line_takes_its_words_at_two_and_a_half_a_second_never_under_0_6_s() {
    assert_eq!(words("Too bad I already caught it all."), 7);
    assert_eq!(words("... !?"), 0);
    assert!(close(line_seconds("Too bad I already caught it all."), 2.8));
    assert!(close(line_seconds("No."), 0.6));
    assert!(close(line_seconds(""), 0.6));
    assert!(close(line_seconds("one two three four five six seven eight nine ten"), 4.0));
}

#[test]
fn the_dialogue_floor_is_lead_lines_gaps_and_tail() {
    let lines = [line("Too bad I already caught it all."), line("No.")];
    // 0.9 lead + 2.8 + 0.6 + one 0.35 gap + 0.4 tail.
    assert!(close(spoken_seconds(&lines, SceneKind::Dialogue), 5.05), "{}", spoken_seconds(&lines, SceneKind::Dialogue));
    // Action leads in faster (0.5).
    assert!(close(spoken_seconds(&lines, SceneKind::Action), 4.65));
    // Sound effects are never voiced; blank lines take no time; the manga words stand in for an empty spoken text.
    let sfx = Line { kind: LineKind::Sfx, spoken: "BOOM".into(), ..Line::default() };
    let blank = line("   ");
    let original = Line { original: "Run!".into(), ..Line::default() };
    assert!(close(spoken_seconds(&[sfx.clone(), blank.clone(), original], SceneKind::Dialogue), 0.9 + 0.6 + 0.4));
    assert_eq!(spoken_seconds(&[sfx, blank], SceneKind::Dialogue), 0.);
    assert_eq!(spoken_seconds(&[], SceneKind::Dread), 0.);
}

#[test]
fn a_scene_runs_by_its_panels_but_never_shorter_than_its_lines() {
    assert!(close(scene_seconds(SceneKind::Dialogue, 40, 60.), 160.));
    assert!(close(scene_seconds(SceneKind::Action, 40, 5.), 80.));
    assert!(close(scene_seconds(SceneKind::Dialogue, 3, 40.), 46.));
    assert!(close(scene_seconds(SceneKind::Dread, 0, 0.), 3.6));
    assert!(close(scene_seconds(SceneKind::Montage, 1000, 0.), 900.));
}

#[test]
fn camera_moves_cover_what_the_reference_measured() {
    // (zoom, travel): dialogue pans 2% of the frame a second.
    let (z, t) = camera_extent(SceneKind::Dialogue, Move::PanRight, 6., 1.);
    assert!(close(z, 0.) && close(t, 0.12), "{z} {t}");
    // A push is at least 8% so it reads as a move.
    let (z, t) = camera_extent(SceneKind::Dialogue, Move::PushIn, 5., 1.);
    assert!(close(z, 0.08) && close(t, 0.));
    assert!(camera_extent(SceneKind::Action, Move::PanLeft, 2., 1.).1 > 0.2);
    assert_eq!(camera_extent(SceneKind::Dread, Move::Hold, 5., 1.), (0., 0.));
    assert!(close(camera_extent(SceneKind::Dialogue, Move::PanLeft, 6., 3.).1, 0.36));
    assert!(close(camera_extent(SceneKind::Dialogue, Move::PanLeft, 20., 6.).1, 0.9));
    assert!(close(camera_extent(SceneKind::Dialogue, Move::Whip, 1., 1.).1, 0.5));
}

#[test]
fn a_shot_lasts_its_lock_else_its_plan_or_its_voices() {
    let shot = Shot { planned_s: 2.0, ..Shot::default() };
    // 0.9 lead + 1.5 + 2.0 + 0.35 + 0.4.
    assert!(close(effective_s(&shot, SceneKind::Dialogue, &[1.5, 2.0]), 5.15));
    let long = Shot { planned_s: 6.0, ..Shot::default() };
    assert!(close(effective_s(&long, SceneKind::Dialogue, &[1.5, 2.0]), 6.0));
    assert!(close(effective_s(&shot, SceneKind::Dialogue, &[]), 2.0));
    let locked = Shot { planned_s: 6.0, locked_s: Some(3.0), ..Shot::default() };
    assert!(close(effective_s(&locked, SceneKind::Dialogue, &[9.0]), 3.0));
}
