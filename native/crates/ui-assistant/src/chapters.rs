//! Chapter lists as the model passes them ("12", "12-15", "Ch. 3.5", an id, "all"), matched
//! strictly: every spec must name a chapter that exists, so a tool never acts on more than the
//! user asked for. Shared by the Studio and manga tools.

#[derive(Clone, Debug, PartialEq)]
pub enum Spec {
    All,
    Number(f64),
    Range(f64, f64),
    Id(String),
}

pub fn parse(raw: &[String]) -> Vec<Spec> {
    raw.iter()
        .map(|r| {
            let r = r.trim();
            if r.eq_ignore_ascii_case("all") {
                return Spec::All;
            }
            // "Ch. 12", "chapter 12" → "12".
            let t = r.trim_start_matches(|c: char| c.is_alphabetic() || c == '.' || c == ' ').trim();
            let t = if t.is_empty() { r } else { t };
            if let Some((a, b)) = t.split_once(['-', '–']) {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
                    return Spec::Range(a.min(b), a.max(b));
                }
            }
            match t.parse::<f64>() {
                Ok(n) => Spec::Number(n),
                Err(_) => Spec::Id(r.to_string()),
            }
        })
        .collect()
}

fn matches(spec: &Spec, id: &str, number: Option<f64>) -> bool {
    match spec {
        Spec::All => true,
        Spec::Number(n) => number.is_some_and(|x| (x - n).abs() < 1e-6),
        Spec::Range(a, b) => number.is_some_and(|x| x >= a - 1e-6 && x <= b + 1e-6),
        Spec::Id(s) => id == s,
    }
}

/// Indexes of `items` (`(id, number)`) the specs name, in the items' order. `Err` holds the specs
/// that matched nothing (then nothing should run).
pub fn pick(items: &[(&str, Option<f64>)], raw: &[String]) -> Result<Vec<usize>, Vec<String>> {
    let specs = parse(raw);
    let mut keep = vec![false; items.len()];
    let mut missing = vec![];
    for (spec, r) in specs.iter().zip(raw) {
        let mut hit = false;
        for (i, (id, n)) in items.iter().enumerate() {
            if matches(spec, id, *n) {
                keep[i] = true;
                hit = true;
            }
        }
        if !hit {
            missing.push(r.trim().to_string());
        }
    }
    if !missing.is_empty() {
        return Err(missing);
    }
    Ok(keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect())
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// "1-3, 5, 7.5": chapter numbers as short ranges.
pub fn compress(nums: &[f64]) -> String {
    let mut v: Vec<f64> = nums.to_vec();
    v.sort_by(f64::total_cmp);
    v.dedup();
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && start.fract() == 0.0 && v[i + 1] == end + 1.0 {
            i += 1;
            end = v[i];
        }
        out.push(if end > start { format!("{}-{}", fmt_num(start), fmt_num(end)) } else { fmt_num(start) });
        i += 1;
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse() {
        let raw: Vec<String> = ["12", "12-15", "15–12", "chapter 3.5", "abc-uuid", "ALL", "Ch. 7"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse(&raw), vec![Spec::Number(12.), Spec::Range(12., 15.), Spec::Range(12., 15.), Spec::Number(3.5), Spec::Id("abc-uuid".into()), Spec::All, Spec::Number(7.)]);
    }

    #[test]
    fn pick_is_strict() {
        let items = [("a", Some(1.)), ("b", Some(2.)), ("c", Some(3.)), ("d", None)];
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(pick(&items, &s(&["3", "1"])).unwrap(), [0, 2]);
        assert_eq!(pick(&items, &s(&["2-3", "d"])).unwrap(), [1, 2, 3]);
        assert_eq!(pick(&items, &s(&["all"])).unwrap(), [0, 1, 2, 3]);
        assert_eq!(pick(&items, &s(&["2", "9", "x"])).unwrap_err(), ["9", "x"]);
        assert_eq!(pick(&items, &[]).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn numbers_compress_to_ranges() {
        assert_eq!(compress(&[3., 1., 2., 5., 7.5, 8., 9.]), "1-3, 5, 7.5, 8-9");
        assert_eq!(compress(&[]), "");
    }
}
