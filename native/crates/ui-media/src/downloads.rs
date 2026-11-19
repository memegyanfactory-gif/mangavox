//! Downloads of anime episodes, series episodes and movies for offline watching.
//!
//! A source picked in a source list is saved whole to the data folder, in a library layout a
//! media player understands (never the temp or cache folder):
//!
//! ```text
//! <data>/Downloads/Anime/Solo Leveling/Season 01/Solo Leveling - S01E01.mkv
//! <data>/Downloads/Series/Breaking Bad/Season 02/Breaking Bad - S02E05.mkv
//! <data>/Downloads/Movies/Dune Part Two/Dune Part Two.mkv
//! ```
//!
//! A torrent is read through the app's own torrent engine (the same one the player streams from,
//! so only the picked file is fetched), a direct link straight from its server. The file is
//! written as `<name>.part` and renamed when complete, so a half-finished download never looks
//! like a finished one. Every download shows in Activity with its progress, speed and Cancel;
//! a finished one offers Play and Show in folder.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use gpui_kit::*;
use mv_ui::activity::{Card, Lead};
use mv_ui::nav::PlayRequest;
use mv_ui::{btn, ph, ButtonKind, Route};

/// The kind of thing downloaded: its top folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Anime,
    Series,
    Movie,
}

impl Kind {
    fn folder(self) -> &'static str {
        match self {
            Kind::Anime => "Anime",
            Kind::Series => "Series",
            Kind::Movie => "Movies",
        }
    }

    /// From what is being played: an anime has a MAL id, a series episode a season and episode.
    pub fn of(req: &PlayRequest) -> Kind {
        if req.mal_id.is_some() {
            Kind::Anime
        } else if req.season.is_some() && req.episode.is_some() {
            Kind::Series
        } else {
            Kind::Movie
        }
    }
}

/// Where every download lives.
pub fn root() -> PathBuf {
    mv_core::paths::root().join("Downloads")
}

/// A name Windows (and every other system) accepts for a file or folder.
pub fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { ' ' } else { c }).collect();
    let mut out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // No trailing dots or spaces, and none of the names Windows keeps for devices.
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let upper = out.to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str()) || ((upper.starts_with("COM") || upper.starts_with("LPT")) && upper.len() == 4 && upper.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.push('_');
    }
    let out: String = out.chars().take(120).collect();
    if out.trim().is_empty() {
        "Untitled".into()
    } else {
        out.trim().to_string()
    }
}

/// The show's name from a play title ("Solo Leveling - Episode 3", "Dark S01E02" → the name).
pub fn show_name(title: &str) -> String {
    let t = title.trim();
    if let Some((name, rest)) = t.rsplit_once(" - ") {
        if rest.to_lowercase().starts_with("episode") {
            return name.trim().to_string();
        }
    }
    if let Some((name, rest)) = t.rsplit_once(' ') {
        let r = rest.to_lowercase();
        if r.len() >= 6 && r.starts_with('s') && r.contains('e') && r.chars().filter(|c| c.is_ascii_digit()).count() >= 2 {
            return name.trim().to_string();
        }
    }
    t.to_string()
}

/// Where a download is saved (without its extension, which comes from the file itself).
pub fn target_stem(req: &PlayRequest, kind: Kind) -> PathBuf {
    let show = safe_name(&show_name(&req.title));
    let base = root().join(kind.folder()).join(&show);
    match (kind, req.season, req.episode) {
        (Kind::Movie, ..) => base.join(&show),
        (_, s, Some(e)) => {
            let s = s.unwrap_or(1);
            base.join(format!("Season {s:02}")).join(format!("{show} - S{s:02}E{e:02}"))
        }
        _ => base.join(&show),
    }
}

/// The video's extension from its file name, else mp4.
fn ext_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .filter(|e| matches!(e.as_str(), "mkv" | "mp4" | "avi" | "webm" | "mov" | "m4v" | "ts" | "wmv" | "flv"))
        .unwrap_or_else(|| "mp4".into())
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Starting,
    Downloading,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
struct Progress {
    phase: Phase,
    done: u64,
    total: u64,
    /// Bytes a second, smoothed.
    speed: f64,
    /// The finished file (or where it is going).
    file: PathBuf,
}

struct Item {
    id: u64,
    title: String,
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    /// The card was closed (the download goes on if it is still running).
    hidden: bool,
}

/// Every download of this session. A global entity: the shell observes it, so Activity redraws
/// while a download runs.
pub struct MediaDownloads {
    items: Vec<Item>,
    ticking: bool,
}

struct Global(Entity<MediaDownloads>);
impl gpui_kit::Global for Global {}

pub fn downloads(cx: &mut App) -> Entity<MediaDownloads> {
    if let Some(g) = cx.try_global::<Global>() {
        return g.0.clone();
    }
    let e = cx.new(|_| MediaDownloads { items: vec![], ticking: false });
    cx.set_global(Global(e.clone()));
    e
}

static IDS: AtomicU64 = AtomicU64::new(1);

/// Starts downloading `req` (a source from a source list). The same file already downloading
/// is not started twice; one already downloaded says where it is.
pub fn start(req: PlayRequest, cx: &mut App) {
    let kind = Kind::of(&req);
    let stem = target_stem(&req, kind);
    let title = show_title(&req, kind);
    let e = downloads(cx);
    let same = e.read(cx).items.iter().any(|i| {
        let p = i.progress.lock().unwrap_or_else(|x| x.into_inner());
        matches!(p.phase, Phase::Starting | Phase::Downloading) && p.file.with_extension("") == stem
    });
    if same {
        mv_ui::toast::show(cx, mv_ui::toast::Toast::info(format!("{title} is already downloading")).icon(ph::DOWNLOAD_SIMPLE));
        return;
    }
    let progress = Arc::new(Mutex::new(Progress { phase: Phase::Starting, done: 0, total: 0, speed: 0., file: stem.with_extension("mp4") }));
    let cancel = Arc::new(AtomicBool::new(false));
    let id = IDS.fetch_add(1, Ordering::Relaxed);
    {
        let (progress, cancel) = (progress.clone(), cancel.clone());
        mv_ui::bg::runtime().spawn(async move {
            let r = run(&req, &stem, &progress, &cancel).await;
            let mut p = progress.lock().unwrap_or_else(|x| x.into_inner());
            p.phase = match r {
                Ok(()) => Phase::Done,
                Err(_) if cancel.load(Ordering::Relaxed) => Phase::Cancelled,
                Err(err) => {
                    mv_core::logging::global().warn("download", &format!("{}: {err:#}", req.title));
                    Phase::Failed(format!("{err:#}"))
                }
            };
        });
    }
    e.update(cx, |d, cx| {
        d.items.push(Item { id, title: title.clone(), progress, cancel, hidden: false });
        d.tick(cx);
        cx.notify();
    });
    mv_ui::toast::show(cx, mv_ui::toast::Toast::success(format!("Downloading {title}. It shows in Activity.")).icon(ph::DOWNLOAD_SIMPLE));
}

/// "Solo Leveling · S01E01", "Dune Part Two".
fn show_title(req: &PlayRequest, kind: Kind) -> String {
    let show = show_name(&req.title);
    match (kind, req.season, req.episode) {
        (Kind::Movie, ..) => show,
        (_, s, Some(e)) => format!("{show} · S{:02}E{e:02}", s.unwrap_or(1)),
        _ => show,
    }
}

impl MediaDownloads {
    /// Repaints Activity twice a second while anything downloads.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let alive = this.update(cx, |d, cx| {
                cx.notify();
                let alive = d.items.iter().any(|i| matches!(i.progress.lock().map(|p| p.phase.clone()), Ok(Phase::Starting | Phase::Downloading)));
                if !alive {
                    d.ticking = false;
                }
                alive
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// A download is running.
    pub fn busy(&self) -> bool {
        self.items.iter().any(|i| matches!(i.progress.lock().map(|p| p.phase.clone()), Ok(Phase::Starting | Phase::Downloading)))
    }

    fn cancel(&mut self, id: u64) {
        if let Some(i) = self.items.iter().find(|i| i.id == id) {
            i.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn hide(&mut self, id: u64) {
        if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
            i.hidden = true;
        }
        // Finished ones that were closed are forgotten.
        self.items.retain(|i| !(i.hidden && !matches!(i.progress.lock().map(|p| p.phase.clone()), Ok(Phase::Starting | Phase::Downloading))));
    }
}

/// One Activity card per download not closed.
pub fn cards(cx: &mut App) -> Vec<Card> {
    let e = downloads(cx);
    let d = e.read(cx);
    d.items.iter().filter(|i| !i.hidden).map(|i| card(&e, i)).collect()
}

fn card(e: &Entity<MediaDownloads>, i: &Item) -> Card {
    let p = i.progress.lock().map(|p| p.clone()).unwrap_or_else(|x| x.into_inner().clone());
    let id = i.id;
    let key = format!("media-dl-{id}");
    let order = id + (1 << 40);
    let close = {
        let e = e.clone();
        move |_: &mut Window, cx: &mut App| e.update(cx, |d, cx| {
            d.hide(id);
            cx.notify();
        })
    };
    let reveal = {
        let f = p.file.clone();
        btn("show", "Show in folder", ButtonKind::Ghost).small().icon(ph::FOLDER_OPEN).on_click(move |_, _, _| reveal(&f))
    };
    match &p.phase {
        Phase::Done => {
            let (f, title) = (p.file.clone(), i.title.clone());
            Card::new(key, order, Lead::Done, i.title.clone(), format!("Downloaded · {} · {}", mv_ui::fmt_bytes(p.done.max(p.total)), p.file.display()))
                .action(btn("play", "Play", ButtonKind::Ghost).small().icon(ph::PLAY).on_click(move |_, w, cx| {
                    let request = PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                    mv_ui::navigate(Route::Player { request }, w, cx);
                }))
                .action(reveal)
                .on_close(close)
        }
        Phase::Failed(why) => Card::new(key, order, Lead::Failed, i.title.clone(), format!("Download failed: {why}")).on_close(close),
        Phase::Cancelled => Card::new(key, order, Lead::Failed, i.title.clone(), "Download cancelled; the unfinished file was removed").on_close(close),
        Phase::Starting | Phase::Downloading => {
            let frac = if p.total > 0 { (p.done as f64 / p.total as f64).min(1.) as f32 } else { 0. };
            let line = if p.phase == Phase::Starting {
                "Connecting to the source…".to_string()
            } else if p.total > 0 {
                let left = if p.speed > 1000. { format!(" · {} left", eta(((p.total - p.done.min(p.total)) as f64 / p.speed) as u64)) } else { String::new() };
                format!("{} of {} · {}/s{left}", mv_ui::fmt_bytes(p.done), mv_ui::fmt_bytes(p.total), mv_ui::fmt_bytes(p.speed as u64))
            } else {
                format!("{} · {}/s", mv_ui::fmt_bytes(p.done), mv_ui::fmt_bytes(p.speed as u64))
            };
            let e2 = e.clone();
            Card::new(key, order, Lead::Progress(frac), i.title.clone(), line)
                .progress(frac, true)
                .count(format!("{}%", (frac * 100.).round() as u32))
                .action(btn("cancel", "Cancel", ButtonKind::Ghost).small().icon(ph::X).tooltip("Stop this download and remove the unfinished file").on_click(move |_, _, cx| e2.update(cx, |d, cx| {
                    d.cancel(id);
                    cx.notify();
                })))
                .action(reveal)
                .on_close(close)
        }
    }
}

/// "1 h 05 min", "12 min", "40 s".
fn eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", (secs + 30) / 60),
        _ => format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60),
    }
}

fn reveal(f: &Path) {
    #[cfg(windows)]
    {
        let target = if f.is_file() { format!("/select,{}", f.display()) } else { f.parent().unwrap_or(f).display().to_string() };
        let _ = std::process::Command::new("explorer").arg(target).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = f;
    }
}

// ------------------------------------------------------------------ the transfer

/// Downloads `req` to `stem.<ext>`.
async fn run(req: &PlayRequest, stem: &Path, progress: &Arc<Mutex<Progress>>, cancel: &Arc<AtomicBool>) -> Result<()> {
    let src = req.source.trim();
    let (url, name) = if src.starts_with("http://") || src.starts_with("https://") {
        let name = src.split('?').next().unwrap_or(src).rsplit('/').next().unwrap_or("").to_string();
        (src.to_string(), name)
    } else {
        // A torrent: the player's engine picks the file (the episode in a season pack) and serves it.
        let engine = ui_player::backend::engine().await.context("the torrent engine did not start")?;
        let source = mv_torrent::Source::parse(src).map_err(|e| anyhow!("this source cannot be downloaded: {e}"))?;
        let added = engine.add(source, mv_torrent::AddOptions { file_idx: req.file_idx, ..Default::default() }).await.context("the torrent could not be opened")?;
        set_total(progress, added.file_len);
        (added.stream_url.clone(), added.file_name.clone())
    };
    let file = stem.with_extension(ext_of(&name));
    progress.lock().unwrap_or_else(|x| x.into_inner()).file = file.clone();
    if file.is_file() {
        let len = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        set_total(progress, len);
        progress.lock().unwrap_or_else(|x| x.into_inner()).done = len;
        return Ok(());
    }
    if let Some(d) = file.parent() {
        tokio::fs::create_dir_all(d).await.with_context(|| format!("could not make the folder {}", d.display()))?;
    }
    let part = file.with_extension(format!("{}.part", ext_of(&name)));
    let r = fetch(&url, &part, progress, cancel).await;
    if r.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        return r;
    }
    tokio::fs::rename(&part, &file).await.with_context(|| format!("could not finish {}", file.display()))?;
    Ok(())
}

fn set_total(progress: &Arc<Mutex<Progress>>, total: u64) {
    let mut p = progress.lock().unwrap_or_else(|x| x.into_inner());
    if total > 0 {
        p.total = total;
    }
}

/// Reads `url` to `out`, reporting progress, until done or cancelled. A dropped connection is
/// picked up where it stopped (a range request), a few times.
async fn fetch(url: &str, out: &Path, progress: &Arc<Mutex<Progress>>, cancel: &Arc<AtomicBool>) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;
    // No overall timeout: a long film over a slow swarm takes hours; a stall is caught below.
    let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).build()?;
    let mut f = tokio::fs::File::create(out).await.with_context(|| format!("could not write {}", out.display()))?;
    let mut done: u64 = 0;
    let mut tries = 0;
    let (mut window_at, mut window_bytes) = (Instant::now(), 0u64);
    loop {
        let mut rq = http.get(url);
        if done > 0 {
            rq = rq.header("Range", format!("bytes={done}-"));
        }
        let resp = match rq.send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => bail!("the source answered {}", r.status()),
            Err(e) if tries < 5 => {
                tries += 1;
                mv_core::logging::global().warn("download", &format!("retrying after: {e}"));
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            Err(e) => return Err(e).context("the source could not be reached"),
        };
        if done > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            // The server ignored the range: start over.
            done = 0;
            f.set_len(0).await?;
            f = tokio::fs::File::create(out).await?;
        }
        if let Some(len) = resp.content_length() {
            set_total(progress, done + len);
        }
        progress.lock().unwrap_or_else(|x| x.into_inner()).phase = Phase::Downloading;
        let mut resp = resp;
        let mut broke = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            // A swarm that sends nothing for two minutes has stalled: reconnect.
            let chunk = match tokio::time::timeout(Duration::from_secs(120), resp.chunk()).await {
                Ok(Ok(Some(c))) => c,
                Ok(Ok(None)) => break,
                Ok(Err(_)) | Err(_) => {
                    broke = true;
                    break;
                }
            };
            f.write_all(&chunk).await.context("could not write the file (is the disk full?)")?;
            done += chunk.len() as u64;
            window_bytes += chunk.len() as u64;
            let el = window_at.elapsed().as_secs_f64();
            let mut p = progress.lock().unwrap_or_else(|x| x.into_inner());
            p.done = done;
            if el >= 1.0 {
                let now = window_bytes as f64 / el;
                p.speed = if p.speed == 0. { now } else { p.speed * 0.6 + now * 0.4 };
                window_at = Instant::now();
                window_bytes = 0;
            }
        }
        let total = progress.lock().map(|p| p.total).unwrap_or(0);
        if !broke && (total == 0 || done >= total) {
            break;
        }
        tries += 1;
        if tries > 8 {
            bail!("the connection kept dropping ({} of {} saved)", mv_ui::fmt_bytes(done), mv_ui::fmt_bytes(total));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    f.flush().await?;
    if done == 0 {
        bail!("the source sent nothing");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Named imports: `super::*` would bring in the UI kit's own `test` attribute.
    use super::{ext_of, safe_name, show_name, show_title, target_stem, Kind, PlayRequest};
    use std::path::Path;

    fn req(title: &str, mal: bool, s: Option<u32>, e: Option<u32>) -> PlayRequest {
        PlayRequest { title: title.into(), source: String::new(), file_idx: None, imdb_id: None, mal_id: mal.then(|| "1".into()), season: s, episode: e }
    }

    #[test]
    fn downloads_are_filed_like_a_library() {
        let a = req("Solo Leveling - Episode 3", true, Some(1), Some(3));
        assert_eq!(Kind::of(&a), Kind::Anime);
        assert!(target_stem(&a, Kind::Anime).ends_with(Path::new("Downloads/Anime/Solo Leveling/Season 01/Solo Leveling - S01E03")));
        let s = req("Dark S02E05", false, Some(2), Some(5));
        assert_eq!(Kind::of(&s), Kind::Series);
        assert!(target_stem(&s, Kind::Series).ends_with(Path::new("Downloads/Series/Dark/Season 02/Dark - S02E05")));
        let m = req("Dune: Part Two", false, None, None);
        assert_eq!(Kind::of(&m), Kind::Movie);
        assert!(target_stem(&m, Kind::Movie).ends_with(Path::new("Downloads/Movies/Dune Part Two/Dune Part Two")));
        assert!(!target_stem(&m, Kind::Movie).starts_with(std::env::temp_dir()));
        assert_eq!(show_title(&a, Kind::Anime), "Solo Leveling · S01E03");
    }

    #[test]
    fn names_are_safe_everywhere() {
        assert_eq!(safe_name("What?  Is <this>: a/b."), "What Is this a b");
        assert_eq!(safe_name("CON"), "CON_");
        assert_eq!(safe_name("  "), "Untitled");
        assert_eq!(show_name("Re:Zero - Episode 12"), "Re:Zero");
        assert_eq!(show_name("Movie Title"), "Movie Title");
        assert_eq!(ext_of("[Sub] Show - 01 [1080p].mkv"), "mkv");
        assert_eq!(ext_of("stream"), "mp4");
    }
}
