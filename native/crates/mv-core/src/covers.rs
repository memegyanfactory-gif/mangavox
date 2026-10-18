//! The Electron app caches covers as `md5(url [+ referer]).<ext>` in `~/.mangavox/cache/covers`.
use std::path::PathBuf;

const EXTS: [&str; 5] = ["jpg", "webp", "png", "jpeg", "gif"];

/// Local file for a cover URL, if the cache already has it.
pub fn cached_cover(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    // A plain path (e.g. a downloaded cover) is used as-is.
    if !url.starts_with("http") {
        let p = PathBuf::from(url);
        return p.exists().then_some(p);
    }
    let hash = format!("{:x}", md5::compute(url.as_bytes()));
    let dir = crate::paths::covers();
    EXTS.iter().map(|e| dir.join(format!("{hash}.{e}"))).find(|p| p.exists())
}

/// Every cached cover file (used by the Phase 0 grid stress test).
pub fn all_cached_covers() -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(crate::paths::covers()) else { return vec![] };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e)))
        .collect();
    out.sort();
    out
}
