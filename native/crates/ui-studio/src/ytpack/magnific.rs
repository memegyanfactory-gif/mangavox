//! Magnific's image API (the former Freepik API, `docs.magnific.com`): the models it offers,
//! each request shape, a task that is polled until the image is ready, and the credits Studio
//! has spent. The key stays on this machine in `studio/magnific.json`.
//!
//! Web-app "Unlimited" allowances do not cover the API ("API calls always run on the credit
//! model"), so `web_unlimited` is shown for reference only and every API image costs credits.
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use mv_ui::{ph, Ph};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const API: &str = "https://api.magnific.com/v1/ai/";
/// How a 401 error starts, so the UI can ask for a new key.
pub const KEY_REJECTED: &str = "Magnific rejected the API key";
pub const RESOLUTIONS: [&str; 3] = ["1K", "2K", "4K"];

/// How a model's request is built.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// `aspect_ratio` "16:9", `resolution` "2K", `reference_images: [{image, mime_type, text}]`.
    Banana,
    /// Gemini 2.5 Flash Image: `reference_images: [base64]`, no size fields.
    Gemini,
    /// `aspect_ratio` "widescreen_16_9", optional lowercase `resolution`; references go to
    /// `<path>-edit` as `reference_images: [base64]`.
    Named,
    /// Flux 2 Pro / Flex: `width` / `height`, references as `input_image`, `input_image_2`…
    FluxSize,
    /// Flux 2 Klein: named `aspect_ratio`, lowercase `resolution`, `input_image`…
    FluxNamed,
    /// Ideogram 4.5: plain `aspect_ratio`, `images: [base64]`.
    Ideogram,
    /// Z-Image: `image_size` "landscape_16_9".
    ZImage,
}

/// One image model as the API offers it, with the web app's speed and price for the picker.
pub struct ImageModel {
    pub id: &'static str,
    pub name: &'static str,
    pub provider: &'static str,
    /// Under `/v1/ai/`.
    pub path: &'static str,
    pub shape: Shape,
    /// Reference images it takes (0: none).
    pub refs: usize,
    pub speed: &'static str,
    /// Selectable resolutions; empty when the model has no resolution choice.
    pub resolutions: &'static [&'static str],
    /// Credits per image at the lowest and highest resolution (Magnific's list in the web app);
    /// `None` when Magnific shows no price because the web plan makes it unlimited.
    pub credits: Option<(f64, f64)>,
    /// The resolution the web plan makes without credits (web app only).
    pub web_unlimited: Option<&'static str>,
    pub featured: bool,
    pub new: bool,
    /// Edits keep the source's aspect with "auto".
    pub auto_aspect: bool,
}

const NONE: &[&str] = &[];
const K124: &[&str] = &["1K", "2K", "4K"];
const K12: &[&str] = &["1K", "2K"];

/// Nano Banana 2.1 first: it is the default, and the fallback when references are attached to
/// a model that takes none.
pub static CATALOG: &[ImageModel] = &[
    ImageModel { id: "nano-banana-2-1", name: "Google Nano Banana 2.1", provider: "Google", path: "text-to-image/nano-banana-2-1", shape: Shape::Banana, refs: 14, speed: "~42s", resolutions: K124, credits: Some((50., 145.)), web_unlimited: None, featured: true, new: true, auto_aspect: true },
    ImageModel { id: "nano-banana-pro", name: "Google Nano Banana Pro", provider: "Google", path: "text-to-image/nano-banana-pro", shape: Shape::Banana, refs: 0, speed: "~41s", resolutions: K124, credits: Some((75., 150.)), web_unlimited: None, featured: false, new: false, auto_aspect: true },
    ImageModel { id: "nano-banana-pro-flash", name: "Google Nano Banana Pro Flash", provider: "Google", path: "text-to-image/nano-banana-pro-flash", shape: Shape::Banana, refs: 0, speed: "~20s", resolutions: K124, credits: None, web_unlimited: None, featured: false, new: false, auto_aspect: true },
    ImageModel { id: "nano-banana", name: "Google Nano Banana", provider: "Google", path: "gemini-2-5-flash-image-preview", shape: Shape::Gemini, refs: 3, speed: "~13s", resolutions: NONE, credits: None, web_unlimited: Some("any size"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "gpt-image-2-5", name: "GPT 2.5", provider: "GPT", path: "text-to-image/gpt-image-2-5", shape: Shape::Named, refs: 16, speed: "~37s", resolutions: K124, credits: Some((15., 940.)), web_unlimited: None, featured: true, new: false, auto_aspect: false },
    ImageModel { id: "gpt-image-2", name: "GPT 2", provider: "GPT", path: "text-to-image/gpt-image-2", shape: Shape::Named, refs: 16, speed: "~33s", resolutions: K124, credits: Some((15., 690.)), web_unlimited: None, featured: false, new: false, auto_aspect: false },
    ImageModel { id: "seedream-v5-pro", name: "Seedream 5 Pro", provider: "Seedream", path: "text-to-image/seedream-v5-pro", shape: Shape::Named, refs: 10, speed: "~1m 6s", resolutions: &["1.5K", "2K"], credits: Some((75., 100.)), web_unlimited: None, featured: true, new: false, auto_aspect: false },
    ImageModel { id: "seedream-v5-lite", name: "Seedream 5 Lite", provider: "Seedream", path: "text-to-image/seedream-v5-lite", shape: Shape::Named, refs: 5, speed: "~1m 6s", resolutions: NONE, credits: None, web_unlimited: Some("2K"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "seedream-v4-5", name: "Seedream 4.5", provider: "Seedream", path: "text-to-image/seedream-v4-5", shape: Shape::Named, refs: 5, speed: "~57s", resolutions: NONE, credits: None, web_unlimited: Some("2K"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "seedream-v4", name: "Seedream 4", provider: "Seedream", path: "text-to-image/seedream-v4", shape: Shape::Named, refs: 0, speed: "~26s", resolutions: NONE, credits: None, web_unlimited: Some("any size"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "flux-2-pro", name: "Flux.2 Pro", provider: "Flux", path: "text-to-image/flux-2-pro", shape: Shape::FluxSize, refs: 4, speed: "~17s", resolutions: NONE, credits: None, web_unlimited: Some("any size"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "flux-2-flex", name: "Flux.2 Flex", provider: "Flux", path: "text-to-image/flux-2-flex", shape: Shape::FluxSize, refs: 4, speed: "~17s", resolutions: NONE, credits: Some((80., 280.)), web_unlimited: None, featured: false, new: false, auto_aspect: false },
    ImageModel { id: "flux-2-klein", name: "Flux.2 Klein", provider: "Flux", path: "text-to-image/flux-2-klein", shape: Shape::FluxNamed, refs: 4, speed: "~7s", resolutions: K12, credits: None, web_unlimited: Some("1K"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "mystic", name: "Mystic", provider: "Mystic", path: "mystic", shape: Shape::Named, refs: 0, speed: "~30s", resolutions: K124, credits: None, web_unlimited: Some("1K"), featured: false, new: false, auto_aspect: false },
    ImageModel { id: "ideogram-4-5", name: "Ideogram 4.5", provider: "Ideogram", path: "text-to-image/ideogram-4-5", shape: Shape::Ideogram, refs: 5, speed: "~20s", resolutions: NONE, credits: Some((80., 270.)), web_unlimited: None, featured: false, new: false, auto_aspect: true },
    ImageModel { id: "ideogram-4", name: "Ideogram 4", provider: "Ideogram", path: "text-to-image/ideogram-4", shape: Shape::Ideogram, refs: 0, speed: "~16s", resolutions: NONE, credits: Some((120., 200.)), web_unlimited: None, featured: false, new: false, auto_aspect: false },
    ImageModel { id: "z-image", name: "Z-Image", provider: "Z-Image", path: "text-to-image/z-image", shape: Shape::ZImage, refs: 0, speed: "~9s", resolutions: NONE, credits: None, web_unlimited: Some("any size"), featured: false, new: false, auto_aspect: false },
];

impl ImageModel {
    pub fn icon(&self) -> Ph {
        match self.provider {
            "Google" => ph::STAR_FOUR,
            "GPT" => ph::ROBOT,
            "Seedream" => ph::CHART_BAR,
            "Flux" => ph::LIGHTNING,
            "Mystic" => ph::SPARKLE,
            "Ideogram" => ph::TEXT_AA,
            _ => ph::IMAGE,
        }
    }

    /// It takes an aspect ratio. Gemini 2.5 Flash Image has no size field and makes squares.
    pub fn takes_aspect(&self) -> bool {
        self.shape != Shape::Gemini
    }

    /// "2K-4K", "1K-2K", or "" for one size.
    pub fn res_label(&self) -> String {
        match (self.resolutions.first(), self.resolutions.last()) {
            (Some(a), Some(b)) if a != b => format!("{a}-{b}"),
            (Some(a), _) => a.to_string(),
            _ => "Auto size".into(),
        }
    }

    /// Estimated credits for one image at `res` (between the list's low and high); 0 unknown.
    pub fn cost_at(&self, res: &str) -> f64 {
        let Some((lo, hi)) = self.credits else { return 0. };
        let n = self.resolutions.len();
        if n < 2 {
            return lo;
        }
        let i = self.resolutions.iter().position(|r| *r == res).unwrap_or(0);
        lo + (hi - lo) * i as f64 / (n - 1) as f64
    }

    /// The web plan makes this model unlimited at `res` (in the web app only).
    pub fn web_unlimited_at(&self, res: &str) -> bool {
        self.web_unlimited.is_some_and(|u| u == "any size" || u == res || self.resolutions.is_empty())
    }

    /// " · ~95 credits" for the resolution menu.
    pub fn cost_label(&self, res: &str) -> String {
        match self.cost_at(res) {
            c if c > 0. => format!(" · ~{} credits", c.round()),
            _ => String::new(),
        }
    }
}

// ---------------------------------------------------------------- key, prefs, usage

fn key_file() -> PathBuf {
    mv_core::paths::studio().join("magnific.json")
}

/// The saved key, else `MAGNIFIC_API_KEY`.
pub fn key() -> Option<String> {
    let saved = std::fs::read(key_file()).ok().and_then(|d| serde_json::from_slice::<Value>(&d).ok()).and_then(|v| v.get("api_key")?.as_str().map(str::to_string));
    saved.or_else(|| std::env::var("MAGNIFIC_API_KEY").ok()).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// Saves the key, or forgets it when `None`.
pub fn set_key(key: Option<&str>) -> Result<()> {
    let p = key_file();
    match key {
        Some(k) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&p, serde_json::to_vec_pretty(&json!({ "api_key": k.trim() }))?)?;
        }
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
    Ok(())
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    std::fs::read(mv_core::paths::studio().join(name)).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn save_json<T: Serialize>(name: &str, v: &T) {
    let p = mv_core::paths::studio().join(name);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(d) = serde_json::to_vec_pretty(v) {
        let _ = std::fs::write(p, d);
    }
}

/// The generator's last model, size and manga.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub model: String,
    pub aspect: String,
    pub resolution: String,
    /// The manga the Thumbnails page opens on.
    pub manga: String,
}

impl Prefs {
    pub fn load() -> Prefs {
        load_json("magnific_prefs.json")
    }
    pub fn save(&self) {
        save_json("magnific_prefs.json", self)
    }
}

/// Credits Studio has spent through the API (Magnific has no balance endpoint), and the plan's
/// credits the user typed, for "left".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub spent: f64,
    pub images: u32,
    /// Images whose price Studio does not know (counted, not priced).
    pub unpriced: u32,
    pub budget: Option<u64>,
    /// "YYYY-MM" → credits.
    pub months: std::collections::BTreeMap<String, f64>,
}

fn this_month() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

impl Usage {
    pub fn load() -> Usage {
        load_json("magnific_usage.json")
    }
    pub fn save(&self) {
        save_json("magnific_usage.json", self)
    }
    pub fn add(&mut self, credits: f64) {
        self.images += 1;
        if credits <= 0. {
            self.unpriced += 1;
        }
        self.spent += credits;
        *self.months.entry(this_month()).or_default() += credits;
        self.save();
    }
    pub fn month_spent(&self) -> f64 {
        self.months.get(&this_month()).copied().unwrap_or(0.)
    }
}

// ---------------------------------------------------------------- requests

/// One reference image: the file and its caption ("Image 2: Arhan").
pub struct Reference {
    pub file: PathBuf,
    pub caption: String,
}

/// An image as base64 JPEG, at most 1536 px on the long side (a previous 4K thumbnail would be
/// a needlessly large request).
fn encode(file: &Path) -> Result<String> {
    let img = image::open(file).with_context(|| format!("could not read {}", file.display()))?;
    let img = if img.width().max(img.height()) > 1536 { img.resize(1536, 1536, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut out, image::ImageFormat::Jpeg)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

/// "16:9" → "widescreen_16_9".
fn named(aspect: &str) -> &'static str {
    match aspect {
        "9:16" => "social_story_9_16",
        "1:1" => "square_1_1",
        "4:3" => "classic_4_3",
        _ => "widescreen_16_9",
    }
}

/// The request path and body for `m`.
pub fn request(m: &ImageModel, prompt: &str, images: &[(String, String)], aspect: &str, res: &str) -> (String, Value) {
    let lower = res.to_lowercase();
    let has_res = !m.resolutions.is_empty();
    let mut body = json!({ "prompt": prompt });
    let mut path = m.path.to_string();
    match m.shape {
        Shape::Banana => {
            body["aspect_ratio"] = json!(aspect);
            if has_res {
                body["resolution"] = json!(res);
            }
            if !images.is_empty() {
                body["reference_images"] = Value::Array(images.iter().map(|(data, text)| json!({ "image": data, "mime_type": "image/jpeg", "text": text })).collect());
            }
        }
        Shape::Gemini => {
            if !images.is_empty() {
                body["reference_images"] = json!(images.iter().map(|(d, _)| d).collect::<Vec<_>>());
            }
        }
        Shape::Named => {
            body["aspect_ratio"] = json!(if m.id.starts_with("gpt") && aspect == "auto" { "auto" } else { named(aspect) });
            if has_res {
                body["resolution"] = json!(lower);
            }
            if !images.is_empty() {
                path.push_str("-edit");
                body["reference_images"] = json!(images.iter().map(|(d, _)| d).collect::<Vec<_>>());
            }
        }
        Shape::FluxSize | Shape::FluxNamed => {
            if m.shape == Shape::FluxSize {
                let big = m.id == "flux-2-flex";
                let (w, h) = match aspect {
                    "9:16" => if big { (864, 1536) } else { (720, 1280) },
                    "1:1" => if big { (1536, 1536) } else { (1440, 1440) },
                    "4:3" => if big { (1536, 1152) } else { (1408, 1056) },
                    _ => if big { (1536, 864) } else { (1280, 720) },
                };
                body["width"] = json!(w);
                body["height"] = json!(h);
            } else {
                body["aspect_ratio"] = json!(named(aspect));
                body["resolution"] = json!(lower);
            }
            for (k, (d, _)) in images.iter().take(4).enumerate() {
                body[if k == 0 { "input_image".to_string() } else { format!("input_image_{}", k + 1) }] = json!(d);
            }
        }
        Shape::Ideogram => {
            body["aspect_ratio"] = json!(aspect);
            if !images.is_empty() {
                body["images"] = json!(images.iter().map(|(d, _)| d).collect::<Vec<_>>());
            }
        }
        Shape::ZImage => {
            body["image_size"] = json!(match aspect {
                "9:16" => "portrait_9_16",
                "1:1" => "square_hd",
                "4:3" => "landscape_4_3",
                _ => "landscape_16_9",
            });
        }
    }
    (path, body)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?)
}

async fn check(r: reqwest::Response) -> Result<Value> {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let msg = body.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    let detail = body.get("invalid_params").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| Some(format!("{}: {}", p.get("field")?.as_str()?, p.get("reason")?.as_str()?))).collect::<Vec<_>>().join("; ")).unwrap_or_default();
    match status.as_u16() {
        401 => bail!("{KEY_REJECTED}: {}", if msg.is_empty() { "unauthorized" } else { &msg }),
        402 | 403 => bail!("Magnific refused the request: {}", if msg.is_empty() { "not enough API credits, or the plan does not include this model" } else { &msg }),
        429 => bail!("Magnific is rate limiting; wait a moment and try again"),
        _ => bail!("Magnific error {status}: {msg}{}", if detail.is_empty() { String::new() } else { format!(" ({detail})") }),
    }
}

/// The task's data object, wrapped in `data` or not.
fn data(v: &Value) -> &Value {
    v.get("data").unwrap_or(v)
}

/// Checks `key` without spending credits: asks for a task that does not exist. Magnific checks
/// the key first, so 401/403 means a bad key and anything else ("Task not found") a good one.
pub async fn verify(key: &str) -> Result<()> {
    let http = client()?;
    let r = http.get(format!("{API}{}/00000000-0000-0000-0000-000000000000", CATALOG[0].path)).header("x-magnific-api-key", key).send().await.context("could not reach Magnific")?;
    match r.status().as_u16() {
        401 | 402 | 403 | 429 => check(r).await.map(|_| ()),
        s if s >= 500 => bail!("Magnific is having trouble right now ({s}); try again in a minute"),
        _ => Ok(()),
    }
}

/// `img` cut to `aspect` ("16:9") around its centre when its shape is more than 3% off, so a
/// model that ignores the size still gives a thumbnail of the asked shape. "auto" keeps it.
pub fn fit_aspect(img: image::DynamicImage, aspect: &str) -> image::DynamicImage {
    let Some((a, b)) = aspect.split_once(':').and_then(|(a, b)| Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?))).filter(|(a, b)| *a > 0. && *b > 0.) else { return img };
    let (w, h) = (img.width() as f64, img.height() as f64);
    let want = a / b;
    if h == 0. || ((w / h) / want - 1.).abs() <= 0.03 {
        return img;
    }
    let (cw, ch) = if w / h > want { ((h * want).round(), h) } else { (w, (w / want).round()) };
    img.crop_imm(((w - cw) / 2.) as u32, ((h - ch) / 2.) as u32, cw as u32, ch as u32)
}

/// Makes one image with `m` and saves it as PNG at `out`. `on_status` hears the progress.
pub async fn generate(m: &'static ImageModel, key: &str, prompt: &str, refs: &[Reference], aspect: &str, res: &str, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let mut images = vec![];
    for r in refs.iter().take(m.refs) {
        let file = r.file.clone();
        let data = tokio::task::spawn_blocking(move || encode(&file)).await??;
        images.push((data, r.caption.clone()));
    }
    let (path, body) = request(m, prompt, &images, aspect, res);
    let url = format!("{API}{path}");
    let http = client()?;
    on_status("Sending");
    let v = check(http.post(&url).header("x-magnific-api-key", key).json(&body).send().await.context("could not reach Magnific")?).await?;
    let id = data(&v).get("task_id").and_then(Value::as_str).ok_or_else(|| anyhow!("Magnific did not return a task"))?.to_string();
    on_status("Painting");
    // A 4K image can take a few minutes.
    for _ in 0..180 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let v = match http.get(format!("{url}/{id}")).header("x-magnific-api-key", key).send().await {
            Ok(r) => check(r).await?,
            Err(_) => continue,
        };
        let d = data(&v);
        match d.get("status").and_then(Value::as_str).unwrap_or("") {
            "COMPLETED" => {
                let src = d.get("generated").and_then(|g| g.get(0)).and_then(|g| g.as_str().or_else(|| g.get("url").and_then(Value::as_str))).ok_or_else(|| anyhow!("Magnific finished without an image"))?;
                on_status("Downloading");
                let bytes = http.get(src).send().await?.error_for_status()?.bytes().await?;
                let out = out.to_path_buf();
                let aspect = aspect.to_string();
                return tokio::task::spawn_blocking(move || -> Result<()> {
                    let img = fit_aspect(image::load_from_memory(&bytes).context("Magnific sent an unreadable image")?, &aspect);
                    if let Some(d) = out.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    img.save_with_format(&out, image::ImageFormat::Png)?;
                    Ok(())
                })
                .await?;
            }
            "FAILED" => bail!("Magnific could not make this image (its content filter may have blocked it). Change the prompt and try again."),
            _ => {}
        }
    }
    bail!("Magnific took too long; try again")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> &'static ImageModel {
        CATALOG.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn banana_body() {
        let (p, b) = request(model("nano-banana-2-1"), "x", &[("AAA".into(), "Image 1: Arhan".into())], "16:9", "2K");
        assert_eq!(p, "text-to-image/nano-banana-2-1");
        assert_eq!(b["aspect_ratio"], "16:9");
        assert_eq!(b["resolution"], "2K");
        assert_eq!(b["reference_images"][0]["text"], "Image 1: Arhan");
    }

    #[test]
    fn named_models_edit_with_references() {
        let (p, b) = request(model("seedream-v5-pro"), "x", &[("AAA".into(), String::new())], "16:9", "1.5K");
        assert_eq!(p, "text-to-image/seedream-v5-pro-edit");
        assert_eq!(b["aspect_ratio"], "widescreen_16_9");
        assert_eq!(b["resolution"], "1.5k");
        assert_eq!(b["reference_images"][0], "AAA");
        let (p, _) = request(model("seedream-v5-pro"), "x", &[], "16:9", "2K");
        assert_eq!(p, "text-to-image/seedream-v5-pro");
    }

    #[test]
    fn flux_inputs_and_size() {
        let refs: Vec<(String, String)> = (0..3).map(|i| (format!("I{i}"), String::new())).collect();
        let (_, b) = request(model("flux-2-pro"), "x", &refs, "16:9", "");
        assert_eq!((b["width"].as_u64(), b["height"].as_u64()), (Some(1280), Some(720)));
        assert_eq!(b["input_image"], "I0");
        assert_eq!(b["input_image_3"], "I2");
    }

    #[test]
    fn cost_between_low_and_high() {
        let m = model("nano-banana-2-1");
        assert_eq!(m.cost_at("1K"), 50.);
        assert_eq!(m.cost_at("4K"), 145.);
        assert!(m.cost_at("2K") > 50. && m.cost_at("2K") < 145.);
    }

    #[test]
    fn square_only_models_and_aspect_fit() {
        use image::GenericImageView as _;
        let gemini = CATALOG.iter().find(|m| m.shape == Shape::Gemini).unwrap();
        assert!(!gemini.takes_aspect() && CATALOG[0].takes_aspect());
        // A square from a model that ignores the size comes out 16:9, cut around the centre.
        let sq = image::DynamicImage::new_rgb8(1024, 1024);
        let f = fit_aspect(sq.clone(), "16:9");
        assert_eq!((f.width(), f.height()), (1024, 576));
        assert_eq!(fit_aspect(sq.clone(), "9:16").dimensions(), (576, 1024));
        // Close enough, "auto" and "1:1" stay as they are.
        assert_eq!(fit_aspect(image::DynamicImage::new_rgb8(1376, 768), "16:9").dimensions(), (1376, 768));
        assert_eq!(fit_aspect(sq.clone(), "auto").dimensions(), (1024, 1024));
        assert_eq!(fit_aspect(sq, "1:1").dimensions(), (1024, 1024));
    }
}
