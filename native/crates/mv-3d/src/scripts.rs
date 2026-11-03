//! The Blender-side Python, compiled into the app and written next to each other on disk so
//! `asset_runner.py` and `assemble.py` can `import mv3d`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const FILES: [(&str, &str); 4] = [
    ("mv3d.py", include_str!("../blender/mv3d.py")),
    ("asset_runner.py", include_str!("../blender/asset_runner.py")),
    ("assemble.py", include_str!("../blender/assemble.py")),
    ("sandbox.py", include_str!("../blender/sandbox.py")),
];

/// Writes the scripts into `dir` (only the ones that changed) and returns `dir`.
pub(crate) fn install_in(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    for (name, body) in FILES {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body) {
            std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
        }
    }
    Ok(dir.to_path_buf())
}

pub(crate) fn install() -> Result<PathBuf> {
    install_in(&crate::paths::scripts())
}
