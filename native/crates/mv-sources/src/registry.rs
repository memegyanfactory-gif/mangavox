use std::sync::Arc;

use crate::source::{Source, SourceCtx};
use crate::types::SourceInfo;
use crate::{comick, ehentai, hentai2read, mangadex, mangafire, nhentai, webtoon};

/// Lookup table of sources by id, in display order.
#[derive(Default, Clone)]
pub struct SourceRegistry {
    sources: Vec<Arc<dyn Source>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// All seven built-in sources sharing one [`SourceCtx`].
    pub fn builtin(ctx: SourceCtx) -> Self {
        let mut r = Self::new();
        r.register(Arc::new(mangadex::MangaDex::new(ctx.clone())));
        r.register(Arc::new(comick::Comick::new(ctx.clone())));
        r.register(Arc::new(mangafire::MangaFire::new(ctx.clone())));
        r.register(Arc::new(webtoon::Webtoon::new(ctx.clone())));
        r.register(Arc::new(ehentai::EHentai::new(ctx.clone())));
        r.register(Arc::new(hentai2read::Hentai2Read::new(ctx.clone())));
        r.register(Arc::new(nhentai::NHentai::new(ctx)));
        r
    }

    /// Add (or replace, by id) a source.
    pub fn register(&mut self, source: Arc<dyn Source>) {
        let id = source.info().id;
        if let Some(slot) = self.sources.iter_mut().find(|s| s.info().id == id) {
            *slot = source;
        } else {
            self.sources.push(source);
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Source>> {
        self.sources.iter().find(|s| s.info().id == id).cloned()
    }

    /// id / name / lang / nsfw (and more) of every registered source.
    pub fn list(&self) -> Vec<SourceInfo> {
        self.sources.iter().map(|s| s.info()).collect()
    }
}
