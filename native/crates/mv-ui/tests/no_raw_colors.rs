//! Every colour in mv-ui must come from `Tokens` so the Appearance presets reach it.
//! Raw colour literals (hex, `hsla(..)`, `black()` / `white()`) are allowed only in src/theme.rs.
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
fn no_raw_colour_literals_outside_theme() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    // Hex literals and the other GPUI colour constructors (`hsla(..)`, `black()`, `white()`).
    let patterns = [concat!("rgb", "(0x"), concat!("rgba", "(0x"), concat!("hsla", "("), concat!("black", "()"), concat!("white", "()")];
    let mut hits = Vec::new();
    for file in files.iter().filter(|f| f.file_name().is_some_and(|n| n != "theme.rs")) {
        let text = fs::read_to_string(file).expect("read source");
        for (n, line) in text.lines().enumerate() {
            if patterns.iter().any(|p| line.contains(p)) {
                hits.push(format!("{}:{}: {}", file.strip_prefix(&src).unwrap_or(file).display(), n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "raw colour literals outside theme.rs; add a Tokens:: entry instead:\n{}", hits.join("\n"));
}
