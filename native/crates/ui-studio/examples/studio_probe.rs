//! Exercises Studio's backend on this machine without the GUI:
//! `cargo run -p ui-studio --example studio_probe [-- --run]`
//!
//! Prints the setup check (Python, scripts, FFmpeg, voice engines, page-text
//! reader) and the downloaded chapters. With `--run`, copies the first downloaded
//! chapter's pages to a temp folder and runs a real audio-only pipeline job on the
//! copy (the user's library is not touched), streaming its progress and logs.
use std::time::Duration;

use ui_studio::data;
use ui_studio::service::{build_job, NarrationRequest};
use ui_studio::setup::{self, VoiceReady};

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let env = rt.block_on(setup::check());

    println!("== Setup");
    match &env.python {
        Ok(p) => println!("python   : OK  {} ({})", p.version, p.command_line()),
        Err(e) => println!("python   : MISSING  {e}\n           fix: {}", setup::python_install_cmd()),
    }
    match &env.scripts {
        Ok(l) => println!("scripts  : OK  {}", l.dir.display()),
        Err(e) => println!("scripts  : MISSING  {e}"),
    }
    println!("ffmpeg   : {}", if env.ffmpeg { "OK".to_string() } else { format!("MISSING  fix: {}", setup::ffmpeg_install_cmd()) });
    println!("modules  : {:?}", env.modules);
    println!("engines  : {:?}", env.engines);
    if let Some(e) = &env.tts_error {
        println!("tts error: {e}");
    }
    if !env.engines.any() {
        if let Some(c) = env.tts_packages_cmd() {
            println!("fix tts  : {c}");
        }
        if let Some(c) = env.voice_models_cmd() {
            println!("fix model: {c}");
        }
    }
    println!("page text: {:?}", env.ocr);

    println!("\n== Voices (English)");
    for v in setup::voices("en") {
        let r = setup::voice_ready(&env.engines, v);
        let tag = match r {
            VoiceReady::Ready(e) => format!("ready ({e})"),
            VoiceReady::Fallback(e) => format!("fallback ({e})"),
            VoiceReady::Missing => "not installed".into(),
        };
        println!("  {:<24} {:<34} {tag}", v.id, v.label);
    }
    println!("  ... {} Hindi voices", setup::voices("hi").len());

    // `--lab [voice] [kind]`: the voice lab (one sentence at intensity 2/5/8).
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--lab") {
        let voice = args.get(i + 1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "cb_narrator_f".into());
        let kind = args.get(i + 2).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "fight".into());
        println!("\n== Voice lab: {voice}, {kind}");
        if let (Ok(py), Ok(layout)) = (&env.python, &env.scripts) {
            let req = mv_studio::tts::LabRequest::new(&voice, &kind);
            let dir = mv_studio::tts::default_lab_dir();
            match rt.block_on(mv_studio::tts::voice_lab(py, layout, &dir, &std::env::temp_dir(), &req)) {
                Ok(r) => {
                    println!("  engine {} (voice {}): {}", r.engine, r.synth_voice, r.text);
                    for s in &r.samples {
                        println!("  intensity {:>2}: {:.2}s rtf {:?} {} -> {}", s.intensity, s.duration, s.rtf, s.params, s.path.display());
                    }
                }
                Err(e) => println!("  failed: {e}"),
            }
        }
    }

    println!("\n== Downloaded chapters");
    let groups = data::load()?;
    for g in &groups {
        for c in &g.chapters {
            println!(
                "  {} / {}: {} pages, text={}, audio={}, video={}",
                g.title,
                c.label(),
                c.page_count,
                c.has_text,
                c.audio_files,
                c.video.is_some()
            );
        }
    }

    if !std::env::args().any(|a| a == "--run") {
        return Ok(());
    }
    let Some(ch) = groups.iter().flat_map(|g| g.chapters.iter()).next() else {
        println!("no downloaded chapter to run");
        return Ok(());
    };
    let (Ok(py), Ok(layout)) = (&env.python, &env.scripts) else {
        println!("cannot run: python or scripts missing");
        return Ok(());
    };

    // Work on a copy so the user's chapter folder is untouched.
    let tmp = std::env::temp_dir().join(format!("mv-studio-probe-{}", std::process::id()));
    let pages = tmp.join("pages");
    std::fs::create_dir_all(&pages)?;
    for entry in std::fs::read_dir(&ch.pages_dir)?.flatten().take(3) {
        std::fs::copy(entry.path(), pages.join(entry.file_name()))?;
    }
    let mut copy = ch.clone();
    copy.pages_dir = pages;
    copy.output_dir = tmp.join("pipeline");
    copy.has_text = false;
    let req = NarrationRequest { voice: "af_heart".into(), speed: 1.0, video: false, resolution: "720p".into(), format: mv_recap::FORMAT_LANDSCAPE.into(), stages: None, force_regen: true, regen_pages: vec![], new_take: false };
    let job = build_job(&req, &env, &copy);
    println!("\n== Running job on a 3-page copy in {}\nstages: {:?}", tmp.display(), job.stages);

    rt.block_on(async {
        let queue = mv_studio::JobQueue::new(py.clone(), layout.clone());
        let mut rx = queue.subscribe();
        queue.enqueue(copy.id.clone(), mv_studio::JobPayload::Pipeline(job));
        loop {
            match tokio::time::timeout(Duration::from_secs(600), rx.recv()).await {
                Ok(Ok(mv_studio::JobEvent::Message { message, .. })) => println!("  msg  {}", message.raw),
                Ok(Ok(mv_studio::JobEvent::Log { line, .. })) => println!("  log  {line}"),
                Ok(Ok(mv_studio::JobEvent::Started { id, .. })) => println!("  job {id} started"),
                Ok(Ok(mv_studio::JobEvent::Finished { state, .. })) => {
                    println!("  FINISHED: {state:?}");
                    break;
                }
                Ok(Err(e)) => {
                    println!("  channel: {e}");
                    break;
                }
                Err(_) => {
                    println!("  timed out");
                    break;
                }
            }
        }
    });
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
