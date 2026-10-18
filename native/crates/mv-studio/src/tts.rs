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
    } else {
        "kokoro"
    }
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
