//! Python interpreter discovery, requirements check, and the location of the
//! bundled worker scripts (`python/` in the repo or app resources).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::StudioError;

/// `CREATE_NO_WINDOW`: keeps console windows from popping up when the GUI app
/// starts Python (and helpers such as `taskkill`) on Windows.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `std::process::Command` that never opens a console window on Windows.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Minimum accepted Python: the TTS venv bootstrap needs 3.10+, the workers 3.9+.
const MIN_MINOR: u32 = 9;

/// A runnable Python 3 interpreter: `program prefix_args... script args...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonInterpreter {
    pub program: String,
    /// Extra leading arguments, e.g. `["-3"]` for the Windows `py` launcher.
    pub prefix_args: Vec<String>,
    /// `major.minor.micro`.
    pub version: String,
}

impl PythonInterpreter {
    /// Human-readable command, for logs and errors.
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.prefix_args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The app data folder (`~/.mangavox` unless the user moved it; see `mv_core::paths`).
pub fn mangavox_home() -> PathBuf {
    mv_core::paths::root()
}

/// `~/.mangavox/config.json`.
pub fn config_path() -> PathBuf {
    mangavox_home().join("config.json")
}

/// Interpreter configured in `config.json` under `pipeline.python_path`, if any.
/// The Electron app never wrote this key; it is the native build's override.
pub fn configured_python(config: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .pointer("/pipeline/python_path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Finds a Python 3 interpreter.
///
/// `configured` (a command such as `C:\\Py\\python.exe` or `py -3.11`) is tried
/// alone when given. Otherwise candidates are probed in order: `python`,
/// `python3`, `py -3` on Windows and `python3`, `python`, `py -3` elsewhere.
pub fn discover_python(configured: Option<&str>) -> Result<PythonInterpreter, StudioError> {
    if let Some(cfg) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = cfg.split_whitespace().map(str::to_string);
        let program = parts.next().unwrap_or_default();
        let prefix: Vec<String> = parts.collect();
        return probe(&program, &prefix).ok_or_else(|| {
            StudioError::PythonNotFound(format!(
                "configured interpreter `{cfg}` is not a usable Python 3.{MIN_MINOR}+"
            ))
        });
    }

    let candidates: Vec<(&str, Vec<&str>)> = if cfg!(windows) {
        vec![("python", vec![]), ("python3", vec![]), ("py", vec!["-3"])]
    } else {
        vec![("python3", vec![]), ("python", vec![]), ("py", vec!["-3"])]
    };
    for (program, prefix) in candidates {
        let prefix: Vec<String> = prefix.into_iter().map(str::to_string).collect();
        if let Some(found) = probe(program, &prefix) {
            return Ok(found);
        }
    }
    Err(StudioError::PythonNotFound(format!(
        "no Python 3.{MIN_MINOR}+ found on PATH (tried python, python3, py -3)"
    )))
}

/// Runs `<program> <prefix> -c "print(version)"` and accepts Python 3.MIN_MINOR+.
fn probe(program: &str, prefix: &[String]) -> Option<PythonInterpreter> {
    let out = quiet_command(program)
        .args(prefix)
        .arg("-c")
        .arg("import sys; v = sys.version_info; print('%d.%d.%d' % (v[0], v[1], v[2]))")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut nums = version.split('.').map(|p| p.parse::<u32>().ok());
    let major = nums.next()??;
    let minor = nums.next()??;
    if major != 3 || minor < MIN_MINOR {
        return None;
    }
    Some(PythonInterpreter {
        program: program.to_string(),
        prefix_args: prefix.to_vec(),
        version,
    })
}

/// Directory holding the worker scripts (`pipeline_worker.py`, `kokoro_tts.py`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonLayout {
    pub dir: PathBuf,
}

impl PythonLayout {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Resolves the scripts directory: the explicit path, then `MANGAVOX_PYTHON_DIR`,
    /// then a `python/` folder found by walking up from the working directory and
    /// the executable's directory (the dev layout keeps it at the repo root).
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, StudioError> {
        if let Some(dir) = explicit {
            return Ok(Self::new(dir));
        }
        if let Some(dir) = std::env::var_os("MANGAVOX_PYTHON_DIR") {
            return Ok(Self::new(dir));
        }
        let starts = [
            std::env::current_dir().ok(),
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
        ];
        for start in starts.into_iter().flatten() {
            for ancestor in start.ancestors() {
                let candidate = ancestor.join("python");
                if candidate.join("pipeline_worker.py").is_file() {
                    return Ok(Self::new(candidate));
                }
            }
        }
        Err(StudioError::ScriptNotFound(PathBuf::from(
            "python/pipeline_worker.py",
        )))
    }

    /// Absolute path of a worker script, failing if it is missing.
    pub fn script(&self, name: &str) -> Result<PathBuf, StudioError> {
        let path = self.dir.join(name);
        if path.is_file() {
            Ok(path)
        } else {
            Err(StudioError::ScriptNotFound(path))
        }
    }
}

/// Result of comparing `requirements.txt` with the interpreter's site-packages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RequirementsReport {
    pub required: Vec<String>,
    pub missing: Vec<String>,
}

impl RequirementsReport {
    pub fn is_satisfied(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Distribution names from a requirements file: comments, options (`-r`, `--hash`),
/// environment markers, extras and version specifiers are stripped.
pub fn requirement_names(text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('-') {
            continue;
        }
        let end = line
            .find(|c: char| matches!(c, '=' | '<' | '>' | '!' | '~' | '[' | ';' | ' ' | '@'))
            .unwrap_or(line.len());
        let name = line[..end].trim();
        if !name.is_empty() {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}

/// Checks each requirement with `python -m pip show`. Fails when pip itself is unavailable.
pub fn check_requirements(
    interpreter: &PythonInterpreter,
    requirements: &Path,
) -> Result<RequirementsReport, StudioError> {
    let text = std::fs::read_to_string(requirements)?;
    let required = requirement_names(&text);
    if required.is_empty() {
        return Ok(RequirementsReport {
            required,
            missing: vec![],
        });
    }
    let out = quiet_command(&interpreter.program)
        .args(&interpreter.prefix_args)
        .args(["-m", "pip", "show"])
        .args(&required)
        .output()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let installed: BTreeSet<String> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("Name:"))
        .map(normalize_name)
        .collect();
    if installed.is_empty() && !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(StudioError::Requirements(if stderr.is_empty() {
            "pip is not available".into()
        } else {
            stderr
        }));
    }
    let missing = required
        .iter()
        .filter(|r| !installed.contains(&normalize_name(r)))
        .cloned()
        .collect();
    Ok(RequirementsReport { required, missing })
}

/// PEP 503 normalization: lowercase, runs of `-_.` become `-`.
fn normalize_name(name: &str) -> String {
    let mut out = String::new();
    let mut sep = false;
    for c in name.trim().chars() {
        if matches!(c, '-' | '_' | '.') {
            sep = true;
        } else {
            if sep && !out.is_empty() {
                out.push('-');
            }
            sep = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_names_strip_specifiers_and_comments() {
        let text = "# comment\n\npaddlepaddle>=2.6\nopencv-python-headless==4.9  # pinned\n-r other.txt\nuvicorn[standard]~=0.30\nPillow ; python_version > '3'\nfoo @ https://x/y.whl\n";
        assert_eq!(
            requirement_names(text),
            vec![
                "Pillow",
                "foo",
                "opencv-python-headless",
                "paddlepaddle",
                "uvicorn"
            ]
        );
    }

    #[test]
    fn normalize_matches_pep503() {
        assert_eq!(
            normalize_name("Opencv_Python.Headless"),
            "opencv-python-headless"
        );
        assert_eq!(normalize_name("  Pillow "), "pillow");
    }

    #[test]
    fn configured_python_reads_pipeline_key() {
        let dir = std::env::temp_dir().join(format!("mv-studio-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"pipeline":{"python_path":"  /opt/py/bin/python3 "}}"#,
        )
        .unwrap();
        assert_eq!(
            configured_python(&cfg).as_deref(),
            Some("/opt/py/bin/python3")
        );
        std::fs::write(&cfg, r#"{"pipeline":{"python_path":""}}"#).unwrap();
        assert_eq!(configured_python(&cfg), None);
        assert_eq!(configured_python(&dir.join("missing.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layout_reports_missing_script() {
        let layout = PythonLayout::new(std::env::temp_dir().join("mv-studio-no-such-dir"));
        assert!(matches!(
            layout.script("kokoro_tts.py"),
            Err(StudioError::ScriptNotFound(_))
        ));
    }
}
