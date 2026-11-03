//! Image models on services other than Magnific, run with the keys from Settings › AI services:
//! Higgsfield, fal.ai, Replicate, Black Forest Labs and Recraft. Each request is sent, polled
//! until the image is ready, downloaded and saved like a Magnific one (`magnific::generate`
//! hands these models over). Keys travel only in headers, only to their own service's hosts,
//! and are never logged. Each service bills its own account, so nothing here counts credits.
//!
//! Request shapes follow each service's API docs as of November 2026.
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use super::magnific::{self, ImageModel, Reference, Shape};

const HIGGSFIELD: &str = "https://api.higgsfield.ai";

/// Hosts each service's key may be sent to (the URLs it returns for polling included).
fn own_hosts(service: &str) -> &'static [&'static str] {
    match service {
        "higgsfield" => &["higgsfield.ai"],
        "fal" => &["fal.run", "fal.ai"],
        "replicate" => &["api.replicate.com"],
        "bfl" => &["bfl.ai"],
        "recraft" => &["recraft.ai"],
        _ => &[],
    }
}

/// `url` is https on one of `hosts` (or a subdomain of one).
fn on_host(url: &str, hosts: &[&str]) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', ':']).next().unwrap_or("");
    hosts.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// "16:9" in the four sizes the Thumbnails page offers; anything else is 16:9.
fn aspect_or_wide(aspect: &str) -> &str {
    match aspect {
        "9:16" | "1:1" | "4:3" => aspect,
        _ => "16:9",
    }
}

/// At most `n` characters (Recraft takes 1000).
fn clip(prompt: &str, n: usize) -> String {
    prompt.chars().take(n).collect()
}

fn data_uri(b64: &str) -> String {
    format!("data:image/jpeg;base64,{b64}")
}

/// The submit URL and body for `m`. `images` are base64 JPEGs; `ref_url` is an uploaded
/// reference (Higgsfield takes URLs only).
pub(super) fn request(m: &ImageModel, prompt: &str, images: &[String], ref_url: Option<&str>, aspect: &str, res: &str) -> (String, Value) {
    let wide = aspect_or_wide(aspect);
    match m.shape {
        Shape::Higgsfield => {
            let mut body = json!({ "prompt": prompt, "aspect_ratio": wide, "resolution": if m.resolutions.contains(&res) { res } else { "720p" }, "batch_size": 1 });
            if let Some(u) = ref_url {
                body["image_reference_url"] = json!(u);
            }
            (format!("{HIGGSFIELD}/{}", m.path), body)
        }
        Shape::FalBanana => {
            let mut body = json!({ "prompt": prompt, "num_images": 1, "output_format": "png", "aspect_ratio": if aspect == "auto" && !images.is_empty() { "auto" } else { wide } });
            let path = if images.is_empty() {
                m.path.to_string()
            } else {
                body["image_urls"] = json!(images.iter().map(|b| data_uri(b)).collect::<Vec<_>>());
                format!("{}/edit", m.path)
            };
            (format!("https://queue.fal.run/{path}"), body)
        }
        Shape::FalFlux => {
            let size = match wide {
                "9:16" => "portrait_16_9",
                "1:1" => "square_hd",
                "4:3" => "landscape_4_3",
                _ => "landscape_16_9",
            };
            (format!("https://queue.fal.run/{}", m.path), json!({ "prompt": prompt, "image_size": size, "num_images": 1, "output_format": "png" }))
        }
        Shape::Replicate => {
            let mut input = json!({ "prompt": prompt, "aspect_ratio": wide, "output_format": "png" });
            if !images.is_empty() {
                input["image_input"] = json!(images.iter().map(|b| data_uri(b)).collect::<Vec<_>>());
            }
            (format!("https://api.replicate.com/v1/models/{}/predictions", m.path), json!({ "input": input }))
        }
        Shape::Bfl => {
            // FLUX.2 sizes are multiples of 16.
            let (w, h) = match wide {
                "9:16" => (864, 1536),
                "1:1" => (1440, 1440),
                "4:3" => (1536, 1152),
                _ => (1536, 864),
            };
            let mut body = json!({ "prompt": prompt, "width": w, "height": h, "output_format": "png" });
            for (k, b) in images.iter().take(m.refs).enumerate() {
                body[if k == 0 { "input_image".to_string() } else { format!("input_image_{}", k + 1) }] = json!(b);
            }
            (format!("https://api.bfl.ai/v1/{}", m.path), body)
        }
        Shape::Recraft => {
            let size = match wide {
                "9:16" => "1024x1820",
                "1:1" => "1024x1024",
                "4:3" => "1365x1024",
                _ => "1820x1024",
            };
            ("https://external.api.recraft.ai/v1/images/generations".into(), json!({ "prompt": clip(prompt, 1000), "model": m.path, "size": size, "n": 1 }))
        }
        _ => (String::new(), Value::Null),
    }
}

/// The service's own words from an error body.
fn error_text(body: &Value) -> String {
    let t = ["/detail/0/msg", "/detail/message", "/detail", "/error/message", "/error", "/message", "/title"]
        .iter()
        .find_map(|p| body.pointer(p).filter(|v| !v.is_null() && !v.is_array() && !v.is_object()).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))
        .unwrap_or_default();
    t.chars().take(240).collect()
}

/// The JSON body of a good answer, or an error that says what to do.
async fn check(r: reqwest::Response, label: &str) -> Result<Value> {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let msg = error_text(&body);
    let said = if msg.is_empty() { String::new() } else { format!(": {msg}") };
    match status.as_u16() {
        401 | 403 => bail!("{label} rejected the API key{said}. Change it in Settings › AI services."),
        402 => bail!("{label} says the account is out of credits{said}"),
        429 => bail!("{label} is rate limiting; wait a moment and try again"),
        s if s >= 500 => bail!("{label} is having trouble right now ({s}){said}; try again in a minute"),
        _ => bail!("{label} refused the request ({status}){said}"),
    }
}

struct Client {
    http: reqwest::Client,
    service: &'static str,
    label: &'static str,
    headers: Vec<(String, String)>,
}

impl Client {
    fn new(service: &'static str, key: &str) -> Result<Client> {
        let def = mv_ai::service_def(service).ok_or_else(|| anyhow!("unknown image service {service}"))?;
        Ok(Client { http: magnific::client()?, service, label: def.label, headers: mv_ai::services::headers(def, key) })
    }

    /// The request, with the key only when `url` is the service's own.
    fn with_key(&self, mut req: reqwest::RequestBuilder, url: &str) -> reqwest::RequestBuilder {
        if on_host(url, own_hosts(self.service)) {
            for (k, v) in &self.headers {
                req = req.header(k, v);
            }
        }
        req
    }

    async fn post(&self, url: &str, body: &Value, extra: &[(&str, &str)]) -> Result<Value> {
        let mut req = self.with_key(self.http.post(url), url).json(body);
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        check(req.send().await.with_context(|| format!("could not reach {}", self.label))?, self.label).await
    }

    /// `None` when the network hiccupped (polling just tries again).
    async fn get(&self, url: &str) -> Result<Option<Value>> {
        match self.with_key(self.http.get(url), url).send().await {
            Ok(r) => check(r, self.label).await.map(Some),
            Err(_) => Ok(None),
        }
    }

    /// The image at `src`: a data URI, or a download (with the key only on the service's host).
    async fn image(&self, src: &str) -> Result<Vec<u8>> {
        if let Some(rest) = src.strip_prefix("data:") {
            let b64 = rest.split_once(',').map(|(_, d)| d).unwrap_or(rest);
            return Ok(base64::engine::general_purpose::STANDARD.decode(b64.trim())?);
        }
        let r = self.with_key(self.http.get(src), src).send().await?.error_for_status()?;
        Ok(r.bytes().await?.to_vec())
    }
}

/// Polls `url` every 2 s (up to 6 minutes) until `done` says the answer is final.
async fn poll(c: &Client, url: &str, done: impl Fn(&Value) -> Result<Option<String>>) -> Result<String> {
    for _ in 0..180 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Some(v) = c.get(url).await? {
            if let Some(src) = done(&v)? {
                return Ok(src);
            }
        }
    }
    bail!("{} took too long; try again", c.label)
}

fn blocked(label: &str) -> anyhow::Error {
    anyhow!("{label} could not make this image (its content filter may have blocked it). Change the prompt and try again.")
}

/// The first image URL of an answer: a string, a list of strings, or `{url}` objects.
fn first_url(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(first_url),
        Value::Object(o) => o.get("url").and_then(Value::as_str).map(String::from),
        _ => None,
    }
}

/// Higgsfield takes reference images by URL: one is uploaded to the URL it hands out.
async fn higgsfield_upload(c: &Client, jpeg: Vec<u8>) -> Result<String> {
    let v = c.post(&format!("{HIGGSFIELD}/files/generate-upload-url"), &json!({ "content_type": "image/jpeg" }), &[]).await?;
    let (Some(put), Some(public)) = (v.get("upload_url").and_then(Value::as_str), v.get("public_url").and_then(Value::as_str)) else {
        bail!("Higgsfield did not return an upload address for the reference image");
    };
    // The storage URL gets the upload headers Higgsfield names, never the key.
    let mut req = c.http.put(put).body(jpeg);
    let mut typed = false;
    if let Some(h) = v.get("upload_headers").and_then(Value::as_object) {
        for (k, val) in h {
            if let Some(s) = val.as_str() {
                typed |= k.eq_ignore_ascii_case("content-type");
                req = req.header(k.as_str(), s);
            }
        }
    }
    if !typed {
        req = req.header("content-type", "image/jpeg");
    }
    req.send().await.context("could not upload the reference image to Higgsfield")?.error_for_status().context("Higgsfield did not accept the reference image")?;
    Ok(public.to_string())
}

/// Makes one image with `m` on `service` and saves it as PNG at `out`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate(service: &'static str, m: &'static ImageModel, key: &str, prompt: &str, refs: &[Reference], aspect: &str, res: &str, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let c = Client::new(service, key)?;
    let label = c.label;
    if m.shape == Shape::Higgsfield && !key.contains(':') {
        bail!("Higgsfield keys have two parts: paste them as KEY_ID:KEY_SECRET in Settings › AI services");
    }
    let mut jpegs = vec![];
    for r in refs.iter().take(m.refs) {
        let file = r.file.clone();
        jpegs.push(tokio::task::spawn_blocking(move || magnific::jpeg(&file)).await??);
    }
    let mut ref_url = None;
    let mut images = vec![];
    if m.shape == Shape::Higgsfield {
        if let Some(j) = jpegs.into_iter().next() {
            on_status("Uploading");
            ref_url = Some(higgsfield_upload(&c, j).await?);
        }
    } else {
        images = jpegs.iter().map(|j| base64::engine::general_purpose::STANDARD.encode(j)).collect();
    }
    let (url, body) = request(m, prompt, &images, ref_url.as_deref(), aspect, res);
    if url.is_empty() {
        bail!("{} is not set up for {label}", m.name);
    }
    on_status("Sending");
    let src = match m.shape {
        Shape::Higgsfield => {
            let v = c.post(&url, &body, &[]).await?;
            let status_url = v.get("status_url").and_then(Value::as_str).map(String::from).or_else(|| v.get("request_id").and_then(Value::as_str).map(|id| format!("{HIGGSFIELD}/requests/{id}/status"))).ok_or_else(|| anyhow!("Higgsfield did not return a request"))?;
            on_status("Painting");
            poll(&c, &status_url, |v| match v.get("status").and_then(Value::as_str).unwrap_or("") {
                "completed" => first_url(v.get("images").unwrap_or(&Value::Null)).map(Some).ok_or_else(|| anyhow!("Higgsfield finished without an image")),
                "nsfw" | "failed" => Err(blocked(label)),
                "canceled" | "cancelled" => Err(anyhow!("The Higgsfield request was cancelled")),
                _ => Ok(None),
            })
            .await?
        }
        Shape::FalBanana | Shape::FalFlux => {
            let v = c.post(&url, &body, &[]).await?;
            let status_url = v.get("status_url").and_then(Value::as_str).ok_or_else(|| anyhow!("fal.ai did not queue the request"))?.to_string();
            let response_url = v.get("response_url").and_then(Value::as_str).ok_or_else(|| anyhow!("fal.ai did not queue the request"))?.to_string();
            on_status("Painting");
            poll(&c, &status_url, |v| match v.get("status").and_then(Value::as_str).unwrap_or("") {
                "COMPLETED" => match v.get("error").and_then(Value::as_str) {
                    Some(e) if !e.is_empty() => Err(anyhow!("fal.ai could not make this image: {e}")),
                    _ => Ok(Some(String::new())),
                },
                _ => Ok(None),
            })
            .await?;
            let result = c.get(&response_url).await?.ok_or_else(|| anyhow!("could not reach fal.ai for the result"))?;
            if result.get("has_nsfw_concepts").and_then(Value::as_array).is_some_and(|a| a.iter().any(|b| b.as_bool() == Some(true))) {
                return Err(blocked(label));
            }
            first_url(result.get("images").unwrap_or(&Value::Null)).ok_or_else(|| anyhow!("fal.ai finished without an image"))?
        }
        Shape::Replicate => {
            // Waits up to a minute in the first call; most images are done by then.
            let mut v = c.post(&url, &body, &[("Prefer", "wait=60")]).await?;
            on_status("Painting");
            if v.get("status").and_then(Value::as_str) != Some("succeeded") {
                let get = v.pointer("/urls/get").and_then(Value::as_str).ok_or_else(|| anyhow!("Replicate did not start the prediction"))?.to_string();
                let fail = |v: &Value| match v.get("status").and_then(Value::as_str).unwrap_or("") {
                    "failed" => Err(anyhow!("Replicate could not make this image: {}", v.get("error").and_then(Value::as_str).unwrap_or("it failed"))),
                    "canceled" => Err(anyhow!("The Replicate prediction was cancelled")),
                    _ => Ok(()),
                };
                fail(&v)?;
                let last = std::sync::Mutex::new(Value::Null);
                poll(&c, &get, |p| {
                    fail(p)?;
                    let done = p.get("status").and_then(Value::as_str) == Some("succeeded");
                    if done {
                        *last.lock().unwrap_or_else(|e| e.into_inner()) = p.clone();
                    }
                    Ok(done.then(String::new))
                })
                .await?;
                v = last.into_inner().unwrap_or_else(|e| e.into_inner());
            }
            first_url(v.get("output").unwrap_or(&Value::Null)).ok_or_else(|| anyhow!("Replicate finished without an image"))?
        }
        Shape::Bfl => {
            let v = c.post(&url, &body, &[]).await?;
            let polling = v.get("polling_url").and_then(Value::as_str).map(String::from).or_else(|| v.get("id").and_then(Value::as_str).map(|id| format!("https://api.bfl.ai/v1/get_result?id={id}"))).ok_or_else(|| anyhow!("Black Forest Labs did not return a task"))?;
            on_status("Painting");
            poll(&c, &polling, |v| match v.get("status").and_then(Value::as_str).unwrap_or("") {
                "Ready" => v.pointer("/result/sample").and_then(Value::as_str).map(|s| Some(s.to_string())).ok_or_else(|| anyhow!("Black Forest Labs finished without an image")),
                s if s.contains("Moderated") => Err(blocked(label)),
                "Error" | "Failed" => Err(anyhow!("Black Forest Labs could not make this image; try again")),
                _ => Ok(None),
            })
            .await?
        }
        Shape::Recraft => {
            let v = c.post(&url, &body, &[]).await?;
            first_url(v.get("data").unwrap_or(&Value::Null)).ok_or_else(|| anyhow!("Recraft answered without an image"))?
        }
        _ => bail!("{} is not set up for {label}", m.name),
    };
    on_status("Downloading");
    let bytes = c.image(&src).await.with_context(|| format!("could not download the image from {label}"))?;
    magnific::save_png(bytes, aspect, out, label).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use magnific::{Via, CATALOG};

    fn model(id: &str) -> &'static ImageModel {
        CATALOG.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn every_service_model_has_a_request_and_a_key_page() {
        for m in CATALOG {
            if let Via::Service(id) = m.via {
                let def = mv_ai::service_def(id).unwrap_or_else(|| panic!("{} uses unknown service {id}", m.id));
                assert!(def.studio_images, "{id} makes images in Studio but its settings say coming soon");
                let (url, body) = request(m, "x", &[], None, "16:9", "");
                assert!(on_host(&url, own_hosts(id)), "{}: {url}", m.id);
                assert!(body.is_object(), "{}", m.id);
                assert!(m.credits.is_none(), "{}: other services are not priced in Magnific credits", m.id);
            }
        }
        // And every service its settings call ready has a model here.
        for def in mv_ai::SERVICES.iter().filter(|d| d.studio_images) {
            assert!(CATALOG.iter().any(|m| m.via == Via::Service(def.id)), "{} has no model", def.id);
        }
    }

    #[test]
    fn keys_stay_on_their_own_hosts() {
        assert!(on_host("https://queue.fal.run/fal-ai/x/requests/1/status", own_hosts("fal")));
        assert!(on_host("https://api.us.bfl.ai/v1/get_result?id=1", own_hosts("bfl")));
        assert!(!on_host("https://delivery-eu1.bfl.ai.evil.com/x", own_hosts("bfl")));
        assert!(!on_host("http://api.higgsfield.ai/x", own_hosts("higgsfield")));
        assert!(!on_host("https://replicate.delivery/x.png", own_hosts("replicate")));
        assert!(!on_host("https://evilfal.run/x", own_hosts("fal")));
    }

    #[test]
    fn higgsfield_body() {
        let (url, b) = request(model("hf-soul"), "x", &[], Some("https://cdn/x.jpg"), "9:16", "1080p");
        assert_eq!(url, "https://api.higgsfield.ai/higgsfield-ai/soul/standard");
        assert_eq!((b["aspect_ratio"].as_str(), b["resolution"].as_str()), (Some("9:16"), Some("1080p")));
        assert_eq!(b["image_reference_url"], "https://cdn/x.jpg");
        let (_, b) = request(model("hf-soul-2"), "x", &[], None, "auto", "4K");
        assert_eq!((b["aspect_ratio"].as_str(), b["resolution"].as_str()), (Some("16:9"), Some("720p")));
        assert!(b.get("image_reference_url").is_none());
    }

    #[test]
    fn fal_edits_with_references() {
        let (url, b) = request(model("fal-nano-banana"), "x", &["AAA".into()], None, "auto", "");
        assert_eq!(url, "https://queue.fal.run/fal-ai/nano-banana/edit");
        assert_eq!(b["image_urls"][0], "data:image/jpeg;base64,AAA");
        assert_eq!(b["aspect_ratio"], "auto");
        let (url, b) = request(model("fal-nano-banana"), "x", &[], None, "4:3", "");
        assert_eq!(url, "https://queue.fal.run/fal-ai/nano-banana");
        assert_eq!(b["aspect_ratio"], "4:3");
        let (_, b) = request(model("fal-flux-pro"), "x", &[], None, "9:16", "");
        assert_eq!(b["image_size"], "portrait_16_9");
    }

    #[test]
    fn replicate_bfl_and_recraft_bodies() {
        let (url, b) = request(model("rep-nano-banana"), "x", &["AAA".into()], None, "16:9", "");
        assert_eq!(url, "https://api.replicate.com/v1/models/google/nano-banana/predictions");
        assert_eq!((b["input"]["aspect_ratio"].as_str(), b["input"]["image_input"][0].as_str()), (Some("16:9"), Some("data:image/jpeg;base64,AAA")));
        let (_, b) = request(model("bfl-flux-2-pro"), "x", &["A".into(), "B".into()], None, "16:9", "");
        assert_eq!((b["width"].as_u64(), b["height"].as_u64()), (Some(1536), Some(864)));
        assert_eq!((b["input_image"].as_str(), b["input_image_2"].as_str()), (Some("A"), Some("B")));
        let long = "y".repeat(1500);
        let (_, b) = request(model("recraft-v3"), &long, &[], None, "1:1", "");
        assert_eq!((b["size"].as_str(), b["prompt"].as_str().map(str::len)), (Some("1024x1024"), Some(1000)));
    }

    #[test]
    fn answers_and_errors() {
        assert_eq!(first_url(&json!("https://x/1.png")).as_deref(), Some("https://x/1.png"));
        assert_eq!(first_url(&json!(["https://x/1.png"])).as_deref(), Some("https://x/1.png"));
        assert_eq!(first_url(&json!([{ "url": "https://x/2.png" }])).as_deref(), Some("https://x/2.png"));
        assert_eq!(first_url(&Value::Null), None);
        assert_eq!(error_text(&json!({ "detail": [{ "msg": "prompt too long" }] })), "prompt too long");
        assert_eq!(error_text(&json!({ "detail": "Invalid credentials" })), "Invalid credentials");
        assert_eq!(error_text(&json!({ "error": { "message": "bad size" } })), "bad size");
    }
}
