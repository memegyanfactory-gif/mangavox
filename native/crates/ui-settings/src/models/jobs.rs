//! The jobs MangaVox can hand to a local model, which detected models fit each job, and
//! where the choice is stored. Pure logic plus small blocking config helpers.
use anyhow::Result;
use serde_json::{json, Map, Value};

use super::scan::{is_comfy_base, Category, EngineId, Inventory, LocalModel, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    Assistant,
    Vision,
    Voice,
    Transcription,
    Image,
}

impl Job {
    pub const ALL: [Job; 5] = [Job::Assistant, Job::Vision, Job::Voice, Job::Transcription, Job::Image];

    pub fn id(self) -> &'static str {
        match self {
            Job::Assistant => "assistant",
            Job::Vision => "vision",
            Job::Voice => "voice",
            Job::Transcription => "transcription",
            Job::Image => "image",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Job::Assistant => "Assistant chat",
            Job::Vision => "Reading manga pages",
            Job::Voice => "Narration voice",
            Job::Transcription => "Transcription",
            Job::Image => "Image generation",
        }
    }

    pub fn category(self) -> Category {
        match self {
            Job::Assistant | Job::Vision => Category::Language,
            Job::Voice => Category::Voice,
            Job::Transcription => Category::Transcription,
            Job::Image => Category::ImageGen,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Job::Assistant => "icons/message-square.svg",
            Job::Vision => "icons/scan-eye.svg",
            Job::Voice => "icons/audio-lines.svg",
            Job::Transcription => "icons/captions.svg",
            Job::Image => "icons/image.svg",
        }
    }

    /// One line on what the model does for MangaVox.
    pub fn explain(self) -> &'static str {
        match self {
            Job::Assistant => "Answers questions and runs tools in the assistant panel. Local models run through Ollama or LM Studio.",
            Job::Vision => "Studio reads speech bubbles with a vision model in Ollama before narrating a chapter.",
            Job::Voice => "Studio narrates chapters with this voice. Kokoro and Piper run offline on this PC.",
            Job::Transcription => "Turns speech into text. Saved now so upcoming subtitle features can use it.",
            Job::Image => "Generates artwork through ComfyUI. Saved now so upcoming Studio features can use it.",
        }
    }

    /// True when a MangaVox feature already reads this choice.
    pub fn wired(self) -> bool {
        matches!(self, Job::Assistant | Job::Vision | Job::Voice)
    }

    /// The engine that has to run for the job, given the chosen model.
    pub fn engine(self, model: Option<&LocalModel>) -> Option<EngineId> {
        match self {
            Job::Assistant => match model.map(|m| m.source) {
                Some(Source::LmStudio) => Some(EngineId::LmStudio),
                _ => Some(EngineId::Ollama),
            },
            Job::Vision => Some(EngineId::Ollama),
            Job::Voice | Job::Transcription => Some(EngineId::PythonTts),
            Job::Image => Some(EngineId::ComfyUi),
        }
    }
}

/// One entry of a job's model picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// Stored value (Ollama tag, `provider::model`, voice id, or a model id).
    pub value: String,
    pub label: String,
    /// The detected model behind it, for the info line.
    pub model_id: Option<String>,
}

/// Piper voice ids from the Studio catalog and the voice files they need (`kokoro_tts.py`).
pub const PIPER_VOICE_FILES: &[(&str, &str)] = &[
    ("hi_piper_f", "hi_IN-priyamvada-medium"),
    ("hi_piper_m", "hi_IN-pratham-medium"),
    ("hi_piper_rohan", "hi_IN-rohan-medium"),
    ("en_piper_f", "en_US-amy-medium"),
    ("en_piper_m", "en_US-lessac-medium"),
    ("en_piper_f2", "en_US-kristin-medium"),
    ("en_piper_m2", "en_US-ryan-medium"),
];

fn voice_model(id: &str) -> Option<String> {
    match mv_studio::tts::engine_hint(id) {
        "kokoro" => Some("kokoro:v1.0".into()),
        "piper" => PIPER_VOICE_FILES.iter().find(|(v, _)| *v == id).map(|(_, stem)| format!("piper:{stem}")),
        _ => None,
    }
}

pub fn voice_label(id: &str) -> String {
    mv_studio::tts::find_voice(id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string())
}

fn assistant_value(m: &LocalModel) -> Option<String> {
    match m.source {
        Source::Ollama => Some(format!("ollama::{}", m.key)),
        Source::LmStudio => Some(format!("lmstudio::{}", m.key)),
        _ => None,
    }
}

/// Models that can do `job`, as picker entries. Labels are unique.
pub fn choices(job: Job, inv: &Inventory) -> Vec<Choice> {
    let mut out: Vec<Choice> = match job {
        Job::Assistant => inv
            .models
            .iter()
            .filter(|m| m.category == Category::Language)
            .filter_map(|m| Some(Choice { value: assistant_value(m)?, label: format!("{} · {}", m.name, m.source.label()), model_id: Some(m.id.clone()) }))
            .collect(),
        Job::Vision => inv
            .models
            .iter()
            .filter(|m| m.source == Source::Ollama && m.vision)
            .map(|m| Choice { value: m.key.clone(), label: m.name.clone(), model_id: Some(m.id.clone()) })
            .collect(),
        Job::Voice => {
            let present: std::collections::HashSet<String> = inv.models.iter().map(|m| m.id.clone()).collect();
            mv_studio::tts::voice_catalog()
                .iter()
                .filter_map(|v| {
                    let model = voice_model(v.id)?;
                    present.contains(&model).then(|| Choice { value: v.id.to_string(), label: v.label.to_string(), model_id: Some(model) })
                })
                .collect()
        }
        Job::Transcription => inv
            .models
            .iter()
            .filter(|m| m.category == Category::Transcription)
            .map(|m| Choice { value: m.id.clone(), label: m.name.clone(), model_id: Some(m.id.clone()) })
            .collect(),
        Job::Image => inv
            .models
            .iter()
            .filter(|m| m.category == Category::ImageGen && (m.source != Source::ComfyUi || is_comfy_base(&m.family)))
            .map(|m| Choice { value: m.id.clone(), label: m.name.clone(), model_id: Some(m.id.clone()) })
            .collect(),
    };
    // Select items are matched by label, so make duplicates distinct.
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for c in &mut out {
        let n = seen.entry(c.label.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            c.label = format!("{} ({})", c.label, n);
        }
    }
    out
}

/// What is chosen today, as read from config (and the assistant's own prefs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selections {
    /// (provider id, model) from the assistant prefs.
    pub assistant: Option<(String, String)>,
    pub vision: Option<String>,
    pub voice: Option<String>,
    pub transcription: Option<String>,
    pub image: Option<String>,
}

impl Selections {
    pub fn raw(&self, job: Job) -> Option<String> {
        match job {
            Job::Assistant => self.assistant.as_ref().map(|(p, m)| format!("{p}::{m}")),
            Job::Vision => self.vision.clone(),
            Job::Voice => self.voice.clone(),
            Job::Transcription => self.transcription.clone(),
            Job::Image => self.image.clone(),
        }
    }

    pub fn set(&mut self, job: Job, value: String) {
        match job {
            Job::Assistant => {
                if let Some((p, m)) = value.split_once("::") {
                    self.assistant = Some((p.to_string(), m.to_string()));
                }
            }
            Job::Vision => self.vision = Some(value),
            Job::Voice => self.voice = Some(value),
            Job::Transcription => self.transcription = Some(value),
            Job::Image => self.image = Some(value),
        }
    }
}

/// How a job is covered right now.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// The chosen model is on this PC.
    Uses { label: String, model_id: Option<String> },
    /// Nothing chosen; MangaVox picks this one by itself.
    Auto { label: String, model_id: Option<String> },
    /// The assistant is set to a cloud provider.
    Cloud { label: String },
    /// Chosen, but the files are not on this PC.
    Missing { label: String },
    None,
}

impl Status {
    pub fn model_id(&self) -> Option<&str> {
        match self {
            Status::Uses { model_id, .. } | Status::Auto { model_id, .. } => model_id.as_deref(),
            _ => None,
        }
    }
}

fn ollama_matches(installed: &str, wanted: &str) -> bool {
    installed == wanted || installed.strip_suffix(":latest") == Some(wanted) || wanted.strip_suffix(":latest") == Some(installed)
}

pub fn status(job: Job, sel: &Selections, inv: &Inventory, list: &[Choice]) -> Status {
    let find = |value: &str| list.iter().find(|c| c.value == value);
    match job {
        Job::Assistant => {
            let Some((provider, model)) = sel.assistant.as_ref().filter(|(_, m)| !m.is_empty()) else { return Status::None };
            match provider.as_str() {
                "ollama" | "lmstudio" => {
                    let src = if provider == "ollama" { Source::Ollama } else { Source::LmStudio };
                    match inv.models.iter().find(|m| m.source == src && (m.key == *model || src == Source::Ollama && ollama_matches(&m.key, model))) {
                        Some(m) => Status::Uses { label: format!("{} ({})", m.name, src.label()), model_id: Some(m.id.clone()) },
                        None => Status::Missing { label: model.clone() },
                    }
                }
                other => Status::Cloud { label: format!("{model} via {}", provider_label(other)) },
            }
        }
        Job::Vision => {
            let configured = sel.vision.as_deref().filter(|v| !v.is_empty());
            if let Some(v) = configured {
                let base = v.split(':').next().unwrap_or(v);
                if let Some(c) = list.iter().find(|c| ollama_matches(&c.value, v)).or_else(|| list.iter().find(|c| c.value.starts_with(base))) {
                    return Status::Uses { label: c.label.clone(), model_id: c.model_id.clone() };
                }
            }
            match list.first() {
                Some(c) => Status::Auto { label: c.label.clone(), model_id: c.model_id.clone() },
                None => match configured {
                    Some(v) => Status::Missing { label: v.to_string() },
                    None => Status::None,
                },
            }
        }
        _ => match sel.raw(job).filter(|v| !v.is_empty()) {
            Some(v) => match find(&v) {
                Some(c) => Status::Uses { label: c.label.clone(), model_id: c.model_id.clone() },
                None => Status::Missing { label: if job == Job::Voice { voice_label(&v) } else { inv.model(&v).map(|m| m.name.clone()).unwrap_or(v) } },
            },
            None => Status::None,
        },
    }
}

fn provider_label(id: &str) -> String {
    mv_ai::PROVIDERS.iter().find(|p| p.id.as_str() == id).map(|p| p.label.to_string()).unwrap_or_else(|| id.to_string())
}

// ── Config ──────────────────────────────────────────────────────────────────

const STORE_KEY: &str = "modelLibrary";

fn str_at(v: &Option<Value>, key: &str) -> Option<String> {
    v.as_ref()?.get(key)?.as_str().map(str::to_string).filter(|s| !s.trim().is_empty())
}

/// Reads the config-backed selections (the assistant's lives in its own store).
pub fn load_selections() -> Selections {
    let cfg = mv_core::config::Config::open_default();
    let ollama = cfg.get("ollama");
    let pipeline = cfg.get("pipeline");
    let picks = cfg.get(STORE_KEY).and_then(|v| v.get("selections").cloned());
    Selections {
        assistant: None,
        vision: str_at(&ollama, "vision_model"),
        voice: str_at(&pipeline, "tts_voice"),
        transcription: str_at(&picks, Job::Transcription.id()),
        image: str_at(&picks, Job::Image.id()),
    }
}

/// Persists a choice: the key the feature reads (`ollama.vision_model`, `pipeline.tts_voice`)
/// plus `modelLibrary.selections.<job>`. Other keys are left untouched.
pub fn save_choice(job: Job, value: &str) -> Result<()> {
    match job {
        Job::Vision => crate::data::update_object("ollama", |m| {
            m.insert("vision_model".into(), json!(value));
        })?,
        Job::Voice => crate::data::update_object("pipeline", |m| {
            m.insert("tts_voice".into(), json!(value));
        })?,
        _ => {}
    }
    crate::data::update_object(STORE_KEY, |m| {
        let sel = m.entry("selections").or_insert_with(|| Value::Object(Map::new()));
        if !sel.is_object() {
            *sel = Value::Object(Map::new());
        }
        if let Some(o) = sel.as_object_mut() {
            o.insert(job.id().into(), json!(value));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::scan::{Engine, LocalModel};
    use super::*;
    use std::path::PathBuf;

    fn model(id: &str, key: &str, source: Source, category: Category, vision: bool) -> LocalModel {
        LocalModel { id: id.into(), key: key.into(), name: key.into(), family: "f".into(), category, source, size: 1, path: PathBuf::from(id), vision, quant: None, thumbnail: None }
    }

    fn inv(models: Vec<LocalModel>) -> Inventory {
        Inventory { models, engines: Vec::<Engine>::new(), models_dir: PathBuf::new(), free_bytes: None, ollama_url: String::new() }
    }

    #[test]
    fn choices_per_job() {
        let i = inv(vec![
            model("ollama:llava:7b", "llava:7b", Source::Ollama, Category::Language, true),
            model("ollama:dolphin3:8b", "dolphin3:8b", Source::Ollama, Category::Language, false),
            model("lmstudio:qwen/qwen3.5-9b", "qwen/qwen3.5-9b", Source::LmStudio, Category::Language, true),
            model("gguf:a/b.gguf", "C:/b.gguf", Source::MangaVox, Category::Language, false),
            model("kokoro:v1.0", "k", Source::Kokoro, Category::Voice, false),
            model("piper:en_US-amy-medium", "en_US-amy-medium", Source::Piper, Category::Voice, false),
            model("comfy:loras/x", "x", Source::ComfyUi, Category::ImageGen, false),
        ]);
        let a = choices(Job::Assistant, &i);
        assert_eq!(a.len(), 3);
        assert!(a.iter().any(|c| c.value == "lmstudio::qwen/qwen3.5-9b"));
        let v = choices(Job::Vision, &i);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].value, "llava:7b");
        let voices = choices(Job::Voice, &i);
        assert!(voices.iter().any(|c| c.value == "bm_george"));
        assert!(voices.iter().any(|c| c.value == "en_piper_f"));
        assert!(!voices.iter().any(|c| c.value == "en_piper_m"));
        assert!(choices(Job::Image, &i).is_empty());
    }

    #[test]
    fn status_resolution() {
        let i = inv(vec![
            model("ollama:llava:latest", "llava:latest", Source::Ollama, Category::Language, true),
            model("lmstudio:qwen/qwen3.5-9b", "qwen/qwen3.5-9b", Source::LmStudio, Category::Language, true),
        ]);
        let mut s = Selections::default();
        let list = choices(Job::Vision, &i);
        assert!(matches!(status(Job::Vision, &s, &i, &list), Status::Auto { .. }));
        s.vision = Some("llava".into());
        assert!(matches!(status(Job::Vision, &s, &i, &list), Status::Uses { .. }));

        let list = choices(Job::Assistant, &i);
        assert_eq!(status(Job::Assistant, &s, &i, &list), Status::None);
        s.set(Job::Assistant, "lmstudio::qwen/qwen3.5-9b".into());
        assert!(matches!(status(Job::Assistant, &s, &i, &list), Status::Uses { .. }));
        s.assistant = Some(("openrouter".into(), "google/gemini-2.5-flash".into()));
        assert_eq!(status(Job::Assistant, &s, &i, &list), Status::Cloud { label: "google/gemini-2.5-flash via OpenRouter".into() });
        s.assistant = Some(("ollama".into(), "gone:1b".into()));
        assert!(matches!(status(Job::Assistant, &s, &i, &list), Status::Missing { .. }));

        let voices = choices(Job::Voice, &i);
        assert_eq!(status(Job::Voice, &s, &i, &voices), Status::None);
        s.voice = Some("bm_george".into());
        assert!(matches!(status(Job::Voice, &s, &i, &voices), Status::Missing { .. }));
    }
}
