//! Static voice catalog and preview/Kokoro helpers. Ids, labels, descriptions,
//! languages and tiers are generated from `src/renderer/pages/AudioPage.tsx`
//! (`VOICE_PRESETS`); engine/preview rules mirror `src/main/ipc/tts.ipc.ts`.

use std::path::{Path, PathBuf};

use crate::bridge::run_capture;
use crate::error::StudioError;
use crate::python::{PythonInterpreter, PythonLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Ultra,
    Natural,
    Standard,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    /// `hi` or `en`.
    pub lang: &'static str,
    pub tier: Tier,
}

/// All voices offered in the Audio tab, in display order.
pub const VOICES: &[VoiceInfo] = &[
    VoiceInfo {
        id: "f5_hi_female",
        label: "Hindi — Studio Female",
        desc: "F5-TTS · most natural Hindi",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_hi_male",
        label: "Hindi — Studio Male",
        desc: "F5-TTS · realistic male narrator",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_hi_storyteller",
        label: "Hindi — Studio Storyteller",
        desc: "F5-TTS · expressive cinematic",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_hi_narrator",
        label: "Hindi — Studio Narrator",
        desc: "F5-TTS · clean long-form voice",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_hi_female_clear",
        label: "Hindi — Clear Female",
        desc: "Parler-TTS · clean studio Hindi",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_hi_female_warm",
        label: "Hindi — Warm Female",
        desc: "Parler-TTS · warm storyteller",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_hi_male_deep",
        label: "Hindi — Deep Male",
        desc: "Parler-TTS · deep narrator",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_hi_male_bright",
        label: "Hindi — Bright Male",
        desc: "Parler-TTS · youthful male tone",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_hi_female",
        label: "Hindi — Ultra Female",
        desc: "XTTS v2 · most human-like",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_hi_male",
        label: "Hindi — Ultra Male",
        desc: "XTTS v2 · deep natural narrator",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_hi_storyteller",
        label: "Hindi — Storyteller",
        desc: "XTTS v2 · cinematic female",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_hi_narrator",
        label: "Hindi — Narrator",
        desc: "XTTS v2 · audiobook male",
        lang: "hi",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "hf_alpha",
        label: "Hindi — Natural Female α",
        desc: "Kokoro-82M · warm Devanagari",
        lang: "hi",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "hf_beta",
        label: "Hindi — Natural Female β",
        desc: "Kokoro-82M · soft alternate",
        lang: "hi",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "hm_omega",
        label: "Hindi — Natural Male Ω",
        desc: "Kokoro-82M · confident male",
        lang: "hi",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "hm_psi",
        label: "Hindi — Natural Male ψ",
        desc: "Kokoro-82M · deep male",
        lang: "hi",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "hi_piper_f",
        label: "Hindi — Priyamvada",
        desc: "Piper · clear female",
        lang: "hi",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "hi_piper_m",
        label: "Hindi — Pratham",
        desc: "Piper · steady male",
        lang: "hi",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "hi_piper_rohan",
        label: "Hindi — Rohan",
        desc: "Piper · young male",
        lang: "hi",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "hi-lekha",
        label: "Hindi — Lekha (macOS)",
        desc: "System · no download",
        lang: "hi",
        tier: Tier::System,
    },
    VoiceInfo {
        id: "cb_narrator_f",
        label: "English — Expressive Female",
        desc: "Chatterbox · acts out each scene (GPU)",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "cb_narrator_m",
        label: "English — Expressive Male",
        desc: "Chatterbox · acts out each scene (GPU)",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "cb_storyteller_m",
        label: "English — Expressive Storyteller",
        desc: "Chatterbox · deeper male, acts out each scene (GPU)",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_en_female",
        label: "English — Studio Female",
        desc: "F5-TTS · warm premium narrator",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_en_male",
        label: "English — Studio Male",
        desc: "F5-TTS · calm realistic narrator",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_en_narrator",
        label: "English — Studio Narrator",
        desc: "F5-TTS · audiobook voice",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "f5_en_storyteller",
        label: "English — Studio Storyteller",
        desc: "F5-TTS · expressive drama",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_en_female_warm",
        label: "English — Warm Female",
        desc: "Parler-TTS · comfortable narration",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "parler_en_male_narrator",
        label: "English — Deep Narrator",
        desc: "Parler-TTS · premium male tone",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_female",
        label: "English — Ultra Female",
        desc: "XTTS v2 · human audiobook tone",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_male",
        label: "English — Ultra Male",
        desc: "XTTS v2 · calm narrator",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_narrator",
        label: "English — Ultra Narrator",
        desc: "XTTS v2 · female audiobook",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_storyteller",
        label: "English — Ultra Storyteller",
        desc: "XTTS v2 · cinematic male",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_warm",
        label: "English — Ultra Warm",
        desc: "XTTS v2 · friendly female",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "xtts_en_clear",
        label: "English — Ultra Clear",
        desc: "XTTS v2 · crisp male",
        lang: "en",
        tier: Tier::Ultra,
    },
    VoiceInfo {
        id: "af_heart",
        label: "English — Heart (US)",
        desc: "Kokoro · warm female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "af_bella",
        label: "English — Bella (US)",
        desc: "Kokoro · bright female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "af_nova",
        label: "English — Nova (US)",
        desc: "Kokoro · energetic female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "af_sky",
        label: "English — Sky (US)",
        desc: "Kokoro · soft female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "af_nicole",
        label: "English — Nicole (US)",
        desc: "Kokoro · smooth female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "af_jessica",
        label: "English — Jessica (US)",
        desc: "Kokoro · friendly female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "am_michael",
        label: "English — Michael (US)",
        desc: "Kokoro · confident male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "am_adam",
        label: "English — Adam (US)",
        desc: "Kokoro · deep male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "am_echo",
        label: "English — Echo (US)",
        desc: "Kokoro · calm male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "am_eric",
        label: "English — Eric (US)",
        desc: "Kokoro · steady male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "am_liam",
        label: "English — Liam (US)",
        desc: "Kokoro · young male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "bf_emma",
        label: "English — Emma (UK)",
        desc: "Kokoro · British female",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "bf_isabella",
        label: "English — Isabella (UK)",
        desc: "Kokoro · elegant British",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "bm_george",
        label: "English — George (UK)",
        desc: "Kokoro · British male",
        lang: "en",
        tier: Tier::Natural,
    },
    VoiceInfo {
        id: "en_piper_f",
        label: "English — Amy",
        desc: "Piper · US female",
        lang: "en",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "en_piper_m",
        label: "English — Lessac",
        desc: "Piper · US male",
        lang: "en",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "en_piper_f2",
        label: "English — Kristin",
        desc: "Piper · smooth female",
        lang: "en",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "en_piper_m2",
        label: "English — Ryan",
        desc: "Piper · natural male",
        lang: "en",
        tier: Tier::Standard,
    },
    VoiceInfo {
        id: "hi-male",
        label: "Indian — Rishi (macOS)",
        desc: "System · Indian-English male",
        lang: "en",
        tier: Tier::System,
    },
    VoiceInfo {
        id: "say-default",
        label: "macOS — Default",
        desc: "System fallback",
        lang: "en",
        tier: Tier::System,
    },
];

/// The full voice catalog.
pub fn voice_catalog() -> &'static [VoiceInfo] {
    VOICES
}

/// Looks up a voice by id.
pub fn find_voice(id: &str) -> Option<&'static VoiceInfo> {
    VOICES.iter().find(|v| v.id == id)
}

/// Engine implied by a voice id (prefix rules from `tts_bridge.py` / `kokoro_tts.py`).
pub fn engine_hint(voice: &str) -> &'static str {
    if matches!(voice, "hi-lekha" | "hi-male" | "say-default") {
        "say"
    } else if voice.starts_with("xtts_") {
        "xtts"
    } else if voice.starts_with("f5_") {
        "f5-tts"
    } else if voice.starts_with("parler_") {
        "parler-tts"
    } else if voice.starts_with("hi_piper_") || voice.starts_with("en_piper_") {
        "piper"
    } else if voice.starts_with("cb_") {
        "chatterbox"
    } else {
        "kokoro"
    }
}

/// Display name of a `kokoro_tts.py` engine id.
pub fn engine_label(engine: &str) -> &'static str {
    match engine {
        "kokoro" => "Kokoro",
        "piper" => "Piper",
        "f5-tts" => "F5-TTS",
        "parler-tts" => "Parler-TTS",
        "xtts" => "XTTS",
        "chatterbox" => "Chatterbox",
        "say" => "macOS",
        _ => "Local",
    }
}

/// `hi` or `en` for a voice id (catalog language, else `kokoro_tts.py`'s prefix rules).
pub fn voice_lang(voice: &str) -> &'static str {
    if let Some(v) = find_voice(voice) {
        return v.lang;
    }
    let hindi = ["xtts_hi_", "hi_piper", "f5_hi_", "parler_hi_", "hf_", "hm_"].iter().any(|p| voice.starts_with(p))
        || matches!(voice, "hi-lekha" | "hi-female" | "hi-male" | "hi-male-aman" | "hi-male-deep");
    if hindi {
        "hi"
    } else {
        "en"
    }
}

/// The Kokoro voice that stands in for each studio-grade voice (F5-TTS, Parler-TTS, XTTS v2,
/// Chatterbox) until its engine is installed. Same table as `STUDIO_KOKORO_STAND_IN` in `kokoro_tts.py`.
pub const STAND_INS: &[(&str, &str)] = &[
    ("f5_hi_female", "hf_alpha"),
    ("f5_hi_male", "hm_omega"),
    ("f5_hi_storyteller", "hm_psi"),
    ("f5_hi_narrator", "hm_omega"),
    ("parler_hi_female_clear", "hf_beta"),
    ("parler_hi_female_warm", "hf_alpha"),
    ("parler_hi_male_deep", "hm_psi"),
    ("parler_hi_male_bright", "hm_omega"),
    ("xtts_hi_female", "hf_alpha"),
    ("xtts_hi_male", "hm_omega"),
    ("xtts_hi_storyteller", "hf_beta"),
    ("xtts_hi_narrator", "hm_psi"),
    ("f5_en_female", "af_heart"),
    ("f5_en_male", "am_michael"),
    ("f5_en_narrator", "bf_emma"),
    ("f5_en_storyteller", "af_nova"),
    ("parler_en_female_neutral", "af_nicole"),
    ("parler_en_female_warm", "af_heart"),
    ("parler_en_male_narrator", "am_adam"),
    ("parler_en_male_casual", "am_liam"),
    ("xtts_en_female", "af_heart"),
    ("xtts_en_male", "am_echo"),
    ("xtts_en_narrator", "bf_emma"),
    ("xtts_en_storyteller", "am_adam"),
    ("xtts_en_warm", "af_jessica"),
    ("xtts_en_clear", "bm_george"),
    ("cb_narrator_f", "af_heart"),
    ("cb_narrator_m", "am_michael"),
    ("cb_storyteller_m", "am_adam"),
];

/// The Piper voice matching each Kokoro voice (`PIPER_VOICE_MAP` in `kokoro_tts.py`).
const KOKORO_PIPER: &[(&str, &str)] = &[
    ("hf_alpha", "hi_piper_f"),
    ("hf_beta", "hi_piper_f"),
    ("hm_omega", "hi_piper_m"),
    ("hm_psi", "hi_piper_rohan"),
    ("af_heart", "en_piper_f"),
    ("af_bella", "en_piper_f"),
    ("af_nova", "en_piper_f"),
    ("af_sky", "en_piper_f"),
    ("af_nicole", "en_piper_f2"),
    ("af_jessica", "en_piper_f2"),
    ("am_michael", "en_piper_m"),
    ("am_adam", "en_piper_m"),
    ("am_echo", "en_piper_m2"),
    ("am_eric", "en_piper_m2"),
    ("am_liam", "en_piper_m2"),
    ("bf_emma", "en_piper_f2"),
    ("bf_isabella", "en_piper_f2"),
    ("bm_george", "en_piper_m2"),
];

/// Piper voices by language and gender, preferred first (`PIPER_BY_LANG_GENDER`).
fn piper_voices(lang: &str, male: bool) -> &'static [&'static str] {
    match (lang, male) {
        ("hi", false) => &["hi_piper_f"],
        ("hi", true) => &["hi_piper_m", "hi_piper_rohan"],
        (_, false) => &["en_piper_f", "en_piper_f2"],
        (_, true) => &["en_piper_m", "en_piper_m2"],
    }
}

/// Kokoro stand-in of a studio-grade voice.
pub fn stand_in(voice: &str) -> Option<&'static str> {
    STAND_INS.iter().find(|(v, _)| *v == voice).map(|(_, k)| *k)
}

fn is_male(voice: &str) -> bool {
    let v = stand_in(voice).unwrap_or(voice);
    ["am_", "bm_", "hm_"].iter().any(|p| v.starts_with(p)) || matches!(v, "en_piper_m" | "en_piper_m2" | "hi_piper_m" | "hi_piper_rohan")
}

/// Voices that stand in, best first, when `voice`'s own engine cannot run; mirrors
/// `fallback_candidates` in `kokoro_tts.py`. Studio-grade voices try their Kokoro stand-in,
/// then Piper voices of their language (same gender first); Kokoro voices try their matching
/// Piper voice, then the same list. Piper and system voices have none.
pub fn fallback_candidates(voice: &str) -> Vec<&'static str> {
    let hint = engine_hint(voice);
    if matches!(hint, "piper" | "say") {
        return vec![];
    }
    let lang = voice_lang(voice);
    let kokoro = if hint == "kokoro" { Some(voice) } else { stand_in(voice) };
    let mut out: Vec<&'static str> = vec![];
    if hint != "kokoro" {
        out.extend(stand_in(voice));
    }
    if let Some(p) = kokoro.and_then(|k| KOKORO_PIPER.iter().find(|(v, _)| *v == k)).map(|(_, p)| *p) {
        out.push(p);
    }
    let male = is_male(voice);
    out.extend(piper_voices(lang, male).iter().chain(piper_voices(lang, !male)));
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| *v != voice && seen.insert(*v));
    out
}

/// Engine `tts.ipc.ts` sends for a preview when the caller does not name one.
pub fn default_preview_engine(voice: &str) -> &'static str {
    if matches!(voice, "hi-lekha" | "hi-male" | "say-default") {
        "say"
    } else {
        "kokoro"
    }
}

/// Voice id reduced to a filesystem-safe token, as `tts.ipc.ts` does.
pub fn safe_voice_name(voice: &str) -> String {
    voice
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Cache file for a preview clip: `<samples>/<voice>_<engine>_<speed:.1>_v3.wav`.
pub fn preview_cache_path(samples_dir: &Path, voice: &str, engine: &str, speed: f64) -> PathBuf {
    samples_dir.join(format!(
        "{}_{}_{:.1}_v3.wav",
        safe_voice_name(voice),
        engine,
        speed
    ))
}

/// Default samples directory: `~/.mangavox/voice-samples`.
pub fn default_samples_dir() -> PathBuf {
    crate::python::mangavox_home().join("voice-samples")
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewResult {
    pub path: PathBuf,
    pub engine: String,
    pub cached: bool,
    pub preview_fallback: bool,
}

/// Synthesizes (or reuses) a short preview clip for `voice`, like the
/// `TTS_VOICE_PREVIEW` handler. Cached clips of 500 bytes or less are regenerated.
/// `tmp_dir` receives the temporary args file, which is removed afterwards.
pub async fn voice_preview(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
    samples_dir: &Path,
    tmp_dir: &Path,
    voice: &str,
    speed: f64,
    engine: Option<&str>,
) -> Result<PreviewResult, StudioError> {
    let speed = if speed.is_finite() && speed != 0.0 {
        speed
    } else {
        1.0
    };
    let eng = engine
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| default_preview_engine(voice));
    std::fs::create_dir_all(samples_dir)?;
    let out = preview_cache_path(samples_dir, voice, eng, speed);
    if std::fs::metadata(&out)
        .map(|m| m.len() > 500)
        .unwrap_or(false)
    {
        return Ok(PreviewResult {
            path: out,
            engine: eng.to_string(),
            cached: true,
            preview_fallback: false,
        });
    }

    let script = layout.script("voice_preview.py")?;
    let args = serde_json::json!({
        "voice": voice,
        "speed": speed,
        "engine": eng,
        "output_path": out,
    });
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let args_path = tmp_dir.join(format!("mangavox-preview-{stamp}.json"));
    std::fs::write(&args_path, serde_json::to_string(&args)?)?;

    let run = run_capture(
        interpreter,
        &script,
        &[args_path.display().to_string()],
        &layout.dir,
    )
    .await;
    let _ = std::fs::remove_file(&args_path);
    let (code, stdout, stderr) = run?;

    let parsed: Option<serde_json::Value> = serde_json::from_str(last_nonempty_line(&stdout)).ok();
    let ok = parsed
        .as_ref()
        .and_then(|p| p.get("ok"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    match parsed {
        Some(p) if ok => {
            let path = p
                .get("path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .ok_or_else(|| StudioError::Preview("Preview synthesis failed".into()))?;
            Ok(PreviewResult {
                path,
                engine: p
                    .get("engine")
                    .and_then(|v| v.as_str())
                    .unwrap_or(eng)
                    .to_string(),
                cached: false,
                preview_fallback: p
                    .get("previewFallback")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        }
        other => {
            let msg = other
                .as_ref()
                .and_then(|p| p.get("message"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Err(StudioError::Preview(
                msg.or_else(|| non_empty(&stderr))
                    .or_else(|| non_empty(&stdout))
                    .unwrap_or_else(|| format!("preview exited {code:?}")),
            ))
        }
    }
}

/// Installs one engine's Python packages (`kokoro` or `piper`) into the managed voice
/// environment with `kokoro_tts.py install-engine`, reporting each step's message.
pub async fn install_engine(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
    engine: &str,
    cancel: &crate::bridge::CancelToken,
    mut on_step: impl FnMut(&str) + Send,
) -> Result<(), StudioError> {
    let spec = crate::bridge::WorkerSpec {
        interpreter: interpreter.clone(),
        script: layout.script("kokoro_tts.py")?,
        args: vec!["install-engine".into(), engine.into()],
        cwd: layout.dir.clone(),
        strict_errors: true,
    };
    crate::bridge::run_worker_logged(
        &spec,
        cancel,
        |m| {
            if let Some(text) = m.message.as_deref() {
                on_step(text);
            }
        },
        |_| {},
    )
    .await
    .map(|_| ())
}

// ---------------------------------------------------------------- emotion and voice lab

/// Scene kinds the emotion mapping knows (`narration_tts.KINDS`, from `mv_recap::Scene::kind`).
pub const SCENE_KINDS: &[&str] = &["fight", "action", "dialogue", "emotional", "comedy", "reveal", "calm", "flashback", "romance", "horror", "other"];

/// Intensities (0-10) the voice lab renders when the caller names none.
pub const LAB_INTENSITIES: &[u8] = &[2, 5, 8];

/// The voice acts out each scene (graded emotion from the scene's kind and intensity).
/// Other engines only follow the scene's pace (speed).
pub fn is_expressive(voice: &str) -> bool {
    engine_hint(voice) == "chatterbox"
}

/// What the voice lab renders: one sentence for a scene kind at several intensities.
#[derive(Debug, Clone, PartialEq)]
pub struct LabRequest {
    pub voice: String,
    /// One of [`SCENE_KINDS`].
    pub kind: String,
    /// The sentence; `None` uses the kind's built-in sample (`tts_bridge.LAB_TEXT`).
    pub text: Option<String>,
    /// 0-10 each; empty means [`LAB_INTENSITIES`].
    pub intensities: Vec<u8>,
    /// The chapter's emotion strength, 0-2 (1 = as read).
    pub strength: f32,
    /// A WAV for a Chatterbox voice to clone instead of its own reference clip.
    pub reference: Option<PathBuf>,
}

impl LabRequest {
    pub fn new(voice: &str, kind: &str) -> Self {
        Self { voice: voice.into(), kind: kind.into(), text: None, intensities: vec![], strength: 1.0, reference: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabSample {
    pub intensity: u8,
    pub path: PathBuf,
    /// Seconds of audio.
    pub duration: f64,
    /// Synthesis time / audio time (lower is faster; < 1 is faster than real time).
    pub rtf: Option<f64>,
    /// What the engine was given: Chatterbox `{exaggeration, cfg_weight, temperature, ...}`,
    /// Kokoro/Piper `{speed}`.
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabResult {
    pub voice: String,
    /// Engine that spoke (`chatterbox`, `kokoro`, `piper`).
    pub engine: String,
    /// The voice that spoke (a stand-in when the voice's own engine is missing).
    pub synth_voice: String,
    pub kind: String,
    pub text: String,
    pub samples: Vec<LabSample>,
}

/// Default folder for voice lab clips: `~/.mangavox/voice-samples/lab`.
pub fn default_lab_dir() -> PathBuf {
    default_samples_dir().join("lab")
}

/// Voice lab: renders one sample sentence for `req.kind` at each intensity with
/// `voice_preview.py lab` (Chatterbox loads once for all of them). Clips land in `out_dir`.
pub async fn voice_lab(interpreter: &PythonInterpreter, layout: &PythonLayout, out_dir: &Path, tmp_dir: &Path, req: &LabRequest) -> Result<LabResult, StudioError> {
    std::fs::create_dir_all(out_dir)?;
    let script = layout.script("voice_preview.py")?;
    let intensities: Vec<u8> = if req.intensities.is_empty() { LAB_INTENSITIES.to_vec() } else { req.intensities.iter().map(|i| (*i).min(10)).collect() };
    let args = serde_json::json!({
        "voice": req.voice,
        "kind": req.kind,
        "text": req.text,
        "intensities": intensities,
        "strength": req.strength.clamp(0.0, 2.0),
        "reference": req.reference,
        "out_dir": out_dir,
    });
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let args_path = tmp_dir.join(format!("mangavox-lab-{stamp}.json"));
    std::fs::write(&args_path, serde_json::to_string(&args)?)?;
    let run = run_capture(interpreter, &script, &["lab".to_string(), args_path.display().to_string()], &layout.dir).await;
    let _ = std::fs::remove_file(&args_path);
    let (code, stdout, stderr) = run?;
    match serde_json::from_str::<serde_json::Value>(last_nonempty_line(&stdout)) {
        Ok(v) => parse_lab(&v),
        Err(_) => Err(StudioError::Preview(non_empty(&stderr).or_else(|| non_empty(&stdout)).unwrap_or_else(|| format!("voice lab exited {code:?}")))),
    }
}

/// Parses `voice_preview.py lab` output.
pub fn parse_lab(v: &serde_json::Value) -> Result<LabResult, StudioError> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    if !v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
        return Err(StudioError::Preview(v.get("message").and_then(|x| x.as_str()).unwrap_or("The voice lab failed").to_string()));
    }
    let samples = v
        .get("samples")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(LabSample {
                        intensity: x.get("intensity")?.as_f64()?.round().clamp(0.0, 10.0) as u8,
                        path: PathBuf::from(x.get("path")?.as_str()?),
                        duration: x.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0),
                        rtf: x.get("rtf").and_then(|d| d.as_f64()),
                        params: x.get("params").cloned().unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(LabResult { voice: s("voice"), engine: s("engine"), synth_voice: s("synthVoice"), kind: s("kind"), text: s("text"), samples })
}

/// Runs `kokoro_tts.py status` and returns its JSON object.
pub async fn kokoro_status(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
) -> Result<serde_json::Value, StudioError> {
    kokoro_json(interpreter, layout, &["status".to_string()]).await
}

/// Runs `kokoro_tts.py voice-ready <voice>`.
pub async fn kokoro_voice_ready(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
    voice: &str,
) -> Result<serde_json::Value, StudioError> {
    let voice = voice.trim();
    if voice.is_empty() {
        return Ok(serde_json::json!({ "ready": false, "engine": "none" }));
    }
    kokoro_json(
        interpreter,
        layout,
        &["voice-ready".to_string(), voice.to_string()],
    )
    .await
}

/// Runs `kokoro_tts.py voices-ready`: `{voice: {ready, engine, fallbackVoice, fallbackEngine,
/// missing, message?}}` for every catalog voice, probed in one process.
pub async fn kokoro_voices_ready(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
) -> Result<serde_json::Value, StudioError> {
    kokoro_json(interpreter, layout, &["voices-ready".to_string()]).await
}

/// Mirrors `runKokoroJson`: the last stdout line is parsed; a non-zero exit is
/// only an error when the payload does not report `installed`.
async fn kokoro_json(
    interpreter: &PythonInterpreter,
    layout: &PythonLayout,
    args: &[String],
) -> Result<serde_json::Value, StudioError> {
    let script = layout.script("kokoro_tts.py")?;
    let (code, stdout, stderr) = run_capture(interpreter, &script, args, &layout.dir).await?;
    match serde_json::from_str::<serde_json::Value>(last_nonempty_line(&stdout)) {
        Ok(parsed) => {
            let installed = parsed
                .get("installed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if code != Some(0) && !installed {
                let msg = parsed
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                Err(StudioError::Preview(
                    msg.or_else(|| non_empty(&stderr))
                        .unwrap_or_else(|| format!("kokoro_tts exited {code:?}")),
                ))
            } else {
                Ok(parsed)
            }
        }
        Err(_) if code != Some(0) => Err(StudioError::Preview(
            non_empty(&stderr)
                .or_else(|| non_empty(&stdout))
                .unwrap_or_else(|| format!("kokoro_tts exited {code:?}")),
        )),
        Err(_) => Ok(serde_json::json!({ "raw": stdout })),
    }
}

fn last_nonempty_line(s: &str) -> &str {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_lookup_works() {
        let mut ids: Vec<&str> = VOICES.iter().map(|v| v.id).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(total, ids.len());
        assert_eq!(find_voice("hf_alpha").map(|v| v.lang), Some("hi"));
        assert_eq!(find_voice("af_heart").map(|v| v.tier), Some(Tier::Natural));
        assert!(find_voice("nope").is_none());
    }

    #[test]
    fn engine_rules_match_ipc() {
        assert_eq!(default_preview_engine("hi-male"), "say");
        assert_eq!(default_preview_engine("say-default"), "say");
        assert_eq!(default_preview_engine("f5_en_male"), "kokoro");
        assert_eq!(engine_hint("xtts_en_warm"), "xtts");
        assert_eq!(engine_hint("parler_hi_male_deep"), "parler-tts");
        assert_eq!(engine_hint("en_piper_m"), "piper");
        assert_eq!(engine_hint("bm_george"), "kokoro");
        assert_eq!(engine_hint("cb_narrator_f"), "chatterbox");
        assert_eq!(engine_label("chatterbox"), "Chatterbox");
        assert!(is_expressive("cb_narrator_m") && !is_expressive("af_heart"));
    }

    #[test]
    fn lab_output_parses() {
        let v = serde_json::json!({"ok": true, "voice": "cb_narrator_m", "synthVoice": "cb_narrator_m", "engine": "chatterbox", "kind": "fight", "text": "x",
            "samples": [{"intensity": 2, "path": "a.wav", "duration": 3.5, "rtf": 0.4, "params": {"exaggeration": 0.7}},
                        {"intensity": 8, "path": "b.wav", "duration": 3.1, "rtf": null, "params": {"exaggeration": 1.15}}]});
        let r = parse_lab(&v).unwrap();
        assert_eq!(r.engine, "chatterbox");
        assert_eq!(r.samples.len(), 2);
        assert_eq!(r.samples[1].intensity, 8);
        assert_eq!(r.samples[0].rtf, Some(0.4));
        assert_eq!(r.samples[1].params["exaggeration"], 1.15);
        let err = parse_lab(&serde_json::json!({"ok": false, "message": "needs Chatterbox"})).unwrap_err();
        assert!(err.to_string().contains("needs Chatterbox"));
    }

    /// Scene kinds must match `KINDS` in python/narration_tts.py.
    #[test]
    fn scene_kinds_match_narration_tts_py() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../python/narration_tts.py");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let line = text.lines().find(|l| l.starts_with("KINDS = (")).expect("KINDS in narration_tts.py");
        let py: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        assert_eq!(py, SCENE_KINDS);
    }

    #[test]
    fn studio_voices_fall_back_to_kokoro_then_piper() {
        assert_eq!(fallback_candidates("f5_en_storyteller"), vec!["af_nova", "en_piper_f", "en_piper_f2", "en_piper_m", "en_piper_m2"]);
        assert_eq!(fallback_candidates("xtts_hi_narrator"), vec!["hm_psi", "hi_piper_rohan", "hi_piper_m", "hi_piper_f"]);
        assert_eq!(fallback_candidates("am_echo"), vec!["en_piper_m2", "en_piper_m", "en_piper_f", "en_piper_f2"]);
        assert!(fallback_candidates("en_piper_m2").is_empty());
        assert!(fallback_candidates("say-default").is_empty());
        assert_eq!(fallback_candidates("cb_narrator_m"), vec!["am_michael", "en_piper_m", "en_piper_m2", "en_piper_f", "en_piper_f2"]);
        assert_eq!(fallback_candidates("cb_narrator_f")[0], "af_heart");
        // Every studio-grade voice in the catalog has a Kokoro stand-in in its own language.
        for v in VOICES.iter().filter(|v| matches!(engine_hint(v.id), "f5-tts" | "parler-tts" | "xtts" | "chatterbox")) {
            let k = stand_in(v.id).unwrap_or_else(|| panic!("{} has no stand-in", v.id));
            assert_eq!(engine_hint(k), "kokoro", "{}", v.id);
            assert_eq!(voice_lang(k), v.lang, "{}", v.id);
        }
    }

    /// The stand-in table must match `STUDIO_KOKORO_STAND_IN` in python/kokoro_tts.py.
    #[test]
    fn stand_ins_match_kokoro_tts_py() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../python/kokoro_tts.py");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let start = text.find("STUDIO_KOKORO_STAND_IN").expect("table in kokoro_tts.py");
        let body = &text[start..];
        let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
        let pairs: Vec<(String, String)> = body
            .lines()
            .filter_map(|l| {
                let mut q = l.split('"').skip(1).step_by(2);
                Some((q.next()?.to_string(), q.next()?.to_string()))
            })
            .collect();
        let rust: Vec<(String, String)> = STAND_INS.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(pairs, rust);
    }

    #[test]
    fn cache_path_matches_ipc_naming() {
        let p = preview_cache_path(Path::new("s"), "my voice/1", "kokoro", 1.25);
        assert_eq!(p.file_name().unwrap(), "my_voice_1_kokoro_1.2_v3.wav");
        let q = preview_cache_path(Path::new("s"), "hf_alpha", "kokoro", 1.0);
        assert_eq!(q.file_name().unwrap(), "hf_alpha_kokoro_1.0_v3.wav");
    }

    #[test]
    fn last_line_helper_skips_blank_tail() {
        assert_eq!(last_nonempty_line("a\n{\"ok\":true}\n\n"), "{\"ok\":true}");
        assert_eq!(last_nonempty_line(""), "");
    }
}
