//! In-app updates from GitHub Releases, and the install helpers the setup exe shares.
//!
//! Every push to `main` publishes a release (`.github/workflows/release.yml`) with
//! `MangaVox-<ver>-win64.zip` (the whole app folder) and `MangaVox-Setup-<ver>.exe`.
//! The app checks `releases/latest`; updating downloads the zip, unpacks it next to the
//! install, swaps the files in (moving locked ones aside) and starts the new copy before
//! quitting. No installer is shown for updates.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

/// Repository the releases are published to (`MANGAVOX_UPDATE_REPO` overrides it).
pub const REPO: &str = "memegyanfactory-gif/mangavox";
pub const AUTHOR: &str = "Aayush Datta";
pub const PRODUCT: &str = "MangaVox";
/// The version this build was made from (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const EXE: &str = "mangavox.exe";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

fn repo() -> String {
    std::env::var("MANGAVOX_UPDATE_REPO").ok().filter(|r| r.contains('/')).unwrap_or_else(|| REPO.to_string())
}

/// A published release that can be installed.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub zip_url: String,
    pub zip_size: u64,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// `[major, minor, patch]` from "v8.0.12" / "8.0.12-beta"; missing parts are 0.
pub fn parse_version(v: &str) -> Option<[u64; 3]> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    for (i, part) in core.split('.').take(3).enumerate() {
        out[i] = part.parse().ok()?;
    }
    Some(out)
}

/// True when `candidate` is a higher version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("{PRODUCT}/{VERSION} (+https://github.com/{REPO})"))
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

/// The newest published release, whatever its version.
pub async fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let res = client()?.get(&url).header("Accept", "application/vnd.github+json").timeout(Duration::from_secs(20)).send().await.context("Could not reach GitHub")?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("No release has been published yet");
    }
    if !res.status().is_success() {
        bail!("GitHub answered {}", res.status());
    }
    let r: GhRelease = res.json().await.context("Unexpected answer from GitHub")?;
    if r.draft {
        bail!("No release has been published yet");
    }
    let zip = r.assets.iter().find(|a| a.name.ends_with("-win64.zip")).ok_or_else(|| anyhow!("The latest release has no Windows package yet"))?;
    Ok(Release {
        version: r.tag_name.trim_start_matches(['v', 'V']).to_string(),
        notes: r.body.unwrap_or_default(),
        url: r.html_url,
        zip_url: zip.browser_download_url.clone(),
        zip_size: zip.size,
    })
}

/// The latest release when it is newer than this build.
pub async fn check() -> Result<Option<Release>> {
    let r = latest().await?;
    Ok(is_newer(&r.version, VERSION).then_some(r))
}

/// Where updates are staged: `%LOCALAPPDATA%\MangaVox\updates`.
pub fn updates_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(PRODUCT).join("updates")
}

/// Downloads `url` to `dest`, reporting `(done, total)` bytes.
pub async fn download(url: &str, dest: &Path, progress: impl Fn(u64, u64)) -> Result<()> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let res = client()?.get(url).send().await.context("Download failed to start")?;
    if !res.status().is_success() {
        bail!("Download failed: {}", res.status());
    }
    let total = res.content_length().unwrap_or(0);
    let part = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await?;
    let mut done = 0u64;
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Download interrupted")?;
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await?;
    drop(file);
    if total > 0 && done != total {
        bail!("Download was incomplete ({done} of {total} bytes)");
    }
    tokio::fs::rename(&part, dest).await?;
    Ok(())
}

/// Unpacks a zip with Windows' built-in `tar` (fast, handles large archives).
pub fn extract(zip: &Path, dir: &Path) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).ok();
    }
    std::fs::create_dir_all(dir)?;
    // Windows' own bsdtar; a GNU tar earlier on PATH (Git for Windows) misreads "C:\…" as a remote host.
    let tar = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe")).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("tar"));
    let mut cmd = Command::new(tar);
    cmd.arg("-xf").arg(zip).arg("-C").arg(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().context("Could not run tar")?;
    if !out.status.success() {
        bail!("Could not unpack the update: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if !dir.join(EXE).is_file() {
        bail!("The update package is missing {EXE}");
    }
    Ok(())
}

/// Folder of the running app, or `None` for a development build (`target\debug|release`),
/// which must not overwrite itself.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let s = dir.to_string_lossy().to_lowercase();
    if s.contains("\\target\\") || s.contains("/target/") {
        return None;
    }
    Some(dir)
}

/// Default per-user install folder: `%LOCALAPPDATA%\Programs\MangaVox` (no admin rights needed).
pub fn default_install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(PRODUCT)
}

/// Downloads and unpacks `release`, ready for [`apply_and_restart`]. Returns the staged folder.
pub async fn prepare(release: &Release, progress: impl Fn(u64, u64)) -> Result<PathBuf> {
    let dir = updates_dir();
    let zip = dir.join(format!("{PRODUCT}-{}-win64.zip", release.version));
    let have = std::fs::metadata(&zip).map(|m| release.zip_size > 0 && m.len() == release.zip_size).unwrap_or(false);
    if !have {
        download(&release.zip_url, &zip, &progress).await?;
    } else {
        progress(release.zip_size, release.zip_size);
    }
    let staged = dir.join(&release.version);
    let (z, s) = (zip.clone(), staged.clone());
    tokio::task::spawn_blocking(move || extract(&z, &s)).await.map_err(|e| anyhow!(e))??;
    let _ = std::fs::remove_file(&zip);
    Ok(staged)
}

/// Copies `staged` over `install` while this app is still running, then starts the new
/// copy; the caller then quits. Windows lets a running exe or loaded DLL be renamed but
/// not overwritten, so locked files are moved aside to `*.old` (removed by [`cleanup`] on
/// the next start). The swap happens before quitting, so even if the restart fails the
/// next launch is the new version.
pub fn apply_and_restart(staged: &Path, install: &Path, version: &str) -> Result<()> {
    std::fs::create_dir_all(install)?;
    copy_over(staged, install).context("Could not replace the app files")?;
    let _ = reg_add("DisplayVersion", "REG_SZ", version);
    let _ = std::fs::remove_dir_all(staged);
    restart(install)
}

fn copy_over(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_over(&entry.path(), &to)?;
            continue;
        }
        if to.exists() && std::fs::remove_file(&to).is_err() {
            // In use: move it aside under a fresh name.
            let mut aside = to.as_os_str().to_owned();
            aside.push(format!(".{}.old", std::process::id()));
            std::fs::rename(&to, &aside).with_context(|| format!("{} is in use", to.display()))?;
        }
        std::fs::copy(entry.path(), &to).with_context(|| format!("Could not write {}", to.display()))?;
    }
    Ok(())
}

/// Starts the installed exe, outside any job object this process runs in so it survives our exit.
fn restart(install: &Path) -> Result<()> {
    let exe = install.join(EXE);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let spawn = |flags: u32| Command::new(&exe).current_dir(install).creation_flags(flags).spawn();
        if spawn(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB).is_ok() {
            return Ok(());
        }
        spawn(DETACHED_PROCESS).context("Could not restart MangaVox")?;
    }
    #[cfg(not(windows))]
    Command::new(&exe).current_dir(install).spawn().context("Could not restart MangaVox")?;
    Ok(())
}

/// Deletes the `*.old` files an earlier update moved aside. Call once at startup.
pub fn cleanup() {
    fn walk(dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p);
            } else if p.extension().is_some_and(|x| x == "old") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    if let Some(dir) = install_dir() {
        walk(&dir);
    }
    // Left over from the old script-based updater.
    let _ = std::fs::remove_file(updates_dir().join("apply.cmd"));
}

// ---------------------------------------------------------------- install integration (setup exe)

fn powershell(script: &str) -> Result<()> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn shortcut_paths() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(app) = std::env::var_os("APPDATA").map(PathBuf::from) {
        v.push(app.join(r"Microsoft\Windows\Start Menu\Programs").join(format!("{PRODUCT}.lnk")));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        v.push(home.join("Desktop").join(format!("{PRODUCT}.lnk")));
    }
    v
}

/// Start Menu and Desktop shortcuts to the installed exe.
pub fn create_shortcuts(install: &Path) -> Result<()> {
    let exe = install.join(EXE);
    for lnk in shortcut_paths() {
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Description='{PRODUCT} by {AUTHOR}';$s.Save()",
            lnk.display().to_string().replace('\'', "''"),
            exe.display().to_string().replace('\'', "''"),
            install.display().to_string().replace('\'', "''"),
            exe.display().to_string().replace('\'', "''"),
        );
        powershell(&script).with_context(|| format!("Could not create {}", lnk.display()))?;
    }
    Ok(())
}

fn reg_add(value: &str, kind: &str, data: &str) -> Result<()> {
    let mut cmd = Command::new("reg");
    cmd.args(["add", &format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{PRODUCT}"), "/v", value, "/t", kind, "/d", data, "/f"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        bail!("reg add {value} failed");
    }
    Ok(())
}

/// Lists the app in Settings → Apps with an uninstall entry (`mangavox.exe --uninstall`).
pub fn register_uninstall(install: &Path, version: &str) -> Result<()> {
    let exe = install.join(EXE);
    reg_add("DisplayName", "REG_SZ", PRODUCT)?;
    reg_add("DisplayVersion", "REG_SZ", version)?;
    reg_add("Publisher", "REG_SZ", AUTHOR)?;
    reg_add("DisplayIcon", "REG_SZ", &format!("{},0", exe.display()))?;
    reg_add("InstallLocation", "REG_SZ", &install.display().to_string())?;
    reg_add("UninstallString", "REG_SZ", &format!("\"{}\" --uninstall", exe.display()))?;
    reg_add("NoModify", "REG_DWORD", "1")?;
    reg_add("NoRepair", "REG_DWORD", "1")?;
    Ok(())
}

/// Removes shortcuts, the uninstall entry and (after this process exits) the install folder.
/// User data in `~/.mangavox` is kept.
pub fn uninstall() -> Result<()> {
    for lnk in shortcut_paths() {
        let _ = std::fs::remove_file(lnk);
    }
    let mut cmd = Command::new("reg");
    cmd.args(["delete", &format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{PRODUCT}"), "/f"]);
    let _ = cmd.output();
    // The Phone access firewall rule (ui-phone::firewall::RULE_NAME) can only go with admin
    // rights; Settings → Apps runs this per-user uninstall without them, so this best-effort,
    // prompt-free try usually leaves it (it then points at a deleted exe and allows nothing).
    let mut cmd = Command::new("netsh");
    cmd.args(["advfirewall", "firewall", "delete", "rule", "name=MangaVox phone access"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd.output();
    if let Some(dir) = install_dir() {
        let pid = std::process::id();
        let script = std::env::temp_dir().join("mangavox-uninstall.cmd");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\n:wait\r\ntasklist /FI \"PID eq {pid}\" 2>nul | find \"{pid}\" >nul && (ping -n 2 127.0.0.1 >nul & goto wait)\r\nrmdir /s /q \"{}\"\r\ndel \"%~f0\"\r\n",
                dir.display()
            ),
        )?;
        let mut cmd = Command::new("cmd");
        cmd.arg("/c").arg(&script);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
        }
        cmd.spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v8.0.12"), Some([8, 0, 12]));
        assert_eq!(parse_version("8.1"), Some([8, 1, 0]));
        assert_eq!(parse_version("8.0.1-beta.2"), Some([8, 0, 1]));
        assert!(parse_version("latest").is_none());
        assert!(is_newer("v8.0.10", "8.0.9"));
        assert!(is_newer("9.0.0", "8.99.99"));
        assert!(!is_newer("8.0.0", "8.0.0"));
        assert!(!is_newer("7.23.3", "8.0.0"));
        assert!(!is_newer("garbage", "8.0.0"));
    }

    #[test]
    fn release_json_picks_the_windows_zip() {
        let raw = r#"{"tag_name":"v8.0.3","body":"notes","html_url":"https://x","draft":false,"assets":[
            {"name":"MangaVox-Setup-8.0.3.exe","browser_download_url":"https://x/setup","size":5},
            {"name":"MangaVox-8.0.3-win64.zip","browser_download_url":"https://x/zip","size":9}]}"#;
        let r: GhRelease = serde_json::from_str(raw).unwrap();
        let zip = r.assets.iter().find(|a| a.name.ends_with("-win64.zip")).unwrap();
        assert_eq!(zip.browser_download_url, "https://x/zip");
        assert_eq!(r.tag_name.trim_start_matches('v'), "8.0.3");
    }
}
