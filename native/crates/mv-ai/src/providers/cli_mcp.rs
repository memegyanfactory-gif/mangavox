//! The one-time opt-in that lets Grok and Antigravity use the app's tools. Neither CLI has a
//! per-run way to load an MCP server (Grok loads a project's servers only in a folder the user
//! trusted, Antigravity only from its global config or a plugin), so when the user asks for it,
//! a `mangavox` stdio server is added to that CLI's own settings with its own `mcp add` command.
//! The server is the app's exe in bridge mode (`mangavox.exe --mcp-bridge`), which forwards to
//! whichever turn's MCP server is running (see `ui_assistant::mcp_bridge`).
//!
//! The entry being there, enabled and pointing at the bridge, is the opt-in: nothing else is
//! stored, and removing it (here or with the CLI itself) turns it off.
use super::cli::{find_binary, MCP_SERVER};
use crate::stream::{AiError, Result};
use crate::types::AiProviderId;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The app's command-line flag for the stdio bridge.
pub const BRIDGE_FLAG: &str = "--mcp-bridge";
/// Antigravity run settings (workspace file) that let the `mangavox` tools run without prompts.
pub const AGY_SETTINGS_FILE: &str = ".mangavox-agy-settings.json";
/// Grok's default MCP tool timeout is 60 s; a call can wait on the user's OK in Ask mode.
const GROK_TOOL_TIMEOUT_SECS: u64 = 3600;

/// CLIs that load the app's tools only through the opt-in entry.
pub fn bridged(id: AiProviderId) -> bool {
    matches!(id, AiProviderId::GrokCli | AiProviderId::Antigravity)
}

/// `grok mcp add ...` / `agy mcp add ...` arguments that register the bridge (after the binary).
pub fn add_args(id: AiProviderId, exe: &str) -> Option<Vec<String>> {
    let v: &[&str] = match id {
        AiProviderId::GrokCli => &["mcp", "add", "--scope", "user", MCP_SERVER, "--", exe, BRIDGE_FLAG],
        // Flags go before the name; `--` keeps `--mcp-bridge` for the server.
        AiProviderId::Antigravity => &["mcp", "add", MCP_SERVER, "--", exe, BRIDGE_FLAG],
        _ => return None,
    };
    Some(v.iter().map(|s| s.to_string()).collect())
}

/// Arguments that remove the entry again.
pub fn remove_args(id: AiProviderId) -> Option<Vec<String>> {
    let v: &[&str] = match id {
        AiProviderId::GrokCli => &["mcp", "remove", "--scope", "user", MCP_SERVER],
        AiProviderId::Antigravity => &["mcp", "remove", MCP_SERVER],
        _ => return None,
    };
    Some(v.iter().map(|s| s.to_string()).collect())
}

/// The CLI's own settings file that holds its global MCP servers.
pub fn config_file(id: AiProviderId) -> Option<PathBuf> {
    match id {
        AiProviderId::GrokCli => {
            let home = std::env::var_os("GROK_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".grok")))?;
            Some(home.join("config.toml"))
        }
        AiProviderId::Antigravity => Some(dirs::home_dir()?.join(".gemini").join("config").join("mcp_config.json")),
        _ => None,
    }
}

/// A `mangavox` entry that runs the bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeEntry {
    /// The exe it starts (may be another copy of the app).
    pub command: String,
    pub enabled: bool,
}

/// The bridge entry in a config file's text, if there is one.
pub fn parse_entry(id: AiProviderId, text: &str) -> Option<BridgeEntry> {
    match id {
        AiProviderId::GrokCli => {
            let body = grok_section(text)?;
            let value = |key: &str| body.lines().map(str::trim).find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('=').map(|v| v.trim().to_string()));
            if !value("args").is_some_and(|a| a.contains(BRIDGE_FLAG)) {
                return None;
            }
            Some(BridgeEntry { command: value("command").map(|c| toml_string(&c)).unwrap_or_default(), enabled: value("enabled").is_none_or(|v| v != "false") })
        }
        AiProviderId::Antigravity => {
            let v: Value = serde_json::from_str(text).ok()?;
            let e = v.get("mcpServers")?.get(MCP_SERVER)?;
            if !e.get("args")?.as_array()?.iter().any(|a| a.as_str() == Some(BRIDGE_FLAG)) {
                return None;
            }
            Some(BridgeEntry { command: e.get("command").and_then(Value::as_str).unwrap_or_default().to_string(), enabled: e.get("disabled").and_then(Value::as_bool) != Some(true) })
        }
        _ => None,
    }
}

/// The lines of Grok's `[mcp_servers.mangavox]` table (up to the next table).
fn grok_section(text: &str) -> Option<String> {
    let header = |l: &str| {
        let l = l.trim();
        l == format!("[mcp_servers.{MCP_SERVER}]") || l == format!("[mcp_servers.\"{MCP_SERVER}\"]")
    };
    let mut lines = text.lines().skip_while(|l| !header(l));
    lines.next()?;
    Some(lines.take_while(|l| !l.trim_start().starts_with('[')).collect::<Vec<_>>().join("\n"))
}

/// A TOML string value: 'literal' as is, "basic" with `\\` and `\"` unescaped.
fn toml_string(v: &str) -> String {
    if let Some(s) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return s.to_string();
    }
    let s = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
    s.replace("\\\\", "\u{0}").replace("\\\"", "\"").replace('\u{0}', "\\")
}

/// Grok's config with a long tool timeout added to the `mangavox` table (`None` when it has one
/// already or there is no such table).
pub fn with_grok_timeout(text: &str) -> Option<String> {
    let body = grok_section(text)?;
    if body.lines().any(|l| l.trim_start().starts_with("tool_timeout_sec")) {
        return None;
    }
    let mut out = String::new();
    let mut done = false;
    for line in text.split_inclusive('\n') {
        out.push_str(line);
        if !done && line.trim() == format!("[mcp_servers.{MCP_SERVER}]") {
            if !line.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("tool_timeout_sec = {GROK_TOOL_TIMEOUT_SECS}\n"));
            done = true;
        }
    }
    done.then_some(out)
}

#[cfg(test)]
thread_local! {
    /// Tests decide whether the entry is there instead of reading the real config.
    pub static FORCE_ON: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// The CLI's bridge entry as its settings file has it now.
pub fn entry(id: AiProviderId) -> Option<BridgeEntry> {
    let text = std::fs::read_to_string(config_file(id)?).ok()?;
    parse_entry(id, &text)
}

/// The opt-in is on: the entry is there and enabled.
pub fn bridge_on(id: AiProviderId) -> bool {
    #[cfg(test)]
    if let Some(on) = FORCE_ON.with(|f| f.get()) {
        return on && bridged(id);
    }
    bridged(id) && entry(id).is_some_and(|e| e.enabled)
}

/// The contents of [`AGY_SETTINGS_FILE`]: pre-approves the `mangavox` tools for that session only
/// (`agy --settings` is never saved). The app asks the user itself where needed.
pub fn agy_settings() -> String {
    json!({ "globalPermissionGrants": { "allow": [format!("mcp({MCP_SERVER}/*)")] } }).to_string()
}

/// Turns the opt-in on (registering `exe` as the bridge) or off, with the CLI's own command.
/// Returns the command's output; fails when the entry did not end up as asked.
pub async fn set_bridge(id: AiProviderId, on: bool, exe: &Path) -> Result<String> {
    let bin = super::cli::cli_binary(id).filter(|_| bridged(id)).ok_or_else(|| AiError::msg("This agent needs no opt-in for the app's tools"))?;
    let exe = exe.to_string_lossy().into_owned();
    let args = if on { add_args(id, &exe) } else { remove_args(id) }.unwrap_or_default();
    let path = find_binary(bin).await.ok_or_else(|| AiError::msg(format!("`{bin}` was not found on PATH")))?;
    // The exe path may hold spaces: the binary runs directly (std quotes each argument), not through cmd.
    let mut cmd = tokio::process::Command::new(&path);
    cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("NO_COLOR", "1").kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| AiError::msg(format!("`{bin} mcp` timed out")))?
        .map_err(|e| AiError::msg(format!("{bin} could not start: {e}")))?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
    if on && id == AiProviderId::GrokCli {
        if let Some(file) = config_file(id) {
            if let Some(new) = std::fs::read_to_string(&file).ok().and_then(|t| with_grok_timeout(&t)) {
                let _ = std::fs::write(&file, new);
            }
        }
    }
    if entry(id).is_some_and(|e| e.enabled) != on {
        let last = text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("no output").to_string();
        return Err(AiError::msg(format!("`{bin} mcp {}` did not take: {last}", if on { "add" } else { "remove" })));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_commands() {
        let exe = r"C:\Program Files\MangaVox\mangavox.exe";
        assert_eq!(add_args(AiProviderId::GrokCli, exe).unwrap(), ["mcp", "add", "--scope", "user", "mangavox", "--", exe, "--mcp-bridge"]);
        assert_eq!(add_args(AiProviderId::Antigravity, exe).unwrap(), ["mcp", "add", "mangavox", "--", exe, "--mcp-bridge"]);
        assert_eq!(remove_args(AiProviderId::GrokCli).unwrap(), ["mcp", "remove", "--scope", "user", "mangavox"]);
        assert_eq!(remove_args(AiProviderId::Antigravity).unwrap(), ["mcp", "remove", "mangavox"]);
        assert!(add_args(AiProviderId::Codex, exe).is_none() && remove_args(AiProviderId::ClaudeCode).is_none());
        assert!(bridged(AiProviderId::GrokCli) && bridged(AiProviderId::Antigravity) && !bridged(AiProviderId::GeminiCli));
        assert!(config_file(AiProviderId::Antigravity).unwrap().ends_with(".gemini/config/mcp_config.json"));
        assert!(config_file(AiProviderId::GrokCli).unwrap().ends_with("config.toml"));
        let s: Value = serde_json::from_str(&agy_settings()).unwrap();
        assert_eq!(s["globalPermissionGrants"]["allow"][0], "mcp(mangavox/*)");
    }

    // Written by `grok mcp add` 1.0.70 and `agy mcp add` 1.4.12.
    const GROK_CFG: &str = "[model]\ndefault = \"grok-4.7\"\n\n[mcp_servers.mangavox]\ncommand = 'D:\\a b\\mangavox.exe'\nargs = [\"--mcp-bridge\"]\nenabled = true\n\n[mcp_servers.other]\ncommand = \"npx\"\n";
    const AGY_CFG: &str = r#"{"mcpServers":{"mangavox":{"args":["--mcp-bridge"],"command":"D:\\x\\mangavox.exe","disabled":false}}}"#;

    #[test]
    fn entries_are_read_from_each_cli_config() {
        assert_eq!(parse_entry(AiProviderId::GrokCli, GROK_CFG), Some(BridgeEntry { command: r"D:\a b\mangavox.exe".into(), enabled: true }));
        assert_eq!(parse_entry(AiProviderId::GrokCli, &GROK_CFG.replace("enabled = true", "enabled = false")).map(|e| e.enabled), Some(false));
        let basic = "[mcp_servers.\"mangavox\"]\ncommand = \"C:\\\\x\\\\mangavox.exe\"\nargs = [\"--mcp-bridge\"]\n";
        assert_eq!(parse_entry(AiProviderId::GrokCli, basic).unwrap().command, r"C:\x\mangavox.exe");
        // Another server under the same name, or none at all, is not the opt-in.
        assert!(parse_entry(AiProviderId::GrokCli, "[mcp_servers.mangavox]\ncommand = \"npx\"\nargs = [\"x\"]\n").is_none());
        assert!(parse_entry(AiProviderId::GrokCli, "[mcp_servers.other]\nargs = [\"--mcp-bridge\"]\n").is_none());
        assert_eq!(parse_entry(AiProviderId::Antigravity, AGY_CFG), Some(BridgeEntry { command: r"D:\x\mangavox.exe".into(), enabled: true }));
        assert_eq!(parse_entry(AiProviderId::Antigravity, &AGY_CFG.replace("false", "true")).map(|e| e.enabled), Some(false));
        assert!(parse_entry(AiProviderId::Antigravity, "").is_none() && parse_entry(AiProviderId::Antigravity, r#"{"mcpServers":{}}"#).is_none());
    }

    #[test]
    fn grok_gets_a_long_tool_timeout_once() {
        let t = with_grok_timeout(GROK_CFG).unwrap();
        assert!(t.contains("[mcp_servers.mangavox]\ntool_timeout_sec = 3600\ncommand"));
        assert!(t.ends_with("[mcp_servers.other]\ncommand = \"npx\"\n"));
        assert_eq!(parse_entry(AiProviderId::GrokCli, &t).unwrap().command, r"D:\a b\mangavox.exe");
        assert!(with_grok_timeout(&t).is_none());
        assert!(with_grok_timeout("[model]\n").is_none());
    }
}
