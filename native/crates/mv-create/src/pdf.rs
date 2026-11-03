//! A PDF of page images: one page per image, the image filling it, sized for print at 300 dpi.
//! The images go in as JPEG (DCTDecode), so no PDF library is needed.
use std::io::Write as _;
use std::path::Path;

use anyhow::{bail, Context as _, Result};

const DPI: f32 = 300.;

fn jpeg(file: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::open(file).with_context(|| format!("could not read {}", file.display()))?.to_rgb8();
    let (w, h) = img.dimensions();
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92))?;
    Ok((w, h, out.into_inner()))
}

/// Writes `pages` (in order) as a PDF at `out`, titled `title`.
pub fn write(pages: &[std::path::PathBuf], title: &str, out: &Path) -> Result<()> {
    if pages.is_empty() {
        bail!("there are no painted pages to export yet");
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    // Objects: 1 catalog, 2 pages, 3 info, then per page: page, contents, image.
    let n = pages.len();
    let page_obj = |i: usize| 4 + i * 3;
    let obj = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(buf.len());
        let id = offsets.len();
        buf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut buf, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect::<Vec<_>>().join(" ");
    obj(&mut buf, &mut offsets, format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").as_bytes());
    let safe: String = title.chars().filter(|c| c.is_ascii() && !matches!(c, '(' | ')' | '\\')).collect();
    obj(&mut buf, &mut offsets, format!("<< /Title ({safe}) /Producer (MangaVox Creative) >>").as_bytes());
    for (i, file) in pages.iter().enumerate() {
        let (w, h, data) = jpeg(file)?;
        let (pw, ph) = (w as f32 * 72. / DPI, h as f32 * 72. / DPI);
        let (contents, image) = (page_obj(i) + 1, page_obj(i) + 2);
        obj(&mut buf, &mut offsets, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im{i} {image} 0 R >> >> /Contents {contents} 0 R >>").as_bytes());
        let draw = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im{i} Do Q");
        obj(&mut buf, &mut offsets, format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).as_bytes());
        let mut body = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        obj(&mut buf, &mut offsets, &body);
    }
    let xref = buf.len();
    buf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        buf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::File::create(out).with_context(|| format!("could not write {}", out.display()))?;
    f.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_looking_pdf() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a.png");
        let b = t.path().join("b.png");
        image::RgbImage::from_pixel(600, 900, image::Rgb([200, 10, 10])).save(&a).unwrap();
        image::RgbImage::from_pixel(600, 900, image::Rgb([10, 10, 200])).save(&b).unwrap();
        let out = t.path().join("x.pdf");
        write(&[a, b], "My (Manga)", &out).unwrap();
        let data = std::fs::read(&out).unwrap();
        let has = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
        assert!(data.starts_with(b"%PDF-1.4"));
        assert!(has(b"/Count 2") && has(b"/Title (My Manga)"));
        assert!(data.ends_with(b"%%EOF\n"));
        // The xref offsets point at the objects (byte offsets, so read the tail as bytes).
        let tail_at = data.windows(10).rposition(|w| w == b"startxref\n").unwrap();
        let tail = std::str::from_utf8(&data[tail_at..]).unwrap();
        let start: usize = tail.lines().nth(1).unwrap().parse().unwrap();
        assert!(data[start..].starts_with(b"xref"));
        let table = std::str::from_utf8(&data[start..tail_at]).unwrap();
        let first: usize = table.lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(data[first..].starts_with(b"1 0 obj"));
        assert!(write(&[], "x", &out).is_err());
    }
}
