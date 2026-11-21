#![cfg(feature = "cut")]
//! The real render (design 8.2), run by hand because it needs ffmpeg on the PC:
//!
//!   cb.ps1 test -p mv-anime --no-default-features --features cut -- --ignored --nocapture
//!
//! Three shots from generated PNG stills and WAV voices in a temp folder (push in, pan, shake),
//! CPU encoding only. ffprobe then checks the planned length, the picture size, the frame count
//! and that there is an audio and a subtitle stream. A second render reuses every cached segment.

use std::io::Cursor;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use mv_anime::animatic::{self, CutPlan};
use mv_anime::ffmpeg;
use mv_anime::{Camera, Card, Episode, Line, LineKind, Move, Movement, Scene, SceneKind, SceneShots, Shot, Speed, Status, Store, Story, Target, TakeKind, TakeMeta};
use serde_json::Value;

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

/// A 1376 x 768 still: a colour gradient with a bright block, so a move is visible.
fn still_png(seed: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(1376, 768, |x, y| {
        let block = (500..800).contains(&x) && (250..500).contains(&y);
        if block {
            image::Rgb([240, 240, 240])
        } else {
            image::Rgb([(x / 6) as u8 ^ seed, (y / 4) as u8, seed.wrapping_mul(40)])
        }
    });
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png");
    bytes
}

/// A 24 kHz mono tone of `secs` seconds (Kokoro's rate, so the mix must resample it).
fn voice_wav(dir: &Path, name: &str, secs: f32, hz: f32) -> Vec<u8> {
    let path = dir.join(name);
    let spec = hound::WavSpec { channels: 1, sample_rate: 24_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).expect("wav");
    for i in 0..(secs * 24_000.) as u32 {
        let t = i as f32 / 24_000.;
        w.write_sample(((t * hz * std::f32::consts::TAU).sin() * 0.3 * i16::MAX as f32) as i16).expect("sample");
    }
    w.finalize().expect("wav");
    std::fs::read(&path).expect("wav bytes")
}

fn add(s: &Store, bytes: &[u8], ext: &str, kind: TakeKind, seconds: f32) -> String {
    s.add_take(bytes, ext, TakeMeta { kind, seconds, target: Target::Upload, ..TakeMeta::default() }).expect("take").id
}

fn shot(id: &str, lines: &[&str], mv: Move, still: String) -> Shot {
    Shot {
        id: id.into(),
        planned_s: 2.0,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        camera: Camera { movement: Movement { kind: mv, speed: Speed::Medium, amount: 1. }, ..Camera::default() },
        frame: Card { status: Status::Approved, selected: Some(still.clone()), takes: vec![still], ..Card::default() },
        ..Shot::default()
    }
}

fn probe(exe: &Path, file: &Path) -> Value {
    let out = std::process::Command::new(exe)
        .args(["-v", "error", "-count_frames", "-show_entries", "format=duration:stream=codec_type,codec_name,width,height,nb_read_frames,sample_rate", "-of", "json"])
        .arg(file)
        .output()
        .expect("ffprobe runs");
    serde_json::from_slice(&out.stdout).expect("ffprobe json")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs ffmpeg and ffprobe on this PC; a few seconds of CPU work"]
async fn renders_a_three_shot_animatic_with_ffmpeg() {
    home();
    let tmp = tempfile::tempdir().expect("temp folder");
    let s = Store { id: "p1".into(), dir: tmp.path().join("anime").join("projects").join("p1") };
    let stills: Vec<String> = (0..3).map(|i| add(&s, &still_png(i * 70 + 10), "png", TakeKind::Image, 0.)).collect();
    let v1 = add(&s, &voice_wav(tmp.path(), "v1.wav", 1.2, 220.), "wav", TakeKind::Audio, 1.2);
    let v3 = add(&s, &voice_wav(tmp.path(), "v3.wav", 0.8, 330.), "wav", TakeKind::Audio, 0.8);
    s.save_episode(&Episode { id: "ep01".into(), ..Episode::default() }).expect("episode");
    let voiced = |id: &str, text: &str, take: &str| {
        let mut l = Line { id: id.into(), scene: "sc1".into(), spoken: text.into(), kind: LineKind::Speech, ..Line::default() };
        l.voice.selected = Some(take.to_string());
        l.voice.takes = vec![take.to_string()];
        l
    };
    s.save_story(&Story {
        episode: "ep01".into(),
        scenes: vec![Scene { id: "sc1".into(), kind: SceneKind::Dialogue, ..Scene::default() }],
        lines: vec![voiced("l1", "We should not be here.", &v1), voiced("l3", "Run!", &v3)],
        ..Story::default()
    })
    .expect("story");
    s.save_scene_shots(&SceneShots {
        episode: "ep01".into(),
        scene: "sc1".into(),
        shots: vec![shot("sh1", &["l1"], Move::PushIn, stills[0].clone()), shot("sh2", &[], Move::PanRight, stills[1].clone()), shot("sh3", &["l3"], Move::Shake, stills[2].clone())],
        ..SceneShots::default()
    })
    .expect("shots");
    mv_anime::store::write_json(&s.path("episodes/ep01/cut.json"), &CutPlan { subtitles: true, ..CutPlan::default() }).expect("cut settings");

    let plan = animatic::plan_cut(&s, "ep01").expect("plan");
    assert_eq!(plan.shots.len(), 3);
    assert!(plan.shots.iter().all(|c| c.missing.is_empty() && c.still.is_some()), "{:?}", plan.shots.iter().map(|c| &c.missing).collect::<Vec<_>>());
    let frames: u32 = plan.shots.iter().map(|c| animatic::frames(c.seconds)).sum();
    let out = tmp.path().join("exports").join("animatic-test.mp4");
    let seen = Mutex::new(Vec::<f32>::new());
    let token = mv_ai::CancellationToken::new();
    let started = Instant::now();
    let made = animatic::render(&s, &plan, &out, |p, _| seen.lock().expect("lock").push(p), &token).await.expect("rendered");
    let first = started.elapsed();
    assert_eq!(made, out);
    assert!(out.is_file() && out.with_extension("srt").is_file());
    let seen = seen.into_inner().expect("lock");
    assert!(seen.windows(2).all(|w| w[1] >= w[0] - 1e-6), "progress only goes forward");
    assert!((seen.last().copied().unwrap_or(0.) - 1.).abs() < 1e-6);

    let exe = ffmpeg::ffprobe().expect("ffprobe installed");
    let info = probe(&exe, &out);
    let streams = info["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").expect("a video stream");
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").expect("an audio stream");
    let subs = streams.iter().find(|s| s["codec_type"] == "subtitle").expect("a subtitle stream");
    let duration: f32 = info["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.);
    let read_frames: u32 = video["nb_read_frames"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0);
    println!(
        "ANIMATIC planned {:.3} s / {frames} frames; ffprobe: duration {duration:.3} s, video {} {}x{} {read_frames} frames, audio {} {} Hz, subtitles {}; rendered in {:.1} s",
        plan.seconds,
        video["codec_name"],
        video["width"],
        video["height"],
        audio["codec_name"],
        audio["sample_rate"],
        subs["codec_name"],
        first.as_secs_f32()
    );
    assert_eq!(video["width"], 1280);
    assert_eq!(video["height"], 720);
    assert_eq!(read_frames, frames, "every planned frame is there");
    assert!((duration - plan.seconds).abs() < 0.1, "{duration} vs {}", plan.seconds);
    assert_eq!(audio["sample_rate"], "48000");

    // Again: every segment comes from the cache, and the dialogue track is the planned length.
    let started = Instant::now();
    animatic::render(&s, &plan, &out, |_, _| {}, &token).await.expect("rendered again");
    let again = started.elapsed();
    let segs = std::fs::read_dir(s.work_dir().join("cut").join("seg")).expect("segments").flatten().filter(|e| e.path().extension().is_some_and(|x| x == "mp4")).count();
    assert_eq!(segs, 3, "one cached segment per shot");
    let dialogue = hound::WavReader::open(s.work_dir().join("cut").join("dialogue.wav")).expect("dialogue");
    assert_eq!(dialogue.spec().sample_rate, 48_000);
    assert_eq!(dialogue.duration(), frames * 2000, "48 kHz × frames / 24");
    println!("ANIMATIC second render (cached segments) {:.1} s", again.as_secs_f32());
}
