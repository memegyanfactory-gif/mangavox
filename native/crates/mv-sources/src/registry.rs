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

/// The built-in source that reads the site at `url` (any scheme, `www.` or path), if there is
/// one. A repository extension for such a site is served by the built-in source instead of a
/// generated scraper, which cannot read API-driven sites like MangaDex.
pub fn builtin_for_host(url: &str) -> Option<&'static str> {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let host = rest.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    Some(match () {
        _ if is("mangadex.org") => mangadex::ID,
        _ if host.starts_with("comick.") => comick::ID,
        _ if is("mangafire.to") => mangafire::ID,
        _ if is("webtoons.com") => webtoon::ID,
        _ if is("e-hentai.org") || is("exhentai.org") => ehentai::ID,
        _ if is("hentai2read.com") => hentai2read::ID,
        _ if is("nhentai.net") => nhentai::ID,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::builtin_for_host;

    #[test]
    fn repository_sites_map_to_built_in_sources() {
        assert_eq!(builtin_for_host("https://mangadex.org"), Some("mangadex"));
        assert_eq!(builtin_for_host("https://www.webtoons.com/en/"), Some("webtoons"));
        assert_eq!(builtin_for_host("http://comick.io"), Some("comick"));
        assert_eq!(builtin_for_host("https://hentaifox.com"), None);
        assert_eq!(builtin_for_host("https://notmangadex.org"), None);
    }
}
