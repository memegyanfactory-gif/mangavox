//! Video for a painted chapter: each panel becomes a short clip, then the clips are stitched in
//! reading order into one video. Clips come from:
//! - FFmpeg on this PC (pan and zoom over the panel, free and offline),
//! - a local ComfyUI server running the user's own image-to-video workflow,
//! - Magnific's API (Kling), or fal.ai (Seedance, Kling) with the key from Settings › AI services.
//!
//! Request shapes follow Magnific's and fal.ai's API docs as of November 2026. Keys travel only
//! in headers to their own service.
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use base64::Engine as _;
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoVia {
    /// FFmpeg pan and zoom.
    Local,
    ComfyUi,
    /// The LTX 2.3 image + audio workflow built in here, on a local ComfyUI: the clip is made
    /// from the panel and the voice of its lines, and comes back with that sound.
    ComfyLtx,
    Magnific,
    /// A service id from `mv_ai::SERVICES`.
    Service(&'static str),
}

pub struct VideoModel {
    pub id: &'static str,
    pub name: &'static str,
    pub via: VideoVia,
    /// Submit path (Magnific: under `/v1/ai/`; fal: the model id).
    pub path: &'static str,
    /// Magnific polls a shorter path than it submits to.
    pub poll: &'static str,
    /// Clip lengths it takes, in seconds.
    pub seconds: &'static [u32],
    pub note: &'static str,
}

/// The id of the LTX 2.3 image + audio model.
pub const LTX_VOICE: &str = "ltx23-ia2v";

const MG_API: &str = "https://api.magnific.com/v1/ai/";

pub static VIDEO_MODELS: &[VideoModel] = &[
    VideoModel { id: "local-motion", name: "Pan & zoom (FFmpeg, on this PC)", via: VideoVia::Local, path: "", poll: "", seconds: &[3, 4, 5, 6, 8], note: "Free and offline: moves the camera over each panel." },
    VideoModel { id: "comfyui", name: "ComfyUI workflow (local model)", via: VideoVia::ComfyUi, path: "", poll: "", seconds: &[3, 4, 5, 6, 8, 10], note: "Your own image-to-video workflow (Wan, LTX, Hunyuan…) on a local ComfyUI." },
    VideoModel { id: LTX_VOICE, name: "LTX 2.3 with voice (ComfyUI, on this PC)", via: VideoVia::ComfyLtx, path: "", poll: "", seconds: &[3, 4, 5, 6, 8, 9, 10], note: "Each scene is voiced, then LTX 2.3 in your ComfyUI films it from the picture and that voice. Needs ComfyUI open with the LTX 2.3 models." },
    VideoModel { id: "mg-kling-3-turbo-1080p", name: "Kling 3.0 Turbo 1080p · Magnific", via: VideoVia::Magnific, path: "image-to-video/kling-v3-turbo-1080p", poll: "image-to-video/kling-v3-turbo", seconds: &[3, 5, 8, 10, 15], note: "Fast, keeps the panel's shape." },
    VideoModel { id: "mg-kling-3-turbo-720p", name: "Kling 3.0 Turbo 720p · Magnific", via: VideoVia::Magnific, path: "image-to-video/kling-v3-turbo-720p", poll: "image-to-video/kling-v3-turbo", seconds: &[3, 5, 8, 10, 15], note: "Cheaper Kling Turbo." },
    VideoModel { id: "mg-kling-2-6-pro", name: "Kling 2.6 Pro · Magnific", via: VideoVia::Magnific, path: "image-to-video/kling-v2-6-pro", poll: "image-to-video/kling-v2-6", seconds: &[5, 10], note: "16:9, 9:16 or 1:1." },
    VideoModel { id: "fal-seedance-1-pro", name: "Seedance 1.0 Pro · fal.ai", via: VideoVia::Service("fal"), path: "fal-ai/bytedance/seedance/v1/pro/image-to-video", poll: "", seconds: &[3, 5, 8, 10, 12], note: "ByteDance Seedance, 1080p." },
    VideoModel { id: "fal-kling-o3", name: "Kling O3 · fal.ai", via: VideoVia::Service("fal"), path: "fal-ai/kling-video/o3/standard/image-to-video", poll: "", seconds: &[3, 5, 8, 10, 15], note: "Kling's current image-to-video on fal.ai." },
];

pub fn video_model(id: &str) -> &'static VideoModel {
    VIDEO_MODELS.iter().find(|m| m.id == id).unwrap_or(&VIDEO_MODELS[0])
}

impl VideoModel {
    /// The nearest length it takes.
    pub fn fit_seconds(&self, want: u32) -> u32 {
        self.seconds.iter().copied().min_by_key(|s| s.abs_diff(want)).unwrap_or(5)
    }

    pub fn is_local(&self) -> bool {
        matches!(self.via, VideoVia::Local | VideoVia::ComfyUi | VideoVia::ComfyLtx)
    }
}

/// `(width, height)` of the stitched video.
pub fn frame(aspect: &str) -> (u32, u32) {
    if aspect == "9:16" {
        (1080, 1920)
    } else {
        (1920, 1080)
    }
}

fn ffmpeg() -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new("ffmpeg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

pub fn has_ffmpeg() -> bool {
    ffmpeg().arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

fn run(mut cmd: std::process::Command, what: &str) -> Result<()> {
    let out = cmd.stdin(std::process::Stdio::null()).output().with_context(|| format!("could not start FFmpeg for {what}; install FFmpeg and put it on PATH"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" / ");
        bail!("FFmpeg failed on {what}: {tail}");
    }
    Ok(())
}

/// The fitted frame filter: the image over a blurred copy of itself, so any panel shape fills it.
fn fill_filter(w: u32, h: u32) -> String {
    format!("[0:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=28:2,eq=brightness=-0.08[bg];[0:v]scale={w}:{h}:force_original_aspect_ratio=decrease[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2,setsar=1")
}

/// A pan-and-zoom clip of `image` with FFmpeg. `style` picks the move (0 push in, 1 pull out,
/// 2 pan across), so neighbouring panels do not all move alike.
pub fn local_motion(image: &Path, seconds: u32, aspect: &str, style: usize, out: &Path) -> Result<()> {
    let (w, h) = frame(aspect);
    let frames = seconds.max(1) * 30;
    let z = match style % 3 {
        0 => format!("z='1+0.12*on/{frames}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'"),
        1 => format!("z='1.12-0.12*on/{frames}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'"),
        _ => format!("z='1.1':x='(iw-iw/zoom)*on/{frames}':y='ih/2-(ih/zoom/2)'"),
    };
    // Upscaled first so the zoom does not shimmer.
    let filter = format!("{},scale={}:{},zoompan={z}:d={frames}:s={w}x{h}:fps=30,format=yuv420p", fill_filter(w, h), w * 2, h * 2);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut cmd = ffmpeg();
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(image).args(["-filter_complex", &filter, "-frames:v", &frames.to_string(), "-c:v", "libx264", "-preset", "veryfast", "-crf", "19", "-an"]).arg(out);
    run(cmd, "a panel clip")
}

/// Stitches `clips` in order into one video at `out`: each is fitted to the frame at 30 fps, then
/// they are joined. `music`, when given, plays under it (looped, faded out).
pub fn stitch(clips: &[PathBuf], aspect: &str, music: Option<&Path>, work: &Path, out: &Path) -> Result<()> {
    if clips.is_empty() {
        bail!("there are no clips to stitch yet");
    }
    let (w, h) = frame(aspect);
    std::fs::create_dir_all(work)?;
    let mut list = String::new();
    // Clips with their own sound (voiced scenes) keep it; the silent ones among them get silence,
    // so every part joins without re-encoding.
    let sound: Vec<bool> = clips.iter().map(|c| has_audio(c)).collect();
    let any_sound = sound.iter().any(|s| *s);
    for (i, c) in clips.iter().enumerate() {
        let norm = work.join(format!("n{i:04}.mp4"));
        let mut cmd = ffmpeg();
        cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(c);
        if any_sound && !sound[i] {
            cmd.args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
        }
        cmd.args(["-filter_complex", &format!("{},fps=30,format=yuv420p[v]", fill_filter(w, h)), "-map", "[v]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "19"]);
        if any_sound {
            cmd.args(["-map", if sound[i] { "0:a:0" } else { "1:a:0" }, "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-ac", "2", "-shortest"]);
        } else {
            cmd.arg("-an");
        }
        cmd.arg(&norm);
        run(cmd, &format!("clip {}", i + 1))?;
        list.push_str(&format!("file '{}'\n", norm.to_string_lossy().replace('\\', "/").replace('\'', "'\\''")));
    }
    let list_file = work.join("list.txt");
    std::fs::write(&list_file, list)?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut cmd = ffmpeg();
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"]).arg(&list_file);
    // Voiced clips keep their own sound: music under them is not mixed here.
    match music.filter(|_| !any_sound) {
        Some(m) => {
            cmd.args(["-stream_loop", "-1", "-i"]).arg(m).args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-af", "afade=t=in:d=1"]);
        }
        None => {
            cmd.args(["-c", "copy"]);
        }
    }
    cmd.args(["-movflags", "+faststart"]).arg(out);
    run(cmd, "the chapter video")
}

/// `file` has a sound track.
pub fn has_audio(file: &Path) -> bool {
    // FFmpeg lists the streams (on stderr) and then stops for want of an output: that is enough.
    ffmpeg().args(["-hide_banner", "-i"]).arg(file).stdin(std::process::Stdio::null()).output().is_ok_and(|o| String::from_utf8_lossy(&o.stderr).lines().any(|l| l.contains("Stream #") && l.contains("Audio:")))
}

/// Seconds of sound or picture in `file`, when FFmpeg can tell.
pub fn media_seconds(file: &Path) -> Option<f64> {
    let out = ffmpeg().args(["-hide_banner", "-i"]).arg(file).stdin(std::process::Stdio::null()).output().ok()?;
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    let t = err.split("Duration: ").nth(1)?.split(',').next()?.trim().to_string();
    let mut parts = t.split(':');
    let (h, m, s) = (parts.next()?.parse::<f64>().ok()?, parts.next()?.parse::<f64>().ok()?, parts.next()?.parse::<f64>().ok()?);
    Some(h * 3600. + m * 60. + s)
}

/// `seconds` of silence as a WAV at `out` (a scene nobody speaks in still needs a sound track).
pub fn silence(seconds: f64, out: &Path) -> Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut cmd = ffmpeg();
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", &format!("{seconds:.2}")]).arg(out);
    run(cmd, "the silent track")
}

/// `parts` (voice lines in order) joined into one WAV at `out`, with a short breath after each.
pub fn join_voice(parts: &[PathBuf], out: &Path) -> Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut cmd = ffmpeg();
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    for p in parts {
        cmd.arg("-i").arg(p);
    }
    let mut graph = String::new();
    for i in 0..parts.len() {
        graph.push_str(&format!("[{i}:a]aresample=48000,aformat=channel_layouts=stereo,apad=pad_dur=0.25[a{i}];"));
    }
    for i in 0..parts.len() {
        graph.push_str(&format!("[a{i}]"));
    }
    graph.push_str(&format!("concat=n={}:v=0:a=1[out]", parts.len()));
    cmd.args(["-filter_complex", &graph, "-map", "[out]"]).arg(out);
    run(cmd, "the scene's voice")
}

// ------------------------------------------------------------------ API models

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?)
}

fn jpeg_b64(file: &Path) -> Result<String> {
    let img = image::open(file).with_context(|| format!("could not read {}", file.display()))?;
    let img = if img.width().max(img.height()) > 1920 { img.resize(1920, 1920, image::imageops::FilterType::Lanczos3) } else { img };
    // Kling wants at least 300 px on each side.
    let img = if img.width().min(img.height()) < 320 { img.resize(img.width() * 2, img.height() * 2, image::imageops::FilterType::Lanczos3) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

/// The submit URL and body for an API model.
pub fn request(m: &VideoModel, image_b64: &str, prompt: &str, seconds: u32, aspect: &str) -> (String, Value) {
    let secs = m.fit_seconds(seconds).to_string();
    match m.via {
        VideoVia::Magnific => {
            let mut body = json!({ "image": image_b64, "prompt": prompt, "duration": secs });
            if m.id == "mg-kling-2-6-pro" {
                body["aspect_ratio"] = json!(if aspect == "9:16" { "social_story_9_16" } else { "widescreen_16_9" });
            }
            (format!("{MG_API}{}", m.path), body)
        }
        VideoVia::Service(_) => {
            let mut body = json!({ "image_url": format!("data:image/jpeg;base64,{image_b64}"), "prompt": prompt, "duration": secs });
            if m.id == "fal-seedance-1-pro" {
                body["aspect_ratio"] = json!(aspect);
                body["resolution"] = json!("1080p");
            }
            (format!("https://queue.fal.run/{}", m.path), body)
        }
        _ => (String::new(), Value::Null),
    }
}

async fn answer(r: reqwest::Response, label: &str) -> Result<Value> {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let msg = ["/message", "/detail", "/error/message", "/error", "/detail/0/msg"].iter().find_map(|p| body.pointer(p).and_then(Value::as_str)).unwrap_or("").chars().take(240).collect::<String>();
    match status.as_u16() {
        401 | 403 => bail!("{label} rejected the API key{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }),
        402 => bail!("{label} says the account is out of credits"),
        429 => bail!("{label} is rate limiting; wait a moment and try again"),
        _ => bail!("{label} refused the request ({status}){}", if msg.is_empty() { String::new() } else { format!(": {msg}") }),
    }
}

/// The first video URL in an answer.
fn video_url(v: &Value) -> Option<String> {
    for p in ["/data/generated/0", "/data/generated/0/url", "/video/url", "/video", "/output/0", "/output"] {
        if let Some(s) = v.pointer(p).and_then(Value::as_str).filter(|s| s.starts_with("http")) {
            return Some(s.to_string());
        }
    }
    None
}

/// Makes one clip of `image` with an API model and saves it as MP4 at `out`.
pub async fn api_clip(m: &'static VideoModel, key: &str, image: &Path, prompt: &str, seconds: u32, aspect: &str, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let file = image.to_path_buf();
    let b64 = tokio::task::spawn_blocking(move || jpeg_b64(&file)).await??;
    let (url, body) = request(m, &b64, prompt, seconds, aspect);
    let http = client()?;
    on_status("Sending");
    let src = match m.via {
        VideoVia::Magnific => {
            let label = "Magnific";
            let v = answer(http.post(&url).header("x-magnific-api-key", key).json(&body).send().await.context("could not reach Magnific")?, label).await?;
            let id = v.pointer("/data/task_id").or_else(|| v.get("task_id")).and_then(Value::as_str).ok_or_else(|| anyhow!("Magnific did not return a task"))?.to_string();
            on_status("Animating");
            let poll = format!("{MG_API}{}/{id}", m.poll);
            let mut found = None;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let Ok(r) = http.get(&poll).header("x-magnific-api-key", key).send().await else { continue };
                let v = answer(r, label).await?;
                match v.pointer("/data/status").and_then(Value::as_str).unwrap_or("") {
                    "COMPLETED" => {
                        found = Some(video_url(&v).ok_or_else(|| anyhow!("Magnific finished without a video"))?);
                        break;
                    }
                    "FAILED" => bail!("Magnific could not animate this panel (its content filter may have blocked it)"),
                    _ => {}
                }
            }
            found.ok_or_else(|| anyhow!("Magnific took too long; try again"))?
        }
        VideoVia::Service(service) => {
            let def = mv_ai::service_def(service).ok_or_else(|| anyhow!("unknown video service {service}"))?;
            let headers = mv_ai::services::headers(def, key);
            let with_key = |mut req: reqwest::RequestBuilder, u: &str| {
                if u.starts_with("https://queue.fal.run/") || u.starts_with("https://fal.run/") {
                    for (k, v) in &headers {
                        req = req.header(k, v);
                    }
                }
                req
            };
            let v = answer(with_key(http.post(&url), &url).json(&body).send().await.with_context(|| format!("could not reach {}", def.label))?, def.label).await?;
            let status_url = v.get("status_url").and_then(Value::as_str).ok_or_else(|| anyhow!("{} did not queue the request", def.label))?.to_string();
            let response_url = v.get("response_url").and_then(Value::as_str).ok_or_else(|| anyhow!("{} did not queue the request", def.label))?.to_string();
            on_status("Animating");
            let mut done = false;
            for _ in 0..300 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let Ok(r) = with_key(http.get(&status_url), &status_url).send().await else { continue };
                let v = answer(r, def.label).await?;
                if v.get("status").and_then(Value::as_str) == Some("COMPLETED") {
                    if let Some(e) = v.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()) {
                        bail!("{} could not animate this panel: {e}", def.label);
                    }
                    done = true;
                    break;
                }
            }
            if !done {
                bail!("{} took too long; try again", def.label);
            }
            let r = answer(with_key(http.get(&response_url), &response_url).send().await?, def.label).await?;
            video_url(&r).ok_or_else(|| anyhow!("{} finished without a video", def.label))?
        }
        _ => bail!("{} is not an API model", m.name),
    };
    on_status("Downloading");
    // Result links are public and short-lived; the key is not sent with them.
    let bytes = http.get(&src).send().await?.error_for_status()?.bytes().await?;
    if let Some(d) = out.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    tokio::fs::write(out, &bytes).await?;
    Ok(())
}

// ------------------------------------------------------------------ ComfyUI

/// The workflow with `{{image}}`, `{{prompt}}`, `{{seconds}}` and `{{frames}}` filled in (as JSON
/// string contents, so quotes in the prompt cannot break it).
pub fn fill_workflow(workflow: &str, image_name: &str, prompt: &str, seconds: u32) -> Result<Value> {
    let esc = |s: &str| serde_json::to_string(s).map(|q| q[1..q.len() - 1].to_string()).unwrap_or_default();
    let filled = workflow.replace("{{image}}", &esc(image_name)).replace("{{prompt}}", &esc(prompt)).replace("\"{{seconds}}\"", &seconds.to_string()).replace("{{seconds}}", &seconds.to_string()).replace("\"{{frames}}\"", &(seconds * 16 + 1).to_string()).replace("{{frames}}", &(seconds * 16 + 1).to_string());
    let v: Value = serde_json::from_str(&filled).context("the ComfyUI workflow is not valid JSON (export it with \"Save (API Format)\")")?;
    if !v.is_object() {
        bail!("the ComfyUI workflow should be the API-format JSON object");
    }
    Ok(v)
}

/// The first video (or animated image) a finished ComfyUI prompt produced.
fn comfy_output(history: &Value) -> Option<(String, String, String)> {
    let outputs = history.get("outputs")?.as_object()?;
    for node in outputs.values() {
        for k in ["gifs", "videos", "images"] {
            for f in node.get(k).and_then(Value::as_array).into_iter().flatten() {
                let name = f.get("filename").and_then(Value::as_str)?;
                let lower = name.to_lowercase();
                if k != "images" || lower.ends_with(".mp4") || lower.ends_with(".webm") || lower.ends_with(".gif") || lower.ends_with(".webp") {
                    return Some((name.to_string(), f.get("subfolder").and_then(Value::as_str).unwrap_or("").to_string(), f.get("type").and_then(Value::as_str).unwrap_or("output").to_string()));
                }
            }
        }
    }
    None
}

/// Runs the user's ComfyUI workflow on `image` and saves the video at `out`.
pub async fn comfy_clip(base: &str, workflow_file: &Path, image: &Path, prompt: &str, seconds: u32, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let base = local_comfy(base)?;
    let workflow = tokio::fs::read_to_string(workflow_file).await.with_context(|| format!("could not read the workflow {}", workflow_file.display()))?;
    let http = client()?;
    on_status("Uploading");
    let bytes = tokio::fs::read(image).await?;
    let name = image.file_name().and_then(|n| n.to_str()).unwrap_or("panel.png").to_string();
    let form = reqwest::multipart::Form::new().part("image", reqwest::multipart::Part::bytes(bytes).file_name(name.clone()).mime_str("image/png")?).text("overwrite", "true");
    let up: Value = http.post(format!("{base}/upload/image")).multipart(form).send().await.context("could not reach ComfyUI; is it running?")?.error_for_status()?.json().await?;
    let uploaded = up.get("name").and_then(Value::as_str).unwrap_or(&name).to_string();
    let graph = fill_workflow(&workflow, &uploaded, prompt, seconds)?;
    comfy_run(&http, base, graph, out, false, 1200, on_status).await
}

/// The built-in LTX 2.3 image + audio workflow (as ComfyUI's own template runs it), filled in:
/// the uploaded picture and voice, what happens, the length, the frame and a seed.
pub fn ltx_graph(image_name: &str, audio_name: &str, prompt: &str, seconds: f64, aspect: &str, seed: u64) -> Result<Value> {
    let mut g: Value = serde_json::from_str(include_str!("ltx23_ia2v.json")).context("the built-in LTX workflow is damaged")?;
    let (w, h) = if aspect == "9:16" { (720, 1280) } else { (1280, 720) };
    let mut put = |node: &str, key: &str, v: Value| -> Result<()> {
        *g.get_mut(node).and_then(|n| n.get_mut("inputs")).and_then(|i| i.get_mut(key)).ok_or_else(|| anyhow!("the built-in LTX workflow has no {node}.{key}"))? = v;
        Ok(())
    };
    put("269", "image", json!(image_name))?;
    put("276", "audio", json!(audio_name))?;
    put("340:319", "value", json!(prompt))?;
    put("340:331", "value", json!((seconds.clamp(1., 20.) * 10.).round() / 10.))?;
    put("340:330", "value", json!(w))?;
    put("340:324", "value", json!(h))?;
    put("340:286", "noise_seed", json!(seed))?;
    Ok(g)
}

async fn comfy_upload(http: &reqwest::Client, base: &str, file: &Path, mime: &str) -> Result<String> {
    let bytes = tokio::fs::read(file).await.with_context(|| format!("could not read {}", file.display()))?;
    // A name of its own, so two scenes never overwrite each other's upload.
    let name = format!("mangavox-{}-{}", crate::now(), file.file_name().and_then(|n| n.to_str()).unwrap_or("file"));
    let form = reqwest::multipart::Form::new().part("image", reqwest::multipart::Part::bytes(bytes).file_name(name.clone()).mime_str(mime)?).text("overwrite", "true");
    let up: Value = http.post(format!("{base}/upload/image")).multipart(form).send().await.context("could not reach ComfyUI; open ComfyUI first")?.error_for_status()?.json().await?;
    Ok(up.get("name").and_then(Value::as_str).unwrap_or(&name).to_string())
}

fn local_comfy(base: &str) -> Result<&str> {
    let base = base.trim().trim_end_matches('/');
    if !(base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost") || base.starts_with("http://[::1]") || base.starts_with("http://192.168.") || base.starts_with("http://10.")) {
        bail!("ComfyUI should be on this PC or your local network (e.g. http://127.0.0.1:8188)");
    }
    Ok(base)
}

/// Films one scene with LTX 2.3 on the local ComfyUI: `image` is its first frame, `audio` its
/// voice (kept as the clip's sound; the mouths follow it), `prompt` what happens. Saved with its
/// sound as MP4 at `out`. It waits its turn behind whatever ComfyUI is already making.
pub async fn ltx_clip(base: &str, image: &Path, audio: &Path, prompt: &str, seconds: f64, aspect: &str, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let base = local_comfy(base)?;
    let http = client()?;
    on_status("Uploading the picture and the voice");
    let img = comfy_upload(&http, base, image, "image/png").await?;
    let snd = comfy_upload(&http, base, audio, "audio/wav").await?;
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_nanos() % 1_000_000_000_000) as u64).unwrap_or(7);
    let graph = ltx_graph(&img, &snd, prompt, seconds, aspect, seed)?;
    // A long clip on a 10 GB card takes a good while, more so behind another job.
    comfy_run(&http, base, graph, out, true, 2400, on_status).await
}

/// Queues `graph` on ComfyUI, waits for its video and saves it at `out` (with its sound when
/// `keep_sound`).
async fn comfy_run(http: &reqwest::Client, base: &str, graph: Value, out: &Path, keep_sound: bool, polls: u32, on_status: impl Fn(&str)) -> Result<()> {
    on_status("Queued");
    let q: Value = http.post(format!("{base}/prompt")).json(&json!({ "prompt": graph, "client_id": "mangavox-creative" })).send().await.context("could not reach ComfyUI; open ComfyUI first")?.json().await?;
    if let Some(e) = q.get("error") {
        // The first node it could not take says what is missing (a model file, most often).
        let why = q.get("node_errors").and_then(Value::as_object).and_then(|n| n.values().next()).and_then(|n| n.pointer("/errors/0/details")).and_then(Value::as_str).unwrap_or("");
        bail!("ComfyUI refused the workflow: {}{}", e.get("message").and_then(Value::as_str).unwrap_or("invalid prompt"), if why.is_empty() { String::new() } else { format!(" ({why})") });
    }
    let id = q.get("prompt_id").and_then(Value::as_str).ok_or_else(|| anyhow!("ComfyUI did not queue the workflow"))?.to_string();
    on_status("Animating");
    for _ in 0..polls {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let Ok(r) = http.get(format!("{base}/history/{id}")).send().await else { continue };
        let h: Value = r.json().await.unwrap_or(Value::Null);
        let Some(entry) = h.get(&id) else { continue };
        if entry.pointer("/status/status_str").and_then(Value::as_str) == Some("error") {
            bail!("the ComfyUI workflow failed; check ComfyUI's console");
        }
        if let Some((file, sub, kind)) = comfy_output(entry) {
            on_status("Downloading");
            let data = http.get(format!("{base}/view")).query(&[("filename", file.as_str()), ("subfolder", sub.as_str()), ("type", kind.as_str())]).send().await?.error_for_status()?.bytes().await?;
            if let Some(d) = out.parent() {
                tokio::fs::create_dir_all(d).await?;
            }
            // Saved under its own extension; stitching re-encodes it to MP4.
            let ext = Path::new(&file).extension().and_then(|e| e.to_str()).unwrap_or("mp4");
            let target = out.with_extension(ext);
            tokio::fs::write(&target, &data).await?;
            if target != out {
                let mut cmd = ffmpeg();
                cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(&target).args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);
                if keep_sound {
                    cmd.args(["-c:a", "aac", "-b:a", "192k"]);
                } else {
                    cmd.arg("-an");
                }
                cmd.arg(out);
                run(cmd, "the ComfyUI clip")?;
            }
            return Ok(());
        }
    }
    bail!("ComfyUI took too long")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_follow_each_api() {
        let kling = video_model("mg-kling-2-6-pro");
        let (url, b) = request(kling, "AAA", "move", 7, "9:16");
        assert_eq!(url, "https://api.magnific.com/v1/ai/image-to-video/kling-v2-6-pro");
        assert_eq!((b["image"].as_str(), b["duration"].as_str(), b["aspect_ratio"].as_str()), (Some("AAA"), Some("5"), Some("social_story_9_16")));
        let turbo = video_model("mg-kling-3-turbo-1080p");
        let (_, b) = request(turbo, "AAA", "move", 8, "16:9");
        assert_eq!(b["duration"], "8");
        assert!(b.get("aspect_ratio").is_none());
        let sd = video_model("fal-seedance-1-pro");
        let (url, b) = request(sd, "AAA", "move", 5, "16:9");
        assert_eq!(url, "https://queue.fal.run/fal-ai/bytedance/seedance/v1/pro/image-to-video");
        assert_eq!(b["image_url"], "data:image/jpeg;base64,AAA");
        assert_eq!(b["aspect_ratio"], "16:9");
        assert_eq!(video_model("nope").id, "local-motion");
        for m in VIDEO_MODELS {
            if let VideoVia::Service(id) = m.via {
                assert!(mv_ai::service_def(id).is_some(), "{id}");
            }
        }
    }

    #[test]
    fn results_and_workflows() {
        assert_eq!(video_url(&json!({ "data": { "generated": ["https://x/v.mp4"] } })).as_deref(), Some("https://x/v.mp4"));
        assert_eq!(video_url(&json!({ "video": { "url": "https://x/f.mp4" } })).as_deref(), Some("https://x/f.mp4"));
        let wf = r#"{"1": {"inputs": {"image": "{{image}}", "text": "{{prompt}}", "length": "{{frames}}"}}}"#;
        let v = fill_workflow(wf, "p.png", "say \"hi\"", 5).unwrap();
        assert_eq!(v["1"]["inputs"]["text"], "say \"hi\"");
        assert_eq!(v["1"]["inputs"]["length"], 81);
        assert!(fill_workflow("[1]", "a", "b", 1).is_err());
        let h = json!({ "outputs": { "9": { "gifs": [{ "filename": "clip.mp4", "subfolder": "", "type": "output" }] } } });
        assert_eq!(comfy_output(&h).map(|o| o.0), Some("clip.mp4".to_string()));
        assert_eq!(comfy_output(&json!({ "outputs": { "3": { "images": [{ "filename": "still.png" }] } } })), None);
    }

    #[test]
    fn the_ltx_workflow_takes_the_scene() {
        let g = ltx_graph("p.png", "v.wav", "She says \"run\"", 6.44, "9:16", 5).unwrap();
        assert_eq!(g["269"]["inputs"]["image"], "p.png");
        assert_eq!(g["276"]["inputs"]["audio"], "v.wav");
        assert_eq!(g["340:319"]["inputs"]["value"], "She says \"run\"");
        assert_eq!(g["340:331"]["inputs"]["value"], 6.4);
        assert_eq!((g["340:330"]["inputs"]["value"].as_u64(), g["340:324"]["inputs"]["value"].as_u64()), (Some(720), Some(1280)));
        assert_eq!(g["340:286"]["inputs"]["noise_seed"], 5);
        // Every link points at a node that is there.
        for (id, n) in g.as_object().unwrap() {
            for v in n["inputs"].as_object().unwrap().values() {
                if let Some(to) = v.as_array().and_then(|a| a.first()).and_then(Value::as_str) {
                    assert!(g.get(to).is_some(), "{id} links to the missing {to}");
                }
            }
        }
        assert!(video_model(LTX_VOICE).is_local());
    }

    #[test]
    fn local_motion_and_stitch_when_ffmpeg_is_here() {
        if !has_ffmpeg() {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let img = t.path().join("p.png");
        image::RgbImage::from_fn(400, 600, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 90])).save(&img).unwrap();
        let a = t.path().join("a.mp4");
        let b = t.path().join("b.mp4");
        local_motion(&img, 1, "16:9", 0, &a).unwrap();
        local_motion(&img, 1, "16:9", 2, &b).unwrap();
        let out = t.path().join("out.mp4");
        stitch(&[a, b], "16:9", None, &t.path().join("work"), &out).unwrap();
        assert!(std::fs::metadata(&out).unwrap().len() > 1000);
    }
}
