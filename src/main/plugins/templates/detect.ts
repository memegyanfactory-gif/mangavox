/**
 * Engine (Mihon multisrc theme) detection from a site's home page HTML.
 *
 * DETECT_JS is embedded into generated plugins for lazy runtime (re)detection;
 * detectEngineFromHtml() evaluates the very same code on the host side for
 * install-time detection, so both paths always agree.
 */
export const ENGINES = ['madara', 'mangathemesia', 'mangabox', 'foolslide', 'heancms', 'keyoapp', 'generic'] as const
export type EngineId = (typeof ENGINES)[number]

export const DETECT_JS = String.raw`
function detectEngine(html) {
  var h = String(html || '');
  // NB: no bare "wp-manga" — search pages echo our own ?post_type=wp-manga query.
  if (/wp-content\/themes\/madara|madara-core|class=["'][^"']*\bwp-manga-|manga_get_chapters|madara_load_more|c-tabs-item__content|page-item-detail|class=["'][^"']*\bc-blog__heading|Powered by Madara/i.test(h)) return 'madara';
  if (/ts_reader|mangathemesia|wp-content\/themes\/mangareader|wp-content\/themes\/mangastream|wp-content\/themes\/kiryuu|class=["'][^"']*\bbsx\b|class=["'][^"']*\blistupd\b|class=["'][^"']*\bbixbox\b|id=["']chapterlist["']/i.test(h)) return 'mangathemesia';
  // Structural markers only — brand names show up in unrelated sites' FAQ/SEO text.
  if (/class=["'][^"']*\b(itemupdate|xem-nhieu-item|slide-caption|list-story-item)\b[\s\S]{0,4000}?class=["'][^"']*\b(itemupdate|xem-nhieu-item|list-story-item)\b|panel-story-info|list-truyen-item-wrap|content-genres-item|panel-content-genres|list-comic-item-wrap|container-chapter-reader|panel-search-story|search-story-item|story_item_right/i.test(h)) return 'mangabox';
  if (/FoOlSlide|\/foolslide\/|fs-reader|\/content\/themes\/default\/style\.css/.test(h)) return 'foolslide';
  if (/href=["'](?:https?:\/\/[^"']+)?\/series\/\?genre=/i.test(h) && /\/assets\/images\/|realUrl\s*=|id=["']chapters["']/i.test(h)) return 'keyoapp';
  if (/heancms|hean-cms|api\.[a-z0-9-]+\.[a-z]+\/(query|series|chapter)|"series_slug"|series_slug\\?"/i.test(h)) return 'heancms';
  return 'generic';
}
function detectCanonicalBase(html) {
  var h = String(html || '');
  var m = /<link\b[^>]*rel=["']canonical["'][^>]*href=["'](https?:\/\/[^\/"']+)/i.exec(h) ||
    /<link\b[^>]*href=["'](https?:\/\/[^\/"']+)[^>]*rel=["']canonical["']/i.exec(h) ||
    /<meta\b[^>]*property=["']og:url["'][^>]*content=["'](https?:\/\/[^\/"']+)/i.exec(h);
  return m ? m[1] : '';
}
`

let detectFn: ((html: string) => string) | null = null

/** Detect the site engine from home-page HTML (host side). */
export function detectEngineFromHtml(html: string): EngineId {
  if (!detectFn) {
    // eslint-disable-next-line no-new-func
    detectFn = new Function(DETECT_JS + '\nreturn detectEngine;')() as (html: string) => string
  }
  const e = detectFn(html)
  return (ENGINES as readonly string[]).includes(e) ? (e as EngineId) : 'generic'
}
