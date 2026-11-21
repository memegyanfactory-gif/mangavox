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

/// Longest side of a picture shown large (picture viewers, the animatic stage): a full-HD view.
pub const DISPLAY_MAX: u32 = 1920;
/// Decoded large pictures kept in memory (and on the GPU) at most, in bytes: about 24 full-HD
/// pictures, kept apart from the thumbnails' [`BUDGET`].
pub const DISPLAY_BUDGET: usize = 192 << 20;

/// Size of a `w` x `h` picture scaled so its longest side is at most `max` (never enlarged).
pub fn fit_within(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max || long == 0 {
        return (w, h);
    }
    let scale = |v: u32| ((v as u64 * max as u64) / long as u64).max(1) as u32;
    (scale(w), scale(h))
}

/// RGBA pixels of `src` with its longest side at most [`DISPLAY_MAX`]. Nothing is saved: large
/// views open a handful of pictures, and the in-memory cache keeps the recent ones.
pub fn load_display(src: &Path) -> anyhow::Result<RgbaImage> {
    let img = decode(&std::fs::read(src)?)?;
    let (w, h) = fit_within(img.width(), img.height(), DISPLAY_MAX);
    if (w, h) == (img.width(), img.height()) {
        return Ok(img.into_rgba8());
    }
    Ok(img.resize_exact(w, h, FilterType::Triangle).into_rgba8())
}

/// A picture file a user hands in, checked before it is kept: at most `max_bytes`, a real PNG,
/// JPG or WebP (whatever its name says), at most `max_side` px on a side, and it decodes to the
/// end. Answers the extension of its real format; the error is a plain phrase ("it is not a
/// picture"), never a path.
pub fn check_upload(path: &Path, max_side: u32, max_bytes: u64) -> anyhow::Result<&'static str> {
    let len = std::fs::metadata(path).map_err(|_| anyhow::anyhow!("it could not be opened"))?.len();
    if len > max_bytes {
        anyhow::bail!("it is larger than {} MB", max_bytes / 1_000_000);
    }
    let bytes = std::fs::read(path).map_err(|_| anyhow::anyhow!("it could not be opened"))?;
    let reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|_| anyhow::anyhow!("it is not a picture"))?;
    let ext = match reader.format() {
        Some(image::ImageFormat::Png) => "png",
        Some(image::ImageFormat::Jpeg) => "jpg",
        Some(image::ImageFormat::WebP) => "webp",
        _ => anyhow::bail!("it is not a PNG, JPG or WebP picture"),
    };
    let (w, h) = reader.into_dimensions().map_err(|_| anyhow::anyhow!("it is not a picture"))?;
    if w == 0 || h == 0 {
        anyhow::bail!("it is not a picture");
    }
    if w > max_side || h > max_side {
        anyhow::bail!("it is larger than {max_side} pixels on a side");
    }
    decode(&bytes).map_err(|_| anyhow::anyhow!("it is damaged and could not be read to the end"))?;
    Ok(ext)
}

/// Pixels as GPUI draws them (BGRA).
fn render_image(mut px: RgbaImage) -> Arc<RenderImage> {
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(px)]))
}

/// GPUI asset: a cover file decoded at display size on the background executor.
enum ThumbAsset {}

impl Asset for ThumbAsset {
    type Source = Arc<Path>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(src: Self::Source, _: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move { load(&src).map(render_image).map_err(|e| ImageCacheError::Other(Arc::new(e))) }
    }
}

/// GPUI asset: a picture decoded at most [`DISPLAY_MAX`] on a side on the background executor.
enum DisplayAsset {}

impl Asset for DisplayAsset {
    type Source = Arc<Path>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(src: Self::Source, _: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move { load_display(&src).map(render_image).map_err(|e| ImageCacheError::Other(Arc::new(e))) }
    }
}

struct Entry {
    used: Instant,
    bytes: usize,
    image: Option<Arc<RenderImage>>,
}

/// Loaded images of asset kind `A` by path, for the byte budget (GPUI's asset map itself never
/// evicts).
struct Lru<A> {
    map: HashMap<Arc<Path>, Entry>,
    bytes: usize,
    kind: std::marker::PhantomData<fn() -> A>,
}

impl<A> Default for Lru<A> {
    fn default() -> Self {
        Self { map: HashMap::new(), bytes: 0, kind: std::marker::PhantomData }
    }
}

impl<A: 'static> Global for Lru<A> {}

/// `img` source for a local cover at display size; drop-in for `img(path)`.
pub fn cover_source(path: PathBuf) -> ImageSource {
    let src: Arc<Path> = path.into();
    ImageSource::Custom(Arc::new(move |window, cx| {
        let out = window.use_asset::<ThumbAsset>(&src, cx);
        touch::<ThumbAsset>(&src, out.as_ref(), BUDGET, window, cx);
        out
    }))
}

/// `img` source for a local picture shown large (a viewer, a stage): decoded off the UI thread
/// at most [`DISPLAY_MAX`] on a side and kept in a cache bounded by [`DISPLAY_BUDGET`], so
/// stepping through hundreds of pictures never keeps them all. Drop-in for `img(path)`.
pub fn display_source(path: PathBuf) -> ImageSource {
    let src: Arc<Path> = path.into();
    ImageSource::Custom(Arc::new(move |window, cx| {
        let out = window.use_asset::<DisplayAsset>(&src, cx);
        touch::<DisplayAsset>(&src, out.as_ref(), DISPLAY_BUDGET, window, cx);
        out
    }))
}

fn touch<A>(src: &Arc<Path>, out: Option<&Result<Arc<RenderImage>, ImageCacheError>>, budget: usize, window: &mut Window, cx: &mut App)
where
    A: Asset<Source = Arc<Path>> + 'static,
{
    let now = Instant::now();
    let lru = cx.default_global::<Lru<A>>();
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
    if lru.bytes <= budget {
        return;
    }
    // Over budget: drop the least recently drawn images that are not on screen.
    let mut old: Vec<(Instant, Arc<Path>)> = lru.map.iter().filter(|(_, e)| e.image.is_some() && now.duration_since(e.used) > KEEP).map(|(k, e)| (e.used, k.clone())).collect();
    old.sort_unstable_by_key(|(t, _)| *t);
    let mut evicted = Vec::new();
    for (_, k) in old {
        if lru.bytes <= budget * 3 / 4 {
            break;
        }
        if let Some(e) = lru.map.remove(&k) {
            lru.bytes -= e.bytes;
            evicted.extend(e.image.map(|i| (k, i)));
        }
    }
    for (k, img) in evicted {
        cx.remove_asset::<A>(&k);
        cx.drop_image(img, Some(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // gpui_kit's glob import brings its own `test` macro; use the standard one.
    #[::core::prelude::v1::test]
    fn large_views_are_bounded_on_their_longest_side() {
        assert_eq!(fit_within(3840, 2160, DISPLAY_MAX), (1920, 1080));
        assert_eq!(fit_within(1000, 4000, DISPLAY_MAX), (480, 1920));
        assert_eq!(fit_within(800, 600, DISPLAY_MAX), (800, 600), "small pictures are never enlarged");
        assert_eq!(fit_within(0, 0, DISPLAY_MAX), (0, 0));
        assert_eq!(fit_within(100_000, 1, DISPLAY_MAX), (1920, 1));
        let tmp = std::env::temp_dir().join(format!("mv-display-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("tall.png");
        RgbaImage::from_pixel(1200, 2400, image::Rgba([1, 2, 3, 255])).save(&src).unwrap();
        assert_eq!(load_display(&src).unwrap().dimensions(), (960, 1920));
        std::fs::write(tmp.join("bad.png"), b"not a picture").unwrap();
        assert!(load_display(&tmp.join("bad.png")).is_err());
        // Uploads: the real format counts, not the name; too big or broken files are refused.
        assert_eq!(check_upload(&src, 8192, 50_000_000).unwrap(), "png");
        let renamed = tmp.join("really-a-png.jpg");
        std::fs::copy(&src, &renamed).unwrap();
        assert_eq!(check_upload(&renamed, 8192, 50_000_000).unwrap(), "png");
        assert!(check_upload(&tmp.join("bad.png"), 8192, 50_000_000).is_err());
        assert!(check_upload(&src, 2000, 50_000_000).is_err(), "2400 px tall is over a 2000 px cap");
        assert!(check_upload(&src, 8192, 10).is_err(), "over the byte cap");
        let bytes = std::fs::read(&src).unwrap();
        std::fs::write(tmp.join("cut.png"), &bytes[..bytes.len() / 2]).unwrap();
        assert!(check_upload(&tmp.join("cut.png"), 8192, 50_000_000).is_err(), "a truncated file is refused");
        assert!(check_upload(&tmp.join("missing.png"), 8192, 50_000_000).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
