//! Pictures in and out of the page: references go in as small JPEGs (base64), results come back
//! as `data:` URLs and leave as PNG bytes. Ported from the scrapped attempt's `mv-create`
//! (`small_jpeg_b64`, `save_data_url`), with the results kept in memory.

use std::path::Path;

use base64::Engine as _;

use crate::WebError;

/// What a picture in a format other than JPEG, PNG or WebP is refused with.
const UNSUPPORTED: &str = "Magnific's website sent the picture in a format MangaVox cannot read. Try again.";

/// `file` as a JPEG (quality 88) whose longer side is at most `max_side`, in base64.
pub fn small_jpeg_b64(file: &Path, max_side: u32) -> Result<String, WebError> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.display().to_string());
    let img = image::open(file).map_err(|e| WebError::Site(format!("The reference picture \"{name}\" could not be read: {e}")))?;
    let img = if img.width().max(img.height()) > max_side { img.resize(max_side, max_side, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8())
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88))
        .map_err(|e| WebError::Site(format!("The reference picture \"{name}\" could not be prepared: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

/// A picture that came as a `data:image/...;base64,` URL, re-encoded as PNG: (bytes, width, height).
/// JPEG, PNG and WebP are read; anything else is refused.
pub fn png_from_data_url(data: &str) -> Result<(Vec<u8>, u32, u32), WebError> {
    let b64 = data
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once("base64,"))
        .map(|(_, b)| b)
        .ok_or_else(|| WebError::Site("Magnific's website did not send the picture as image data.".into()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| WebError::Site(format!("The picture from Magnific's website is damaged: {e}")))?;
    let format = image::guess_format(&bytes).ok();
    if !matches!(format, Some(image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP)) {
        return Err(WebError::Site(UNSUPPORTED.into()));
    }
    let img = image::load_from_memory(&bytes).map_err(|e| WebError::Site(format!("The picture from Magnific's website could not be read: {e}")))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| WebError::Site(format!("The picture from Magnific's website could not be saved as PNG: {e}")))?;
    Ok((out.into_inner(), img.width(), img.height()))
}

/// Saves a `data:` URL picture as PNG at `out` (folders made as needed). Returns its size.
pub fn save_data_url(data: &str, out: &Path) -> Result<(u32, u32), WebError> {
    let (png, w, h) = png_from_data_url(data)?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| WebError::Site(format!("Could not make the folder {}: {e}", d.display())))?;
    }
    std::fs::write(out, png).map_err(|e| WebError::Site(format!("Could not save the picture at {}: {e}", out.display())))?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: image::ImageFormat, w: u32, h: u32) -> String {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    /// A folder of its own under the system temp folder, removed when dropped.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let d = std::env::temp_dir().join(format!("mv-magnific-test-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn data_urls_of_the_three_formats_become_png() {
        for (format, mime) in [(image::ImageFormat::Png, "png"), (image::ImageFormat::Jpeg, "jpeg"), (image::ImageFormat::WebP, "webp")] {
            let url = format!("data:image/{mime};base64,{}", encoded(format, 64, 40));
            let (png, w, h) = png_from_data_url(&url).unwrap_or_else(|e| panic!("{mime}: {e}"));
            assert_eq!((w, h), (64, 40), "{mime}");
            assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png, "{mime} comes back as PNG");
        }
    }

    #[test]
    fn other_data_is_refused_in_words() {
        let gif = base64::engine::general_purpose::STANDARD.encode(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;");
        assert_eq!(png_from_data_url(&format!("data:image/gif;base64,{gif}")), Err(WebError::Site(UNSUPPORTED.into())));
        assert!(matches!(png_from_data_url("https://cdn.example/x.png"), Err(WebError::Site(m)) if m.contains("image data")));
        assert!(matches!(png_from_data_url("data:image/png;base64,***"), Err(WebError::Site(m)) if m.contains("damaged")));
        // Recognised as PNG but cut short.
        assert!(matches!(png_from_data_url("data:image/png;base64,iVBORw0KGgo="), Err(WebError::Site(m)) if m.contains("could not be read")));
    }

    #[test]
    fn references_go_small_and_results_are_saved() {
        let t = Scratch::new("data");
        let big = t.0.join("big.png");
        image::RgbImage::from_fn(2000, 1000, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])).save(&big).unwrap();
        let b64 = small_jpeg_b64(&big, 1280).unwrap();
        let out = t.0.join("sub").join("back.png");
        assert_eq!(save_data_url(&format!("data:image/jpeg;base64,{b64}"), &out).unwrap(), (1280, 640));
        assert!(out.is_file());
        assert!(matches!(small_jpeg_b64(&t.0.join("missing.png"), 1280), Err(WebError::Site(m)) if m.contains("missing.png")));
    }
}
