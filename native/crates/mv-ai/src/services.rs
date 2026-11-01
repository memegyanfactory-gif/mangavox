//! Keys for AI services that are not chat models: image / video generation and voices.
//!
//! Keys live next to the provider keys in config.json, under `aiServices.<id>.apiKey`, else in
//! the service's usual environment variable. They are only ever sent to their own service, in a
//! header (never in a URL), and never logged. Each service has a "Test key" call that costs
//! nothing: an account, balance or voice-list read, or a lookup of a task that does not exist
//! (the key is checked before the task, so 401 means a bad key and 404 a good one).
//!
//! Endpoints were checked against each service's API docs in November 2026.

use crate::config::{object_entry, ConfigStore};
use crate::registry::{EnvFn, TestResult};
use crate::stream::{build_client, hdr};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// Image and video generation.
    Image,
    /// Text to speech.
    Voice,
}

/// How the key travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `Authorization: <scheme> <key>` ("Key" for fal.ai and Higgsfield, "Token" for Deepgram).
    Scheme(&'static str),
    /// `<header>: <key>`.
    Header(&'static str),
}

/// One free request that proves a key. Tried in order: a 401 / 403 moves on to the next probe
/// (a key scoped to fewer permissions, or for the service's other API), and only the last one
/// decides that the key is bad.
pub struct Probe {
    /// May hold `{now_ms}`, `{day_ago_ms}` and `{day_ago_rfc3339}`.
    pub url: &'static str,
    /// Statuses besides 2xx that still prove the key (404 for an unknown task id).
    pub accept: &'static [u16],
    /// A figure worth showing after a good test: (label, JSON pointer), e.g. ("Credits", "/credits").
    pub show: Option<(&'static str, &'static str)>,
}

pub struct ServiceDef {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ServiceKind,
    pub description: &'static str,
    /// Where to create a key.
    pub key_url: &'static str,
    pub key_env: &'static [&'static str],
    /// Input placeholder; says so when the key has two parts.
    pub placeholder: &'static str,
    pub auth: Auth,
    /// Headers every request needs besides the key (API version pins).
    pub extra: &'static [(&'static str, &'static str)],
    pub probes: &'static [Probe],
}

const fn probe(url: &'static str) -> Probe {
    Probe { url, accept: &[], show: None }
}

const fn showing(url: &'static str, label: &'static str, pointer: &'static str) -> Probe {
    Probe { url, accept: &[], show: Some((label, pointer)) }
}

const NO_EXTRA: &[(&str, &str)] = &[];

/// The catalog, in UI order within each kind.
pub static SERVICES: &[ServiceDef] = &[
    // ── Images and video ────────────────────────────────────────────────────────────────
    ServiceDef {
        id: "higgsfield",
        label: "Higgsfield",
        kind: ServiceKind::Image,
        description: "Soul images and cinematic video",
        key_url: "https://console.higgsfield.ai",
        key_env: &[],
        placeholder: "KEY_ID:KEY_SECRET",
        auth: Auth::Scheme("Key"),
        extra: NO_EXTRA,
        probes: &[Probe { url: "https://api.higgsfield.ai/requests/00000000-0000-0000-0000-000000000000/status", accept: &[404], show: None }],
    },
    ServiceDef {
        id: "fal",
        label: "fal.ai",
        kind: ServiceKind::Image,
        description: "Flux, Kling, Veo and hundreds more image and video models",
        key_url: "https://fal.ai/dashboard/keys",
        key_env: &["FAL_KEY"],
        placeholder: "Paste your fal.ai key",
        auth: Auth::Scheme("Key"),
        extra: NO_EXTRA,
        // 403: a key without the billing scope; it still generates.
        probes: &[Probe { url: "https://api.fal.ai/v1/account/billing?expand=credits", accept: &[403], show: Some(("Credits", "/credits/current_balance")) }],
    },
    ServiceDef {
        id: "replicate",
        label: "Replicate",
        kind: ServiceKind::Image,
        description: "Community image, video and audio models",
        key_url: "https://replicate.com/account/api-tokens",
        key_env: &["REPLICATE_API_TOKEN"],
        placeholder: "Paste your Replicate token",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        probes: &[showing("https://api.replicate.com/v1/account", "Account", "/username")],
    },
    ServiceDef {
        id: "runway",
        label: "Runway",
        kind: ServiceKind::Image,
        description: "Gen-4 video and images",
        key_url: "https://dev.runway.com",
        key_env: &["RUNWAYML_API_SECRET"],
        placeholder: "Paste your Runway API secret",
        auth: Auth::Bearer,
        extra: &[("X-Runway-Version", "2024-11-06")],
        probes: &[showing("https://api.dev.runwayml.com/v1/organization", "Credits", "/creditBalance")],
    },
    ServiceDef {
        id: "luma",
        label: "Luma AI",
        kind: ServiceKind::Image,
        description: "Dream Machine video and Photon images",
        key_url: "https://platform.lumalabs.ai",
        key_env: &["LUMAAI_API_KEY", "LUMA_AGENTS_API_KEY"],
        placeholder: "Paste your Luma key",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        // Dream Machine keys, then keys for Luma's newer Agents API.
        probes: &[
            showing("https://api.lumalabs.ai/dream-machine/v1/credits", "Credits", "/credit_balance"),
            showing("https://agents.lumalabs.ai/v1/credits", "Credits", "/balance_credits"),
        ],
    },
    ServiceDef {
        id: "kling",
        label: "Kling AI",
        kind: ServiceKind::Image,
        description: "Kling video and images (API key, not the older access key + secret pair)",
        key_url: "https://kling.ai/dev/api-key",
        key_env: &["KLING_API_KEY"],
        placeholder: "Paste your Kling API key",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        // "This API is free to call."
        probes: &[probe("https://api-singapore.klingai.com/account/costs?start_time={day_ago_ms}&end_time={now_ms}")],
    },
    ServiceDef {
        id: "stability",
        label: "Stability AI",
        kind: ServiceKind::Image,
        description: "Stable Image and Stable Diffusion",
        key_url: "https://platform.stability.ai/dashboard/api-keys",
        key_env: &["STABILITY_API_KEY"],
        placeholder: "Paste your Stability key",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        probes: &[showing("https://api.stability.ai/v1/user/balance", "Credits", "/credits")],
    },
    ServiceDef {
        id: "ideogram",
        label: "Ideogram",
        kind: ServiceKind::Image,
        description: "Images with clean text and lettering",
        key_url: "https://ideogram.ai/platform",
        key_env: &["IDEOGRAM_API_KEY"],
        placeholder: "Paste your Ideogram key",
        auth: Auth::Header("Api-Key"),
        extra: NO_EXTRA,
        // 404: a good key whose owner is not an organisation admin.
        probes: &[Probe { url: "https://api.ideogram.ai/v2/account/usage?start_time={day_ago_rfc3339}", accept: &[404], show: None }],
    },
    ServiceDef {
        id: "leonardo",
        label: "Leonardo.ai",
        kind: ServiceKind::Image,
        description: "Leonardo image models",
        key_url: "https://app.leonardo.ai/api-access",
        key_env: &["LEONARDO_API_KEY"],
        placeholder: "Paste your Leonardo key",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        probes: &[probe("https://cloud.leonardo.ai/api/rest/v1/me")],
    },
    ServiceDef {
        id: "bfl",
        label: "Black Forest Labs",
        kind: ServiceKind::Image,
        description: "FLUX models from their makers",
        key_url: "https://dashboard.bfl.ai",
        key_env: &["BFL_API_KEY"],
        placeholder: "Paste your BFL key",
        auth: Auth::Header("x-key"),
        extra: NO_EXTRA,
        probes: &[showing("https://api.bfl.ai/v1/credits", "Credits", "/credits")],
    },
    // ── Voices ──────────────────────────────────────────────────────────────────────────
    ServiceDef {
        id: "elevenlabs",
        label: "ElevenLabs",
        kind: ServiceKind::Voice,
        description: "Lifelike narration voices and voice cloning",
        key_url: "https://elevenlabs.io/app/api/api-keys",
        key_env: &["ELEVENLABS_API_KEY"],
        placeholder: "Paste your ElevenLabs key",
        auth: Auth::Header("xi-api-key"),
        extra: NO_EXTRA,
        // A restricted key may not read the account (403); the voice list proves it too.
        probes: &[showing("https://api.elevenlabs.io/v1/user", "Plan", "/subscription/tier"), probe("https://api.elevenlabs.io/v2/voices?page_size=1")],
    },
    ServiceDef {
        id: "cartesia",
        label: "Cartesia",
        kind: ServiceKind::Voice,
        description: "Sonic voices, fast and expressive",
        key_url: "https://play.cartesia.ai/keys",
        key_env: &["CARTESIA_API_KEY"],
        placeholder: "Paste your Cartesia key",
        auth: Auth::Bearer,
        extra: &[("Cartesia-Version", "2026-08-14")],
        probes: &[probe("https://api.cartesia.ai/voices")],
    },
    ServiceDef {
        id: "deepgram",
        label: "Deepgram",
        kind: ServiceKind::Voice,
        description: "Aura voices",
        key_url: "https://console.deepgram.com",
        key_env: &["DEEPGRAM_API_KEY"],
        placeholder: "Paste your Deepgram key",
        auth: Auth::Scheme("Token"),
        extra: NO_EXTRA,
        probes: &[probe("https://api.deepgram.com/v1/projects")],
    },
    ServiceDef {
        id: "fish",
        label: "Fish Audio",
        kind: ServiceKind::Voice,
        description: "Voices and cloning in many languages",
        key_url: "https://fish.audio/app/api-keys",
        key_env: &["FISH_API_KEY"],
        placeholder: "Paste your Fish Audio key",
        auth: Auth::Bearer,
        extra: NO_EXTRA,
        probes: &[showing("https://api.fish.audio/wallet/self/api-credit", "Credit", "/credit")],
    },
    ServiceDef {
        id: "hume",
        label: "Hume AI",
        kind: ServiceKind::Voice,
        description: "Octave voices that act out a line",
        key_url: "https://app.hume.ai/keys",
        key_env: &["HUME_API_KEY"],
        placeholder: "Paste your Hume key",
        auth: Auth::Header("X-Hume-Api-Key"),
        extra: NO_EXTRA,
        probes: &[probe("https://api.hume.ai/v0/tts/voices?provider=HUME_AI&page_size=1")],
    },
];

pub fn service_def(id: &str) -> Option<&'static ServiceDef> {
    SERVICES.iter().find(|s| s.id == id)
}

/// Where a service's key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Saved,
    Env,
}

/// The URL with its time placeholders filled in.
pub fn fill_url(template: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let ago = now - chrono::Duration::days(1);
    template
        .replace("{now_ms}", &now.timestamp_millis().to_string())
        .replace("{day_ago_ms}", &ago.timestamp_millis().to_string())
        .replace("{day_ago_rfc3339}", &ago.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The headers for one request with `key`.
pub fn headers(def: &ServiceDef, key: &str) -> Vec<(String, String)> {
    let mut h = vec![match def.auth {
        Auth::Bearer => hdr("authorization", format!("Bearer {key}")),
        Auth::Scheme(s) => hdr("authorization", format!("{s} {key}")),
        Auth::Header(name) => hdr(name, key),
    }];
    h.extend(def.extra.iter().map(|(k, v)| hdr(k, *v)));
    h
}

/// What one probe's answer means.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Ok(String),
    /// Refused; try the next probe if there is one.
    Refused(String),
    Failed(String),
}

/// The service's own words from an error body (`error.message`, `message`, `detail`, ...).
fn error_text(body: &str) -> String {
    let text = match serde_json::from_str::<Value>(body) {
        Ok(j) => ["/error/message", "/detail/message", "/message", "/err_msg", "/detail", "/error"]
            .iter()
            .find_map(|p| j.pointer(p).filter(|v| !v.is_null()).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))
            .unwrap_or_default(),
        Err(_) => body.trim().to_string(),
    };
    text.chars().take(160).collect()
}

/// Reads a probe's status and body.
pub fn judge(def: &ServiceDef, probe: &Probe, status: u16, body: &str) -> Verdict {
    let said = |prefix: String| {
        let t = error_text(body);
        if t.is_empty() {
            prefix
        } else {
            format!("{prefix}: {t}")
        }
    };
    if (200..300).contains(&status) || probe.accept.contains(&status) {
        let figure = probe.show.filter(|_| (200..300).contains(&status)).and_then(|(label, ptr)| {
            let v = serde_json::from_str::<Value>(body).ok()?.pointer(ptr).cloned()?;
            let shown = match v {
                Value::String(s) if !s.is_empty() => s,
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(format!(" · {label}: {shown}"))
        });
        return Verdict::Ok(format!("Key works{}", figure.unwrap_or_default()));
    }
    match status {
        401 | 403 => Verdict::Refused(said(format!("{} rejected the key ({status})", def.label))),
        // Past the key check: the account just has nothing to spend.
        402 => Verdict::Ok("Key works, but the account is out of credits".into()),
        429 => Verdict::Failed(format!("{} is rate limiting; try again in a minute", def.label)),
        s if s >= 500 => Verdict::Failed(format!("{} is having trouble right now ({s}); try again later", def.label)),
        s => Verdict::Failed(said(format!("{} answered {s}", def.label))),
    }
}

/// Saved service keys and their checks.
pub struct Services {
    config: ConfigStore,
    env: EnvFn,
    client: reqwest::Client,
}

impl Services {
    /// `config_path` is the app's config.json.
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self { config: ConfigStore::new(config_path), env: Arc::new(|k| std::env::var(k).ok()), client: build_client() }
    }

    /// Replace environment lookup (tests).
    pub fn with_env(mut self, env: EnvFn) -> Self {
        self.env = env;
        self
    }

    fn saved(&self, id: &str) -> Option<String> {
        let cfg = self.config.read();
        cfg.get("aiServices")?.get(id)?.get("apiKey")?.as_str().map(str::trim).filter(|k| !k.is_empty()).map(String::from)
    }

    fn from_env(&self, def: &ServiceDef) -> Option<String> {
        let get = |k: &str| (self.env)(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        // Higgsfield's SDKs read the two halves from two variables.
        if def.id == "higgsfield" {
            return Some(format!("{}:{}", get("HF_API_KEY_ID")?, get("HF_API_KEY_SECRET")?));
        }
        def.key_env.iter().find_map(|k| get(k))
    }

    /// The saved key, else the environment's.
    pub fn key(&self, id: &str) -> Option<String> {
        self.saved(id).or_else(|| self.from_env(service_def(id)?))
    }

    pub fn key_source(&self, id: &str) -> Option<KeySource> {
        if self.saved(id).is_some() {
            return Some(KeySource::Saved);
        }
        service_def(id).and_then(|d| self.from_env(d)).map(|_| KeySource::Env)
    }

    /// Saves the key, or forgets the saved one when `None` / empty.
    pub fn set_key(&self, id: &str, key: Option<&str>) -> std::io::Result<()> {
        let key = key.map(str::trim).filter(|k| !k.is_empty()).map(String::from);
        self.config.update(|cfg| {
            let all = object_entry(cfg, "aiServices");
            let entry = object_entry(all, id);
            match key {
                Some(k) => {
                    entry.insert("apiKey".into(), Value::String(k));
                }
                None => {
                    entry.remove("apiKey");
                }
            }
            if entry.is_empty() {
                all.remove(id);
            }
        })
    }

    /// One free round trip that proves the key works.
    pub async fn test(&self, id: &str) -> TestResult {
        let started = Instant::now();
        let done = |ok: bool, message: String| TestResult { ok, message, ms: started.elapsed().as_millis() as u64 };
        let Some(def) = service_def(id) else { return done(false, format!("Unknown service {id}")) };
        let Some(key) = self.key(id) else { return done(false, "Add a key first".into()) };
        if def.placeholder.contains(':') && !key.contains(':') {
            return done(false, format!("{} keys have two parts: paste them as {}", def.label, def.placeholder));
        }
        let mut last = String::new();
        for p in def.probes {
            let mut req = self.client.get(fill_url(p.url, chrono::Utc::now())).timeout(Duration::from_secs(15));
            for (k, v) in headers(def, &key) {
                req = req.header(k, v);
            }
            let res = match req.send().await {
                Ok(r) => r,
                Err(e) => return done(false, format!("Could not reach {}: {}", def.label, e.without_url())),
            };
            let status = res.status().as_u16();
            let body = res.text().await.unwrap_or_default();
            match judge(def, p, status, &body) {
                Verdict::Ok(m) => return done(true, m),
                Verdict::Failed(m) => return done(false, m),
                Verdict::Refused(m) => last = m,
            }
        }
        done(false, last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn def(id: &str) -> &'static ServiceDef {
        service_def(id).unwrap()
    }

    fn services(dir: &std::path::Path, env: &[(&str, &str)]) -> Services {
        let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Services::new(dir.join("config.json")).with_env(Arc::new(move |k| env.get(k).cloned()))
    }

    #[test]
    fn catalog_is_sound() {
        let ids: HashSet<_> = SERVICES.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), SERVICES.len());
        for s in SERVICES {
            assert!(s.key_url.starts_with("https://"), "{}", s.id);
            assert!(!s.probes.is_empty(), "{}", s.id);
            // Keys only ever travel in headers, never in a URL.
            for p in s.probes {
                assert!(p.url.starts_with("https://") && !p.url.to_lowercase().contains("key="), "{}", s.id);
            }
        }
        assert!(SERVICES.iter().any(|s| s.kind == ServiceKind::Voice) && SERVICES.iter().any(|s| s.kind == ServiceKind::Image));
    }

    #[test]
    fn headers_follow_each_service() {
        assert_eq!(headers(def("fal"), "k"), vec![hdr("authorization", "Key k")]);
        assert_eq!(headers(def("higgsfield"), "id:secret"), vec![hdr("authorization", "Key id:secret")]);
        assert_eq!(headers(def("deepgram"), "k"), vec![hdr("authorization", "Token k")]);
        assert_eq!(headers(def("elevenlabs"), "k"), vec![hdr("xi-api-key", "k")]);
        assert_eq!(headers(def("bfl"), "k"), vec![hdr("x-key", "k")]);
        assert_eq!(headers(def("runway"), "k"), vec![hdr("authorization", "Bearer k"), hdr("X-Runway-Version", "2024-11-06")]);
        assert_eq!(headers(def("cartesia"), "k")[1].0, "Cartesia-Version");
    }

    #[test]
    fn time_placeholders() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
        assert_eq!(fill_url("x?s={day_ago_rfc3339}", now), "x?s=2026-10-31T12:00:00Z");
        let u = fill_url(def("kling").probes[0].url, now);
        assert!(u.ends_with(&format!("start_time={}&end_time={}", now.timestamp_millis() - 86_400_000, now.timestamp_millis())), "{u}");
        assert!(!u.contains('{'));
    }

    #[test]
    fn verdicts() {
        let st = def("stability");
        assert_eq!(judge(st, &st.probes[0], 200, r#"{"credits":12.5}"#), Verdict::Ok("Key works · Credits: 12.5".into()));
        assert_eq!(judge(st, &st.probes[0], 200, "not json"), Verdict::Ok("Key works".into()));
        assert_eq!(
            judge(st, &st.probes[0], 401, r#"{"name":"unauthorized","message":"invalid key"}"#),
            Verdict::Refused("Stability AI rejected the key (401): invalid key".into())
        );
        assert_eq!(judge(st, &st.probes[0], 402, ""), Verdict::Ok("Key works, but the account is out of credits".into()));
        assert!(matches!(judge(st, &st.probes[0], 503, ""), Verdict::Failed(m) if m.contains("having trouble")));
        assert!(matches!(judge(st, &st.probes[0], 429, ""), Verdict::Failed(m) if m.contains("rate limiting")));
        // An unknown task id is a good key.
        let hf = def("higgsfield");
        assert_eq!(judge(hf, &hf.probes[0], 404, r#"{"detail":"Not found"}"#), Verdict::Ok("Key works".into()));
        assert!(matches!(judge(hf, &hf.probes[0], 401, r#"{"detail":"Invalid credentials"}"#), Verdict::Refused(m) if m.ends_with("Invalid credentials")));
        // fal: a key without billing scope still works.
        let fal = def("fal");
        assert_eq!(judge(fal, &fal.probes[0], 403, ""), Verdict::Ok("Key works".into()));
        assert_eq!(judge(fal, &fal.probes[0], 200, r#"{"username":"u","credits":{"current_balance":3}}"#), Verdict::Ok("Key works · Credits: 3".into()));
        // ElevenLabs nests its message.
        let el = def("elevenlabs");
        assert_eq!(
            judge(el, &el.probes[0], 401, r#"{"detail":{"code":"invalid_api_key","message":"Invalid API key"}}"#),
            Verdict::Refused("ElevenLabs rejected the key (401): Invalid API key".into())
        );
        assert!(matches!(judge(el, &el.probes[0], 400, r#"{"err_msg":"bad"}"#), Verdict::Failed(m) if m == "ElevenLabs answered 400: bad"));
    }

    #[test]
    fn keys_save_clear_and_fall_back_to_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), r#"{"theme":"dark"}"#).unwrap();
        let s = services(dir.path(), &[("ELEVENLABS_API_KEY", "env-el"), ("HF_API_KEY_ID", "id"), ("HF_API_KEY_SECRET", "sec")]);
        assert_eq!((s.key("elevenlabs").as_deref(), s.key_source("elevenlabs")), (Some("env-el"), Some(KeySource::Env)));
        assert_eq!(s.key("higgsfield").as_deref(), Some("id:sec"));
        assert_eq!((s.key("fal"), s.key_source("fal")), (None, None));
        s.set_key("elevenlabs", Some("  mine  ")).unwrap();
        assert_eq!((s.key("elevenlabs").as_deref(), s.key_source("elevenlabs")), (Some("mine"), Some(KeySource::Saved)));
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap()).unwrap();
        assert_eq!((cfg["theme"].as_str(), cfg["aiServices"]["elevenlabs"]["apiKey"].as_str()), (Some("dark"), Some("mine")));
        s.set_key("elevenlabs", None).unwrap();
        assert_eq!(s.key("elevenlabs").as_deref(), Some("env-el"));
        let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap()).unwrap();
        assert!(cfg["aiServices"].get("elevenlabs").is_none());
        assert!(s.key("nope").is_none());
    }

    #[tokio::test]
    async fn test_without_a_usable_key_makes_no_request() {
        let dir = tempfile::tempdir().unwrap();
        let s = services(dir.path(), &[]);
        let r = s.test("bfl").await;
        assert!(!r.ok && r.message == "Add a key first");
        s.set_key("higgsfield", Some("only-one-part")).unwrap();
        let r = s.test("higgsfield").await;
        assert!(!r.ok && r.message.contains("KEY_ID:KEY_SECRET"));
        assert!(!s.test("nope").await.ok);
    }
}
