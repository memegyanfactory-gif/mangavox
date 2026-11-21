#![cfg(feature = "cut")]
//! CUT (design 8.2): the cut plan's timing (lengths from lines and pacing, lines after their
//! cut by the measured lead, trims, whole frames), the camera curves (start, end, monotonic,
//! eased, never frozen), the ffmpeg command lines, the dialogue sum, the cache keys and the
//! readable errors when ffmpeg or an encoder is missing. Nothing here runs ffmpeg.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mv_anime::animatic::{self, CutPlan, CutShot};
use mv_anime::ffmpeg::{self, SegmentSpec, Subs, VideoCodec};
use mv_anime::{Camera, Card, Episode, Line, LineKind, Move, Movement, Scene, SceneKind, SceneShots, Shot, Speed, Status, Store, Story, Target, TakeKind, TakeMeta};

fn home() -> &'static Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp folder");
        std::env::set_var("MANGAVOX_HOME", dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        dir
    })
    .path()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn take(s: &Store, bytes: &[u8], ext: &str, kind: TakeKind, seconds: f32) -> String {
    s.add_take(bytes, ext, TakeMeta { kind, seconds, target: Target::Upload, ..TakeMeta::default() }).expect("take").id
}

fn line(id: &str, scene: &str, text: &str, voice: Option<String>) -> Line {
    let mut l = Line { id: id.into(), scene: scene.into(), spoken: text.into(), kind: LineKind::Speech, ..Line::default() };
    l.voice.selected = voice.clone();
    l.voice.takes = voice.into_iter().collect();
    l
}

fn shot(id: &str, planned: f32, lines: &[&str], mv: Move, frame: Option<String>) -> Shot {
    Shot {
        id: id.into(),
        planned_s: planned,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        camera: Camera { movement: Movement { kind: mv, speed: Speed::Medium, amount: 1. }, ..Camera::default() },
        frame: Card { status: if frame.is_some() { Status::Approved } else { Status::Draft }, selected: frame.clone(), takes: frame.into_iter().collect(), ..Card::default() },
        ..Shot::default()
    }
}

/// sc1 (dialogue): sh1 2.0 s planned with two voiced lines (1.0 s, 1.5 s); sh2 3.0 s planned with
/// one unvoiced 5-word line and a still. sc2 (action): sh3 locked to 2.5 s, a push in.
fn project(dir: &Path) -> (Store, Vec<String>) {
    let s = Store { id: "p1".into(), dir: dir.join("anime").join("projects").join("p1") };
    let v1 = take(&s, b"voice one", "wav", TakeKind::Audio, 1.0);
    let v2 = take(&s, b"voice two", "wav", TakeKind::Audio, 1.5);
    let still = take(&s, b"a still picture", "png", TakeKind::Image, 0.);
    s.save_episode(&Episode { id: "ep01".into(), ..Episode::default() }).expect("episode");
    s.save_story(&Story {
        episode: "ep01".into(),
        scenes: vec![
            Scene { id: "sc1".into(), kind: SceneKind::Dialogue, ..Scene::default() },
            Scene { id: "sc2".into(), kind: SceneKind::Action, ..Scene::default() },
        ],
        lines: vec![
            line("l1", "sc1", "We go in.", Some(v1.clone())),
            line("l2", "sc1", "Stay behind me.", Some(v2.clone())),
            line("l3", "sc1", "one two three four five", None),
            Line { kind: LineKind::Sfx, ..line("l4", "sc2", "CRASH", None) },
        ],
        ..Story::default()
    })
    .expect("story");
    s.save_scene_shots(&SceneShots {
        episode: "ep01".into(),
        scene: "sc1".into(),
        shots: vec![shot("sh1", 2.0, &["l1", "l2"], Move::Hold, None), shot("sh2", 3.0, &["l3"], Move::PanRight, Some(still.clone()))],
        ..SceneShots::default()
    })
    .expect("shots 1");
    let mut sh3 = shot("sh3", 1.0, &["l4"], Move::PushIn, None);
    sh3.locked_s = Some(2.5);
    s.save_scene_shots(&SceneShots { episode: "ep01".into(), scene: "sc2".into(), shots: vec![sh3], ..SceneShots::default() }).expect("shots 2");
    (s, vec![v1, v2, still])
}

#[test]
fn the_cut_follows_lines_pacing_and_whole_frames() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let (s, ids) = project(tmp.path());
    let plan = animatic::plan_cut(&s, "ep01").expect("plan");
    assert_eq!(plan.schema, mv_anime::SCHEMA);
    assert_eq!(plan.size, (1280, 720));
    let order: Vec<&str> = plan.shots.iter().map(|c| c.shot.as_str()).collect();
    assert_eq!(order, ["sh1", "sh2", "sh3"]);
    let (a, b, c) = (&plan.shots[0], &plan.shots[1], &plan.shots[2]);
    // sh1: lead 0.9 + 1.0 + 0.35 + 1.5 + tail 0.4 = 4.15 s -> 100 frames.
    assert_eq!(animatic::frames(a.seconds), 100);
    assert!(near(a.seconds, 100. / 24.));
    assert!(near(a.start_s, 0.));
    let take_file = |id: &str| s.take(id).expect("take").file;
    assert_eq!(a.lines.len(), 2);
    assert_eq!(a.lines[0].0, take_file(&ids[0]));
    assert!(near(a.lines[0].1, 0.9), "the first line starts the dialogue lead after the cut");
    assert!(near(a.lines[1].1, 0.9 + 1.0 + 0.2), "the same voice goes on after a 0.2 s breath");
    assert_eq!(a.missing, vec!["picture".to_string()]);
    assert_eq!(a.subs.len(), 2);
    assert_eq!(a.subs[1].2, "Stay behind me.");
    // sh2: the unvoiced line is estimated (5 words = 2.0 s): 0.9 + 2.0 + 0.4 = 3.3 s -> 79 frames.
    assert_eq!(animatic::frames(b.seconds), 79);
    assert!(near(b.start_s, 100. / 24.));
    assert_eq!(b.still.as_deref(), Some(take_file(&ids[2]).as_str()));
    assert!(b.lines.is_empty());
    assert_eq!(b.missing, vec!["voice l3".to_string()]);
    assert_eq!(b.subs.len(), 1, "an unvoiced line still has a subtitle");
    // sh3: locked to 2.5 s (60 frames); sound effects are not timed.
    assert_eq!(animatic::frames(c.seconds), 60);
    assert!(near(c.start_s, 179. / 24.));
    assert!(c.subs.is_empty());
    assert!(near(plan.seconds, 239. / 24.));
    // The push of a 2.5 s action cut: 0.05/s × 2.5 = 12.5%, at the pace of a 3 s push.
    assert!(near(c.zoom, 0.125 * 2.5 / 3.), "{}", c.zoom);
    assert!(near(b.travel, (0.02f32 * 79. / 24.).max(0.03)), "{}", b.travel);
    assert!(!plan.timing_hash.is_empty());
    // The episode's own scene order wins.
    let mut ep = s.episode("ep01").expect("episode");
    ep.scene_order = vec!["sc2".into(), "sc1".into()];
    s.save_episode(&ep).expect("saved");
    let again = animatic::plan_cut(&s, "ep01").expect("plan");
    let order: Vec<&str> = again.shots.iter().map(|c| c.shot.as_str()).collect();
    assert_eq!(order, ["sh3", "sh1", "sh2"]);
    assert_ne!(again.timing_hash, plan.timing_hash);
}

#[test]
fn trims_and_switches_saved_by_the_user_are_kept() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let (s, _) = project(tmp.path());
    let before = animatic::plan_cut(&s, "ep01").expect("plan");
    let saved = CutPlan {
        shots: vec![CutShot { shot: "sh1".into(), trim_in: 0.5, trim_out: 0.25, ..CutShot::default() }],
        subtitles: true,
        size: (1920, 1080),
        ..CutPlan::default()
    };
    mv_anime::store::write_json(&s.path("episodes/ep01/cut.json"), &saved).expect("saved");
    let plan = animatic::plan_cut(&s, "ep01").expect("plan");
    let a = &plan.shots[0];
    assert!(near(a.trim_in, 0.5) && near(a.trim_out, 0.25));
    // 4.15 − 0.75 = 3.4 s -> 82 frames; lines move 0.5 s earlier.
    assert_eq!(animatic::frames(a.seconds), 82);
    assert!(near(a.lines[0].1, 0.4));
    assert!(near(plan.shots[1].start_s, 82. / 24.));
    assert!(plan.subtitles);
    assert_eq!(plan.size, (1920, 1080));
    assert_ne!(plan.timing_hash, before.timing_hash, "a trim changes the timing");
}

#[test]
fn the_timing_hash_moves_only_with_timing() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let (s, _) = project(tmp.path());
    let first = animatic::plan_cut(&s, "ep01").expect("plan").timing_hash;
    // A changed note on a shot is not timing.
    let mut shots = s.scene_shots("ep01", "sc1").expect("shots");
    shots.shots[0].tells = "They walk in, scared".into();
    shots.shots[0].edited = true;
    s.save_scene_shots(&shots).expect("saved");
    assert_eq!(animatic::plan_cut(&s, "ep01").expect("plan").timing_hash, first);
    // Another voice take is.
    let other = take(&s, b"voice one, take two", "wav", TakeKind::Audio, 1.0);
    let mut story = s.story("ep01").expect("story");
    story.lines[0].voice.selected = Some(other);
    s.save_story(&story).expect("saved");
    let second = animatic::plan_cut(&s, "ep01").expect("plan").timing_hash;
    assert_ne!(second, first);
    // So is another move.
    let mut shots = s.scene_shots("ep01", "sc1").expect("shots");
    shots.shots[1].camera.movement.kind = Move::PanLeft;
    s.save_scene_shots(&shots).expect("saved");
    assert_ne!(animatic::plan_cut(&s, "ep01").expect("plan").timing_hash, second);
}

// ------------------------------------------------------------------------------ camera curves

fn cs(mv: Move, seconds: f32, zoom: f32, travel: f32) -> CutShot {
    CutShot { shot: "x".into(), mv, seconds, zoom, travel, ..CutShot::default() }
}

/// Samples (zoom, x, y) every frame.
fn samples(s: &CutShot) -> Vec<(f32, f32, f32)> {
    (0..=animatic::frames(s.seconds)).map(|n| animatic::camera_at(s, n as f32 / 24.)).collect()
}

fn monotonic(v: &[f32], up: bool) -> bool {
    v.windows(2).all(|w| if up { w[1] >= w[0] - 1e-6 } else { w[1] <= w[0] + 1e-6 })
}

/// Eased: the first and last frame move much less than the average frame.
fn eased(v: &[f32]) -> bool {
    let n = v.len() - 1;
    let avg = (v[n] - v[0]).abs() / n as f32;
    (v[1] - v[0]).abs() < 0.2 * avg && (v[n] - v[n - 1]).abs() < 0.2 * avg
}

fn within(s: &[(f32, f32, f32)]) -> bool {
    s.iter().all(|(z, x, y)| *z >= 1. && *x >= -1e-6 && *y >= -1e-6 && *x <= 1. - 1. / z + 1e-6 && *y <= 1. - 1. / z + 1e-6)
}

#[test]
fn push_and_pull_start_end_and_ease() {
    let push = cs(Move::PushIn, 4., 0.1, 0.);
    let v = samples(&push);
    assert!(near(v[0].0, 1.) && near(v.last().expect("end").0, 1.1));
    let z: Vec<f32> = v.iter().map(|s| s.0).collect();
    assert!(monotonic(&z, true) && eased(&z));
    assert!(within(&v));
    // Centred when nothing is known.
    let (zz, x, y) = animatic::camera_at(&push, 4.);
    assert!(near(x, (1. - 1. / zz) / 2.) && near(y, (1. - 1. / zz) / 2.));
    let pull = cs(Move::PullOut, 4., 0.1, 0.);
    let z: Vec<f32> = samples(&pull).iter().map(|s| s.0).collect();
    assert!(near(z[0], 1.1) && near(*z.last().expect("end"), 1.) && monotonic(&z, false) && eased(&z));
    // A push centres on the focus mark: at the end the window's middle is the mark's screen x.
    let focused = CutShot { focus_x: Some(0.55), ..cs(Move::PushIn, 4., 0.2, 0.) };
    let (z, x, _) = animatic::camera_at(&focused, 4.);
    assert!(near(x + 0.5 / z, 0.55), "{x} {z}");
    // A mark at the edge: the window stops at the edge.
    let edge = CutShot { focus_x: Some(1.), ..cs(Move::PushIn, 4., 0.2, 0.) };
    let (z, x, _) = animatic::camera_at(&edge, 4.);
    assert!(near(x, 1. - 1. / z));
}

#[test]
fn pans_and_tilts_slide_across_the_room_eased() {
    for (mv, axis, up) in [(Move::PanRight, 1, true), (Move::PanLeft, 1, false), (Move::TiltDown, 2, true), (Move::TiltUp, 2, false), (Move::TruckRight, 1, true), (Move::BgSlide, 1, true)] {
        let s = cs(mv, 5., 0., 0.2);
        let v = samples(&s);
        assert!(within(&v), "{mv:?}");
        assert!(v.iter().all(|p| near(p.0, 1.2)), "{mv:?} keeps its headroom zoom");
        let a: Vec<f32> = v.iter().map(|p| if axis == 1 { p.1 } else { p.2 }).collect();
        let room = 1. - 1. / 1.2;
        let (start, end) = if up { (0., room) } else { (room, 0.) };
        assert!(near(a[0], start) && near(*a.last().expect("end"), end), "{mv:?}: {} -> {}", a[0], a[a.len() - 1]);
        assert!(monotonic(&a, up) && eased(&a), "{mv:?}");
        let other: Vec<f32> = v.iter().map(|p| if axis == 1 { p.2 } else { p.1 }).collect();
        assert!(other.iter().all(|o| near(*o, room / 2.)), "{mv:?} stays centred on the other axis");
    }
}

#[test]
fn a_hold_creeps_and_fast_moves_land_softly() {
    // Anime never freezes: a hold is a slow 1.5% push.
    let hold = cs(Move::Hold, 3., 0., 0.);
    let z: Vec<f32> = samples(&hold).iter().map(|s| s.0).collect();
    assert!(near(z[0], 1.) && near(*z.last().expect("end"), 1. + animatic::HOLD_CREEP));
    assert!(monotonic(&z, true) && eased(&z));
    assert_eq!(animatic::camera_at(&cs(Move::RackFocus, 3., 0., 0.), 3.).0, animatic::camera_at(&hold, 3.).0);
    // A crash zoom does most of its move in the first half second, then holds.
    let crash = cs(Move::CrashZoom, 2., 0.14, 0.);
    let (z_half, _, _) = animatic::camera_at(&crash, 0.4);
    assert!(z_half > 1. + 0.14 * 0.9, "{z_half}");
    assert!(near(animatic::camera_at(&crash, 2.).0, 1.14));
    let z: Vec<f32> = samples(&crash).iter().map(|s| s.0).collect();
    assert!(monotonic(&z, true));
    assert!((z[z.len() - 1] - z[z.len() - 2]).abs() < 1e-6, "it has landed");
    // A whip crosses the room fast.
    let whip = cs(Move::Whip, 1.5, 0., 0.5);
    let room = 1. - 1. / 1.5;
    assert!(animatic::camera_at(&whip, 0.3).1 > 0.9 * room);
    assert!(near(animatic::camera_at(&whip, 1.5).1, room));
    // A shake jolts sideways, decays, and never leaves the picture.
    let shake = cs(Move::Shake, 2., 0., 0.);
    let v = samples(&shake);
    assert!(v.iter().all(|p| near(p.0, 1.04)) && within(&v));
    let centre = (1. - 1. / 1.04) / 2.;
    let early = v[..12].iter().map(|p| (p.1 - centre).abs()).fold(0f32, f32::max);
    let late = v[v.len() - 12..].iter().map(|p| (p.1 - centre).abs()).fold(0f32, f32::max);
    assert!(early > 0.005 && late < early * 0.1, "{early} {late}");
    // Outside the shot the camera rests at its ends.
    assert_eq!(animatic::camera_at(&hold, -1.), animatic::camera_at(&hold, 0.));
    assert_eq!(animatic::camera_at(&hold, 99.), animatic::camera_at(&hold, 3.));
}

#[test]
fn move_amounts_follow_the_anime_limits() {
    // A short push moves at the pace of a 3 s push; at 3 s and more it is the usual extent.
    let (z_short, _) = animatic::move_extent(SceneKind::Dialogue, Move::PushIn, 1.5, 1., Speed::Medium);
    let (z_long, _) = animatic::move_extent(SceneKind::Dialogue, Move::PushIn, 3., 1., Speed::Medium);
    let (base, _) = mv_anime::pacing::camera_extent(SceneKind::Dialogue, Move::PushIn, 1.5, 1.);
    assert!(near(z_short, base * 0.5));
    assert!(near(z_long, mv_anime::pacing::camera_extent(SceneKind::Dialogue, Move::PushIn, 3., 1.).0));
    // A pan never slides faster than one frame width in 7 s, whatever its speed.
    for secs in [1f32, 4., 10.] {
        let (_, t) = animatic::move_extent(SceneKind::Action, Move::PanLeft, secs, 4., Speed::Snap);
        assert!(t <= secs / 7. + 1e-6, "{secs}: {t}");
    }
    // Speed scales the move.
    let (_, slow) = animatic::move_extent(SceneKind::Dialogue, Move::PanRight, 10., 1., Speed::Creep);
    let (_, fast) = animatic::move_extent(SceneKind::Dialogue, Move::PanRight, 10., 1., Speed::Fast);
    assert!(fast > slow);
}

#[test]
fn zoompan_expressions_are_the_preview_curve() {
    let (z, x, y) = animatic::zoompan_exprs(&cs(Move::PushIn, 3., 0.1, 0.));
    assert_eq!(z, "1.000000+(0.100000)*((min(1,on/72.000000))*(min(1,on/72.000000))*(3-2*(min(1,on/72.000000))))");
    assert!(x.starts_with("(iw-iw/zoom)*(0.500000+(0.000000)*"), "{x}");
    assert!(y.starts_with("(ih-ih/zoom)*(0.500000+"), "{y}");
    let (_, x, _) = animatic::zoompan_exprs(&cs(Move::PanLeft, 2., 0., 0.1));
    assert!(x.starts_with("(iw-iw/zoom)*(1.000000+(-1.000000)*"), "{x}");
    let (z, _, _) = animatic::zoompan_exprs(&cs(Move::CrashZoom, 2., 0.14, 0.));
    assert!(z.starts_with("1.000000+(0.140000)*(1-pow(1-(min(1,on/14.4"), "{z}");
    assert!(z.ends_with(")),3))"), "{z}");
    let (_, x, _) = animatic::zoompan_exprs(&cs(Move::Shake, 2., 0., 0.));
    assert!(x.ends_with("+iw*0.012000*sin(on*2.1)*exp(-6*on/48.000000)"), "{x}");
    // No commas outside min(...)/pow(...): the expression sits in single quotes in the graph.
    assert!(!z.contains('\''));
}

// ------------------------------------------------------------------------------ ffmpeg command lines

fn spec(codec: VideoCodec, text: Option<(PathBuf, String)>) -> SegmentSpec {
    SegmentSpec {
        input: PathBuf::from("C:/p/takes/ab/still.png"),
        frames: 72,
        size: (1280, 720),
        exprs: ("Z".into(), "X".into(), "Y".into()),
        text,
        codec,
        out: PathBuf::from("C:/p/work/cut/seg/k.part.mp4"),
    }
}

fn pair(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

#[test]
fn segment_concat_normalise_and_mux_command_lines() {
    let a = ffmpeg::segment_args(&spec(VideoCodec::X264, None));
    assert_eq!(a[0], "-y");
    assert_eq!(pair(&a, "-i").as_deref(), Some("C:/p/takes/ab/still.png"));
    assert_eq!(pair(&a, "-vf").as_deref(), Some("scale=2560:1440:force_original_aspect_ratio=increase:flags=lanczos,crop=2560:1440,setsar=1,zoompan=z='Z':x='X':y='Y':d=72:s=1280x720:fps=24,setsar=1,format=yuv420p"));
    assert_eq!(pair(&a, "-frames:v").as_deref(), Some("72"));
    assert_eq!(pair(&a, "-r").as_deref(), Some("24"));
    assert_eq!(pair(&a, "-c:v").as_deref(), Some("libx264"));
    assert_eq!(pair(&a, "-preset").as_deref(), Some("veryfast"));
    assert_eq!(pair(&a, "-crf").as_deref(), Some("20"));
    assert_eq!(pair(&a, "-pix_fmt").as_deref(), Some("yuv420p"));
    assert_eq!(pair(&a, "-threads").as_deref(), Some("4"));
    assert!(a.contains(&"-an".to_string()));
    assert_eq!(a.last().map(String::as_str), Some("C:/p/work/cut/seg/k.part.mp4"));
    // CPU only: no hardware encoder or GPU filter anywhere.
    let all = a.join(" ");
    for gpu in ["nvenc", "cuda", "qsv", "amf", "hwaccel", "vulkan", "opencl"] {
        assert!(!all.contains(gpu), "{gpu}");
    }
    // The mpeg4 fallback (risk 9).
    let m = ffmpeg::segment_args(&spec(VideoCodec::Mpeg4, None));
    assert_eq!(pair(&m, "-c:v").as_deref(), Some("mpeg4"));
    assert_eq!(pair(&m, "-q:v").as_deref(), Some("3"));
    assert!(pair(&m, "-crf").is_none());
    // A placeholder card's text: the font path escaped for the filter, the text from a file.
    let t = ffmpeg::segment_args(&spec(VideoCodec::X264, Some((PathBuf::from(r"C:\Windows\Fonts\segoeui.ttf"), "seg/k.txt".into()))));
    let vf = pair(&t, "-vf").unwrap_or_default();
    assert!(vf.contains(",drawtext=fontfile='C\\:/Windows/Fonts/segoeui.ttf':textfile='seg/k.txt':expansion=none:"), "{vf}");
    assert!(vf.ends_with(",format=yuv420p"));

    let c = ffmpeg::concat_args(Path::new("w/list.txt"), Path::new("w/video.mp4"));
    assert_eq!(c, ["-y", "-f", "concat", "-safe", "0", "-i", "w/list.txt", "-c", "copy", "-threads", "4", "w/video.mp4"]);
    let n = ffmpeg::normalise_args(Path::new("t/a.wav"), Path::new("w/a/a.part.wav"));
    assert_eq!(n, ["-y", "-i", "t/a.wav", "-vn", "-ar", "48000", "-ac", "1", "-c:a", "pcm_s16le", "-threads", "4", "w/a/a.part.wav"]);

    let (v, d, o) = (Path::new("w/video.mp4"), Path::new("w/dialogue.wav"), Path::new("e/a.part.mp4"));
    let plain = ffmpeg::mux_args(v, d, &Subs::None, VideoCodec::X264, "en", o);
    assert_eq!(pair(&plain, "-c:v").as_deref(), Some("copy"), "the picture is not encoded twice");
    assert_eq!(pair(&plain, "-af").as_deref(), Some(ffmpeg::LOUDNORM));
    assert_eq!(pair(&plain, "-c:a").as_deref(), Some("aac"));
    assert_eq!(pair(&plain, "-ar").as_deref(), Some("48000"));
    assert!(plain.windows(2).any(|w| w == ["-map", "0:v:0"]) && plain.windows(2).any(|w| w == ["-map", "1:a:0"]));
    assert!(!plain.contains(&"-shortest".to_string()));
    let soft = ffmpeg::mux_args(v, d, &Subs::Soft(PathBuf::from("e/a.srt")), VideoCodec::X264, "en", o);
    assert!(soft.windows(2).any(|w| w == ["-i", "e/a.srt"]));
    assert!(soft.windows(2).any(|w| w == ["-map", "2:s:0"]));
    assert_eq!(pair(&soft, "-c:s").as_deref(), Some("mov_text"));
    assert_eq!(pair(&soft, "-metadata:s:s:0").as_deref(), Some("language=eng"));
    // A Hindi project's subtitle track is labelled Hindi.
    let hindi = ffmpeg::mux_args(v, d, &Subs::Soft(PathBuf::from("e/a.srt")), VideoCodec::X264, "hi", o);
    assert_eq!(pair(&hindi, "-metadata:s:s:0").as_deref(), Some("language=hin"));
    assert_eq!(ffmpeg::iso639_2("pt-BR"), "por");
    assert_eq!(ffmpeg::iso639_2("??"), "und");
    // A hung ffmpeg is stopped: at least 10 minutes, 20 × a long video's length.
    assert_eq!(ffmpeg::watchdog(0.).as_secs(), 600);
    assert_eq!(ffmpeg::watchdog(f32::NAN).as_secs(), 600);
    assert_eq!(ffmpeg::watchdog(60.).as_secs(), 1200);
    let burn = ffmpeg::mux_args(v, d, &Subs::Burn("subs.srt".into()), VideoCodec::Mpeg4, "en", o);
    assert_eq!(pair(&burn, "-vf").as_deref(), Some("subtitles=f='subs.srt',format=yuv420p"));
    assert_eq!(pair(&burn, "-c:v").as_deref(), Some("mpeg4"), "burned text needs a new encode");
    assert!(!burn.windows(2).any(|w| w == ["-map", "2:s:0"]));
}

#[test]
fn progress_lines_and_encoder_listings_are_read() {
    assert_eq!(ffmpeg::parse_progress("out_time_us=1500000"), Some(1.5));
    assert_eq!(ffmpeg::parse_progress("out_time_ms=2000000"), Some(2.0), "ffmpeg's out_time_ms is microseconds too");
    assert_eq!(ffmpeg::parse_progress("out_time=00:01:02.500000"), Some(62.5));
    assert_eq!(ffmpeg::parse_progress("out_time_us=N/A"), None);
    assert_eq!(ffmpeg::parse_progress("frame=12"), None);
    let listing = "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264 H.264\n V.S..D mpeg4                MPEG-4 part 2\n";
    assert!(ffmpeg::listed(listing, "libx264"));
    assert!(ffmpeg::listed(listing, "mpeg4"));
    assert!(!ffmpeg::listed(listing, "libx265"));
    assert!(!ffmpeg::listed(listing, "H.264"));
    let filters = " .. zoompan          V->V       Apply Zoom & Pan effect.\n T. drawtext          V->V       Draw text\n";
    assert!(ffmpeg::listed(filters, "zoompan") && ffmpeg::listed(filters, "drawtext") && !ffmpeg::listed(filters, "subtitles"));
}

#[test]
fn missing_ffmpeg_or_encoders_say_what_to_install() {
    let m = ffmpeg::missing_message();
    assert!(m.contains("ffmpeg is not installed") && m.contains(ffmpeg::install_cmd()), "{m}");
    assert_eq!(ffmpeg::pick_codec(true, true).expect("x264"), VideoCodec::X264);
    assert_eq!(ffmpeg::pick_codec(false, true).expect("fallback"), VideoCodec::Mpeg4);
    let e = ffmpeg::pick_codec(false, false).expect_err("no encoder");
    let e = e.to_string();
    assert!(e.contains("libx264") && e.contains("mpeg4") && e.contains(ffmpeg::install_cmd()), "{e}");
    // The locator: nothing in an empty folder; a binary set by path or found in a folder.
    let tmp = tempfile::tempdir().expect("temp folder");
    assert_eq!(ffmpeg::locate(None, &[tmp.path().to_path_buf()]), (None, None));
    let exe = |n: &str| if cfg!(windows) { format!("{n}.exe") } else { n.to_string() };
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(bin.join(exe("ffmpeg")), b"x").expect("ffmpeg");
    std::fs::write(bin.join(exe("ffprobe")), b"x").expect("ffprobe");
    assert_eq!(ffmpeg::locate(None, &[tmp.path().to_path_buf(), bin.clone()]), (Some(bin.join(exe("ffmpeg"))), Some(bin.join(exe("ffprobe")))));
    assert_eq!(ffmpeg::locate(Some(bin.clone()), &[]).0, Some(bin.join(exe("ffmpeg"))), "MANGAVOX_FFMPEG may name the folder");
    assert_eq!(ffmpeg::locate(Some(tmp.path().join("nope.exe")), &[]).0, None);
}

#[tokio::test]
async fn running_a_missing_ffmpeg_fails_with_the_install_hint() {
    let tmp = tempfile::tempdir().expect("temp folder");
    let gone = tmp.path().join("no-ffmpeg-here.exe");
    let e = ffmpeg::run(&gone, &["-version".to_string()], 0., |_| {}, &mv_ai::CancellationToken::new()).await.expect_err("missing");
    assert!(e.to_string().contains(ffmpeg::install_cmd()), "{e}");
    assert!(!ffmpeg::has(&gone, "libx264"));
    // A cancelled run never starts.
    let token = mv_ai::CancellationToken::new();
    token.cancel();
    let e = ffmpeg::run(&gone, &[], 0., |_| {}, &token).await.expect_err("cancelled");
    assert_eq!(e.to_string(), ffmpeg::CANCELLED);
}

// ------------------------------------------------------------------------------ audio and caches

#[test]
fn dialogue_clips_are_summed_at_their_offsets_and_clipped() {
    let half = i16::MAX / 2 + 1; // 16384 = 0.5
    let clips = vec![(2i64, vec![half; 4]), (4, vec![half; 4]), (-2, vec![half; 3]), (9, vec![half; 5])];
    let mix = animatic::sum_clips(10, &clips, 1.);
    let want = [0.5, 0., 0.5, 0.5, 1., 1., 0.5, 0.5, 0., 0.5];
    for (i, (m, w)) in mix.iter().zip(want).enumerate() {
        assert!((m - animatic::soft_limit(w)).abs() < 1e-4, "sample {i}: {m} != {w} through the soft limiter");
    }
    // Three loud overlapping lines bend under full scale (soft limiter) instead of wrapping.
    let loud = vec![(0i64, vec![i16::MAX; 2]), (0, vec![i16::MAX; 2]), (0, vec![i16::MIN; 1])];
    let m = animatic::sum_clips(2, &loud, 1.);
    assert!(m.iter().all(|s| (-1. ..=1.).contains(s)));
    assert!(m[1] > animatic::LIMIT_KNEE && m[1] < 1.);
    // The gain applies before the sum.
    let g = animatic::sum_clips(1, &[(0, vec![half])], animatic::DIALOGUE_GAIN);
    assert!((g[0] - 0.5 * animatic::DIALOGUE_GAIN).abs() < 1e-4);
    // A block in the middle sees the same samples as the whole track.
    let whole = animatic::sum_clips(10, &clips, 1.);
    assert_eq!(animatic::mix_block(3, 5, &clips, 1.), whole[3..8].to_vec());
}

#[test]
fn segment_cache_keys_change_only_with_their_inputs() {
    let shot = cs(Move::PushIn, 3., 0.1, 0.);
    let e = animatic::zoompan_exprs(&shot);
    let key = |id: &str, n: u32, size: (u32, u32), codec: VideoCodec, e: &(String, String, String), text: bool| animatic::segment_key(id, n, size, codec, e, text);
    let k = key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::X264, &e, false);
    assert_eq!(k.len(), 32);
    assert_eq!(k, key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::X264, &animatic::zoompan_exprs(&shot.clone()), false), "same inputs, same segment");
    // Unrelated fields of the shot (its lines, start, captions) do not touch the segment.
    let moved = CutShot { start_s: 12., lines: vec![("takes/cd/v.wav".into(), 0.9)], subs: vec![(0.9, 2., "Hi".into())], ..shot.clone() };
    assert_eq!(animatic::zoompan_exprs(&moved), e);
    for other in [
        key("takes/ab/abd.png", 72, (1280, 720), VideoCodec::X264, &e, false),
        key("takes/ab/abc.png", 73, (1280, 720), VideoCodec::X264, &e, false),
        key("takes/ab/abc.png", 72, (1920, 1080), VideoCodec::X264, &e, false),
        key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::Mpeg4, &e, false),
        key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::X264, &animatic::zoompan_exprs(&cs(Move::PullOut, 3., 0.1, 0.)), false),
        key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::X264, &animatic::zoompan_exprs(&cs(Move::PushIn, 3., 0.12, 0.)), false),
        key("takes/ab/abc.png", 72, (1280, 720), VideoCodec::X264, &e, true),
    ] {
        assert_ne!(other, k);
    }
}

#[test]
fn subtitles_and_export_names() {
    let plan = CutPlan {
        seconds: 6.,
        shots: vec![
            CutShot { start_s: 0., seconds: 3., subs: vec![(0.9, 2.1, "We go\nin.".into()), (2.5, 3.6, "Now!".into())], ..CutShot::default() },
            CutShot { start_s: 3., seconds: 3., subs: vec![(-0.5, 0.4, "cut short".into()), (2.9, 3.5, "past the end".into()), (1., 1.01, "too short".into())], ..CutShot::default() },
        ],
        ..CutPlan::default()
    };
    assert_eq!(
        animatic::srt(&plan),
        "1\n00:00:00,900 --> 00:00:02,100\nWe go in.\n\n2\n00:00:02,500 --> 00:00:03,600\nNow!\n\n3\n00:00:02,500 --> 00:00:03,400\ncut short\n\n4\n00:00:05,900 --> 00:00:06,000\npast the end\n\n"
    );
    assert_eq!(animatic::stamp(0), "19700101-000000");
    assert_eq!(animatic::stamp(951_782_400 + 3_723), "20000229-010203");
}

#[test]
fn a_finished_render_keeps_what_the_user_changed_meanwhile() {
    // What the render planned when it started (subtitles off, no trims).
    let planned = CutPlan {
        shots: vec![CutShot { shot: "sh1".into(), seconds: 2.0, ..CutShot::default() }, CutShot { shot: "sh2".into(), seconds: 3.0, ..CutShot::default() }],
        seconds: 5.0,
        timing_hash: "render".into(),
        size: (1280, 720),
        ..CutPlan::default()
    };
    // The cut file as the Cut page saved it during the render.
    let mut saved = CutPlan {
        shots: vec![CutShot { shot: "sh2".into(), trim_out: 0.5, ..CutShot::default() }],
        timing_hash: "old".into(),
        subtitles: true,
        burn_subtitles: true,
        size: (1920, 1080),
        ..CutPlan::default()
    };
    animatic::record_render(&mut saved, &planned);
    assert!(saved.subtitles && saved.burn_subtitles, "the switches the user set stay");
    assert_eq!(saved.size, (1920, 1080));
    assert_eq!(saved.timing_hash, "render");
    assert_eq!(saved.seconds, 5.0);
    assert_eq!(saved.shots.iter().map(|s| s.shot.as_str()).collect::<Vec<_>>(), ["sh1", "sh2"]);
    assert_eq!(saved.shots[1].trim_out, 0.5, "a trim made during the render stays");
}

#[test]
fn an_export_is_up_to_date_only_with_the_settings_it_was_made_with() {
    // Planned (and rendered) with subtitles off.
    let planned = CutPlan { shots: vec![CutShot { shot: "sh1".into(), seconds: 2.0, ..CutShot::default() }], seconds: 2.0, timing_hash: "t1".into(), size: (1280, 720), ..CutPlan::default() };
    // The user switched subtitles on while it rendered.
    let mut saved = CutPlan { subtitles: true, ..CutPlan::default() };
    animatic::record_render(&mut saved, &planned);
    assert_eq!(saved.exported_with.as_ref().map(|s| s.subtitles.as_str()), Some("off"), "the render's own settings are recorded");
    // The cut as it would be made now: same timing, subtitles on.
    let now = CutPlan { timing_hash: "t1".into(), subtitles: true, ..CutPlan::default() };
    assert!(!animatic::export_current(&saved, &now), "made without subtitles: not up to date");
    let now_off = CutPlan { timing_hash: "t1".into(), ..CutPlan::default() };
    assert!(animatic::export_current(&saved, &now_off), "same timing, same settings");
    let moved = CutPlan { timing_hash: "t2".into(), ..CutPlan::default() };
    assert!(!animatic::export_current(&saved, &moved), "another timing");
    // An export from before the settings were recorded is not reported up to date.
    let legacy = CutPlan { timing_hash: "t1".into(), ..CutPlan::default() };
    assert!(!animatic::export_current(&legacy, &now_off));
}