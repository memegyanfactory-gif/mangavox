//! Update tracking for the coding-agent CLIs the assistant can drive (Claude Code, Codex, Gemini CLI,
//! OpenCode, Grok, Antigravity): installed version vs latest, one-click update, optional auto-update.
//! Version/command logic is pure and unit-tested; I/O runs on the shared tokio runtime (`mv_ui::bg`).
use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::*;
use mv_ai::AiProviderId;
use mv_core::config::Config;
use serde_json::{json, Value};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub const KEY_AUTO: &str = "cli.autoUpdate";
pub const KEY_NOTIFY: &str = "cli.notifyUpdates";

/// Where the newest version number comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Latest {
    /// `https://registry.npmjs.org/<pkg>/latest`.
    Npm(&'static str),
    /// `grok update --check --json` (`latestVersion`).
    GrokCheck,
    /// No way to ask; only the installed version is shown.
    Unknown,
}

pub struct CliSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub bin: &'static str,
    pub provider: AiProviderId,
    pub latest: Latest,
    /// Program and arguments that update the CLI.
    pub update: &'static [&'static str],
}

pub const CLIS: &[CliSpec] = &[
    CliSpec { id: "claude", name: "Claude Code", bin: "claude", provider: AiProviderId::ClaudeCode, latest: Latest::Npm("@anthropic-ai/claude-code"), update: &["claude", "update"] },
    CliSpec { id: "codex", name: "Codex", bin: "codex", provider: AiProviderId::Codex, latest: Latest::Npm("@openai/codex"), update: &["npm", "i", "-g", "@openai/codex@latest"] },
    CliSpec { id: "gemini", name: "Gemini CLI", bin: "gemini", provider: AiProviderId::GeminiCli, latest: Latest::Npm("@google/gemini-cli"), update: &["npm", "i", "-g", "@google/gemini-cli@latest"] },
    CliSpec { id: "opencode", name: "OpenCode", bin: "opencode", provider: AiProviderId::Opencode, latest: Latest::Npm("opencode-ai"), update: &["npm", "i", "-g", "opencode-ai@latest"] },
    CliSpec { id: "grok", name: "Grok", bin: "grok", provider: AiProviderId::GrokCli, latest: Latest::GrokCheck, update: &["grok", "update"] },
    CliSpec { id: "agy", name: "Antigravity", bin: "agy", provider: AiProviderId::Antigravity, latest: Latest::Unknown, update: &["agy", "update"] },
];

pub fn spec(id: &str) -> Option<&'static CliSpec> {
    CLIS.iter().find(|s| s.id == id)
}

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// First version-looking token (`2.1.4`, `v0.5.1-beta.2`) in a `--version` output, without a leading `v`.
pub fn parse_version(out: &str) -> Option<String> {
    out.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',').find_map(|tok| {
        let t = tok.trim_start_matches(['v', 'V']);
        let numeric = t.split(['-', '+']).next().unwrap_or("");
        let parts: Vec<&str> = numeric.split('.').collect();
        (parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))).then(|| t.to_string())
    })
}

/// (numeric parts, has pre-release tag)
fn split_version(v: &str) -> (Vec<u64>, bool) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let pre = v.split('+').next().unwrap_or("").contains('-');
    let core = v.split(['-', '+']).next().unwrap_or("");
    (core.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre)
}

/// True when `latest` is strictly newer than `installed` (a pre-release is older than its release).
pub fn is_newer(latest: &str, installed: &str) -> bool {
    let (mut a, pa) = split_version(latest);
    let (mut b, pb) = split_version(installed);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => pb && !pa,
        std::cmp::Ordering::Less => false,
    }
}

/// `{"version":"1.2.3",...}` from the npm registry's `/latest` document.
pub fn parse_npm_latest(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok()?.get("version")?.as_str().map(String::from)
}

/// `latestVersion` from `grok update --check --json` (null when the check could not run).
pub fn parse_grok_check(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).find(|l| l.starts_with('{'))?;
    serde_json::from_str::<Value>(line).ok()?.get("latestVersion")?.as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// Last few non-empty lines, for a one-line result.
pub fn tail_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" · ")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliInfo {
    pub id: &'static str,
    /// `None` = not installed.
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub busy: bool,
    /// (ok, text) of the last update run.
    pub result: Option<(bool, String)>,
}

impl CliInfo {
    pub fn update_available(&self) -> bool {
        matches!((&self.installed, &self.latest), (Some(i), Some(l)) if is_newer(l, i))
    }
}

// ── Process / network (blocking-free async) ──────────────────────────────────

fn command(args: &[&str]) -> tokio::process::Command {
    #[cfg(windows)]
    let mut cmd = {
        // npm shims are `.cmd` files, so everything goes through cmd.exe. Arguments are our own constants.
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(format!("/d /s /c \"{}\"", args.join(" ")));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new(args[0]);
        c.args(&args[1..]);
        c
    };
    cmd.stdin(std::process::Stdio::null()).env("NO_COLOR", "1").kill_on_drop(true);
    cmd
}

/// (success, stdout + stderr). A missing program or a timeout is a failure with the reason as text.
async fn run(args: &[&str], timeout: Duration) -> (bool, String) {
    let mut cmd = command(args);
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, e.to_string()),
    };
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => (o.status.success(), format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Ok(Err(e)) => (false, e.to_string()),
        Err(_) => (false, "timed out".into()),
    }
}

async fn npm_latest(pkg: &str) -> Option<String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).user_agent("MangaVox").build().ok()?;
    let body = client.get(format!("https://registry.npmjs.org/{pkg}/latest")).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
    parse_npm_latest(&body)
}

pub async fn check_one(s: &'static CliSpec) -> CliInfo {
    let (ok, out) = run(&[s.bin, "--version"], VERSION_TIMEOUT).await;
    let installed = if ok { parse_version(&out) } else { None };
    let latest = if installed.is_none() {
        None
    } else {
        match s.latest {
            Latest::Npm(pkg) => npm_latest(pkg).await,
            Latest::GrokCheck => parse_grok_check(&run(&[s.bin, "update", "--check", "--json"], VERSION_TIMEOUT).await.1),
            Latest::Unknown => None,
        }
    };
    CliInfo { id: s.id, installed, latest, ..Default::default() }
}

async fn check_all() -> Vec<CliInfo> {
    let handles: Vec<_> = CLIS.iter().map(|s| tokio::spawn(check_one(s))).collect();
    let mut out = Vec::new();
    for (h, s) in handles.into_iter().zip(CLIS) {
        out.push(h.await.unwrap_or_else(|_| CliInfo { id: s.id, ..Default::default() }));
    }
    out
}

// ── Preferences (config.json top-level keys `cli.autoUpdate`, `cli.notifyUpdates`) ──

pub fn load_prefs() -> (bool, bool) {
    let cfg = Config::open_default();
    (
        cfg.get(KEY_AUTO).and_then(|v| v.as_bool()).unwrap_or(false),
        cfg.get(KEY_NOTIFY).and_then(|v| v.as_bool()).unwrap_or(true),
    )
}

fn save_pref(key: &'static str, on: bool) {
    let _ = Config::open_default().set(key, json!(on));
}

// ── State ────────────────────────────────────────────────────────────────────

pub struct CliUpdates {
    pub items: Vec<CliInfo>,
    pub auto_update: bool,
    pub notify_updates: bool,
    pub checking: bool,
    pub checked: bool,
    started: bool,
    /// Banner keys the user dismissed (`<id>@<latest>`, or the text of an "updated" message).
    dismissed: HashSet<String>,
    /// "Claude Code updated to 2.1.5" messages from silent auto-updates.
    pub updated: Vec<String>,
}

struct CliUpdatesGlobal(Entity<CliUpdates>);
impl Global for CliUpdatesGlobal {}

/// The shared tracker (created on first use).
pub fn cli_updates(cx: &mut App) -> Entity<CliUpdates> {
    if let Some(g) = cx.try_global::<CliUpdatesGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|_| CliUpdates {
        items: CLIS.iter().map(|s| CliInfo { id: s.id, ..Default::default() }).collect(),
        auto_update: false,
        notify_updates: true,
        checking: false,
        checked: false,
        started: false,
        dismissed: HashSet::new(),
        updated: Vec::new(),
    });
    cx.set_global(CliUpdatesGlobal(e.clone()));
    e
}

/// Start the background checks: once shortly after launch, then every 6 hours. Safe to call more than once.
pub fn start_cli_update_checks(cx: &mut App) {
    let e = cli_updates(cx);
    e.update(cx, |t, cx| t.start(cx));
}

/// Number of CLIs with an update available (for a badge); 0 when notifications are off.
pub fn cli_updates_available(cx: &App) -> usize {
    cx.try_global::<CliUpdatesGlobal>().map(|g| g.0.read(cx).banners().len()).unwrap_or(0)
}

impl CliUpdates {
    pub fn info(&self, id: &str) -> Option<&CliInfo> {
        self.items.iter().find(|i| i.id == id)
    }

    fn info_mut(&mut self, id: &str) -> Option<&mut CliInfo> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        cx.spawn(async move |this, cx| {
            let (auto, notify) = mv_ui::bg::blocking(load_prefs).await.unwrap_or((false, true));
            this.update(cx, |t, cx| {
                t.auto_update = auto;
                t.notify_updates = notify;
                cx.notify();
            })
            .ok();
            cx.background_executor().timer(FIRST_CHECK_DELAY).await;
            loop {
                if this.update(cx, |t, cx| t.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// Re-check every CLI now.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let infos = mv_ui::bg::run(check_all()).await.unwrap_or_default();
            this.update(cx, |t, cx| t.apply_check(infos, cx)).ok();
        })
        .detach();
    }

    fn apply_check(&mut self, infos: Vec<CliInfo>, cx: &mut Context<Self>) {
        for new in infos {
            if let Some(old) = self.info_mut(new.id) {
                *old = CliInfo { busy: old.busy, result: old.result.take(), ..new };
            }
        }
        self.checking = false;
        self.checked = true;
        if self.auto_update {
            let due: Vec<&'static str> = self.items.iter().filter(|i| i.update_available() && !i.busy).map(|i| i.id).collect();
            for id in due {
                self.run_update(id, true, cx);
            }
        }
        cx.notify();
    }

    /// Run the CLI's updater in the background. `silent` (auto-update) reports through `updated` banners.
    pub fn run_update(&mut self, id: &'static str, silent: bool, cx: &mut Context<Self>) {
        let Some(s) = spec(id) else { return };
        let Some(item) = self.info_mut(id) else { return };
        if item.busy {
            return;
        }
        item.busy = true;
        item.result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move {
                let (ok, out) = run(s.update, UPDATE_TIMEOUT).await;
                (ok, out, check_one(s).await)
            })
            .await;
            this.update(cx, |t, cx| t.finish_update(s, silent, r.ok(), cx)).ok();
        })
        .detach();
    }

    fn finish_update(&mut self, s: &'static CliSpec, silent: bool, r: Option<(bool, String, CliInfo)>, cx: &mut Context<Self>) {
        let (ok, out, fresh) = r.unwrap_or_else(|| (false, "cancelled".into(), CliInfo { id: s.id, ..Default::default() }));
        let mut message = None;
        if let Some(item) = self.info_mut(s.id) {
            item.busy = false;
            let still_old = fresh.update_available();
            let result = match (ok, &fresh.installed) {
                (true, Some(v)) if !still_old => {
                    message = Some(format!("{} updated to {v}", s.name));
                    (true, format!("Up to date ({v})"))
                }
                (true, Some(v)) => (false, format!("Updater finished but {} is still {v}. Restart the app or your terminal and check again", s.name)),
                (true, None) => (true, "Updated".to_string()),
                (false, _) => (false, tail_lines(&out)),
            };
            if fresh.installed.is_some() {
                item.installed = fresh.installed;
                item.latest = fresh.latest.or(item.latest.take());
            }
            item.result = Some(result);
        }
        if silent {
            if let Some(m) = message {
                self.updated.push(m);
            }
        }
        cx.notify();
    }

    pub fn set_auto_update(&mut self, on: bool, cx: &mut Context<Self>) {
        self.auto_update = on;
        cx.notify();
        cx.background_executor().spawn(async move { save_pref(KEY_AUTO, on) }).detach();
        if on {
            self.refresh(cx);
            let due: Vec<&'static str> = self.items.iter().filter(|i| i.update_available() && !i.busy).map(|i| i.id).collect();
            for id in due {
                self.run_update(id, true, cx);
            }
        }
    }

    pub fn set_notify_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.notify_updates = on;
        cx.notify();
        cx.background_executor().spawn(async move { save_pref(KEY_NOTIFY, on) }).detach();
    }

    /// Banner messages to show: (dismiss key, text). Pending updates only while notifications are on and
    /// the CLI is not already being updated; "updated" messages always.
    pub fn banners(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.notify_updates {
            for i in self.items.iter().filter(|i| i.update_available() && !i.busy) {
                let key = format!("{}@{}", i.id, i.latest.as_deref().unwrap_or(""));
                if !self.dismissed.contains(&key) {
                    let name = spec(i.id).map_or(i.id, |s| s.name);
                    out.push((key, format!("{name} {} is available (you have {})", i.latest.as_deref().unwrap_or(""), i.installed.as_deref().unwrap_or("?"))));
                }
            }
        }
        out.extend(self.updated.iter().filter(|m| !self.dismissed.contains(*m)).map(|m| (m.clone(), m.clone())));
        out
    }

    pub fn dismiss(&mut self, key: &str, cx: &mut Context<Self>) {
        self.dismissed.insert(key.to_string());
        self.updated.retain(|m| m != key);
        cx.notify();
    }

    pub fn available_count(&self) -> usize {
        self.items.iter().filter(|i| i.update_available()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn versions_are_parsed_from_real_outputs() {
        assert_eq!(parse_version("2.1.4 (Claude Code)").as_deref(), Some("2.1.4"));
        assert_eq!(parse_version("codex-cli 0.46.0\n").as_deref(), Some("0.46.0"));
        assert_eq!(parse_version("grok 1.0.62 (1474f9cbdd91) [alpha]").as_deref(), Some("1.0.62"));
        assert_eq!(parse_version("1.4.2").as_deref(), Some("1.4.2"));
        assert_eq!(parse_version("v0.5.1-beta.2").as_deref(), Some("0.5.1-beta.2"));
        assert_eq!(parse_version("'claude' is not recognized as a command"), None);
        assert_eq!(parse_version(""), None);
    }

    #[::core::prelude::v1::test]
    fn newer_comparison() {
        assert!(is_newer("2.1.5", "2.1.4"));
        assert!(is_newer("2.10.0", "2.9.9"));
        assert!(is_newer("1.0.63", "1.0.62"));
        assert!(!is_newer("2.1.4", "2.1.4"));
        assert!(!is_newer("2.1.3", "2.1.4"));
        assert!(!is_newer("1.4", "1.4.0"));
        assert!(is_newer("1.0.0", "1.0.0-beta.1")); // release beats its pre-release
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(is_newer("1.1.0-beta.1", "1.0.9"));
    }

    #[::core::prelude::v1::test]
    fn registry_and_grok_documents() {
        assert_eq!(parse_npm_latest(r#"{"name":"@openai/codex","version":"0.47.1","dist":{}}"#).as_deref(), Some("0.47.1"));
        assert_eq!(parse_npm_latest("not json"), None);
        let ok = r#"{"currentVersion":"1.0.62","latestVersion":"1.0.70","updateAvailable":true,"installer":"npm"}"#;
        assert_eq!(parse_grok_check(ok).as_deref(), Some("1.0.70"));
        let failed = r#"{"currentVersion":"1.0.62","latestVersion":null,"updateAvailable":false,"error":"program not found"}"#;
        assert_eq!(parse_grok_check(failed), None);
        assert_eq!(tail_lines("a\n\nb\nc\nd\n"), "b · c · d");
    }

    #[::core::prelude::v1::test]
    fn info_and_catalog() {
        let i = CliInfo { id: "claude", installed: Some("2.1.4".into()), latest: Some("2.1.5".into()), ..Default::default() };
        assert!(i.update_available());
        assert!(!CliInfo { latest: None, ..i.clone() }.update_available());
        assert!(!CliInfo { installed: None, ..i }.update_available());
        for s in CLIS {
            assert_eq!(s.update[0] == s.bin || s.update[0] == "npm", true, "{}", s.id);
            assert!(s.update.iter().all(|a| a.bytes().all(|b| b.is_ascii_alphanumeric() || b"-@./_".contains(&b))), "{}", s.id);
            assert_eq!(mv_ai::providers::cli::cli_binary(s.provider), Some(s.bin));
        }
        assert_eq!(spec("agy").unwrap().latest, Latest::Unknown);
    }
}
