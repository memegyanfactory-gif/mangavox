//! Paints a few cuts of an anime project for real, without the window:
//! `cargo run -p ui-creative --example anime_paint -- "<project title>" <chapter> <cut,cut,...> [res]`
//!
//! For the chosen cuts only: a design sheet of everyone in them (from the manga panels they appear
//! in), their background plates, each cut's characters on the chroma screen, then each cut
//! composited with its camera move and the finish (no video model: held drawings under the
//! camera, as much TV anime is). Uses Magnific's API with Nano Banana 2.1 (credits are printed).
//! Everything is stored in the project, so the result shows in the app.
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use mv_create::anime::{self, CompParts, Cut};
use mv_create::{existing, prompts, Project};
use ui_studio::images::{self, Reference, CATALOG};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [title, chapter, cuts, ..] = args.as_slice() else { bail!("usage: anime_paint \"<project title>\" <chapter> <cut,cut,...> [1K|2K]") };
    let n: u32 = chapter.parse()?;
    let want: Vec<u32> = cuts.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    let res = args.get(3).map(String::as_str).unwrap_or("2K").to_string();
    std::env::set_var("MANGAVOX_DATA", mv_core::paths::root());
    let mut p = Project::list().into_iter().find(|x| &x.title == title).ok_or_else(|| anyhow!("no project called {title}"))?;
    let m = CATALOG.iter().find(|m| m.id == "nano-banana-2-1").ok_or_else(|| anyhow!("Nano Banana 2.1 is not in the catalog"))?;
    let key = images::key().ok_or_else(|| anyhow!("no Magnific key"))?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let mut spent = 0.;
    let ch = p.chapter(n).cloned().ok_or_else(|| anyhow!("no chapter {n}"))?;
    let chosen: Vec<Cut> = ch.anime.cuts.iter().filter(|c| want.contains(&c.number)).cloned().collect();
    if chosen.is_empty() {
        bail!("none of those cuts are in chapter {n}");
    }
    let mut paint = |prompt: &str, refs: Vec<Reference>, aspect: &str, res: &str, out: &Path, what: &str| -> Result<()> {
        println!("  painting {what} ({} refs, {res}, ~{} credits)", refs.len(), m.cost_at(res));
        rt.block_on(images::generate(m, &key, prompt, &refs, aspect, res, out, |s| println!("    · {s}")))?;
        spent += m.cost_at(res);
        Ok(())
    };
    let number = |refs: Vec<(PathBuf, String)>| -> Vec<Reference> { refs.into_iter().filter(|(f, _)| f.is_file()).enumerate().map(|(i, (file, c))| Reference { file, caption: format!("Image {}: {c}", i + 1) }).collect() };

    // 1. Design sheets of everyone in the chosen cuts, from the panels they appear in.
    let mut names: Vec<String> = vec![];
    for c in &chosen {
        for x in &c.characters {
            if p.character(&x.name).is_some() && !names.contains(&x.name) {
                names.push(x.name.clone());
            }
        }
    }
    for name in &names {
        let c = p.character(name).cloned().unwrap();
        if existing(&p, c.sheet.as_ref()).is_some() {
            continue;
        }
        // Panels where they appear, those showing them alone first.
        let mut panels: Vec<&mv_create::anime::SourcePanel> = ch.anime.panels.iter().filter(|sp| sp.story() && sp.characters.iter().any(|x| x.name == c.name)).collect();
        panels.sort_by_key(|sp| sp.characters.len());
        let refs = number(panels.iter().take(3).map(|sp| (p.file(&sp.file), format!("{} as the manga draws them", c.name))).collect());
        let out = p.new_image("sheets", &format!("{}-sheet", c.id));
        paint(&prompts::sheet(&p, &c, !refs.is_empty()), refs, "16:9", "1K", &out, &format!("{}'s design sheet", c.name))?;
        let rel = p.rel(&out);
        if let Some(x) = p.character_mut(name) {
            x.sheet = Some(rel);
        }
        p.save()?;
    }

    // 2. The plates the cuts sit on.
    let plates: Vec<String> = chosen.iter().map(|c| c.plate.clone()).fold(vec![], |mut v, x| {
        if !v.contains(&x) {
            v.push(x);
        }
        v
    });
    for id in &plates {
        let Some(pl) = p.plates.iter().find(|x| &x.id == id).cloned() else { continue };
        if existing(&p, pl.image.as_ref()).is_some() {
            continue;
        }
        let place = pl.location.to_lowercase();
        let refs = number(ch.anime.panels.iter().filter(|sp| sp.story() && !place.is_empty() && (sp.location.to_lowercase().contains(&place) || place.contains(&sp.location.to_lowercase()))).take(2).map(|sp| (p.file(&sp.file), "the manga's drawing of this place".to_string())).collect());
        let out = p.new_image("plates", &format!("plate-{}", pl.id));
        paint(&anime::plate(&p, &pl, &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", &res, &out, &format!("the plate {}", pl.id))?;
        let rel = p.rel(&out);
        if let Some(x) = p.plates.iter_mut().find(|x| &x.id == id) {
            x.image = Some(rel);
        }
        p.save()?;
    }

    // 3. Each cut's characters on the chroma screen, 4. composited.
    let python = mv_core::paths::root().join("models").join("vfx").join("venv").join("Scripts").join("python.exe");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("python").join("anime_comp.py");
    for c in &chosen {
        let kind = ch.anime.kind_of(c).to_string();
        let mut refs: Vec<(PathBuf, String)> = vec![];
        for x in &c.characters {
            if let Some(m) = p.character(&x.name) {
                if let Some(f) = existing(&p, m.sheet.as_ref()) {
                    refs.push((f, format!("{} design sheet (face, hair, eyes, outfit and colours must match)", m.name)));
                }
            }
        }
        for i in c.from.iter().take(1) {
            if let Some(sp) = ch.anime.panels.get(*i) {
                refs.push((p.file(&sp.file), "the manga panel this shot adapts: keep its people, their outfits and acting, redrawn as a colour anime frame in the framing asked for (ignore its balloons, text and screentone)".into()));
            }
        }
        let plate = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(&p, x.image.as_ref()));
        if let Some(f) = &plate {
            refs.push((f.clone(), if c.has_cast() { "the background plate this frame sits on: match its light, colour and perspective, but do NOT draw it".into() } else { "the background of this shot".into() }));
        }
        let refs = number(refs);
        let caps: Vec<String> = refs.iter().map(|r| r.caption.clone()).collect();
        let has_cast = c.has_cast();
        let prompt = if has_cast { anime::layer(&p, c, "16:9", &caps) } else { anime::frame(&p, c, "16:9", &caps) };
        let out = p.new_image("layers", &format!("ch{n:03}-c{:03}-{}", c.number, if has_cast { "layer" } else { "frame" }));
        paint(&prompt, refs, "16:9", &res, &out, &format!("cut {}'s {}", c.number, if has_cast { "characters" } else { "frame" }))?;
        let rel = p.rel(&out);
        if let Some(x) = p.chapter_mut(n).anime.cuts.iter_mut().find(|x| x.number == c.number) {
            if has_cast {
                x.layer = Some(rel.clone());
            } else {
                x.key = Some(rel.clone());
            }
        }
        p.save()?;
        // Composite: the plate (or the whole frame) under the camera, the characters keyed over it.
        let cut_file = p.dir().join("cuts").join(format!("ch{n:03}-c{:03}-{}.mp4", c.number, mv_create::now()));
        let still = cut_file.with_extension("png");
        let (bg, fg) = if has_cast { (plate.as_deref(), Some((out.as_path(), false))) } else { (Some(out.as_path()), None) };
        let job = anime::comp_job(c, &kind, &cut_file, CompParts { plate: bg, layer: fg, ..Default::default() }, &p.settings.video_aspect, true);
        for (cmd, mut j) in [("comp", job.clone()), ("still", job)] {
            if cmd == "still" {
                j["out"] = serde_json::json!(still);
            }
            let work = p.dir().join("anim").join("work");
            std::fs::create_dir_all(&work)?;
            let f = work.join(format!("{cmd}-paint.json"));
            std::fs::write(&f, serde_json::to_vec(&j)?)?;
            let o = std::process::Command::new(&python).arg(&script).arg(cmd).arg(&f).output().context("could not run the compositor")?;
            let text = String::from_utf8_lossy(&o.stdout);
            if !text.lines().any(|l| l.contains("\"ok\": true")) {
                bail!("the compositor failed on cut {}: {}{}", c.number, text.lines().last().unwrap_or(""), String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
            }
        }
        let rel = p.rel(&cut_file);
        if let Some(x) = p.chapter_mut(n).anime.cuts.iter_mut().find(|x| x.number == c.number) {
            x.comp = Some(rel);
        }
        p.save()?;
        println!("  cut {} done: {}", c.number, cut_file.display());
    }
    println!("credits spent: about {spent:.0}");
    Ok(())
}
