//! The player takes every colour from `mv_ui::Tokens` (so Appearance presets reach it) and every
//! icon from Phosphor (`ph::*`), like mv-ui itself. Raw colour literals and Lucide asset paths
//! (`icons/*.svg`) are not allowed in this crate.
use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn colours_come_from_tokens_and_icons_from_phosphor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("examples"), &mut files);
    let patterns = [concat!("rgb", "(0x"), concat!("rgba", "(0x"), concat!("hsla", "("), concat!("black", "()"), concat!("white", "()"), concat!("\"icons", "/")];
    let mut hits = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read source");
        for (n, line) in text.lines().enumerate() {
            if patterns.iter().any(|p| line.contains(p)) {
                hits.push(format!("{}:{}: {}", file.strip_prefix(root).unwrap_or(file).display(), n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "use Tokens:: colours and ph:: icons instead:\n{}", hits.join("\n"));
}
