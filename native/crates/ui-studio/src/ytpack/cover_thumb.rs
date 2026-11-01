//! The one AI image per manga that every Studio video thumbnail of it is drawn on (see
//! `mv_recap::thumb`): painted from the manga's cover with Magnific, using the saved key and the
//! default model and size from Settings › Magnific. Registered at startup so the reader can ask
//! for it; it only ever runs when asked (never in a loop) and records the credits it spends.
use futures::FutureExt as _;
use mv_recap::thumb::{self, PaintJob, Painter};

use super::magnific::{self, ImageModel, Prefs, Usage, CATALOG};
use super::thumbs::{Gallery, Thumb};

pub(crate) fn register() {
    thumb::set_painter(Painter { ready: || magnific::key().is_some(), cost, paint });
}

/// The default model when it takes a reference image (the cover is one), else Nano Banana 2.1;
/// the default size when that model offers it, else its smallest.
fn choice(prefs: &Prefs) -> (&'static ImageModel, &'static str) {
    let m = CATALOG.iter().find(|m| m.id == prefs.model && m.refs > 0).unwrap_or(&CATALOG[0]);
    let res = m.resolutions.iter().copied().find(|r| *r == prefs.resolution).or_else(|| m.resolutions.first().copied()).unwrap_or("");
    (m, res)
}

fn cost() -> f64 {
    let (m, res) = choice(&Prefs::load());
    m.cost_at(res)
}

fn paint(job: PaintJob) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
    async move {
        let key = magnific::key().ok_or_else(|| anyhow::anyhow!("Add a Magnific API key in Settings › Magnific"))?;
        let (model, res) = choice(&Prefs::load());
        let refs = [magnific::Reference { file: job.cover.clone(), caption: "Image 1: the manga's cover".into() }];
        magnific::generate(model, &key, &thumb::cover_prompt(&job.title), &refs, "16:9", res, &job.out, |_| {}).await?;
        let credits = model.cost_at(res);
        let mut usage = Usage::load();
        usage.add(credits);
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
    }
}
