//! A real Kokoro narration run through the worker bridge, with and without the system
//! resource limit. Needs the voice venv and the Kokoro model (Settings → Audio), so it is
//! ignored by default:
//! `cargo test -p mv-studio --test resource_limit -- --ignored --nocapture`
use std::path::PathBuf;
use std::time::Instant;

use mv_studio::bridge::run_capture;
use mv_studio::{PythonInterpreter, PythonLayout};

const TEXT: &str = "The rain had not stopped for three days when Kai finally reached the gate of the old city. \
He pulled his hood lower and counted the guards: four on the wall, two at the door, and one more he could not see but could feel. \
Ruko had warned him that the city never forgave a thief, and it never forgot a face. \
Still, the letter in his coat was worth the risk. If it reached the council before dawn, the war would end before it began. \
He took a breath, stepped out of the shadows, and walked straight toward the light as if he belonged there.";

fn venv_python() -> Option<PythonInterpreter> {
    let home = dirs::home_dir()?.join(".mangavox").join("tts-venv");
    let exe = if cfg!(windows) { home.join("Scripts").join("python.exe") } else { home.join("bin").join("python3") };
    exe.is_file().then(|| PythonInterpreter { program: exe.display().to_string(), prefix_args: vec![], version: String::new() })
}

/// Narrates TEXT `rounds` times under `percent`; returns (wall seconds, share of all CPUs used).
async fn narrate(percent: u8, rounds: usize) -> (f64, f64) {
    mv_core::limits::apply_percent(percent);
    let py = venv_python().expect("voice venv (install Kokoro in Settings → Audio)");
    let layout = PythonLayout::resolve(Some(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../python"))).unwrap();
    let script = layout.script("kokoro_tts.py").unwrap();
    let dir = std::env::temp_dir().join(format!("mv-cap-{}-{percent}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cpu0 = mv_core::limits::heavy_cpu_seconds();
    let start = Instant::now();
    for i in 0..rounds {
        let args = dir.join(format!("a{i}.json"));
        let out = dir.join(format!("o{i}.wav"));
        std::fs::write(&args, serde_json::json!({"text": TEXT, "voice": "af_heart", "speed": 1.0, "output_path": out}).to_string()).unwrap();
        let (code, stdout, stderr) = run_capture(&py, &script, &["synthesize".into(), args.display().to_string()], &layout.dir).await.unwrap();
        assert_eq!(code, Some(0), "{stdout}\n{stderr}");
        assert!(std::fs::metadata(&out).unwrap().len() > 10_000);
    }
    let wall = start.elapsed().as_secs_f64();
    let cpu = mv_core::limits::heavy_cpu_seconds() - cpu0;
    let _ = std::fs::remove_dir_all(&dir);
    (wall, cpu / (wall * mv_core::limits::cores() as f64))
}

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn narration_stays_under_the_cpu_cap() {
    let (wall_full, share_full) = narrate(100, 3).await;
    let (wall_cap, share_cap) = narrate(25, 3).await;
    println!("100%: {wall_full:.1}s wall, {:.0}% of all CPUs", share_full * 100.0);
    println!(" 25%: {wall_cap:.1}s wall, {:.0}% of all CPUs", share_cap * 100.0);
    assert!(share_cap <= 0.27, "capped run used {:.0}% of the CPUs", share_cap * 100.0);
}
