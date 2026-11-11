//! Which account a CLI agent is signed in with, so a change of account is noticed.
//!
//! A run that waits out a usage limit (`mv_recap`'s `Patient`) would sleep for hours after the
//! user signed in with another account: the limit belonged to the old one. The sign-in files each
//! CLI keeps say who is signed in; [`changed`] returns as soon as they name someone else.
//!
//! Only the account id (or, where a CLI stores none, the sign-in file's date) is read: never a
//! token, and nothing is logged or returned beyond an opaque string to compare.
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

use crate::types::AiProviderId;

/// How often a waiting run looks at the sign-in files.
const POLL: Duration = Duration::from_secs(10);

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The files that change when `provider` is signed in or out, first the one naming the account.
pub fn sign_in_files(provider: AiProviderId, home: &Path) -> Vec<PathBuf> {
    match provider {
        // With CLAUDE_CONFIG_DIR set, Claude Code keeps `.claude.json` inside it.
        AiProviderId::ClaudeCode => match env_dir("CLAUDE_CONFIG_DIR") {
            Some(dir) => vec![dir.join(".claude.json"), dir.join(".credentials.json")],
            None => vec![home.join(".claude.json"), home.join(".claude").join(".credentials.json")],
        },
        AiProviderId::Codex => vec![env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex")).join("auth.json")],
        AiProviderId::GeminiCli => vec![home.join(".gemini").join("google_accounts.json"), home.join(".gemini").join("oauth_creds.json")],
        AiProviderId::GrokCli => vec![home.join(".grok").join("auth.json")],
        AiProviderId::Opencode => vec![home.join(".local").join("share").join("opencode").join("auth.json")],
        _ => vec![],
    }
}

/// Size and date of the sign-in files: cheap to read, and different after any sign-in.
fn stamp(files: &[PathBuf]) -> Vec<Option<(u64, u128)>> {
    files
        .iter()
        .map(|f| {
            let m = std::fs::metadata(f).ok()?;
            Some((m.len(), m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos()))
        })
        .collect()
}

/// Whether a new sign-in of `provider` can be seen ([`changed`] returns for it). Antigravity keeps
/// its sign-in where it cannot be read, so a run waiting on it asks again more often instead.
pub fn tracked(provider: AiProviderId) -> bool {
    home().is_some_and(|h| !sign_in_files(provider, &h).is_empty())
}

/// Size and date of the file holding `provider`'s sign-in (the last of [`sign_in_files`]; the
/// others, like Claude Code's `.claude.json`, are rewritten all the time) as one string: it
/// differs after any sign-in and after a token renewal. Reads no file contents, so it is cheap
/// enough to ask every few seconds.
pub fn sign_in_stamp(provider: AiProviderId) -> String {
    home().and_then(|h| sign_in_files(provider, &h).pop()).map(|f| format!("{:?}", stamp(&[f]))).unwrap_or_default()
}

/// The account id a sign-in file names, when the CLI stores one.
pub fn account_in(provider: AiProviderId, file: &Value) -> Option<String> {
    let text = |p: &str| file.pointer(p).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    match provider {
        // The organization is part of it: one login can hold a personal and a team plan.
        AiProviderId::ClaudeCode => text("/oauthAccount/accountUuid").map(|a| format!("{a}/{}", text("/oauthAccount/organizationUuid").unwrap_or_default())),
        AiProviderId::Codex => text("/tokens/account_id").or_else(|| text("/OPENAI_API_KEY").map(|_| "api-key".into())),
        AiProviderId::GeminiCli => text("/active"),
        _ => None,
    }
}

/// Who `provider` is signed in as: the account id where the CLI stores one, else the date of its
/// sign-in file (which also moves when a token is renewed: then a waiting run only asks once
/// more). `None`: not a CLI with sign-in files, or not signed in.
pub fn fingerprint(provider: AiProviderId) -> Option<String> {
    fingerprint_in(provider, &home()?)
}

pub fn fingerprint_in(provider: AiProviderId, home: &Path) -> Option<String> {
    let files = sign_in_files(provider, home);
    let named = files.first().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| account_in(provider, &v));
    if named.is_some() {
        return named;
    }
    let stamp = stamp(&files);
    stamp.iter().any(Option::is_some).then(|| format!("{stamp:?}"))
}

/// Returns once `provider` is signed in as someone other than `was` (a [`fingerprint`] taken
/// earlier). Never returns for a provider without sign-in files.
pub async fn changed(provider: AiProviderId, was: Option<String>) {
    let Some(home) = home() else { return std::future::pending().await };
    let files = sign_in_files(provider, &home);
    if files.is_empty() {
        return std::future::pending().await;
    }
    let mut seen = stamp(&files);
    loop {
        tokio::time::sleep(POLL).await;
        let now = stamp(&files);
        if now == seen {
            continue;
        }
        seen = now;
        // A CLI writes its files in steps: read once they have settled.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let h = home.clone();
        let is = tokio::task::spawn_blocking(move || fingerprint_in(provider, &h)).await.ok().flatten();
        if is.is_some() && is != was {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_account_is_read_and_never_a_token() {
        let claude = json!({"oauthAccount": {"accountUuid": "a-1", "organizationUuid": "o-1", "emailAddress": "x@y.z"}});
        assert_eq!(account_in(AiProviderId::ClaudeCode, &claude).as_deref(), Some("a-1/o-1"));
        assert_eq!(account_in(AiProviderId::ClaudeCode, &json!({"numStartups": 3})), None);
        let codex = json!({"tokens": {"account_id": "acc_9", "access_token": "secret"}});
        assert_eq!(account_in(AiProviderId::Codex, &codex).as_deref(), Some("acc_9"));
        assert_eq!(account_in(AiProviderId::GeminiCli, &json!({"active": "me@x.y", "old": []})).as_deref(), Some("me@x.y"));
        assert_eq!(account_in(AiProviderId::GrokCli, &json!({"token": "secret"})), None);
    }

    #[test]
    fn another_sign_in_gives_another_fingerprint() {
        let home = tempfile::tempdir().unwrap();
        let write = |uuid: &str| std::fs::write(home.path().join(".claude.json"), json!({"oauthAccount": {"accountUuid": uuid}}).to_string()).unwrap();
        if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
            return;
        }
        assert_eq!(fingerprint_in(AiProviderId::ClaudeCode, home.path()), None, "not signed in");
        write("first");
        let first = fingerprint_in(AiProviderId::ClaudeCode, home.path());
        write("first");
        assert_eq!(fingerprint_in(AiProviderId::ClaudeCode, home.path()), first, "the same account, written again");
        write("second");
        assert_ne!(fingerprint_in(AiProviderId::ClaudeCode, home.path()), first);
        // A CLI that stores no account id is told apart by its sign-in file.
        std::fs::create_dir_all(home.path().join(".grok")).unwrap();
        std::fs::write(home.path().join(".grok").join("auth.json"), "{}").unwrap();
        let grok = fingerprint_in(AiProviderId::GrokCli, home.path());
        std::fs::write(home.path().join(".grok").join("auth.json"), "{\"a\":1}").unwrap();
        assert!(grok.is_some() && fingerprint_in(AiProviderId::GrokCli, home.path()) != grok);
        assert_eq!(fingerprint_in(AiProviderId::Ollama, home.path()), None);
    }
}
