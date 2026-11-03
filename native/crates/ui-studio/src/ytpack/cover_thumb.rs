//! The one AI image per manga that every Studio video thumbnail of it is drawn on (see
//! `mv_recap::thumb`): painted from the manga's cover with the default model and size from
//! Settings › Magnific, or with another service's model when only that service has a key. Registered at startup so the reader can ask
//! for it; it only ever runs when asked (never in a loop) and records the credits it spends.
use futures::FutureExt as _;
use mv_recap::thumb::{self, PaintJob, Painter};

use super::magnific::{self, ImageModel, Keys, Prefs, CATALOG};
use super::thumbs::{Gallery, Thumb};

pub(crate) fn register() {
    thumb::set_painter(Painter { ready, cost, paint });
}

/// Some model with a key can paint from the cover (Recraft alone cannot: it takes no image).
fn ready() -> bool {
    let keys = Keys::load();
    keys.has(choice(&Prefs::load(), &keys).0)
}

/// The default model when it has a key and takes a reference image (the cover is one), else
/// another model with a key that does (Nano Banana 2.1 first); the default size when that model
/// offers it, else its smallest.
fn choice(prefs: &Prefs, keys: &Keys) -> (&'static ImageModel, &'static str) {
    let m = magnific::usable(CATALOG.iter().find(|m| m.id == prefs.model).unwrap_or(&CATALOG[0]), keys, true, "16:9");
    let res = m.resolutions.iter().copied().find(|r| *r == prefs.resolution).or_else(|| m.resolutions.first().copied()).unwrap_or("");
    (m, res)
}

fn cost() -> f64 {
    let (m, res) = choice(&Prefs::load(), &Keys::load());
    m.cost_at(res)
}

fn paint(job: PaintJob) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
    async move {
        let keys = Keys::load();
        let (model, res) = choice(&Prefs::load(), &keys);
        let key = magnific::key_for(model).ok_or_else(|| match keys.any() {
            true => anyhow::anyhow!("{}", magnific::no_fit(&keys, true, "16:9")),
            false => anyhow::anyhow!("Add a Magnific API key in Settings › Magnific, or another image service's key in Settings › AI services"),
        })?;
        let refs = [magnific::Reference { file: job.cover.clone(), caption: "Image 1: the manga's cover".into() }];
        magnific::generate(model, &key, &thumb::cover_prompt(&job.title), &refs, "16:9", res, &job.out, |_| {}).await?;
        let credits = model.cost_at(res);
        magnific::record(model, credits);
        // Also in the manga's gallery on the Thumbnails page, to reuse or edit there.
        let id = format!("cover-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        if std::fs::create_dir_all(Gallery::dir(&job.manga_id)).is_ok() && std::fs::copy(&job.out, Gallery::image(&job.manga_id, &id)).is_ok() {
            let mut g = Gallery::load(&job.manga_id);
            let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            g.thumbs.insert(0, Thumb { id, prompt: thumb::cover_prompt(&job.title), label: "Video thumbnail (from the cover)".into(), parent: None, model: model.name.to_string(), resolution: res.to_string(), cost: credits, created });
            g.save(&job.manga_id);
        }
        Ok(())
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_model_that_takes_the_cover() {
        let p = |model: &str, res: &str| Prefs { model: model.into(), resolution: res.into(), ..Default::default() };
        let keys = Keys { magnific: true, services: vec![] };
        let choice = |p: &Prefs| choice(p, &keys);
        // Nothing saved: Nano Banana 2.1 at its smallest size (the cheapest default).
        let (m, r) = choice(&p("", ""));
        assert_eq!((m.id, r), ("nano-banana-2-1", "1K"));
        // The saved default and size when the model takes references.
        let (m, r) = choice(&p("seedream-v5-pro", "2K"));
        assert_eq!((m.id, r), ("seedream-v5-pro", "2K"));
        // A size the model does not offer falls back to its first.
        assert_eq!(choice(&p("seedream-v5-pro", "4K")).1, "1.5K");
        // A model without reference images cannot use the cover.
        assert_eq!(choice(&p("nano-banana-pro", "2K")).0.id, "nano-banana-2-1");
        // Only Higgsfield has a key: its Soul model, which takes one reference.
        let hf = Keys { magnific: false, services: vec!["higgsfield"] };
        let (m, r) = super::choice(&p("", ""), &hf);
        assert_eq!((m.id, r), ("hf-soul", "720p"));
        // Only Recraft, which takes no image: nothing can paint, and the reason names Recraft.
        let rc = Keys { magnific: false, services: vec!["recraft"] };
        assert!(!rc.has(super::choice(&p("", ""), &rc).0));
        let why = magnific::no_fit(&rc, true, "16:9");
        assert!(why.starts_with("Recraft can't use reference images") && why.contains("Higgsfield") && !why.contains("Recraft,"), "{why}");
        assert!(magnific::no_fit(&rc, false, "16:9").contains("can't make 16:9"));
    }
}
