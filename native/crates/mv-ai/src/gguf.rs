//! GGUF file-name helpers (port of src/shared/gguf-files.ts).

use crate::types::HubModelFile;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

const QUANT_BITS: &[(&str, f64)] = &[
    ("Q2_K", 2.6), ("Q3_K_S", 3.5), ("Q3_K_M", 3.9), ("Q3_K_L", 4.3), ("Q4_0", 4.5), ("Q4_1", 5.0),
    ("Q4_K_S", 4.6), ("Q4_K_M", 4.8), ("Q4_K_XL", 5.0), ("Q5_0", 5.5), ("Q5_1", 6.0), ("Q5_K_S", 5.5),
    ("Q5_K_M", 5.7), ("Q6_K", 6.6), ("Q8_0", 8.5), ("F16", 16.0), ("BF16", 16.0), ("F32", 32.0),
    ("MXFP4", 4.25), ("MXFP4_MOE", 4.25), ("IQ1_S", 1.6), ("IQ1_M", 1.8), ("IQ2_XXS", 2.1), ("IQ2_XS", 2.3),
    ("IQ2_S", 2.5), ("IQ2_M", 2.7), ("IQ3_XXS", 3.1), ("IQ3_XS", 3.3), ("IQ3_S", 3.4), ("IQ3_M", 3.7),
    ("IQ4_XS", 4.3), ("IQ4_NL", 4.5),
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuantInfo {
    pub quant: Option<String>,
    pub bits: Option<f64>,
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

fn quant_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // The JS original ends with a lookahead `(?=[-_.]|$)`; Rust has none, so it is checked by hand.
    RE.get_or_init(|| Regex::new(r"(?i)(?:^|[-_.])((?:I?Q\d(?:_[A-Z0-9]+)*)|F16|BF16|F32|MXFP4(?:_MOE)?)").unwrap())
}

pub fn parse_quant(file: &str) -> QuantInfo {
    let name = base_name(file);
    let stem = if name.len() >= 5 && name.is_char_boundary(name.len() - 5) && name[name.len() - 5..].eq_ignore_ascii_case(".gguf") {
        &name[..name.len() - 5]
    } else {
        name
    };
    let re = quant_re();
    let mut pos = 0;
    while pos <= stem.len() {
        let Some(caps) = re.captures_at(stem, pos) else { return QuantInfo::default() };
        let whole = caps.get(0).unwrap();
        let g = caps.get(1).unwrap();
        let next_ok = match stem[g.end()..].chars().next() {
            None => true,
            Some(c) => matches!(c, '-' | '_' | '.'),
        };
        if next_ok {
            let quant = g.as_str().to_uppercase();
            let bits = QUANT_BITS
                .iter()
                .find(|(k, _)| quant == *k)
                .or_else(|| QUANT_BITS.iter().find(|(k, _)| quant.starts_with(k)))
                .map(|(_, b)| *b);
            return QuantInfo { quant: Some(quant), bits };
        }
        // retry from one char past the start of this match
        pos = whole.start() + stem[whole.start()..].chars().next().map_or(1, |c| c.len_utf8());
    }
    QuantInfo::default()
}

fn shard_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(.*)-(\d{5})-of-(\d{5})\.gguf$").unwrap())
}

/// Group GGUF files by quant; split shards (-00001-of-00003) become one entry.
pub fn group_gguf_files(entries: &[(String, u64)]) -> Vec<HubModelFile> {
    let mut order: Vec<HubModelFile> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (path, size) in entries {
        let lower = path.to_lowercase();
        if !lower.ends_with(".gguf") || lower.contains("mmproj") {
            continue;
        }
        let shard = shard_re().captures(path);
        let key = shard.as_ref().map(|c| c[1].to_string()).unwrap_or_else(|| path.clone());
        if let Some(&i) = index.get(&key) {
            let g = &mut order[i];
            g.size += size;
            let mut parts = g.parts.take().unwrap_or_else(|| vec![g.path.clone()]);
            parts.push(path.clone());
            parts.sort();
            g.path = parts[0].clone();
            g.parts = Some(parts);
        } else {
            let q = parse_quant(path);
            index.insert(key, order.len());
            order.push(HubModelFile {
                path: path.clone(),
                size: *size,
                quant: q.quant,
                bits: q.bits,
                parts: shard.map(|_| vec![path.clone()]),
            });
        }
    }
    order.sort_by_key(|f| f.size);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant_names_are_read_from_common_gguf_file_names() {
        let q = parse_quant("Qwen3.5-4B-Q4_K_M.gguf");
        assert_eq!((q.quant.as_deref(), q.bits), (Some("Q4_K_M"), Some(4.8)));
        let q = parse_quant("gemma-4-12b-it-UD-Q4_K_XL.gguf");
        assert_eq!((q.quant.as_deref(), q.bits), (Some("Q4_K_XL"), Some(5.0)));
        assert_eq!(parse_quant("model.IQ3_XXS.gguf").quant.as_deref(), Some("IQ3_XXS"));
        assert_eq!(parse_quant("gpt-oss-20b-MXFP4.gguf").quant.as_deref(), Some("MXFP4"));
        assert_eq!(parse_quant("Llama-3.2-3B-Instruct-f16.gguf").quant.as_deref(), Some("F16"));
        assert_eq!(parse_quant("README.md"), QuantInfo::default());
    }

    #[test]
    fn split_gguf_shards_become_one_download_option_with_summed_size() {
        let e = |p: &str, s: u64| (p.to_string(), s);
        let files = group_gguf_files(&[
            e("Q8_0/big-Q8_0-00002-of-00002.gguf", 4),
            e("Q8_0/big-Q8_0-00001-of-00002.gguf", 6),
            e("big-Q4_K_M.gguf", 5),
            e("mmproj-F16.gguf", 1),
            e("config.json", 1),
        ]);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "big-Q4_K_M.gguf");
        let q8 = &files[1];
        assert_eq!(q8.size, 10);
        assert_eq!(q8.path, "Q8_0/big-Q8_0-00001-of-00002.gguf");
        assert_eq!(
            q8.parts.as_deref(),
            Some(&["Q8_0/big-Q8_0-00001-of-00002.gguf".to_string(), "Q8_0/big-Q8_0-00002-of-00002.gguf".to_string()][..])
        );
        assert_eq!(q8.quant.as_deref(), Some("Q8_0"));
    }
}
