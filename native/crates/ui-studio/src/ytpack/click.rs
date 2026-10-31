//! The thumbnail click checklist, built in `thumbnail-model/` from the videos of manhwa / manga
//! recap channels: each feature is weighted by its effect (views against what the video's own
//! channel usually gets at that age, and how much more big channels use it than small ones) times
//! how reliably it held. It scores an idea from its prompt and words, and a made thumbnail from its
//! pixels too, with the reasons; its rules (each with evidence and example videos) go into the AI's
//! prompts and the Rules guide. Held-out validation is in `model().eval`.
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

const MODEL_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/thumbnail-model/model.json"));

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Weight {
    pub key: String,
    pub label: String,
    /// Per standard deviation of the feature.
    pub weight: f32,
    pub mean: f32,
    pub std: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Stage {
    pub intercept: f32,
    pub features: Vec<Weight>,
}

/// A video from the training data, shown as evidence.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Example {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub views: u64,
    /// Views ÷ what its channel usually gets at that age.
    pub x: f32,
    /// The words on its thumbnail.
    pub text: String,
}

impl Example {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
    pub fn thumb(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub feature: String,
    /// "do" | "write" | "avoid"
    pub kind: String,
    pub text: String,
    pub why: String,
    /// Within a channel: views ÷ expected with the feature, over without it (titles: over all).
    pub lift: f32,
    /// Thumbnails (or titles) with it.
    pub n: u32,
    /// Channels where it pointed the rule's way, of `channels` that had it both ways.
    pub held: u32,
    pub channels: u32,
    /// How often the biggest / smallest third of the channels use it (0-1).
    pub big_share: f32,
    pub small_share: f32,
    pub examples: Vec<Example>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Eval {
    /// Held-out channels: rank correlation of the score with a channel's own videos' performance,
    /// and the same with shuffled performance (mean, sd).
    pub within: f32,
    pub within_shuffled: f32,
    pub within_sd: f32,
    /// Held-out channels: rank correlation of a channel's mean score with its usual daily views.
    pub between: f32,
    pub between_channels: u32,
    /// Titles + length, held out, against shuffled.
    pub titles: f32,
    pub titles_shuffled: f32,
    pub titles_sd: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DataInfo {
    pub channels: u32,
    pub videos: u32,
    pub labelled: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Model {
    pub trained: String,
    pub data: DataInfo,
    pub target: String,
    pub eval: Eval,
    pub pixel: Stage,
    pub concept: Stage,
    /// The labelled thumbnails' summed scores at each percentile 0..=100.
    pub percentiles: Vec<f32>,
    /// The biggest third of the studied channels.
    pub big_channels: Vec<String>,
    pub rules: Vec<Rule>,
    pub title_rules: Vec<Rule>,
    pub text_wins: Vec<Example>,
    pub text_losses: Vec<Example>,
    pub hits: Vec<Example>,
    pub recent: Vec<Example>,
}

pub fn model() -> &'static Model {
    static M: OnceLock<Model> = OnceLock::new();
    M.get_or_init(|| serde_json::from_str(MODEL_JSON).unwrap_or_default())
}

impl Model {
    pub fn rules_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Rule> + 'a {
        self.rules.iter().filter(move |r| r.kind == kind)
    }

    /// The learned rules as prompt lines for the AI that writes thumbnails.
    pub fn thumbnail_digest(&self) -> String {
        let mut s = format!("Measured on {} thumbnails from {} recap channels (each video's views against its own channel's usual, and big vs small channels):\n", self.data.labelled, self.data.channels);
        for (head, kind) in [("DO", "do"), ("WRITE", "write"), ("AVOID", "avoid")] {
            for r in self.rules_of(kind) {
                s.push_str(&format!("- {head}: {} [{}]\n", r.text, r.evidence()));
            }
        }
        let wins: Vec<String> = self.text_wins.iter().take(10).map(|e| format!("\"{}\"", e.text)).collect();
        if !wins.is_empty() {
            s.push_str(&format!("Words on the top outperformers' thumbnails: {}\n", wins.join(", ")));
        }
        s
    }

    /// The learned title rules as prompt lines.
    pub fn title_digest(&self) -> String {
        let mut s = String::from("Title patterns, measured on long recap videos:\n");
        for r in &self.title_rules {
            s.push_str(&format!("- {}: {} ({:.2}x views)\n", if r.kind == "do" { "DO" } else { "AVOID" }, r.text, r.lift));
        }
        let titles: Vec<String> = self.hits.iter().take(5).map(|e| format!("\"{}\"", e.title)).collect();
        if !titles.is_empty() {
            s.push_str(&format!("Titles of the biggest outperformers: {}\n", titles.join("; ")));
        }
        s
    }
}

impl Rule {
    /// "big channels 31% vs small 8% · held in 13/19 channels · 0.45x views"
    pub fn evidence(&self) -> String {
        let mut parts = vec![];
        if self.big_share > 0. || self.small_share > 0. {
            parts.push(format!("big channels {:.0}% vs small {:.0}%", self.big_share * 100., self.small_share * 100.));
        }
        if self.channels >= 3 {
            parts.push(format!("held in {}/{} channels", self.held, self.channels));
        }
        if self.lift > 0. {
            parts.push(format!("{:.2}x views", self.lift));
        }
        parts.join(" · ")
    }
}

// ---------------------------------------------------------------- features from a prompt

fn has(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

/// The words printed on the thumbnail: `text`, or the prompt's `text: "…"`.
pub fn overlay_text(prompt: &str, text: &str) -> String {
    if !text.trim().is_empty() {
        return text.trim().to_string();
    }
    let low = prompt.to_lowercase();
    let Some(at) = low.find("text:").or_else(|| low.find("text \"")) else { return String::new() };
    let rest = &prompt[at..];
    let Some(open) = rest.find(['"', '“']) else { return String::new() };
    let after = &rest[open + rest[open..].chars().next().map_or(1, char::len_utf8)..];
    after.find(['"', '”']).map(|close| after[..close].trim().to_string()).unwrap_or_default()
}

/// The concept features of an idea: what its image prompt describes and the words it prints.
/// The keys match the trainer's `concept()`.
pub fn concept_features(prompt: &str, text: &str) -> HashMap<&'static str, f32> {
    let p = format!(" {} ", prompt.to_lowercase());
    let words = overlay_text(prompt, text);
    let wl = words.to_lowercase();
    let n_words = words.split(|c: char| c.is_whitespace() || c == '/').filter(|w| w.chars().any(char::is_alphanumeric)).count();
    let mentions = prompt.matches('@').count();
    let onlookers = has(&p, &["onlooker", "staring", "stare at", "stares at", "watching him", "watching her", "watch in", "in awe", "jaw drop", "jaws drop", "look on", "looks on", "react", "gasp", "behind him", "behind her", "all eyes"]);
    let crowd = has(&p, &["crowd", "army", "classmates", "students", "nobles", "knights", "villagers", "everyone", "audience", "soldiers", "horde"]);
    let women = has(&p, &[" girl", " woman", " women", "princess", "heroine", " lady", " queen", "female", "saintess", " maid", " wife", " she ", " her "]);
    let arrow = has(&p, &["arrow", "circle", "circled", "pointing at"]);
    let dark = has(&p, &["dark background", "darkness", " night", " cave", " void", "black background", "shadowy", "dark sky", "dark simple background"]);
    let mut f: HashMap<&'static str, f32> = HashMap::new();
    let mut set = |k: &'static str, on: bool| {
        f.insert(k, if on { 1. } else { 0. });
    };
    set("face_closeup", has(&p, &["close-up", "close up", "closeup", "face fills", "big face", "headshot", "portrait", "face takes", "face large", "large face", "huge face"]));
    set("no_face", false);
    set("expr_intense", has(&p, &["shock", "terrified", "scared", "furious", "angry", " rage", "scream", "horrified", "stunned", "panic", "wide-eyed", "wide eyes", "fear", "jaw drop", "gasp", "intense"]));
    set("expr_smirk", has(&p, &["smirk", "cocky", "smug", "sly grin", "confident grin", "arrogant"]));
    set("expr_happy", has(&p, &["laugh", "smiling", "blush", "giggl", "happy"]));
    set("crowd", crowd);
    set("onlookers", onlookers);
    set("solo", mentions <= 1 && !crowd && !onlookers && !women && !has(&p, &["rival", "enemy", "enemies", "two ", "together", "behind"]));
    set("women", women);
    set("text", n_words > 0);
    set("text_short", (1..=3).contains(&n_words));
    set("text_long", n_words >= 6);
    set("text_yellow", has(&p, &["yellow text", "yellow letter", "yellow bold", "yellow caption", "yellow font", "text in yellow", "yellow with a thick black outline", "yellow with black outline"]));
    set("text_label", n_words > 0 && n_words <= 2 && arrow && !wl.contains('!'));
    set("text_exclaim", wl.contains('!') || wl.contains("??") || has(&wl, &["what", "impossible", "no way", "how"]));
    set("text_dialogue", n_words >= 3 && (words.contains('\'') || has(&format!(" {wl} "), &[" i ", " you ", " me ", " my ", " i'm ", " your "])));
    set("text_stat", words.chars().any(|c| c.is_ascii_digit()) || has(&format!(" {wl} "), &[" sss", " ss-", "s-rank", "lv", "level", " rank"]));
    set("text_headline", words.contains(" / ") || words.contains('→') || words.contains("->"));
    set("text_question", wl.contains('?'));
    set("arrow_circle", arrow);
    set("glow_aura", has(&p, &["aura", "glow", "lightning", "flame", "fire", "energy", "magic", "electric", "spark", "burning", "radiant", "blazing", "power effect"]));
    set("glowing_eyes", has(&p, &["glowing eye", "eyes glow", "eye glow", "glowing red eye", "glowing blue eye", "eyes blaz", "eye blaz", "glowing pupil"]));
    set("weapon", has(&p, &["sword", "blade", "katana", "spear", " axe", " bow ", "staff", " gun", "dagger", "weapon", "scythe", "hammer"]));
    set("monster", has(&p, &["monster", "beast", "dragon", "demon", "creature", "titan", " boss", "wolf", "serpent"]));
    set("scale_contrast", has(&p, &["tiny", "gigantic", "colossal", "towering", "dwarfed", "massive", "enormous", "huge monster", "giant "]));
    set("split", has(&p, &["split", "side by side", "left half", "right half", "two panels", "three panels", "triptych", "divided"]));
    set("before_after", has(&p, &["before", " after", "then vs", "transformation", "used to be", "weak on the left", "past and present"]));
    set("system_ui", has(&p, &["status window", "system window", "stat card", "status screen", "hud", "game ui", "notification", "level up", "lv "]));
    set("kneeling", has(&p, &["kneel", "bowing", " bow to", "bows to", "prostrat"]));
    set("blood", has(&p, &["blood", "bleeding", "gore", "wound", "injur"]));
    set("humor", has(&p, &["comedic", "funny", "meme", "chibi", "sweat drop", "comical"]));
    set("romance", has(&p, &["romance", "romantic", " kiss", "couple", "embrace", " hug"]));
    set("wealth", has(&p, &["money", "gold coin", "luxury", "cash", " rich", "treasure", "mansion", "supercar", "feast", "billion"]));
    set("dark_setting", dark);
    set("busy_background", has(&p, &["detailed background", "busy background", "city street", "market", "cluttered"]) && !has(&p, &["simple background", "blurred background", "plain background"]));
    set("off_center", has(&p, &["left third", "right third", "on the right", "on the left", "right half", "left half", "rule of thirds", "right side", "left side"]));
    set("dark_mood", dark && !has(&p, &["bright", "sunny", "daylight"]));
    set("raw_panel", false);
    f
}

// ---------------------------------------------------------------- features from pixels

/// The trainer's pixel features (`pixels.py`) of an image: colour, contrast, clutter, on a
/// 160×90 copy. `None` if it cannot be read.
pub fn pixel_features(path: &Path) -> Option<HashMap<&'static str, f32>> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgb8();
    let small = image::imageops::resize(&img, 160, 90, image::imageops::FilterType::Triangle);
    Some(pixels_of(&small))
}

pub fn pixels_of(img: &image::RgbImage) -> HashMap<&'static str, f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = (w * h).max(1) as f64;
    let mut lum = vec![0f64; w * h];
    let (mut sat_sum, mut red, mut warm, mut cool, mut bright, mut darkc) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let (mut rg, mut yb) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h));
    for (i, p) in img.pixels().enumerate() {
        let [r, g, b] = p.0.map(|c| c as f64 / 255.);
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        lum[i] = l;
        let sat = if mx > 0. { (mx - mn) / mx } else { 0. };
        sat_sum += sat;
        let d = mx - mn;
        let hue = if d <= 1e-9 {
            0.
        } else if mx == r {
            ((g - b) / d).rem_euclid(6.) * 60.
        } else if mx == g {
            ((b - r) / d + 2.) * 60.
        } else {
            ((r - g) / d + 4.) * 60.
        };
        if sat > 0.35 && mx > 0.35 {
            red += f64::from(!(15.0..340.0).contains(&hue));
            warm += f64::from(!(60.0..330.0).contains(&hue));
            cool += f64::from((180.0..300.0).contains(&hue));
        }
        bright += f64::from(l > 0.85);
        darkc += f64::from(l < 0.15);
        rg.push(r - g);
        yb.push(0.5 * (r + g) - b);
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let std = |v: &[f64]| {
        let m = mean(v);
        (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    };
    let (ml, sl) = (mean(&lum), std(&lum));
    let colorfulness = (std(&rg).powi(2) + std(&yb).powi(2)).sqrt() + 0.3 * (mean(&rg).powi(2) + mean(&yb).powi(2)).sqrt();
    // Edges: |dx| + |dy| of luminance over 0.12, on the (w-1)×(h-1) grid.
    let (ew, eh) = (w.saturating_sub(1), h.saturating_sub(1));
    let mut edges = vec![false; ew * eh];
    for y in 0..eh {
        for x in 0..ew {
            let c = lum[y * w + x];
            edges[y * ew + x] = (lum[y * w + x + 1] - c).abs() + (lum[(y + 1) * w + x] - c).abs() > 0.12;
        }
    }
    let edge_density = edges.iter().filter(|e| **e).count() as f64 / edges.len().max(1) as f64;
    let (y0, y1, x0, x1) = (eh / 4, 3 * eh / 4, ew / 4, 3 * ew / 4);
    let mut cen = (0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            cen.0 += usize::from(edges[y * ew + x]);
            cen.1 += 1;
        }
    }
    let center = cen.0 as f64 / cen.1.max(1) as f64;
    HashMap::from([
        ("brightness", ml as f32),
        ("contrast", sl as f32),
        ("saturation", (sat_sum / n) as f32),
        ("colorfulness", colorfulness as f32),
        ("red_share", (red / n) as f32),
        ("warm_share", (warm / n) as f32),
        ("cool_share", (cool / n) as f32),
        ("bright_share", (bright / n) as f32),
        ("dark_share", (darkc / n) as f32),
        ("edge_density", edge_density as f32),
        ("center_focus", (center / edge_density.max(1e-6)) as f32),
    ])
}

// ---------------------------------------------------------------- scoring

/// One feature's push on the score.
#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub label: String,
    /// Logit contribution; positive helps.
    pub effect: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Score {
    /// 0-100: the percentile of the studied recap thumbnails it beats on the checklist.
    pub value: u8,
    /// What it has, strongest push first.
    pub reasons: Vec<Reason>,
    /// The missing features that would help most, as rule text.
    pub tips: Vec<String>,
    /// Pixels were read (a made thumbnail), not only the prompt.
    pub from_image: bool,
}

fn stage_logit(s: &Stage, x: &HashMap<&'static str, f32>, out: &mut Vec<(String, String, f32, bool)>) -> f32 {
    let mut z = s.intercept;
    for w in &s.features {
        let Some(v) = x.get(w.key.as_str()) else { continue };
        let c = w.weight * (v - w.mean) / w.std.max(1e-6);
        z += c;
        out.push((w.key.clone(), w.label.clone(), c, *v > w.mean));
    }
    z
}

impl Model {
    /// Scores concept features, with pixel features when there is an image.
    pub fn score(&self, concept: &HashMap<&'static str, f32>, pixels: Option<&HashMap<&'static str, f32>>) -> Score {
        let mut parts = vec![];
        let none = HashMap::new();
        let zp = stage_logit(&self.pixel, pixels.unwrap_or(&none), &mut parts);
        let zc = stage_logit(&self.concept, concept, &mut parts);
        let value = self.percentile(zp + zc);
        let concept_keys: Vec<&str> = self.concept.features.iter().map(|w| w.key.as_str()).collect();
        let mut reasons: Vec<Reason> = parts
            .iter()
            .filter(|(k, _, c, present)| c.abs() >= 0.015 && (*present || !concept_keys.contains(&k.as_str())))
            .map(|(k, label, c, present)| {
                let pixel = !concept_keys.contains(&k.as_str());
                Reason { label: if pixel { format!("{label} {}", if *present { "high" } else { "low" }) } else { label.clone() }, effect: *c }
            })
            .collect();
        reasons.sort_by(|a, b| b.effect.abs().total_cmp(&a.effect.abs()));
        reasons.truncate(6);
        // Tips: absent features the model rewards most, said as their rule.
        let mut missing: Vec<(&Weight, f32)> = self.concept.features.iter().filter(|w| w.weight > 0.02 && concept.get(w.key.as_str()).copied().unwrap_or(0.) < 0.5).map(|w| (w, w.weight)).collect();
        missing.sort_by(|a, b| b.1.total_cmp(&a.1));
        let tips = missing
            .iter()
            .filter_map(|(w, _)| self.rules.iter().find(|r| r.feature == w.key && r.kind != "avoid").map(|r| r.text.clone()))
            .take(2)
            .collect();
        Score { value, reasons, tips, from_image: pixels.is_some() }
    }

    /// Where a summed score sits among the studied thumbnails, 0-100.
    fn percentile(&self, z: f32) -> u8 {
        if self.percentiles.len() < 2 {
            return 50;
        }
        let below = self.percentiles.iter().filter(|q| **q < z).count();
        ((below as f32 / self.percentiles.len() as f32) * 100.).round().clamp(1., 99.) as u8
    }

    /// An idea, from its prompt and words.
    pub fn score_idea(&self, prompt: &str, text: &str) -> Score {
        self.score(&concept_features(prompt, text), None)
    }

    /// A made thumbnail: its prompt and its pixels.
    pub fn score_image(&self, prompt: &str, path: &Path) -> Score {
        let px = pixel_features(path);
        self.score(&concept_features(prompt, ""), px.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_model_loads() {
        let m = model();
        assert!(m.data.videos > 500, "trained on {} videos", m.data.videos);
        assert!(!m.concept.features.is_empty() && !m.pixel.features.is_empty());
        assert!(m.rules.iter().any(|r| r.kind == "do") && m.rules.iter().any(|r| r.kind == "avoid"));
        assert!(m.thumbnail_digest().contains("DO:"));
        // Every concept feature the model knows is read from prompts.
        let f = concept_features("x", "");
        for w in &m.concept.features {
            assert!(f.contains_key(w.key.as_str()), "{} is not read from prompts", w.key);
        }
    }

    #[test]
    fn reads_the_words_and_features_of_a_prompt() {
        assert_eq!(overlay_text("Hero walks, bold text: \"HE EATS SWORDS\" on the left", ""), "HE EATS SWORDS");
        assert_eq!(overlay_text("anything", " WHAT?? "), "WHAT??");
        let f = concept_features("Extreme close-up of @Arhan, glowing eyes, blue aura; a crowd of students stare in awe behind him. Dark background. bold text: \"LV 999\"", "");
        for k in ["face_closeup", "glowing_eyes", "glow_aura", "crowd", "onlookers", "dark_setting", "text", "text_short", "text_stat"] {
            assert_eq!(f[k], 1., "{k}");
        }
        for k in ["split", "solo", "text_long", "romance"] {
            assert_eq!(f[k], 0., "{k}");
        }
    }

    #[test]
    fn scores_follow_the_weights() {
        let m = model();
        let base = concept_features("@Arhan stands in a field", "");
        let s0 = m.score(&base, None);
        // Switching on the most rewarded feature raises the score; the most penalised lowers it.
        let best = m.concept.features.iter().max_by(|a, b| a.weight.total_cmp(&b.weight)).unwrap();
        let worst = m.concept.features.iter().min_by(|a, b| a.weight.total_cmp(&b.weight)).unwrap();
        let mut up = base.clone();
        up.insert(Box::leak(best.key.clone().into_boxed_str()), 1.);
        let mut down = base.clone();
        down.insert(Box::leak(worst.key.clone().into_boxed_str()), 1.);
        assert!(m.score(&up, None).value >= s0.value);
        assert!(m.score(&down, None).value <= s0.value);
        assert!((1..=99).contains(&s0.value));
    }

    #[test]
    fn pixel_features_of_a_plain_image() {
        let red = image::RgbImage::from_pixel(160, 90, image::Rgb([230, 20, 20]));
        let f = pixels_of(&red);
        assert!(f["red_share"] > 0.99 && f["warm_share"] > 0.99 && f["cool_share"] < 0.01);
        assert!(f["contrast"] < 1e-6 && f["edge_density"] < 1e-6);
        let mut half = image::RgbImage::from_pixel(160, 90, image::Rgb([0, 0, 0]));
        for y in 0..90 {
            for x in 80..160 {
                half.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let f = pixels_of(&half);
        assert!((f["brightness"] - 0.5).abs() < 0.01 && (f["dark_share"] - 0.5).abs() < 0.01 && (f["bright_share"] - 0.5).abs() < 0.01);
    }
}
