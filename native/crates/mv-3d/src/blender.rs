//! Blender bridge: finds the install, runs a script headless (`blender -b -P script -- job.json`)
//! and reads the one `MV3D_RESULT` / `MV3D_ERROR` line the script prints. Also finds ffmpeg,
//! which joins rendered frames into clips.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use mv_ai::CancellationToken;
use serde_json::Value;

/// What the 3D tab shows when Blender is missing.
pub const SETUP_MESSAGE: &str = "Blender is needed to build and render 3D scenes. Install it free from blender.org (4.2 or newer), \
     then open this tab again. If it is installed somewhere unusual, set the MANGAVOX_BLENDER environment variable to blender.exe.";

/// The newest Blender on this machine: `MANGAVOX_BLENDER`, then the standard install folders
/// (`Blender Foundation\Blender X.Y`, Steam), then `PATH`.
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MANGAVOX_BLENDER").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = if cfg!(windows) { "blender.exe" } else { "blender" };
    let mut found: Vec<(Vec<u32>, PathBuf)> = vec![];
    for base in install_roots() {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let dir = e.path();
            let p = dir.join(exe);
            if p.is_file() {
                let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                found.push((version_key(&name), p));
            }
        }
    }
    for p in ["C:/Program Files (x86)/Steam/steamapps/common/Blender", "C:/Program Files/Steam/steamapps/common/Blender"] {
        let p = Path::new(p).join(exe);
        if p.is_file() {
            found.push((vec![0], p));
        }
    }
    if cfg!(target_os = "macos") {
        let p = PathBuf::from("/Applications/Blender.app/Contents/MacOS/Blender");
        if p.is_file() {
            found.push((vec![0], p));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).next().or_else(|| which(exe))
}

fn install_roots() -> Vec<PathBuf> {
    let mut v = vec![];
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(p) = std::env::var_os(var) {
            v.push(PathBuf::from(p).join("Blender Foundation"));
        }
    }
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(p).join("Programs").join("Blender Foundation"));
    }
    v.dedup();
    v
}

/// `"Blender 5.2"` -> `[5, 2]`.
fn version_key(name: &str) -> Vec<u32> {
    name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

/// A program on `PATH` (and WinGet's links folder, where ffmpeg usually lands on Windows).
pub fn which(exe: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(p) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(p).join("Microsoft").join("WinGet").join("Links"));
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// ffmpeg for turning frames into clips: `MANGAVOX_FFMPEG`, else `PATH`.
pub fn ffmpeg() -> Option<PathBuf> {
    std::env::var_os("MANGAVOX_FFMPEG").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| which(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }))
}

/// How much of Blender's output goes to the log file; the rest is dropped.
const LOG_CAP: u64 = 8 * 1024 * 1024;
/// Lines kept in memory for the error message.
const TAIL_LINES: usize = 40;
/// Longest line kept; a script printing one endless line cannot grow memory.
const LINE_CAP: usize = 64 * 1024;

/// Blender's output as it streams in: written to the log up to [`LOG_CAP`], with only the
/// `MV3D_` lines and a short tail kept in memory.
struct Capture {
    log: Option<std::fs::File>,
    written: u64,
    marks: Vec<String>,
    tail: std::collections::VecDeque<String>,
}

impl Capture {
    fn new(log: &Path) -> Capture {
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Capture { log: std::fs::File::create(log).ok(), written: 0, marks: vec![], tail: Default::default() }
    }

    fn raw(&mut self, bytes: &[u8]) {
        use std::io::Write;
        let Some(f) = self.log.as_mut() else { return };
        let room = LOG_CAP.saturating_sub(self.written) as usize;
        let n = bytes.len().min(room);
        let _ = f.write_all(&bytes[..n]);
        self.written += n as u64;
        if n < bytes.len() {
            let _ = f.write_all(b"\n[log truncated]\n");
            self.log = None;
        }
    }

    fn line(&mut self, bytes: &[u8]) {
        let l = String::from_utf8_lossy(bytes).trim_end_matches('\r').to_string();
        if l.starts_with("MV3D_") && self.marks.len() < 32 {
            self.marks.push(l.clone());
        }
        self.tail.push_back(l);
        if self.tail.len() > TAIL_LINES {
            self.tail.pop_front();
        }
    }

    fn text(&self) -> String {
        self.marks.iter().chain(self.tail.iter()).cloned().collect::<Vec<_>>().join("\n")
    }
}

async fn pump(mut r: impl tokio::io::AsyncRead + Unpin, cap: &std::sync::Mutex<Capture>) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut c = cap.lock().unwrap_or_else(|e| e.into_inner());
        c.raw(&buf[..n]);
        for &b in &buf[..n] {
            if b == b'\n' {
                c.line(&line);
                line.clear();
            } else if line.len() < LINE_CAP {
                line.push(b);
            }
        }
    }
    if !line.is_empty() {
        cap.lock().unwrap_or_else(|e| e.into_inner()).line(&line);
    }
}

/// Runs `script` headless with `job` as its argument. Output streams to `log` (capped), so a
/// script that prints without end cannot run the app out of memory. Returns the JSON after
/// `MV3D_RESULT` when Blender exits cleanly, else the script's own error message.
pub async fn run(blender: &Path, script: &Path, job: &Path, log: &Path, timeout: Duration, cancel: &CancellationToken) -> Result<Value> {
    let mut cmd = tokio::process::Command::new(blender);
    // No auto-run of Python stored in .blend files (downloaded or AI-built assets).
    cmd.arg("-b").arg("--factory-startup").arg("--disable-autoexec").arg("--python-exit-code").arg("1").arg("-P").arg(script).arg("--").arg(job);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().with_context(|| format!("start {}", blender.display()))?;
    let (stdout, stderr) = (child.stdout.take().context("no stdout")?, child.stderr.take().context("no stderr")?);
    let cap = std::sync::Mutex::new(Capture::new(log));
    let work = async {
        let (_, _, status) = tokio::join!(pump(stdout, &cap), pump(stderr, &cap), child.wait());
        status
    };
    let status = tokio::select! {
        s = work => s.context("Blender did not finish")?,
        _ = cancel.cancelled() => bail!("cancelled"),
        _ = tokio::time::sleep(timeout) => bail!("Blender took longer than {} minutes and was stopped", timeout.as_secs() / 60),
    };
    let text = cap.into_inner().unwrap_or_else(|e| e.into_inner()).text();
    parse_output(&text, status.success())
}

/// A result counts only when Blender exited cleanly (the last `MV3D_RESULT` line), so a script
/// that prints a fake result and then fails is still a failure.
pub(crate) fn parse_output(text: &str, success: bool) -> Result<Value> {
    if success {
        if let Some(j) = text.lines().filter_map(|l| l.strip_prefix("MV3D_RESULT ")).last() {
            return serde_json::from_str(j).map_err(|e| anyhow!("unreadable result from Blender: {e}"));
        }
    }
    if let Some(j) = text.lines().filter_map(|l| l.strip_prefix("MV3D_ERROR ")).last() {
        let v: Value = serde_json::from_str(j).unwrap_or_default();
        let msg = v.get("trace").and_then(|t| t.as_str()).or_else(|| v.get("error").and_then(|e| e.as_str())).unwrap_or(j);
        bail!("{}", msg.trim());
    }
    let tail: String = text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    if success {
        bail!("Blender finished without a result:\n{tail}")
    }
    bail!("Blender failed:\n{tail}")
}

/// Joins `frames_dir/f_0001.png ...` into an H.264 clip.
pub async fn encode_clip(ffmpeg: &Path, frames_dir: &Path, fps: u32, out: &Path) -> Result<()> {
    let pattern = frames_dir.join("f_%04d.png");
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(["-y", "-loglevel", "error", "-framerate", &fps.to_string(), "-i"]).arg(&pattern);
    cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-movflags", "+faststart"]).arg(out);
    ffmpeg_status(cmd).await
}

/// Joins scene clips into the episode (same codec and size, so no re-encode).
pub async fn concat_clips(ffmpeg: &Path, clips: &[PathBuf], out: &Path) -> Result<()> {
    let list = out.with_extension("txt");
    let body: String = clips.iter().map(|c| format!("file '{}'\n", c.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"))).collect();
    std::fs::write(&list, body)?;
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(["-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"]).arg(&list).args(["-c", "copy"]).arg(out);
    let r = ffmpeg_status(cmd).await;
    let _ = std::fs::remove_file(&list);
    r
}

async fn ffmpeg_status(mut cmd: tokio::process::Command) -> Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = cmd.output().await.context("start ffmpeg")?;
    if !out.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
