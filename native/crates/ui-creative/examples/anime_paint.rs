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
    // `MANGAVOX_PAINT_WEB=1`: painted on Magnific's website in the app's own browser (its profile,
    // so its sign-in), on the Unlimited offer at 1K, instead of the API's credits.
    let web = std::env::var("MANGAVOX_PAINT_WEB").is_ok_and(|v| v == "1");
    if web {
        let wall = mv_webwall::WebWall::start(mv_webwall::WallConfig { profile_dir: mv_core::paths::root().join("webview"), ..Default::default() }).map_err(|e| anyhow!("the app's browser: {e}"))?;
        mv_webwall::set_global(wall);
    }
    let key = if web { String::new() } else { images::key().ok_or_else(|| anyhow!("no Magnific key"))? };
    let res = if web { "1K".to_string() } else { res };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let mut spent = 0.;
    // The props the board's characters rest on (a plate painted with one in it is painted again).
    if mv_create::anime::sync_props(&mut p) {
        p.save()?;
    }
    let ch = p.chapter(n).cloned().ok_or_else(|| anyhow!("no chapter {n}"))?;
    let chosen: Vec<Cut> = ch.anime.cuts.iter().filter(|c| want.contains(&c.number)).cloned().collect();
    if chosen.is_empty() {
        bail!("none of those cuts are in chapter {n}");
    }
    let mut paint = |prompt: &str, refs: Vec<Reference>, aspect: &str, res: &str, out: &Path, what: &str| -> Result<()> {
        if web {
            println!("  painting {what} on the website ({} refs, 1K Unlimited)", refs.len());
            // Signed out: the window stays up for the owner to sign in; tried again for 15 minutes.
            for _ in 0..45 {
                match rt.block_on(ui_creative::paint_on_website(prompt, &refs, aspect, out, |s| println!("    · {s}"))) {
                    Err(e) if e.to_string().starts_with("Sign in to Magnific") || e.to_string().contains("confirm you are a person") => {
                        println!("    · waiting for you in the Magnific window (sign in, or the check for a person)");
                        std::thread::sleep(std::time::Duration::from_secs(20));
                    }
                    r => return r,
                }
            }
            bail!("nobody signed in to Magnific in the window");
        }
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

    // 2. The props the chosen cuts' plates carry: a design sheet each, from the panels where
    // someone rests on it.
    let props: Vec<mv_create::anime::Prop> = p.props.iter().filter(|x| chosen.iter().any(|c| c.plate == x.plate) && existing(&p, x.image.as_ref()).is_none()).cloned().collect();
    for x in &props {
        let refs = number(ch.anime.cuts.iter().filter(|c| c.plate == x.plate && c.touched().as_deref() == Some(x.what.as_str())).filter_map(|c| c.from.first().and_then(|i| ch.anime.panels.get(*i))).take(3).map(|sp| (p.file(&sp.file), format!("the {} as the manga draws it (ignore its people, balloons and text)", x.what))).collect());
        let out = p.new_image("props", &format!("prop-{}", x.id));
        paint(&anime::prop_sheet(&p, x, &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", "1K", &out, &format!("the {} prop sheet", x.what))?;
        let rel = p.rel(&out);
        if let Some(y) = p.props.iter_mut().find(|y| y.id == x.id) {
            y.image = Some(rel);
        }
        p.save()?;
    }

    // 3. The plates the cuts sit on (without their props).
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
        // By place name, else the panels the plate's own cuts adapt (as the app does).
        let mut found: Vec<(PathBuf, String)> = ch.anime.panels.iter().filter(|sp| sp.story() && !place.is_empty() && (sp.location.to_lowercase().contains(&place) || place.contains(&sp.location.to_lowercase()))).take(2).map(|sp| (p.file(&sp.file), "the manga's drawing of this place".to_string())).collect();
        for i in ch.anime.cuts.iter().filter(|c| &c.plate == id).flat_map(|c| c.from.iter().take(1)) {
            if let Some(sp) = ch.anime.panels.get(*i).filter(|sp| sp.story()) {
                let f = p.file(&sp.file);
                if found.len() < 3 && !found.iter().any(|(x, _)| *x == f) {
                    found.push((f, "the manga's drawing of this place (ignore its people, balloons and text)".into()));
                }
            }
        }
        let refs = number(found);
        let out = p.new_image("plates", &format!("plate-{}", pl.id));
        paint(&anime::plate(&p, &pl, &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", &res, &out, &format!("the plate {}", pl.id))?;
        let rel = p.rel(&out);
        if let Some(x) = p.plates.iter_mut().find(|x| &x.id == id) {
            x.image = Some(rel);
        }
        p.save()?;
    }

    // 4. Each cut layout first, as the app draws it: the cut's own background from its camera,
    // the characters and props drawn into it (lit, shadowed), the cel lifted from that frame.
    // 5. Composited: the layout under the camera; and, to check the animated path, the cel over
    // the cut's background with the layout's shadows (saved as "-cel.png").
    let python = mv_core::paths::root().join("models").join("vfx").join("venv").join("Scripts").join("python.exe");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("python").join("anime_comp.py");
    let comp = |job: serde_json::Value, cmd: &str, dir: &Path, number: u32| -> Result<()> {
        let work = dir.join("anim").join("work");
        std::fs::create_dir_all(&work)?;
        let f = work.join(format!("{cmd}-paint.json"));
        std::fs::write(&f, serde_json::to_vec(&job)?)?;
        let o = std::process::Command::new(&python).arg(&script).arg(cmd).arg(&f).output().context("could not run the compositor")?;
        let text = String::from_utf8_lossy(&o.stdout);
        if !text.lines().any(|l| l.contains("\"ok\": true")) {
            bail!("the compositor failed on cut {number}: {}{}", text.lines().last().unwrap_or(""), String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
        }
        Ok(())
    };
    for c in &chosen {
        let kind = ch.anime.kind_of(c).to_string();
        let plate = p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(&p, x.image.as_ref()));
        let panel = c.from.first().and_then(|i| ch.anime.panels.get(*i)).map(|sp| p.file(&sp.file)).filter(|f| f.is_file());
        if !c.has_cast() {
            let mut refs: Vec<(PathBuf, String)> = vec![];
            if let Some(f) = &panel {
                refs.push((f.clone(), "the manga panel this shot adapts".into()));
            }
            if let Some(f) = &plate {
                refs.push((f.clone(), "the background of this shot".into()));
            }
            let refs = number(refs);
            let out = p.new_image("layers", &format!("ch{n:03}-c{:03}-frame", c.number));
            paint(&anime::frame(&p, c, "16:9", &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", &res, &out, &format!("cut {}'s frame", c.number))?;
            continue;
        }
        // (1) The cut's background.
        let mut refs: Vec<(PathBuf, String)> = vec![];
        if let Some(f) = &plate {
            refs.push((f.clone(), "the background plate: the place to paint again from this cut's camera".into()));
        }
        if let Some(f) = &panel {
            refs.push((f.clone(), "the manga panel this cut adapts: its framing and camera angle (ignore its people, balloons and text)".into()));
        }
        let refs = number(refs);
        let bg = p.new_image("layers", &format!("ch{n:03}-c{:03}-bg", c.number));
        paint(&anime::cut_bg(&p, c, "16:9", &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", &res, &bg, &format!("cut {}'s background", c.number))?;
        // (2) The layout drawn into it.
        let mut refs: Vec<(PathBuf, String)> = vec![(bg.clone(), "this cut's background (keep it exactly, draw into it)".into())];
        for x in c.characters.iter().filter_map(|x| p.character(&x.name)) {
            if let Some(f) = existing(&p, x.sheet.as_ref()) {
                refs.push((f, format!("{} model sheet (face, hair, eyes, outfit and colours must match)", x.name)));
            }
        }
        for x in p.props.iter().filter(|x| x.plate == c.plate) {
            if let Some(f) = existing(&p, x.image.as_ref()) {
                refs.push((f, format!("the {} prop sheet: draw it exactly like this", x.what)));
            }
        }
        if let Some(f) = &panel {
            refs.push((f.clone(), "the manga panel this shot adapts: its people, their acting and the framing (ignore its balloons, text and screentone)".into()));
        }
        let refs = number(refs);
        let lay = p.new_image("layers", &format!("ch{n:03}-c{:03}-layout", c.number));
        paint(&anime::layout_frame(&p, c, "16:9", &refs.iter().map(|r| r.caption.clone()).collect::<Vec<_>>()), refs, "16:9", &res, &lay, &format!("cut {}'s layout", c.number))?;
        // (3) The cel.
        let cel = p.new_image("layers", &format!("ch{n:03}-c{:03}-layer", c.number));
        paint(&anime::cel(&p, c), vec![Reference { file: lay.clone(), caption: "Image 1: the cut's layout frame".into() }], "16:9", &res, &cel, &format!("cut {}'s cel", c.number))?;
        let (bg_rel, lay_rel, cel_rel) = (p.rel(&bg), p.rel(&lay), p.rel(&cel));
        if let Some(x) = p.chapter_mut(n).anime.cuts.iter_mut().find(|x| x.number == c.number) {
            x.bg = Some(bg_rel);
            x.layout = Some(lay_rel);
            x.layer = Some(cel_rel);
        }
        p.save()?;
        let cut_file = p.dir().join("cuts").join(format!("ch{n:03}-c{:03}-{}.mp4", c.number, mv_create::now()));
        let job = anime::comp_job(c, &kind, &cut_file, CompParts { plate: Some(&lay), ..Default::default() }, &p.settings.video_aspect, true);
        comp(job.clone(), "comp", &p.dir(), c.number)?;
        let mut still = job;
        still["out"] = serde_json::json!(cut_file.with_extension("png"));
        comp(still, "still", &p.dir(), c.number)?;
        let mut check = anime::comp_job(c, &kind, &cut_file, CompParts { plate: Some(&bg), layout: Some(&lay), layer: Some((cel.as_path(), false)), ..Default::default() }, &p.settings.video_aspect, true);
        check["out"] = serde_json::json!(cut_file.with_file_name(format!("ch{n:03}-c{:03}-cel.png", c.number)));
        comp(check, "still", &p.dir(), c.number)?;
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
