//! Display-size cover thumbnails. Cached covers are full-size files (often 800-1500 px tall);
//! GPUI decoded each one at full size, kept it forever and uploaded it to the GPU, where it was
//! drawn at 150 px. Here a cover is decoded once, scaled to [`MAX_W`] px wide and saved as a JPEG
//! in `cache/thumbs`; grids then decode that small file, off the UI thread, and the decoded
//! images live in a byte-bounded cache ([`BUDGET`]) that frees GPU memory as it evicts.
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use image::{imageops::FilterType, DynamicImage, ImageDecoder, ImageReader, RgbaImage};

/// Thumbnail width: a 150-200 px card at 2x display scale.
pub const MAX_W: u32 = 400;
/// Decoded thumbnails kept in memory (and on the GPU) at most, in bytes.
pub const BUDGET: usize = 384 << 20;
/// An image drawn this recently is never evicted (it is probably on screen).
const KEEP: Duration = Duration::from_secs(5);

pub fn dir() -> PathBuf {
    mv_core::paths::cache().join("thumbs")
}

/// Thumbs kept on disk at most; [`prune`] drops the oldest beyond this.
pub const DISK_CAP: u64 = 256 << 20;

/// Thumb file for `src`: one per cover, keyed by its path inside the data folder (so a moved
/// folder keeps its thumbs). A replaced cover overwrites it; see [`load`].
pub fn thumb_path(src: &Path) -> PathBuf {
    let rel = src.strip_prefix(mv_core::paths::root()).unwrap_or(src);
    let key = rel.to_string_lossy().replace('\\', "/");
    dir().join(format!("{:x}.jpg", u128::from_be_bytes(md5::compute(key.as_bytes()).0)))
}

fn decode(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    let mut dec = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orient = dec.orientation()?;
    let mut img = DynamicImage::from_decoder(dec)?;
    img.apply_orientation(orient);
    Ok(img)
}

/// The saved thumb, when it is newer than the cover and made at today's [`MAX_W`].
fn saved(src: &std::fs::Metadata, thumb: &Path) -> Option<RgbaImage> {
    let t = std::fs::metadata(thumb).ok()?;
    if t.modified().ok()? < src.modified().ok()? {
        return None;
    }
    let img = decode(&std::fs::read(thumb).ok()?).ok()?;
    (img.width() == MAX_W).then(|| img.into_rgba8())
}

/// RGBA pixels at display size: the saved thumb, else `src` scaled down (and the thumb saved).
/// Covers already at most [`MAX_W`] wide are used as they are, with no thumb file.
pub fn load(src: &Path) -> anyhow::Result<RgbaImage> {
    let meta = std::fs::metadata(src)?;
    let thumb = thumb_path(src);
    if let Some(img) = saved(&meta, &thumb) {
        return Ok(img);
    }
    let img = decode(&std::fs::read(src)?)?;
    if img.width() <= MAX_W {
        return Ok(img.into_rgba8());
    }
    let h = ((img.height() as u64 * MAX_W as u64) / img.width() as u64).max(1) as u32;
    let small = img.resize_exact(MAX_W, h, FilterType::Triangle).into_rgb8();
    save(&thumb, &small);
    Ok(DynamicImage::ImageRgb8(small).into_rgba8())
}

/// Keeps `cache/thumbs` under [`DISK_CAP`], oldest first (thumbs of deleted covers age out),
/// and clears temp files a crashed write left. Called once per session, off the UI thread.
pub fn prune() {
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let (p, Ok(m)) = (e.path(), e.metadata()) else { continue };
        let t = m.modified().unwrap_or(std::time::UNIX_EPOCH);
        if p.extension().is_some_and(|x| x != "jpg") {
            if t.elapsed().is_ok_and(|d| d > Duration::from_secs(3600)) {
                let _ = std::fs::remove_file(&p);
            }
            continue;
        }
        files.push((t, m.len(), p));
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= DISK_CAP {
        return;
    }
    files.sort_unstable_by_key(|f| f.0);
    for (_, len, p) in files {
        if total <= DISK_CAP * 3 / 4 {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
}

/// Best effort: a failed write only means the next start scales the cover again.
fn save(path: &Path, img: &image::RgbImage) {
    let _ = std::fs::create_dir_all(dir());
    let mut buf = Vec::with_capacity(48 << 10);
    if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85).encode_image(img).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, &buf).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        // Private mode: thumbs of covers seen in the session are cleared with them.
        mv_core::privacy::record_file(path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// GPUI asset: a cover file decoded at display size on the background executor.
enum ThumbAsset {}

impl Asset for ThumbAsset {
    type Source = Arc<Path>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(src: Self::Source, _: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move {
            let mut px = load(&src).map_err(|e| ImageCacheError::Other(Arc::new(e)))?;
            // GPUI draws BGRA.
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(px)])))
        }
    }
}

struct Entry {
    used: Instant,
    bytes: usize,
    image: Option<Arc<RenderImage>>,
}

/// Loaded thumbnails by path, for the byte budget (GPUI's asset map itself never evicts).
#[derive(Default)]
struct Lru {
    map: HashMap<Arc<Path>, Entry>,
    bytes: usize,
}

impl Global for Lru {}

/// `img` source for a local cover at display size; drop-in for `img(path)`.
pub fn cover_source(path: PathBuf) -> ImageSource {
    let src: Arc<Path> = path.into();
    ImageSource::Custom(Arc::new(move |window, cx| {
        let out = window.use_asset::<ThumbAsset>(&src, cx);
        touch(&src, out.as_ref(), window, cx);
        out
    }))
}

fn touch(src: &Arc<Path>, out: Option<&Result<Arc<RenderImage>, ImageCacheError>>, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let lru = cx.default_global::<Lru>();
    let e = lru.map.entry(src.clone()).or_insert(Entry { used: now, bytes: 0, image: None });
    e.used = now;
    if e.image.is_none() {
        if let Some(Ok(img)) = out {
            let s = img.size(0);
            e.bytes = (s.width.0.max(0) as usize) * (s.height.0.max(0) as usize) * 4;
            e.image = Some(img.clone());
            lru.bytes += e.bytes;
        }
    }
    if lru.bytes <= BUDGET {
        return;
    }
    // Over budget: drop the least recently drawn images that are not on screen.
    let mut old: Vec<(Instant, Arc<Path>)> = lru.map.iter().filter(|(_, e)| e.image.is_some() && now.duration_since(e.used) > KEEP).map(|(k, e)| (e.used, k.clone())).collect();
    old.sort_unstable_by_key(|(t, _)| *t);
    let mut evicted = Vec::new();
    for (_, k) in old {
        if lru.bytes <= BUDGET * 3 / 4 {
            break;
        }
        if let Some(e) = lru.map.remove(&k) {
            lru.bytes -= e.bytes;
            evicted.extend(e.image.map(|i| (k, i)));
        }
    }
    for (k, img) in evicted {
        cx.remove_asset::<ThumbAsset>(&k);
        cx.drop_image(img, Some(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // gpui_kit's glob import brings its own `test` macro; use the standard one.
    #[::core::prelude::v1::test]
    fn scales_wide_covers_and_saves_a_thumb() {
        let tmp = std::env::temp_dir().join(format!("mv-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("MANGAVOX_HOME", &tmp);
        let src = tmp.join("big.png");
        RgbaImage::from_pixel(1000, 1500, image::Rgba([200, 10, 10, 255])).save(&src).unwrap();
        let a = load(&src).unwrap();
        assert_eq!((a.width(), a.height()), (MAX_W, 600));
        let thumb = thumb_path(&src);
        assert!(thumb.is_file());
        // Second load reads the thumb.
        let b = load(&src).unwrap();
        assert_eq!(b.dimensions(), a.dimensions());
        // A replaced cover overwrites its thumb.
        std::thread::sleep(Duration::from_millis(30));
        RgbaImage::from_pixel(800, 800, image::Rgba([0, 10, 10, 255])).save(&src).unwrap();
        assert_eq!(load(&src).unwrap().dimensions(), (MAX_W, MAX_W));
        assert_eq!(std::fs::read_dir(dir()).unwrap().count(), 1);
        // Small covers are used as they are.
        let small = tmp.join("small.png");
        RgbaImage::from_pixel(200, 300, image::Rgba([0, 0, 0, 255])).save(&small).unwrap();
        assert_eq!(load(&small).unwrap().dimensions(), (200, 300));
        assert!(!thumb_path(&small).exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
