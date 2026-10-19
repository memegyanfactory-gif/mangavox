//! Every colour in the assistant comes from `mv_ui::Tokens`, so the Appearance presets reach it
//! (the same rule mv-ui enforces on itself in its own `tests/no_raw_colors.rs`).
use std::fs;
use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_raw_colour_literals() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let patterns = [concat!("rgb", "(0x"), concat!("rgba", "(0x"), concat!("hsla", "("), concat!("black", "()"), concat!("white", "()")];
    let mut hits = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read source");
        for (n, line) in text.lines().enumerate() {
            if patterns.iter().any(|p| line.contains(p)) {
                hits.push(format!("{}:{}: {}", file.strip_prefix(&src).unwrap_or(file).display(), n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "raw colour literals in ui-assistant; use a Tokens:: entry instead:\n{}", hits.join("\n"));
}

#[test]
fn no_lucide_icon_paths() {
    // Icons are Phosphor (`mv_ui::ph::*`); the old Lucide `icons/*.svg` paths are gone from this crate.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let needle = concat!("\"icons", "/");
    let hits: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let text = fs::read_to_string(f).expect("read source");
            text.lines().enumerate().filter(|(_, l)| l.contains(needle)).map(|(n, l)| format!("{}:{}: {}", f.display(), n + 1, l.trim())).collect::<Vec<_>>()
        })
        .collect();
    assert!(hits.is_empty(), "Lucide icon paths left:\n{}", hits.join("\n"));
}
