#![cfg(feature = "shots")]
//! Film-grammar checks (design 6.4): a passing and a failing fixture for every check, the score,
//! and the coverage patterns judged by the same checks. Fixtures are built in code.

use std::collections::BTreeMap;

use mv_anime::checks::{self, check_episode, check_scene, score};
use mv_anime::model::*;
use mv_anime::patterns;

// ------------------------------------------------------------------------- fixtures

fn scene(kind: SceneKind, intensity: u8) -> Scene {
    Scene { id: "sc1".into(), title: "Hall".into(), kind, intensity, ..Scene::default() }
}

fn fight() -> Scene {
    Scene { fight: true, tone: Tone::Fight, ..scene(SceneKind::Action, 7) }
}

fn bible() -> BibleIndex {
    let ch = |id: &str, role: Role| Character { id: id.into(), name: id.to_uppercase(), role, ..Character::default() };
    BibleIndex { characters: vec![ch("hero", Role::Hero), ch("villain", Role::Threat), ch("ally", Role::Ally), ch("a", Role::Neutral), ch("b", Role::Neutral)], ..BibleIndex::default() }
}

fn shot(size: ShotSize, secs: f32) -> Shot {
    let mut s = Shot { planned_s: secs, background: BackgroundKind::Backdrop, ..Shot::default() };
    s.camera.size = size;
    s
}

fn mark(id: &str, x: f32, z: f32, facing: f32, visible: Visibility) -> Mark {
    Mark { character: id.into(), pos: [x, z], facing_deg: facing, visible, ..Mark::default() }
}

/// A shot with a camera at an exact point looking at a character.
fn at(mut s: Shot, cam: [f32; 3], look: &str, marks: Vec<Mark>) -> Shot {
    s.camera.pos = Some(cam);
    s.camera.look_at = LookAt::Character(look.into());
    s.marks = marks;
    s
}

fn with(mut s: Shot, f: impl FnOnce(&mut Shot)) -> Shot {
    f(&mut s);
    s
}

fn line(id: &str, speaker: Option<&str>, text: &str) -> Line {
    Line { id: id.into(), scene: "sc1".into(), speaker: speaker.map(str::to_string), kind: LineKind::Speech, spoken: text.into(), ..Line::default() }
}

fn run(scene: &Scene, shots: &[Shot], lines: &[Line], new_place: bool) -> Vec<Issue> {
    check_scene(scene, shots, lines, &bible(), new_place)
}

fn has(issues: &[Issue], code: &str) -> bool {
    issues.iter().any(|i| i.code == code)
}

fn of<'a>(issues: &'a [Issue], code: &str) -> Vec<&'a Issue> {
    issues.iter().filter(|i| i.code == code).collect()
}

fn ms(n: usize, secs: f32) -> Vec<Shot> {
    (0..n).map(|_| shot(ShotSize::Ms, secs)).collect()
}

// ------------------------------------------------------------------------- score

#[test]
fn score_weighs_errors_three_warnings_one_info_nothing() {
    let i = |s: Severity| Issue { code: "x".into(), severity: s, ..Issue::default() };
    assert_eq!(score(&[i(Severity::Error), i(Severity::Warning), i(Severity::Info), i(Severity::Info)]), 4);
    assert_eq!(score(&[]), 0);
    assert!(checks::needs_revision(&[i(Severity::Error)]));
    assert!(checks::needs_revision(&[i(Severity::Warning), i(Severity::Warning)]));
    assert!(!checks::needs_revision(&[i(Severity::Warning), i(Severity::Info)]));
    let notes = checks::notes(&[Issue { code: "axis".into(), severity: Severity::Error, message: "Cut 3 crosses.".into(), fix: Some("Bridge it.".into()), ..Issue::default() }, i(Severity::Info)]);
    assert!(notes.contains("MUST FIX") && notes.contains("Cut 3 crosses.") && notes.contains("Fix: Bridge it."));
    assert_eq!(notes.lines().count(), 1, "info is not sent back");
}

#[test]
fn an_unplanned_scene_has_no_issues() {
    assert!(run(&scene(SceneKind::Dialogue, 3), &[], &[line("l1", Some("a"), "Hi.")], true).is_empty());
}

// ------------------------------------------------------------------------- layout

#[test]
fn layout_early() {
    let sc = scene(SceneKind::Dialogue, 3);
    // Pass: the wide is cut 2, starting at 3 s.
    let pass = vec![shot(ShotSize::Ms, 3.), shot(ShotSize::Ls, 4.)];
    assert!(!has(&run(&sc, &pass, &[], true), "layout-early"));
    // Fail: no layout in the first two cuts.
    let fail = vec![shot(ShotSize::Ms, 3.), shot(ShotSize::Cu, 2.), shot(ShotSize::Ls, 4.)];
    let issues = run(&sc, &fail, &[], true);
    let e = of(&issues, "layout-early");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].severity, Severity::Error);
    assert!(e[0].fix.as_deref().unwrap_or("").contains("cut 1 or cut 2"));
    // Fail: cut 2 is wide but starts 7 s in.
    let late = vec![shot(ShotSize::Ms, 7.), shot(ShotSize::Ls, 4.)];
    assert!(has(&run(&sc, &late, &[], true), "layout-early"));
    // A full shot on the matched room with two people counts as layout.
    let full = with(shot(ShotSize::Fs, 4.), |s| {
        s.background = BackgroundKind::Matched;
        s.marks = vec![mark("a", -1., 0., 90., Visibility::Full), mark("b", 1., 0., 270., Visibility::Full)];
    });
    assert!(!has(&run(&sc, &[full], &[], true), "layout-early"));
    // A return to a known place is not checked.
    assert!(!has(&run(&sc, &fail, &[], false), "layout-early"));
}

#[test]
fn layout_return() {
    let talk = scene(SceneKind::Dialogue, 3);
    // Pass: a wide every six cuts.
    let mut pass = vec![shot(ShotSize::Ls, 4.)];
    pass.extend(ms(5, 3.));
    pass.push(shot(ShotSize::Els, 4.));
    pass.extend(ms(5, 3.));
    assert!(!has(&run(&talk, &pass, &[], false), "layout-return"));
    // Fail: twelve cuts in a row without one (only 24 s, but too many cuts).
    let mut fail = vec![shot(ShotSize::Ls, 4.)];
    fail.extend(ms(12, 2.));
    let issues = run(&talk, &fail, &[], false);
    let w = of(&issues, "layout-return");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].severity, Severity::Warning);
    assert!(w[0].message.contains("cuts 2-13"), "{}", w[0].message);
    // Fail on time: an action scene goes 15 s without a wide (limit 13 s) ...
    let action = scene(SceneKind::Action, 6);
    let mut slow = vec![shot(ShotSize::Ls, 2.)];
    slow.extend(ms(3, 5.));
    assert!(has(&run(&action, &slow, &[], false), "layout-return"));
    // ... the same 15 s in a talk scene is fine (limit 35 s).
    assert!(!has(&run(&talk, &slow, &[], false), "layout-return"));
    // A run that is all fog or effects replaces the room on purpose.
    let fog: Vec<Shot> = slow.iter().cloned().enumerate().map(|(i, s)| if i == 0 { s } else { with(s, |s| s.background = BackgroundKind::None) }).collect();
    assert!(!has(&run(&action, &fog, &[], false), "layout-return"));
}

// ------------------------------------------------------------------------- close shots

#[test]
fn close_run() {
    let sc = scene(SceneKind::Dialogue, 3);
    let pass = vec![shot(ShotSize::Ls, 4.), shot(ShotSize::Cu, 2.), shot(ShotSize::Mcu, 2.), shot(ShotSize::Cu, 2.), shot(ShotSize::Ms, 3.)];
    assert!(!has(&run(&sc, &pass, &[], false), "close-run"));
    let fail = vec![shot(ShotSize::Ls, 4.), shot(ShotSize::Cu, 2.), shot(ShotSize::Mcu, 2.), shot(ShotSize::Cu, 2.), shot(ShotSize::Ecu, 2.)];
    let issues = run(&sc, &fail, &[], false);
    let w = of(&issues, "close-run");
    assert_eq!(w.len(), 1);
    assert!(w[0].fix.as_deref().unwrap_or("").contains("cut 5"));
    // A panic burst of cuts under a second may run to seven ...
    let mut burst = vec![shot(ShotSize::Ls, 4.)];
    burst.extend((0..7).map(|i| shot(if i % 2 == 0 { ShotSize::Ecu } else { ShotSize::Cu }, 0.5)));
    assert!(!has(&run(&sc, &burst, &[], false), "close-run"));
    // ... not eight.
    burst.push(shot(ShotSize::Ecu, 0.5));
    assert!(has(&run(&sc, &burst, &[], false), "close-run"));
}

#[test]
fn step_down() {
    let sc = scene(SceneKind::Dialogue, 3);
    let pass = vec![shot(ShotSize::Ls, 4.), shot(ShotSize::Ms, 3.), shot(ShotSize::Cu, 2.), shot(ShotSize::Ms, 3.), shot(ShotSize::Mcu, 3.)];
    assert!(!has(&run(&sc, &pass, &[], false), "step-down"));
    let fail = vec![shot(ShotSize::Cu, 3.), shot(ShotSize::Mcu, 3.), shot(ShotSize::Ms, 3.), shot(ShotSize::Cu, 3.), shot(ShotSize::Mcu, 3.)];
    let issues = run(&sc, &fail, &[], false);
    assert_eq!(of(&issues, "step-down")[0].severity, Severity::Info);
}

// ------------------------------------------------------------------------- staging

#[test]
fn threat_before_hit() {
    let sc = fight();
    let both = || vec![mark("hero", -1., 0., 90., Visibility::Full), mark("villain", 1., 0., 270., Visibility::Full)];
    let hit = || with(shot(ShotSize::Insert, 0.5), |s| s.hit = true);
    // Pass: the two share the frame right before the hit.
    let pass = vec![with(shot(ShotSize::Ms, 2.), |s| s.marks = vec![mark("hero", 0., 0., 0., Visibility::Full)]), with(shot(ShotSize::Fs, 2.), |s| s.marks = both()), hit()];
    assert!(!has(&run(&sc, &pass, &[], false), "threat-before-hit"));
    // Fail: they were together three cuts before, then only apart.
    let fail = vec![
        with(shot(ShotSize::Fs, 2.), |s| s.marks = both()),
        with(shot(ShotSize::Ms, 2.), |s| s.marks = vec![mark("hero", 0., 0., 0., Visibility::Full)]),
        with(shot(ShotSize::Ms, 2.), |s| s.marks = vec![mark("villain", 0., 0., 0., Visibility::Full)]),
        hit(),
    ];
    let issues = run(&sc, &fail, &[], false);
    let w = of(&issues, "threat-before-hit");
    assert_eq!(w.len(), 1);
    assert!(w[0].fix.as_deref().unwrap_or("").contains("VILLAIN") && w[0].fix.as_deref().unwrap_or("").contains("HERO"));
    // Not a fight scene: not checked.
    let calm = scene(SceneKind::Dialogue, 3);
    assert!(!has(&run(&calm, &fail, &[], false), "threat-before-hit"));
}

#[test]
fn axis() {
    let sc = scene(SceneKind::Dialogue, 3);
    let pair = || vec![mark("a", -1., 0., 90., Visibility::Full), mark("b", 1., 0., 270., Visibility::Full)];
    let south = |x: f32| at(shot(ShotSize::Ms, 3.), [x, 1.5, 3.], "a", pair());
    let north = at(shot(ShotSize::Ms, 3.), [0., 1.5, -3.], "a", pair());
    // Pass: both cameras south of the a-b line.
    assert!(!has(&run(&sc, &[south(0.), with(south(0.6), |s| s.camera.size = ShotSize::TwoShot)], &[], false), "axis"));
    // Fail: the second camera jumps to the north side.
    let issues = run(&sc, &[south(0.), north.clone()], &[], false);
    let e = of(&issues, "axis");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].severity, Severity::Error);
    assert!(e[0].message.contains("Cut 2") && e[0].fix.as_deref().unwrap_or("").contains("bridge"));
    // Pass: an overhead, an extreme close-up or a marked bridge in between lets it cross.
    let top = with(shot(ShotSize::Overhead, 3.), |s| s.marks = pair());
    assert!(!has(&run(&sc, &[south(0.), top, north.clone()], &[], false), "axis"));
    let bridge = with(south(0.), |s| s.bridge = true);
    assert!(!has(&run(&sc, &[south(0.), bridge, north.clone()], &[], false), "axis"));
    let ecu = with(shot(ShotSize::Ecu, 1.), |s| s.marks = vec![mark("a", -1., 0., 90., Visibility::Partial)]);
    assert!(!has(&run(&sc, &[south(0.), ecu, north], &[], false), "axis"));
}

#[test]
fn eyeline() {
    let sc = scene(SceneKind::Dialogue, 3);
    let a = at(shot(ShotSize::Mcu, 3.), [-0.5, 1.5, 2.], "a", vec![mark("a", -1., 0., 90., Visibility::Full)]);
    let b_left = at(shot(ShotSize::Mcu, 3.), [0.5, 1.5, 2.], "b", vec![mark("b", 1., 0., 270., Visibility::Full)]);
    let b_right = at(shot(ShotSize::Mcu, 3.), [0.5, 1.5, 2.], "b", vec![mark("b", 1., 0., 90., Visibility::Full)]);
    // Pass: a looks screen right, b screen left.
    assert!(!has(&run(&sc, &[a.clone(), b_left], &[], false), "eyeline"));
    // Fail: both look screen right.
    let issues = run(&sc, &[a.clone(), b_right.clone()], &[], false);
    let w = of(&issues, "eyeline");
    assert_eq!(w.len(), 1);
    assert!(w[0].message.contains("screen right") && w[0].fix.as_deref().unwrap_or("").contains("screen left"));
    // Looking at something else is not a shot and its reverse.
    let elsewhere = with(b_right, |s| s.marks[0].gaze = Some("landmark:door".into()));
    assert!(!has(&run(&sc, &[a, elsewhere], &[], false), "eyeline"));
}

#[test]
fn along_lens() {
    let sc = scene(SceneKind::Action, 6);
    let runner = |facing: f32| at(shot(ShotSize::Ls, 2.), [0., 1.5, 5.], "a", vec![Mark { pose: Pose::Run, ..mark("a", 0., 0., facing, Visibility::Full) }]);
    // Pass: running at the lens (the camera is south, the runner faces south).
    assert!(!has(&run(&sc, &[runner(180.)], &[], false), "along-lens"));
    // Fail: running across the frame.
    let issues = run(&sc, &[runner(90.)], &[], false);
    assert_eq!(of(&issues, "along-lens")[0].severity, Severity::Info);
}

#[test]
fn jump_cut() {
    let sc = scene(SceneKind::Dialogue, 3);
    let single = |size: ShotSize, secs: f32| with(shot(size, secs), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)]);
    // Pass: the size changes.
    assert!(!has(&run(&sc, &[single(ShotSize::Ms, 3.), single(ShotSize::Ls, 3.)], &[], false), "jump-cut"));
    // Pass: the angle changes.
    let low = with(single(ShotSize::Ms, 3.), |s| s.camera.angle = Angle::Low);
    assert!(!has(&run(&sc, &[single(ShotSize::Ms, 3.), low], &[], false), "jump-cut"));
    // Fail: same person, size and angle.
    let issues = run(&sc, &[single(ShotSize::Ms, 3.), single(ShotSize::Ms, 3.)], &[], false);
    assert_eq!(of(&issues, "jump-cut")[0].severity, Severity::Error);
    // Pass: a dissolve, or a deliberate burst of cuts under a second.
    let dissolve = with(single(ShotSize::Ms, 3.), |s| s.transition = Transition::Dissolve);
    assert!(!has(&run(&sc, &[dissolve, single(ShotSize::Ms, 3.)], &[], false), "jump-cut"));
    assert!(!has(&run(&sc, &[single(ShotSize::Ecu, 0.4), single(ShotSize::Ecu, 0.4)], &[], false), "jump-cut"));
}

#[test]
fn progression() {
    let sc = scene(SceneKind::Dialogue, 3);
    let ecu = with(shot(ShotSize::Ecu, 2.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Partial)]);
    let ms = with(shot(ShotSize::Ms, 3.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)]);
    // Pass: seen whole first.
    assert!(!has(&run(&sc, &[ms.clone(), ecu.clone()], &[], false), "progression"));
    // Fail: the first sight is an extreme close-up.
    assert_eq!(of(&run(&sc, &[ecu.clone(), ms.clone()], &[], false), "progression")[0].severity, Severity::Warning);
    // Pass: the stunned and reveal patterns open on eyes on purpose.
    let stunned = with(ecu, |s| s.pattern = Some(Pattern::Stunned));
    assert!(!has(&run(&sc, &[stunned, ms], &[], false), "progression"));
}

#[test]
fn fight_wide() {
    let sc = fight();
    let pass = vec![shot(ShotSize::Ls, 2.), shot(ShotSize::Ms, 1.5), shot(ShotSize::Ms, 1.5), shot(ShotSize::Cowboy, 2.), shot(ShotSize::Ms, 1.5), shot(ShotSize::Ms, 1.5)];
    assert!(!has(&run(&sc, &pass, &[], false), "fight-wide"));
    let fail = ms(6, 1.5);
    assert_eq!(of(&run(&sc, &fail, &[], false), "fight-wide")[0].severity, Severity::Warning);
    // Not a fight: not checked.
    assert!(!has(&run(&scene(SceneKind::Dialogue, 3), &fail, &[], false), "fight-wide"));
}

#[test]
fn shake_talk() {
    let shake = with(shot(ShotSize::Ms, 3.), |s| s.camera.movement.kind = Move::Shake);
    assert_eq!(of(&run(&scene(SceneKind::Dialogue, 3), &[shake.clone()], &[], false), "shake-talk")[0].severity, Severity::Warning);
    assert!(has(&run(&scene(SceneKind::Flashback, 3), &[shake.clone()], &[], false), "shake-talk"));
    // A shake on a hit in an action scene is fine.
    assert!(!has(&run(&scene(SceneKind::Action, 7), &[shake], &[], false), "shake-talk"));
}

// ------------------------------------------------------------------------- faces and pace

#[test]
fn faces_by_stakes() {
    // Intensity 9: a full face in at most 25% + 10% of cuts.
    let sc = scene(SceneKind::Dread, 9);
    let face = || with(shot(ShotSize::Mcu, 3.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)]);
    let fail = vec![face(), face(), face(), with(shot(ShotSize::Ls, 4.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)])];
    let issues = run(&sc, &fail, &[], false);
    let w = of(&issues, "faces");
    assert_eq!(w.len(), 1);
    assert!(w[0].message.contains("3 of the scene's 4") && w[0].fix.as_deref().unwrap_or("").contains("cuts 1, 2, 3"), "{:?}", w[0]);
    // Pass: one face; a back, an insert and a wide carry the rest.
    let pass = vec![
        face(),
        with(shot(ShotSize::Ms, 3.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Back)]),
        with(shot(ShotSize::Insert, 2.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Partial)]),
        shot(ShotSize::Ls, 4.),
    ];
    assert!(!has(&run(&sc, &pass, &[], false), "faces"));
    // The same three faces in a calm talk (intensity 3: up to 80%) pass.
    assert!(!has(&run(&scene(SceneKind::Dialogue, 3), &fail, &[], false), "faces"));
    // A character turned away from the camera shows no face even when seen whole.
    let away = at(shot(ShotSize::Mcu, 3.), [0., 1.5, 3.], "a", vec![mark("a", 0., 0., 0., Visibility::Full)]);
    assert!(!checks::face_shown(&away, None));
    let toward = at(shot(ShotSize::Mcu, 3.), [0., 1.5, 3.], "a", vec![mark("a", 0., 0., 180., Visibility::Full)]);
    assert!(checks::face_shown(&toward, None));
    assert_eq!((checks::face_target(3), checks::face_target(6), checks::face_target(8), checks::face_target(10)), (0.70, 0.50, 0.35, 0.25));
}

#[test]
fn pace() {
    let sc = scene(SceneKind::Dialogue, 3);
    assert!(!has(&run(&sc, &ms(4, 4.), &[], false), "pace"));
    let rushed = run(&sc, &ms(4, 1.), &[], false);
    assert!(of(&rushed, "pace")[0].message.contains("rushed"));
    let drags = run(&sc, &ms(4, 10.), &[], false);
    assert!(of(&drags, "pace")[0].message.contains("drags"));
    // Fewer than four shots are not judged.
    assert!(!has(&run(&sc, &ms(3, 1.), &[], false), "pace"));
    // Action is judged against its own (faster) range.
    assert!(!has(&run(&scene(SceneKind::Action, 6), &ms(4, 1.5), &[], false), "pace"));
}

#[test]
fn first_hold() {
    let sc = scene(SceneKind::Dialogue, 3);
    assert!(!has(&run(&sc, &[shot(ShotSize::Els, 4.2)], &[], true), "first-hold"));
    assert_eq!(of(&run(&sc, &[shot(ShotSize::Els, 1.5)], &[], true), "first-hold")[0].severity, Severity::Info);
    assert!(!has(&run(&sc, &[shot(ShotSize::Els, 1.5)], &[], false), "first-hold"));
}

#[test]
fn flash_rate() {
    let sc = scene(SceneKind::Action, 8);
    let flash = || with(shot(ShotSize::Insert, 0.2), |s| s.flash = true);
    // Fail: five flashes within one second.
    let fail: Vec<Shot> = (0..5).map(|_| flash()).collect();
    let issues = run(&sc, &fail, &[], false);
    let e = of(&issues, "flash-rate");
    assert_eq!(e.len(), 1, "reported once");
    assert_eq!(e[0].severity, Severity::Error);
    // Pass: three, then a rest.
    let pass = vec![flash(), flash(), flash(), shot(ShotSize::Ms, 2.), flash()];
    assert!(!has(&run(&sc, &pass, &[], false), "flash-rate"));
    // White transitions count too.
    let white = || with(shot(ShotSize::Insert, 0.2), |s| s.transition = Transition::White);
    let mixed = vec![flash(), white(), flash(), white()];
    assert!(has(&run(&sc, &mixed, &[], false), "flash-rate"));
}

// ------------------------------------------------------------------------- lines and mouths

#[test]
fn lines_fit() {
    let sc = scene(SceneKind::Dialogue, 3);
    let lines: Vec<Line> = (1..=6).map(|i| line(&format!("l{i}"), Some("a"), "Short words here.")).collect();
    let pass = vec![with(shot(ShotSize::Ms, 6.), |s| s.lines = vec!["l1".into(), "l2".into()])];
    assert!(!has(&run(&sc, &pass, &lines[..2], false), "lines-fit"));
    // Fail: six lines in one cut.
    let six = vec![with(shot(ShotSize::Ms, 20.), |s| s.lines = lines.iter().map(|l| l.id.clone()).collect())];
    assert_eq!(of(&run(&sc, &six, &lines, false), "lines-fit")[0].severity, Severity::Error);
    // Fail: one line that takes 24 s to say.
    let long = vec![line("l1", Some("a"), &"word ".repeat(60))];
    let one = vec![with(shot(ShotSize::Ms, 20.), |s| s.lines = vec!["l1".into()])];
    assert!(has(&run(&sc, &one, &long, false), "lines-fit"));
}

#[test]
fn lines_cover() {
    let sc = scene(SceneKind::Dialogue, 3);
    let lines = vec![line("l1", Some("a"), "One."), line("l2", Some("b"), "Two.")];
    let pass = vec![with(shot(ShotSize::Ms, 3.), |s| s.lines = vec!["l1".into()]), with(shot(ShotSize::Ms, 3.), |s| s.lines = vec!["l2".into()])];
    assert!(!has(&run(&sc, &pass, &lines, false), "lines-cover"));
    let missing = vec![with(shot(ShotSize::Ms, 3.), |s| s.lines = vec!["l1".into()])];
    // Messages quote the line's words, never its id.
    let msg = of(&run(&sc, &missing, &lines, false), "lines-cover")[0].message.clone();
    assert!(msg.contains("\u{201c}Two.\u{201d}") && !msg.contains("l2"), "{msg}");
    let twice = vec![with(shot(ShotSize::Ms, 3.), |s| s.lines = vec!["l1".into(), "l2".into()]), with(shot(ShotSize::Ls, 3.), |s| s.lines = vec!["l1".into()])];
    let issues = run(&sc, &twice, &lines, false);
    assert!(of(&issues, "lines-cover").iter().any(|i| i.message.contains("The line \u{201c}One.\u{201d} is in more than one cut (1, 2)") && i.severity == Severity::Error));
}

#[test]
fn mouth() {
    let sc = scene(SceneKind::Dialogue, 3);
    let lines = vec![line("l1", Some("a"), "I am talking now.")];
    let talking = |m: MouthPolicy| {
        with(shot(ShotSize::Cu, 3.), |s| {
            s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)];
            s.lines = vec!["l1".into()];
            s.mouth = m;
        })
    };
    assert_eq!(of(&run(&sc, &[talking(MouthPolicy::Generate)], &lines, false), "mouth")[0].severity, Severity::Warning);
    assert!(!has(&run(&sc, &[talking(MouthPolicy::Flap)], &lines, false), "mouth"));
    // A wide may leave the mouth to the picture: it is too small to matter.
    let wide = with(talking(MouthPolicy::Generate), |s| s.camera.size = ShotSize::Ls);
    assert!(!has(&run(&sc, &[wide], &lines, false), "mouth"));
}

#[test]
fn line_length() {
    let long = line("l1", Some("hero"), "I will not let you take another step toward them now");
    let short = line("l2", Some("hero"), "Run. Now.");
    let one = |id: &str| vec![with(shot(ShotSize::Ls, 6.), |s| s.lines = vec![id.into()])];
    let issues = run(&fight(), &one("l1"), std::slice::from_ref(&long), false);
    let i = of(&issues, "line-length");
    assert_eq!(i[0].severity, Severity::Info);
    assert!(i[0].message.contains("11 words"));
    assert!(!has(&run(&fight(), &one("l2"), std::slice::from_ref(&short), false), "line-length"));
    // Dread allows ten words; calm talk is not checked.
    assert!(has(&run(&scene(SceneKind::Dread, 7), &one("l1"), std::slice::from_ref(&long), false), "line-length"));
    assert!(!has(&run(&scene(SceneKind::Dialogue, 3), &one("l1"), &[long], false), "line-length"));
}

// ------------------------------------------------------------------------- episode

fn episode(shots: Vec<Shot>) -> Vec<Issue> {
    let sc = scene(SceneKind::Dialogue, 3);
    let story = Story { episode: "ep01".into(), scenes: vec![sc], ..Story::default() };
    let map: BTreeMap<String, Vec<Shot>> = [("sc1".to_string(), shots)].into_iter().collect();
    check_episode(&story, &map, &bible())
}

fn ep_has(issues: &[Issue], code: &str) -> bool {
    issues.iter().any(|i| i.code == code && i.scene.is_empty())
}

#[test]
fn episode_faces() {
    let face = || with(shot(ShotSize::Mcu, 3.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)]);
    let other = || with(shot(ShotSize::Ls, 4.), |s| s.marks = vec![mark("a", 0., 0., 180., Visibility::Full)]);
    // 8 of 12 faces (67%) is over the episode's 45%.
    let fail: Vec<Shot> = (0..12).map(|i| if i % 3 == 0 { other() } else { face() }).collect();
    assert!(ep_has(&episode(fail), "faces"));
    // 4 of 12 (33%) is fine.
    let pass: Vec<Shot> = (0..12).map(|i| if i % 3 == 0 { face() } else { other() }).collect();
    assert!(!ep_has(&episode(pass), "faces"));
    // Fewer than 12 shots are not judged for the episode.
    assert!(!ep_has(&episode((0..11).map(|_| face()).collect()), "faces"));
}

#[test]
fn camera_share() {
    let still = || shot(ShotSize::Ms, 3.);
    let moving = || with(shot(ShotSize::Ms, 3.), |s| s.camera.movement.kind = Move::PanRight);
    let fail: Vec<Shot> = (0..12).map(|_| still()).collect();
    let issues = episode(fail);
    assert!(issues.iter().any(|i| i.code == "camera-share" && i.severity == Severity::Info && i.message.starts_with("0%")));
    let pass: Vec<Shot> = (0..12).map(|i| if i % 2 == 0 { moving() } else { still() }).collect();
    assert!(!ep_has(&episode(pass), "camera-share"));
    let busy: Vec<Shot> = (0..12).map(|_| moving()).collect();
    assert!(ep_has(&episode(busy), "camera-share"));
}

#[test]
fn background_mix() {
    let bg = |b: BackgroundKind| with(shot(ShotSize::Ms, 3.), |s| s.background = b);
    let fail: Vec<Shot> = (0..12).map(|_| bg(BackgroundKind::Matched)).collect();
    let issues = episode(fail);
    let i: Vec<&Issue> = issues.iter().filter(|i| i.code == "background-mix").collect();
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].severity, Severity::Info);
    let mix = [BackgroundKind::Matched, BackgroundKind::Backdrop, BackgroundKind::None, BackgroundKind::Backdrop];
    let pass: Vec<Shot> = (0..12).map(|i| bg(mix[i % 4])).collect();
    assert!(!ep_has(&episode(pass), "background-mix"));
}

#[test]
fn episode_runs_every_scene_and_knows_a_new_place() {
    let a = Scene { place: Some("hall".into()), ..scene(SceneKind::Dialogue, 3) };
    let b = Scene { id: "sc2".into(), place: Some("hall".into()), ..scene(SceneKind::Dialogue, 3) };
    let c = Scene { id: "sc3".into(), place: Some("street".into()), ..scene(SceneKind::Dialogue, 3) };
    let story = Story { scenes: vec![a, b, c], ..Story::default() };
    let close = vec![shot(ShotSize::Ms, 3.), shot(ShotSize::Ms, 3.)];
    let map: BTreeMap<String, Vec<Shot>> = ["sc1", "sc2", "sc3"].iter().map(|s| (s.to_string(), close.clone())).collect();
    let issues = check_episode(&story, &map, &bible());
    let early: Vec<&str> = issues.iter().filter(|i| i.code == "layout-early").map(|i| i.scene.as_str()).collect();
    // The first scene and the move to the street are new places; sc2 returns to the hall.
    assert_eq!(early, vec!["sc1", "sc3"]);
}

#[test]
fn the_prompt_states_the_checks_numbers() {
    let t = checks::rules_text(&scene(SceneKind::Dialogue, 9));
    for want in ["layout-early", "within 6 s", "12 cuts or 35 s", "at most 3 close shots", "35%", "at most 3 flashes", "bridge"] {
        assert!(t.contains(want), "missing {want:?} in:\n{t}");
    }
    assert!(checks::rules_text(&fight()).contains("12 cuts or 13 s"));
}

// ------------------------------------------------------------------------- patterns

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_pattern_is_data_the_prompt_lists() {
    let text = patterns::rules_text();
    for d in patterns::defs() {
        assert!(!d.steps.is_empty(), "{}", d.name);
        assert!(text.contains(d.name), "{} missing from the prompt", d.name);
        let shots = patterns::template(d.pattern, &scene(SceneKind::Dialogue, 5), &ids(&["hero", "villain", "ally"]));
        assert_eq!(shots.len(), d.steps.len());
        assert!(shots.iter().all(|s| s.id.is_empty() && s.pattern == Some(d.pattern) && s.planned_s > 0. && !s.tells.is_empty() && s.camera.pos.is_some()));
    }
    for want in ["dread build", "cliffhanger", "fear, shock", "resolve", "DEFAULT CAMERA"] {
        assert!(text.contains(want), "{want}");
    }
    // The reveal holds its full wide at least 3.5 s; a sudden death's impact is under a second.
    let reveal = patterns::def(Pattern::Reveal).map(|d| d.steps.to_vec()).unwrap_or_default();
    assert!(reveal.iter().any(|s| s.size == ShotSize::Els && s.seconds >= 3.5));
    let death = patterns::def(Pattern::SuddenDeath).map(|d| d.steps.to_vec()).unwrap_or_default();
    assert!(death.iter().any(|s| s.hit && s.seconds < 1.));
}

#[test]
fn the_conversation_pattern_passes_the_checks() {
    let sc = scene(SceneKind::Dialogue, 3);
    let shots = patterns::template(Pattern::Conversation, &sc, &ids(&["a", "b"]));
    let issues = run(&sc, &shots, &[], false);
    assert_eq!(score(&issues), 0, "{issues:#?}");
    // In a new place, the arrival funnel before it shows the layout first.
    let mut both = patterns::template(Pattern::Arrival, &sc, &ids(&["a", "b"]));
    both.extend(shots);
    let issues = run(&sc, &both, &[], true);
    assert!(!has(&issues, "layout-early") && !has(&issues, "first-hold") && !has(&issues, "axis") && !has(&issues, "jump-cut"), "{issues:#?}");
}

#[test]
fn fight_patterns_put_threat_and_hero_together_before_the_hit() {
    let sc = fight();
    for p in [Pattern::SuddenDeath, Pattern::Chase] {
        let shots = patterns::template(p, &sc, &ids(&["hero", "villain", "ally"]));
        assert!(shots.iter().any(|s| s.hit));
        let issues = run(&sc, &shots, &[], false);
        assert!(!has(&issues, "threat-before-hit") && !has(&issues, "axis") && !has(&issues, "jump-cut"), "{p:?}: {issues:#?}");
    }
}

#[test]
fn default_camera_follows_the_decision_table() {
    let talk = scene(SceneKind::Dialogue, 3);
    let top = patterns::default_camera(&talk, ShotSize::Overhead);
    assert_eq!((top.angle, top.depth), (Angle::Top, Depth::Deep));
    let mcu = patterns::default_camera(&talk, ShotSize::Mcu);
    assert_eq!((mcu.lens_mm, mcu.depth, mcu.angle, mcu.movement.kind), (85, Depth::Shallow, Angle::Eye, Move::Hold));
    let els = patterns::default_camera(&talk, ShotSize::Els);
    assert_eq!((els.lens_mm, els.angle), (24, Angle::High));
    // Stronger emotion: a wider lens in a fight, a longer one in romance.
    let hot = Scene { intensity: 9, ..fight() };
    assert_eq!(patterns::default_camera(&hot, ShotSize::Mcu).lens_mm, 50);
    let love = Scene { tone: Tone::Romance, intensity: 9, ..scene(SceneKind::Dialogue, 9) };
    let c = patterns::default_camera(&love, ShotSize::Mcu);
    assert_eq!((c.lens_mm, c.light), (135, LightSetup::Backlit));
    // Dread is dark and grainy; a stand-off in action is low.
    let dread = patterns::default_camera(&scene(SceneKind::Dread, 6), ShotSize::Ls);
    assert_eq!((dread.key, dread.grain, dread.movement.kind), (Key::Dark, Grain::Light, Move::PushIn));
    assert_eq!(patterns::default_camera(&fight(), ShotSize::Cowboy).angle, Angle::Low);
    assert_eq!((patterns::snap_lens(80.), patterns::snap_lens(0.), patterns::snap_lens(f32::NAN), patterns::snap_lens(300.)), (85, 35, 35, 135));
}

#[test]
fn generated_shot_sentences_start_with_a_capital() {
    use mv_anime::patterns::capitalised;
    assert_eq!(capitalised("the floor from above, so the move reads"), "The floor from above, so the move reads");
    assert_eq!(capitalised("  re-establish: the survivors"), "Re-establish: the survivors");
    assert_eq!(capitalised("Already fine."), "Already fine.");
    assert_eq!(capitalised("élan"), "Élan");
    assert_eq!(capitalised(""), "");
}