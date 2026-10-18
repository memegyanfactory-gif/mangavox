//! Live smoke tests against the real sites. Run with:
//! `cargo test -p mv-sources --test live -- --ignored --nocapture --test-threads=1`
//! They need network access and may fail when a site is down, rate limiting or walled.

use mv_net::Http;
use mv_sources::*;

fn ctx() -> SourceCtx {
    SourceCtx::new(Http::with_defaults().unwrap())
}

/// search -> details -> chapters -> pages for the first hit; prints what it found.
async fn smoke(src: &dyn Source, query: &str) {
    let id = src.info().id;
    let hits = src.search_manga(query, 1).await.unwrap_or_else(|e| panic!("{id} search: {e}"));
    assert!(!hits.is_empty(), "{id}: no search results for {query:?}");
    println!("{id}: {} hits, first = {:?}", hits.len(), hits[0]);
    let first = &hits[0];
    let d = src.get_manga_details(&first.source_id).await.unwrap_or_else(|e| panic!("{id} details: {e}"));
    assert!(!d.title.is_empty());
    println!("{id}: details title = {:?}, author = {:?}", d.title, d.author);
    let chapters = src.get_chapter_list(&first.source_id).await.unwrap_or_else(|e| panic!("{id} chapters: {e}"));
    assert!(!chapters.is_empty(), "{id}: no chapters");
    println!("{id}: {} chapters, first = {:?}", chapters.len(), chapters[0]);
    let pages = src.get_page_urls(&chapters[0].source_id).await.unwrap_or_else(|e| panic!("{id} pages: {e}"));
    assert!(!pages.is_empty() && pages[0].starts_with("http"), "{id}: bad pages {pages:?}");
    println!("{id}: {} pages, first = {}", pages.len(), pages[0]);
}

#[tokio::test]
#[ignore]
async fn live_mangadex() {
    let s = mangadex::MangaDex::new(ctx());
    // One Piece's English chapters are external-only; pick a series with hosted chapters.
    smoke(&s, "berserk").await;
    assert!(!s.get_popular_manga(1).await.unwrap().is_empty());
    assert!(!s.get_latest_manga(1).await.unwrap().is_empty());
    let tags = s.get_tag_list().await.unwrap();
    assert!(!s.get_manga_by_tag(&tags[0].id, 1).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_webtoon() {
    let s = webtoon::Webtoon::new(ctx());
    smoke(&s, "tower of god").await;
    assert!(!s.get_popular_manga(1).await.unwrap().is_empty());
    assert!(!s.get_latest_manga(1).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_ehentai() {
    let s = ehentai::EHentai::new(ctx());
    let hits = s.search_manga("touhou", 1).await.unwrap();
    assert!(!hits.is_empty());
    let id = &hits[0].source_id;
    let d = s.get_manga_details(id).await.unwrap();
    println!("{:?}", d.title);
    let ch = s.get_chapter_list(id).await.unwrap();
    assert_eq!(ch.len(), 1);
    // Only the first few viewer pages would be polite; resolving a whole gallery counts against the image quota.
    assert!(!s.get_popular_manga(1).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_hentai2read() {
    let s = hentai2read::Hentai2Read::new(ctx());
    smoke(&s, "love").await;
    assert!(!s.get_popular_manga(1).await.unwrap().is_empty());
    assert!(!s.get_tag_list().await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_nhentai() {
    let s = nhentai::NHentai::new(ctx());
    smoke(&s, "love").await;
    assert!(!s.get_popular_manga(1).await.unwrap().is_empty());
    assert!(!s.get_latest_manga(1).await.unwrap().is_empty());
    let tags = s.get_tag_list().await.unwrap();
    assert!(!s.get_manga_by_tag(&tags[0].id, 1).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn live_comick() {
    // ComicK sits behind Cloudflare: without a WallSolver this may report SourceError::Wall.
    let s = comick::Comick::new(ctx());
    smoke(&s, "one piece").await;
}

#[tokio::test]
#[ignore]
async fn live_mangafire_needs_solver() {
    let s = mangafire::MangaFire::new(ctx());
    assert!(matches!(s.search_manga("one piece", 1).await, Err(SourceError::SolverRequired(_))));
}
