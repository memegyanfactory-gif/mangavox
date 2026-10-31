//! Small shared helpers for the scrapers.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use scraper::{ElementRef, Html, Selector};

/// Same character set as JavaScript's `encodeURIComponent`.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// `encodeURIComponent`.
pub fn encode_component(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

fn decode_component(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Append the `#mvref=<referer>` hint the cover proxy and download manager read to inject a
/// per-URL `Referer` header. Existing fragments get `&mvref=...`.
pub fn with_ref_hint(url: &str, referer: &str) -> String {
    let hint = format!("mvref={}", encode_component(referer));
    if url.contains("mvref=") {
        url.to_string()
    } else if url.contains('#') {
        format!("{url}&{hint}")
    } else {
        format!("{url}#{hint}")
    }
}

/// Inverse of [`with_ref_hint`]: `(clean_url, referer)`.
pub fn split_ref_hint(url: &str) -> (String, Option<String>) {
    let Some((base, frag)) = url.split_once('#') else { return (url.to_string(), None) };
    let referer = frag
        .split('&')
        .find_map(|p| p.strip_prefix("mvref="))
        .map(decode_component);
    (base.to_string(), referer)
}

/// `data:image/svg+xml,...` badge icon like the Electron plugins use.
pub fn svg_icon(color: &str, label: &str, font_size: u32) -> String {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 32 32\"><rect width=\"32\" height=\"32\" rx=\"6\" fill=\"{color}\"/><text x=\"16\" y=\"22\" font-family=\"sans-serif\" font-weight=\"bold\" font-size=\"{font_size}\" fill=\"white\" text-anchor=\"middle\">{label}</text></svg>"
    );
    format!("data:image/svg+xml,{}", encode_component(&svg))
}

/// Resolve `//host/x` and `/x` against `base`; only http(s) results are returned.
pub fn abs_url(base: &str, url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        None
    } else if let Some(rest) = url.strip_prefix("//") {
        Some(format!("https://{rest}"))
    } else if url.starts_with('/') {
        Some(format!("{}{}", base.trim_end_matches('/'), url))
    } else if url.starts_with("http") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Decode the HTML entities that show up in JSON API strings (`&#039;`, `&amp;`, ...).
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(semi) = tail[..tail.len().min(12)].find(';') {
            let ent = &tail[1..semi];
            let decoded = match ent {
                "quot" => Some('"'),
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                    .and_then(char::from_u32),
            };
            if let Some(c) = decoded {
                out.push(c);
                rest = &tail[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

pub fn sel(css: &str) -> Selector {
    Selector::parse(css).unwrap_or_else(|e| panic!("bad selector {css:?}: {e:?}"))
}

/// Collapsed text content of an element.
pub fn text_of(el: ElementRef<'_>) -> String {
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapsed text of an element, ignoring everything inside `skip_tag` descendants.
pub fn text_excluding(el: ElementRef<'_>, skip_tag: &str) -> String {
    fn walk(el: ElementRef<'_>, skip: &str, out: &mut String) {
        for c in el.children() {
            if let Some(t) = c.value().as_text() {
                out.push_str(t);
                out.push(' ');
            } else if let Some(ce) = ElementRef::wrap(c) {
                if ce.value().name() != skip {
                    walk(ce, skip, out);
                }
            }
        }
    }
    let mut s = String::new();
    walk(el, skip_tag, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn first_text(doc: &Html, css: &str) -> Option<String> {
    doc.select(&sel(css)).next().map(text_of).filter(|s| !s.is_empty())
}

pub fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_hint_roundtrip_matches_js_encoding() {
        let u = with_ref_hint("https://x.test/a.jpg", "https://www.webtoons.com/");
        assert_eq!(u, "https://x.test/a.jpg#mvref=https%3A%2F%2Fwww.webtoons.com%2F");
        assert_eq!(split_ref_hint(&u), ("https://x.test/a.jpg".into(), Some("https://www.webtoons.com/".into())));
        assert_eq!(split_ref_hint("https://x.test/a.jpg"), ("https://x.test/a.jpg".into(), None));
        assert!(with_ref_hint("https://x/a#f=1", "https://r/").contains("#f=1&mvref="));
    }

    #[test]
    fn people_and_creator_match() {
        assert_eq!(split_people("ONE, Murata Yusuke & one / Unknown"), ["ONE", "Murata Yusuke"]);
        assert!(creator_matches("Murata Yusuke, ONE", "one"));
        assert!(creator_matches("Yusuke Murata", "Murata Yusuke"));
        assert!(!creator_matches("Murata Yusuke", "Murata"));
        assert!(!creator_matches("", "x"));
    }

    #[test]
    fn entities() {
        assert_eq!(decode_entities("Tom &amp; Jerry &#039;s &#x41; &lt;b&gt; &bogus; &"), "Tom & Jerry 's A <b> &bogus; &");
    }

    #[test]
    fn urls() {
        assert_eq!(abs_url("https://a.test", "//b.test/x").as_deref(), Some("https://b.test/x"));
        assert_eq!(abs_url("https://a.test/", "/x").as_deref(), Some("https://a.test/x"));
        assert_eq!(abs_url("https://a.test", "x.jpg"), None);
    }
}

/// The people in an author / artist field ("A, B & C" -> three names), placeholders dropped.
pub fn split_people(field: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in field.split([',', '&', '/', ';', '\u{3001}']) {
        let p = p.trim();
        let key = title_key(p);
        if key.is_empty() || ["unknown", "n a", "na", "none", "various"].contains(&key.as_str()) || out.iter().any(|o| title_key(o) == key) {
            continue;
        }
        out.push(p.to_string());
    }
    out
}

/// Whether an author / artist field names `name` (any of its people, same words in any order).
pub fn creator_matches(field: &str, name: &str) -> bool {
    let words = |s: &str| {
        let mut w: Vec<String> = title_key(s).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect();
        w.sort();
        w
    };
    let want = words(name);
    !want.is_empty() && split_people(field).iter().any(|p| words(p) == want)
}

/// Lowercase words only (punctuation dropped), for comparing titles.
pub fn title_key(s: &str) -> String {
    s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How alike two titles are: 1.0 for the same words, 0.9 when one contains the other (6+
/// characters), otherwise the share of words they have in common.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (title_key(a), title_key(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if (a.len() >= 6 && b.contains(&a)) || (b.len() >= 6 && a.contains(&b)) {
        return 0.9;
    }
    let wa: std::collections::HashSet<&str> = a.split(' ').collect();
    let wb: std::collections::HashSet<&str> = b.split(' ').collect();
    wa.intersection(&wb).count() as f64 / wa.len().max(wb.len()) as f64
}

/// Latin-script text (English and romanized titles, not Korean, Japanese, Thai...).
pub fn is_latin(s: &str) -> bool {
    s.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c))
}
