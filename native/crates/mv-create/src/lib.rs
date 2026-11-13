//! Creative: original manga made with AI. The user chats with a director model that writes the
//! series (bible, a cast that grows past 30, chapter outlines and page scripts); image models
//! paint character model sheets, expression sheets, locations and the pages (a whole page at once
//! when the model can paint a portrait page, else panel by panel, laid out and lettered here); the
//! chapter exports as PDF or images, and its panels animate into a stitched video.
//!
//! This crate is the engine (data, prompts, layout, lettering, PDF, video); `ui-creative` is the
//! page, and it runs the image models through Studio's catalog.
mod letter;
pub mod layout;
pub mod pdf;
pub mod project;
pub mod prompts;
pub mod video;

pub use project::*;

use std::path::{Path, PathBuf};

/// Up to `n` pages from a downloaded manga to show an image model its art style: spread through
/// the chapters, skipping each chapter's first two pages and its last (covers, credits, ads).
pub fn style_pages(pages_dirs: &[PathBuf], n: usize) -> Vec<PathBuf> {
    let mut pool: Vec<PathBuf> = vec![];
    for dir in pages_dirs {
        let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp")))
            .collect();
        pages.sort();
        if pages.len() > 4 {
            let inner = &pages[2..pages.len() - 1];
            pool.push(inner[inner.len() / 3].clone());
            pool.push(inner[inner.len() * 2 / 3].clone());
        } else {
            pool.extend(pages);
        }
    }
    if pool.len() <= n {
        return pool;
    }
    (0..n).map(|i| pool[i * pool.len() / n].clone()).collect()
}

/// Copies `files` into the project's `refs/` folder (so references survive the manga being
/// deleted) and returns their relative paths.
pub fn import_refs(p: &Project, files: &[PathBuf], prefix: &str) -> anyhow::Result<Vec<String>> {
    let dir = p.dir().join("refs");
    std::fs::create_dir_all(&dir)?;
    let mut out = vec![];
    for (i, f) in files.iter().enumerate() {
        let img = image::open(f).map_err(|e| anyhow::anyhow!("could not read {}: {e}", f.display()))?;
        let img = if img.width().max(img.height()) > 2048 { img.resize(2048, 2048, image::imageops::FilterType::Lanczos3) } else { img };
        let target = dir.join(format!("{}-{}-{i}.png", project::slug(prefix), project::now()));
        img.to_rgb8().save(&target)?;
        out.push(p.rel(&target));
    }
    Ok(out)
}

/// `file` as a JPEG of at most `max_side` pixels a side, in base64 (a reference picture light
/// enough to hand to a web page).
pub fn small_jpeg_b64(file: &Path, max_side: u32) -> anyhow::Result<String> {
    use base64::Engine as _;
    let img = image::open(file).map_err(|e| anyhow::anyhow!("could not read {}: {e}", file.display()))?;
    let img = if img.width().max(img.height()) > max_side { img.resize(max_side, max_side, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

/// Saves a picture that came as a `data:image/...;base64,` URL as PNG at `out`. Returns its size.
pub fn save_data_url(data: &str, out: &Path) -> anyhow::Result<(u32, u32)> {
    use base64::Engine as _;
    let b64 = data.split_once("base64,").map(|(_, b)| b).ok_or_else(|| anyhow::anyhow!("the picture did not arrive as image data"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| anyhow::anyhow!("the picture's data is damaged: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| anyhow::anyhow!("the picture could not be read: {e}"))?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    img.save_with_format(out, image::ImageFormat::Png)?;
    Ok((img.width(), img.height()))
}

/// The image path, if it is there.
pub fn existing(p: &Project, rel: Option<&String>) -> Option<PathBuf> {
    rel.map(|r| p.file(r)).filter(|f| Path::new(f).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_go_to_a_page_and_come_back() {
        let t = tempfile::tempdir().unwrap();
        let big = t.path().join("big.png");
        image::RgbImage::from_fn(2000, 1000, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])).save(&big).unwrap();
        let b64 = small_jpeg_b64(&big, 1280).unwrap();
        let out = t.path().join("sub").join("back.png");
        assert_eq!(save_data_url(&format!("data:image/jpeg;base64,{b64}"), &out).unwrap(), (1280, 640));
        assert!(out.is_file());
        assert!(save_data_url("https://x/y.png", &out).is_err() && save_data_url("data:image/png;base64,AAAA", &out).is_err());
    }

    #[test]
    fn style_pages_skip_covers_and_spread() {
        let t = tempfile::tempdir().unwrap();
        let mut dirs = vec![];
        for c in 0..3 {
            let d = t.path().join(format!("ch{c}"));
            std::fs::create_dir_all(&d).unwrap();
            for i in 0..12 {
                std::fs::write(d.join(format!("{i:03}.jpg")), b"x").unwrap();
            }
            std::fs::write(d.join("notes.txt"), b"x").unwrap();
            dirs.push(d);
        }
        let picked = style_pages(&dirs, 3);
        assert_eq!(picked.len(), 3);
        for p in &picked {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            assert!(name != "000.jpg" && name != "001.jpg" && name != "011.jpg", "{name}");
        }
        assert_ne!(picked[0].parent(), picked[2].parent());
    }
}
