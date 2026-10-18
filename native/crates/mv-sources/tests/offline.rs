//! Offline parser tests against real saved responses in tests/fixtures (see the README note at the
//! bottom of each section for how a fixture was captured). MangaFire's fixtures are hand-written
//! from the selectors of the old Electron scraper because the real site is a JS SPA that can
//! only be captured from a browser.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mv_net::{Http, NetConfig};
use mv_sources::util::{split_ref_hint, with_ref_hint};
use mv_sources::*;
use serde_json::{json, Value};

fn fx(n: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{n}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}
fn fj(n: &str) -> Value {
    serde_json::from_str(&fx(n)).unwrap()
}

// ── MangaDex (curl of api.mangadex.org: search "one piece", Berserk details/feed/at-home, /manga/tag)

#[test]
fn mangadex_search() {
    let r = mangadex::parse_manga_list(&fj("mangadex_search.json"));
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].source_id, "a1c7c817-4e59-43b7-9365-09675a149a6f");
    assert_eq!(r[0].title, "One Piece");
    assert!(r[0].author.as_deref().unwrap().starts_with("Oda"));
    assert_eq!(r[0].status.as_deref(), Some("ongoing"));
    let cover = r[0].cover_url.as_deref().unwrap();
    assert!(cover.starts_with("https://uploads.mangadex.org/covers/a1c7c817-4e59-43b7-9365-09675a149a6f/") && cover.ends_with(".256.jpg"));
}

#[test]
fn mangadex_details() {
    let d = mangadex::parse_details(&fj("mangadex_details.json")).unwrap();
    assert_eq!(d.source_id, "801513ba-a712-498c-8f57-cae55b38cc92");
    assert_eq!(d.title, "Berserk");
    assert_eq!(d.author.as_deref(), Some("Miura Kentarou"));
    assert_eq!(d.artist.as_deref(), Some("Miura Kentarou"));
    assert!(d.description.unwrap().starts_with("Guts, known as the Black Swordsman"));
    assert!(d.cover_url.is_some());
    assert_eq!(d.status.as_deref(), Some("ongoing"));
    let g = d.genres.unwrap();
    assert!(g.contains(&"Action".to_string()) && g.contains(&"Horror".to_string()));
    assert!(mangadex::parse_details(&json!({"result": "error"})).is_err());
}

#[test]
fn mangadex_chapters_dedupe_and_order() {
    let feed = fj("mangadex_feed.json");
    let list = mangadex::build_chapter_list(feed["data"].as_array().unwrap());
    assert_eq!(list.len(), 25);
    assert_eq!(list[0].number, Some(0.01));
    assert_eq!(list[0].title.as_deref(), Some("The Black Swordsman"));
    assert_eq!(list[0].scanlation_group.as_deref(), Some("Band of the Hawks"));
    assert_eq!(list[0].page_count, Some(94));
    assert!(list[0].published_at.as_deref().unwrap().starts_with("2018-01-31"));
    assert!(list.windows(2).all(|w| w[0].number <= w[1].number));

    // external / unavailable / zero-page chapters are dropped; duplicates keep the longest version
    let mk = |id: &str, ch: &str, pages: u64, ext: Option<&str>| {
        json!({"id": id, "attributes": {"chapter": ch, "pages": pages, "externalUrl": ext, "translatedLanguage": "en", "publishAt": "2020-01-01T00:00:00+00:00"}, "relationships": []})
    };
    let items = vec![
        mk("a", "1", 10, None),
        mk("b", "1", 20, None),
        mk("c", "2", 5, Some("https://mangaplus.example")),
        mk("d", "3", 0, None),
        json!({"id": "e", "attributes": {"chapter": null, "volume": "2", "pages": 3, "translatedLanguage": "en"}, "relationships": []}),
    ];
    let l = mangadex::build_chapter_list(&items);
    assert_eq!(l.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), ["b", "e"]);
    assert_eq!(l[1].number, None);
    assert_eq!(l[1].title.as_deref(), Some("Vol. 2 Oneshot"));
}

#[test]
fn mangadex_pages_and_tags() {
    let p = mangadex::parse_at_home(&fj("mangadex_athome.json"), None).unwrap();
    assert_eq!(p.len(), 94);
    assert!(p[0].starts_with("https://") && p[0].contains("/data/7dd9b16a7f83881121980b3bf685d5ff/x1-"));
    let ext = json!({"data": {"attributes": {"externalUrl": "https://mangaplus.shueisha.co.jp/x"}}});
    assert!(mangadex::parse_at_home(&fj("mangadex_athome.json"), Some(&ext)).is_err());
    // data-saver fallback
    let saver = json!({"baseUrl": "https://h", "chapter": {"hash": "H", "data": [], "dataSaver": ["a.jpg"]}});
    assert_eq!(mangadex::parse_at_home(&saver, None).unwrap(), ["https://h/data-saver/H/a.jpg"]);

    let tags = mangadex::parse_tags(&fj("mangadex_tags.json"));
    assert!(tags.len() > 30);
    assert!(tags.iter().any(|t| t.name == "Action"));
    assert!(tags.windows(2).all(|w| w[0].name <= w[1].name));
}

// ── WEBTOON (curl of webtoons.com: search "tower", dailySchedule, Tower of God list + viewer ep 1)

#[test]
fn webtoon_search_and_daily_cards() {
    let r = webtoon::parse_title_cards(&fx("webtoon_search.html"));
    assert!(r.len() >= 10);
    let tog = r.iter().find(|c| c.source_id == "fantasy|tower-of-god-urek-mazino|8136").expect("Tower of God");
    assert_eq!(tog.title, "Tower of God: Urek Mazino");
    assert_eq!(tog.author.as_deref(), Some("SIU"));
    let cover = tog.cover_url.as_deref().unwrap();
    assert!(cover.contains("Thumb_Poster_8136") && cover.ends_with("#mvref=https%3A%2F%2Fwww.webtoons.com%2F"));
    let mut ids: Vec<_> = r.iter().map(|c| c.source_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), r.len(), "cards are de-duplicated");

    let daily = webtoon::parse_title_cards(&fx("webtoon_daily.html"));
    assert!(daily.len() > 50);
    assert!(daily.iter().all(|c| !c.title.is_empty() && c.source_id.split('|').count() == 3));
}

#[test]
fn webtoon_details_and_chapters() {
    let id = "fantasy|tower-of-god-urek-mazino|8136";
    let d = webtoon::parse_details(&fx("webtoon_list.html"), id).unwrap();
    assert_eq!(d.title, "Tower of God: Urek Mazino");
    assert_eq!(d.author.as_deref(), Some("SIU"));
    assert_eq!(d.genres, Some(vec!["Fantasy".to_string()]));
    assert!(d.description.unwrap().contains("Urek Mazino"));
    assert!(d.cover_url.unwrap().contains("#mvref="));

    let eps = webtoon::parse_episodes(&fx("webtoon_list.html"), "fantasy", "tower-of-god-urek-mazino", "8136");
    assert_eq!(eps.len(), 10);
    assert_eq!(eps[0].source_id, "fantasy|tower-of-god-urek-mazino|8136|66");
    assert_eq!(eps[0].number, Some(66.0));
    assert!(eps[0].title.as_deref().unwrap().starts_with("Ep. 66"));
    assert_eq!(eps[0].scanlation_group.as_deref(), Some("WEBTOON"));
    assert_eq!(eps[0].published_at.as_deref(), Some("Oct 12, 2026"));
}

#[test]
fn webtoon_pages_keep_referer_hint() {
    let imgs = webtoon::parse_viewer_images(&fx("webtoon_viewer.html"));
    assert!(imgs.len() > 100);
    for u in &imgs {
        let (clean, referer) = split_ref_hint(u);
        assert!(clean.starts_with("https://webtoon-phinf.pstatic.net/"), "{clean}");
        assert_eq!(referer.as_deref(), Some("https://www.webtoons.com/"));
    }
}

// ── e-hentai (curl: ?f_search=touhou extended list, api.php gdata, gallery p=0, viewer page)

#[test]
fn ehentai_list() {
    let l = ehentai::parse_gallery_list(&fx("ehentai_search.html"));
    assert_eq!(l.len(), 25);
    assert_eq!(l[0].source_id, "4264201/d5e8732698");
    assert!(l[0].title.contains("早苗の妖力スポイラー"));
    assert_eq!(l[0].cover_url.as_deref(), Some("https://ehgt.org/w/02/726/75859-rtb0p10h.webp"));
    assert!(l.iter().all(|g| g.source_id.contains('/') && !g.title.is_empty()));
    let mut ids: Vec<_> = l.iter().map(|g| g.source_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 25);
}

#[test]
fn ehentai_details_and_chapter() {
    let g = ehentai::parse_gdata(&fx("ehentai_gdata.json")).unwrap();
    assert_eq!(g.filecount, "22");
    let d = ehentai::details_from_gdata(&g, "4264201/d5e8732698");
    assert_eq!(d.author.as_deref(), Some("pandain"));
    assert_eq!(d.status.as_deref(), Some("completed"));
    let desc = d.description.unwrap();
    assert!(desc.contains("Category: Doujinshi") && desc.contains("22 pages") && desc.contains("Uploader: Dynellen"));
    let genres = d.genres.unwrap();
    assert_eq!(genres[0], "Doujinshi");
    assert!(genres.contains(&"big breasts".to_string()) && !genres.iter().any(|g| g.contains("artist")));
    let ch = ehentai::chapter_from_gdata(&g, "4264201/d5e8732698");
    assert_eq!((ch.number, ch.page_count, ch.source_id.as_str()), (Some(1.0), Some(22), "4264201/d5e8732698"));

    assert!(ehentai::parse_gdata("{\"gmetadata\":[{\"gid\":1,\"error\":\"Key missing\"}]}").is_err());
    assert!(ehentai::parse_gdata("{}").is_err());
}

#[test]
fn ehentai_pages() {
    let links = ehentai::extract_viewer_links(&fx("ehentai_gallery.html"));
    assert_eq!(links.len(), 20);
    assert_eq!(links[0], "https://e-hentai.org/s/ca4071d936/4264201-1");
    match ehentai::parse_viewer_image(&fx("ehentai_viewer.html")) {
        ehentai::ViewerImage::Url(u) => assert!(u.starts_with("https://") && u.ends_with("13_001.webp")),
        other => panic!("{other:?}"),
    }
    assert_eq!(ehentai::parse_viewer_image("<html>You have exceeded your image viewing limits</html>"), ehentai::ViewerImage::LimitHit);
}

#[test]
fn ehentai_blocks_and_tag_queries() {
    assert!(ehentai::check_blocked(200, &fx("ehentai_search.html"), "u").is_ok());
    let banned = "<html>Your IP address has been temporarily banned. The ban expires in 59 minutes and 40 seconds</html>";
    assert!(ehentai::check_blocked(200, banned, "u").unwrap_err().to_string().contains("expires in 59 minutes and 40 seconds"));
    assert!(matches!(ehentai::check_blocked(404, "", "u"), Err(SourceError::NotFound(_))));
    assert!(ehentai::check_blocked(200, "x", "u").is_err()); // sad panda
    assert_eq!(ehentai::tag_query("female:big breasts"), "female:\"big breasts$\"");
    assert_eq!(ehentai::tag_query("category:artist cg"), "category:artist cg");
    assert_eq!(ehentai::tag_query("tag:vanilla"), "\"vanilla$\"");
    assert_eq!(ehentai::tag_query("full-color"), "\"full color$\"");
}

// ── Hentai2Read (curl of search?q=love, a book page, its reader page, /tags/category; Inertia JSON kept, ad props stripped)

#[test]
fn hentai2read_search() {
    let p = hentai2read::parse_props(&fx("h2r_search.html")).unwrap();
    let b = hentai2read::map_books(&p);
    assert_eq!(b.len(), 40);
    assert_eq!(b[0].source_id, "doujinshi/4967-chou-love-ru");
    assert_eq!(b[0].title, "Chou Love-ru");
    assert_eq!(b[0].cover_url.as_deref(), Some("https://hentaicdn.com/hentai/cover/_S4967.jpg"));
}

#[test]
fn hentai2read_details_chapters_pages_tags() {
    let p = hentai2read::parse_props(&fx("h2r_book.html")).unwrap();
    let id = "doujinshi/4967-chou-love-ru";
    let d = hentai2read::parse_details(&p, id).unwrap();
    assert_eq!(d.title, "Chou Love-ru");
    assert_eq!(d.author.as_deref(), Some("Tsunken (Mens)"));
    assert_eq!(d.status.as_deref(), Some("ongoing"));
    assert!(d.genres.unwrap().contains(&"Romance".to_string()));

    let ch = hentai2read::parse_chapters(&p, id);
    assert_eq!(ch.len(), 7);
    assert_eq!(ch[0].number, Some(1.0));
    assert_eq!(ch[0].source_id, "reader/4967-chou-love-ru/6287");
    assert_eq!(ch[0].title.as_deref(), Some("Chapter 1: Chou Love-ru Black"));
    assert_eq!(ch[6].source_id, "reader/4967-chou-love-ru/22288");
    assert!(ch.windows(2).all(|w| w[0].number <= w[1].number));

    let r = hentai2read::parse_props(&fx("h2r_reader.html")).unwrap();
    let imgs = hentai2read::parse_images(&r);
    assert_eq!(imgs.len(), 32);
    assert_eq!(imgs[0], "https://hentaicdn.com/hentai/4967/1/000.jpg");

    let t = hentai2read::parse_tags(&hentai2read::parse_props(&fx("h2r_tags.html")).unwrap());
    assert_eq!(t.len(), 10);
    assert_eq!(t[0], Tag { id: "category/18-adult".into(), name: "Adult".into() });
}

#[test]
fn hentai2read_error_pages() {
    assert!(hentai2read::parse_props("<html>nothing</html>").is_err());
    let err = "<script data-page=\"app\" type=\"application/json\">{\"component\":\"Error\",\"props\":{\"title\":\"Not Found\"}}</script>";
    assert!(hentai2read::parse_props(err).unwrap_err().to_string().contains("Not Found"));
}

// ── ComicK (curl of api.comick.dev: search, comic, chapters; comick.live sv-data page; trimmed)

#[test]
fn comick_search_details() {
    let l = comick::parse_comic_list(&fj("comick_search.json"));
    assert_eq!(l.len(), 5);
    assert_eq!((l[0].source_id.as_str(), l[0].title.as_str()), ("CzcseUMi", "One Piece"));
    assert_eq!(l[0].cover_url.as_deref(), Some("https://meo.comick.pictures/3MzEO.png"));
    assert_eq!(comick::parse_comic_list(&json!({"comics": [{"hid": "x", "title": "T"}]}))[0].source_id, "x");

    let d = comick::parse_details(&fj("comick_details.json"), "CzcseUMi");
    assert_eq!(d.title, "One Piece");
    assert_eq!(d.author.as_deref(), Some("Oda Eiichiro"));
    assert_eq!(d.status.as_deref(), Some("ongoing"));
    assert!(d.genres.unwrap().contains(&"Adventure".to_string()));
    assert!(d.description.unwrap().starts_with("From Viz:"));
}

#[test]
fn comick_chapters_and_pages() {
    let raw = fj("comick_chapters.json");
    let ch = comick::build_chapter_list(raw["chapters"].as_array().unwrap());
    // chapter 1 has an official and a scan release: the readable group wins; extras come last
    assert_eq!(ch[0].number, Some(1.0));
    assert_eq!(ch[0].source_id, "mGL9");
    assert_ne!(ch[0].scanlation_group.as_deref(), Some("Official"));
    assert!(ch.iter().filter(|c| c.number == Some(1.0)).count() == 1);
    let extras: Vec<_> = ch.iter().filter(|c| c.number.is_none()).collect();
    assert_eq!(extras.len(), 3);
    assert_eq!(extras[0].title.as_deref(), Some("Vol. 1 Extra"));
    assert_eq!(ch.last().unwrap().number, None);

    let imgs = comick::parse_sv_data_images(&fx("comick_page.html"));
    assert_eq!(imgs.len(), 5);
    assert!(imgs[0].starts_with("https://cdn1.comicknew.pictures/02-one-piece/"));
    assert!(comick::parse_sv_data_images("<html></html>").is_empty());

    let (chap, lang, api) = comick::parse_chapter_record(&json!({"chapter": {"chap": "7.5", "lang": "es", "md_images": [{"b2key": "a.jpg"}]}}));
    assert_eq!((chap.as_str(), lang.as_str()), ("7.5", "es"));
    assert_eq!(api, ["https://meo.comick.pictures/a.jpg"]);
}

// ── nhentai API v2 (curl of /search, /galleries/688719, /galleries/popular, /tags/tag)

#[test]
fn nhentai_search_details_pages() {
    let l = nhentai::parse_list(&fj("nhentai_search.json"));
    assert_eq!(l.len(), 25);
    assert_eq!(l[0].source_id, "688720");
    assert!(l[0].title.starts_with("(C94)"));
    assert_eq!(l[0].cover_url.as_deref(), Some("https://t1.nhentai.net/galleries/4249008/thumb.webp"));
    assert_eq!(nhentai::parse_list(&fj("nhentai_popular.json")).len(), 5);

    let g = fj("nhentai_gallery.json");
    let d = nhentai::parse_details(&g, "688719");
    assert_eq!(d.title, "[BEAT-POP (Ozaki Miray)] Nearest to Real LOVE 8 [English] [Digital]");
    assert_eq!(d.author.as_deref(), Some("ozaki miray"));
    let genres = d.genres.unwrap();
    assert!(genres.contains(&"netorare".to_string()) && !genres.contains(&"english".to_string()));
    assert!(d.description.unwrap().ends_with("Pages: 34"));
    let ch = nhentai::parse_chapter(&g, "688719");
    assert_eq!((ch.number, ch.page_count), (Some(1.0), Some(34)));

    let pages = nhentai::parse_pages(&g);
    assert_eq!(pages.len(), 34);
    assert_eq!(pages[0], "https://i1.nhentai.net/galleries/4248996/1.webp");
    assert!(pages[33].ends_with("/34.webp") || pages[33].contains("/34."));

    let tags = nhentai::parse_tags(&fj("nhentai_tags.json"));
    assert_eq!(tags.len(), 12);
    assert_eq!(tags[0], Tag { id: "2937".into(), name: "big breasts".into() });
}

// ── MangaFire (hand-written DOM fixtures; see header)

#[test]
fn mangafire_parsers() {
    let cards = mangafire::parse_cards(&fx("mangafire_browse.html"));
    assert_eq!(cards.len(), 2, "chapter links and duplicates are skipped");
    assert_eq!(cards[0].source_id, "abcde-one-piece");
    assert_eq!(cards[0].title, "One Piece");
    assert_eq!(cards[0].cover_url.as_deref(), Some("https://static.mfcdn.example/covers/one-piece.jpg"));
    assert_eq!(cards[1].cover_url.as_deref(), Some("https://static.mfcdn.example/covers/berserk.jpg"));

    let d = mangafire::parse_details(&fx("mangafire_title.html"), "abcde-one-piece").unwrap();
    assert_eq!(d.title, "One Piece");
    assert_eq!(d.author.as_deref(), Some("Oda, Eiichiro"));
    assert_eq!(d.status.as_deref(), Some("ongoing"));
    assert_eq!(d.description.as_deref(), Some("Pirates.\nMore pirates."));
    assert_eq!(d.genres, Some(vec!["Action".to_string(), "Adventure".to_string()]));

    assert_eq!(mangafire::parse_reader_images(&fx("mangafire_reader.html")), ["https://static.mfcdn.example/p/1.jpg", "https://static.mfcdn.example/p/2.jpg"]);

    let items = vec![
        json!({"id": 11, "number": "2", "name": "", "createdAt": 1_700_000_000}),
        json!({"id": 10, "number": 1, "name": "Start", "createdAt": 1_600_000_000}),
        json!({"id": 12, "number": "2", "name": "dup"}),
    ];
    let ch = mangafire::parse_chapter_items("abcde-one-piece", &items);
    assert_eq!(ch.len(), 2);
    assert_eq!(ch[0].source_id, "abcde-one-piece/chapter/10");
    assert_eq!(ch[0].title.as_deref(), Some("Ch. 1: Start"));
    assert_eq!(ch[0].published_at.as_deref(), Some("2020-09-13T12:26:40Z"));
    assert_eq!(ch[1].title.as_deref(), Some("Ch. 2"));

    assert_eq!(mangafire::browse_path("views_total:desc", 2, Some("one piece"), Some("1")), "/browse?keyword=one%20piece&sort=views_total%3Adesc&genres_in=1&page=2");
    assert_eq!(mangafire::slug("Slice of Life"), "slice-of-life");
    assert!(mangafire::api_chapters_js("abc").contains("'/titles/' + \"abc\" + '/chapters'"));
}

// ── Source plumbing: registry, WallSolver hand-off ────────────────────

fn ctx() -> SourceCtx {
    SourceCtx::new(Http::new(NetConfig { default_host_interval: std::time::Duration::ZERO, backoff_base: std::time::Duration::from_millis(5), ..Default::default() }).unwrap())
}

#[test]
fn registry_lists_builtin_sources() {
    let reg = SourceRegistry::builtin(ctx());
    let list = reg.list();
    let ids: Vec<_> = list.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["mangadex", "comick", "mangafire", "webtoons", "ehentai", "hentai2read", "nhentai"]);
    let nsfw: Vec<_> = list.iter().filter(|s| s.nsfw).map(|s| s.id.as_str()).collect();
    assert_eq!(nsfw, ["ehentai", "hentai2read", "nhentai"]);
    assert!(list.iter().all(|s| s.lang == "en" && !s.name.is_empty() && s.icon.is_some()));
    assert_eq!(list.iter().filter(|s| s.requires_wall_solver).count(), 1);
    assert!(reg.get("webtoons").is_some() && reg.get("nope").is_none());
    // SourceInfo serializes with the snake_case fields the UI uses
    let v = serde_json::to_value(&list[0]).unwrap();
    assert_eq!(v["id"], "mangadex");
    assert_eq!(v["nsfw"], false);
}

#[test]
fn types_serialize_like_the_typescript_interfaces() {
    let r = MangaSearchResult { source_id: "1".into(), title: "T".into(), ..Default::default() };
    assert_eq!(serde_json::to_value(&r).unwrap(), json!({"source_id": "1", "title": "T"}));
    let c = ChapterInfo { source_id: "c".into(), number: None, language: "en".into(), ..Default::default() };
    assert_eq!(serde_json::to_value(&c).unwrap(), json!({"source_id": "c", "number": null, "language": "en"}));
    let back: MangaDetails = serde_json::from_value(json!({"source_id": "x", "title": "y", "genres": ["a"]})).unwrap();
    assert_eq!(back.genres, Some(vec!["a".to_string()]));
    assert_eq!(split_ref_hint(&with_ref_hint("https://a/b", "https://r/")).1.as_deref(), Some("https://r/"));
}

/// Records calls and serves canned content, standing in for the WebView solver.
#[derive(Default)]
struct FakeSolver {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl WallSolver for FakeSolver {
    async fn cookies(&self, url: &str) -> Result<Vec<(String, String)>> {
        self.calls.lock().unwrap().push(format!("cookies {url}"));
        Err(SourceError::Solver("no cookies".into()))
    }
    async fn fetch_text(&self, url: &str, _h: &[(String, String)]) -> Result<String> {
        self.calls.lock().unwrap().push(format!("fetch_text {url}"));
        Ok(fx("comick_search.json"))
    }
    async fn fetch_html(&self, url: &str, wait_for: Option<&str>) -> Result<String> {
        self.calls.lock().unwrap().push(format!("fetch_html {url} wait={}", wait_for.unwrap_or("")));
        Ok(if url.contains("/chapter/") {
            fx("mangafire_reader.html")
        } else if url.contains("/title/") {
            fx("mangafire_title.html")
        } else {
            fx("mangafire_browse.html")
        })
    }
    async fn eval_json(&self, page_url: &str, script: &str) -> Result<Value> {
        self.calls.lock().unwrap().push(format!("eval_json {page_url}"));
        assert!(script.contains("\"abcde\""), "hid is the part before the first dash");
        Ok(json!({"items": [{"id": 1, "number": "1", "name": "One", "createdAt": 1_600_000_000}], "total": 1}))
    }
}

#[tokio::test]
async fn mangafire_works_through_a_solver_and_errors_without_one() {
    let none = mangafire::MangaFire::new(ctx());
    assert!(matches!(none.search_manga("x", 1).await, Err(SourceError::SolverRequired(_))));
    assert!(matches!(none.get_chapter_list("abcde-one-piece").await, Err(SourceError::SolverRequired(_))));

    let solver = Arc::new(FakeSolver::default());
    let mf = mangafire::MangaFire::new(ctx().with_solver(solver.clone()));
    let hits = mf.search_manga("one piece", 1).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(mf.get_manga_details("abcde-one-piece").await.unwrap().title, "One Piece");
    let ch = mf.get_chapter_list("abcde-one-piece").await.unwrap();
    assert_eq!(ch[0].source_id, "abcde-one-piece/chapter/1");
    assert_eq!(mf.get_page_urls("abcde-one-piece/chapter/1").await.unwrap().len(), 2);
    let tags = mf.get_tag_list().await.unwrap();
    assert!(tags.iter().any(|t| t.id == "slice-of-life"));
    let calls = solver.calls.lock().unwrap().clone();
    assert!(calls[0].starts_with("fetch_html https://mangafire.to/browse?keyword=one%20piece") && calls[0].contains("wait=a[href^=\"/title/\"] img"));
    assert!(calls.iter().any(|c| c.starts_with("eval_json https://mangafire.to/title/abcde-one-piece")));
}

/// Serve one canned response per connection on localhost.
async fn serve_once(status: u16, content_type: &'static str, body: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { break };
            let body = body.clone();
            tokio::spawn(async move {
                let mut b = vec![0u8; 4096];
                let _ = s.read(&mut b).await;
                let r = format!("HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    format!("http://{addr}/x")
}

#[tokio::test]
async fn walled_response_falls_back_to_the_solver_or_reports_a_wall() {
    let wall = "<html><head><title>Just a moment...</title></head><body>Checking your browser</body></html>".to_string();
    let url = serve_once(403, "text/html", wall).await;

    // No solver: a typed Wall error.
    let err = ctx().fetch("comick", &url, &Default::default()).await.unwrap_err();
    assert!(matches!(err, SourceError::Wall { ref kind, .. } if kind == "cloudflare"), "{err}");

    // With a solver: cookies fail, so the in-browser fetch result is used.
    let solver = Arc::new(FakeSolver::default());
    let c = ctx().with_solver(solver.clone());
    let list: Value = c.fetch_json("comick", &url, &Default::default()).await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 5);
    let calls = solver.calls.lock().unwrap().clone();
    assert_eq!(calls, [format!("cookies {url}"), format!("fetch_text {url}")]);

    // A plain 404 is not a wall: the response comes back untouched.
    let nf = serve_once(404, "text/plain", "nope".into()).await;
    assert_eq!(c.fetch("x", &nf, &Default::default()).await.unwrap().status, 404);
    assert_eq!(solver.calls.lock().unwrap().len(), 2);
}
