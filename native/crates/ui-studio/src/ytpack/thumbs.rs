//! The thumbnail generator's logic: the AI's click analysis (concepts made from the manga and
//! from what wins clicks in the recap niche), `@Character` mentions resolved to the face cards
//! sent as references, and the gallery saved per manga in `studio/<manga>/thumbnails/`.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logic::{looks, short_count, Cast, Video};

// ---------------------------------------------------------------- gallery

/// One made thumbnail. `parent` is the thumbnail it was re-edited from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thumb {
    pub id: String,
    /// The prompt as written (with its `@` mentions).
    pub prompt: String,
    /// The concept's name, "Custom" or "Edit".
    pub label: String,
    pub parent: Option<String>,
    pub model: String,
    pub resolution: String,
    /// Estimated credits (0: unknown).
    pub cost: f64,
    pub created: u64,
    /// What the app lettered on it: the big label on the hero, the small tags, and where each went.
    pub text: String,
    pub tags: Vec<String>,
    pub lettering: mv_recap::thumb::Lettering,
    /// The last time it was put in a feed of competing thumbnails (see `feed`).
    pub feed: Option<super::feed::FeedResult>,
}

/// A thumbnail idea from the AI.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Concept {
    pub name: String,
    /// Why it wins the click.
    pub why: String,
    /// The image prompt, with `@Name` for each character in it.
    pub prompt: String,
    /// The big label on the hero: a role tag of one or two words ("JOBLESS"), not a title.
    pub text: String,
    /// One or two small tags beside someone or something else ("BLANK CHECK", "SUSPICIOUS...").
    pub tags: Vec<String>,
    /// The side of the picture the hero is on: "left" or "right".
    pub side: String,
    /// The part of the big label drawn red ("SSS" of "SSS-RANK").
    pub key: String,
    /// Which of the [`STRATEGIES`] it uses (its style pictures go with it).
    pub strategy: String,
}

impl Concept {
    /// The prompt as the prompt box holds it: the art, then its strategy and labels lines (see
    /// [`split_labels`]).
    pub fn full_prompt(&self) -> String {
        let mut words = vec![self.text.clone()];
        words.extend(self.tags.iter().cloned());
        with_labels(&self.prompt, &Labels { words, hero_left: self.side.trim().to_lowercase() != "right", key: self.key.clone(), strategy: self.strategy.clone() })
    }
}

// ---------------------------------------------------------------- the strategies

/// One way a recap thumbnail sells its story, chosen from the story's hook.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strategy {
    pub name: &'static str,
    /// The story's hook it suits.
    pub hook: &'static str,
    pub scene: &'static str,
    pub labels: &'static str,
    /// Two thumbnails of a channel drawn in the wanted style showing this kind of scene (YouTube
    /// video ids): sent as style pictures when the owner has set none of their own.
    pub refs: [&'static str; 2],
}

/// The strategies, best first: ordered by how they did when put in a feed of competing thumbnails,
/// the first two by what the owner actually clicked in a feed of ~900 real recap thumbnails (the
/// Click Lab, 2026-11-19/20: a big glamorous woman reacting to something absurd was clicked every
/// time it showed; a quest box won only when it took a quarter of the frame).
/// "hero and admirers" is the fallback when nothing above fits.
pub const STRATEGIES: &[Strategy] = &[
    Strategy { name: "glamorous reaction", hook: "the hero does something absurd, funny or outrageous and an adult woman of the story sees it", scene: "a big glamorous adult woman of the story filling half the frame with a huge comic reaction (shock, jaw drop, blush) beside the absurd thing the hero is doing, in a bright daylight place from the story", labels: "the absurd thing named in 1-3 words (100 KG SWORD, +99 CURSED), a short line from her", refs: ["kUc91Dw4QN8", "NWaw3KHneQ0"] },
    Strategy { name: "quest box", hook: "the system gives him a ridiculous task or reward", scene: "a big empty glowing system window taking a quarter of the frame (drawn blank: the quest is lettered into it), the hero reacting to it, a woman of the story reacting to him", labels: "the quest itself as the big label (TAME A DRAGON, LOSE 100 KG), a reward or warning tag", refs: ["htP3o93ZVtE", "kUc91Dw4QN8"] },
    Strategy { name: "awakening comparison", hook: "he awakens or tests far above everyone", scene: "everyone at one table or line with the same object (orb, crystal, card); his is huge and blazing, theirs small and dull; they stare in shock", labels: "SSS-RANK on his, D-RANK / C-RANK on theirs", refs: ["9gsvfXt4_Mc", "RmDVez05OOU"] },
    Strategy { name: "number gap", hook: "he gains faster or has far more", scene: "two piles, two counters or two panels side by side, one tiny and one absurd; a rival points and shouts", labels: "2 XP / 100K XP, +2 / +50", refs: ["RmDVez05OOU", "Ghz-3sM5rik"] },
    Strategy { name: "impact frame", hook: "he beats someone far above him", scene: "the single frame of the hit landing: the opponent folding backwards, speed lines, dust, an onlooker recoiling", labels: "NEW HUNTER on him, S-RANK on the one being hit", refs: ["e-hk_OWpqhE", "TYPyx44rHx0"] },
    Strategy { name: "beaten champion", hook: "he has already won", scene: "the hero crouched or seated, bored, a hand on the head of a floored top-ranker, a crowd in the stands", labels: "BORED on him, #1-RANK on them", refs: ["ERGRXI6e6Hg", "QxZWEjXxdjc"] },
    Strategy { name: "one gesture", hook: "he stops something without effort", scene: "a close-up of two faces: an attack halted by one finger", labels: "SO SLOW..., TOP 1 HERO", refs: ["FUhNMBrbf84", "2d-Dv27roPg"] },
    Strategy { name: "kneeling giant", hook: "a giant power submits to someone small", scene: "a huge figure on one knee before a small calm one", labels: "TOP 1 or DEATH GOD on the giant, a tag on the small one", refs: ["-seaUuYO4pw", "df9yotykOkU"] },
    Strategy { name: "rank irony", hook: "he looks weak and ranks low on paper", scene: "two characters side by side with rank tags; the low-ranked one is smirking", labels: "SS-RANK HUNTER / F-RANK HUNTER", refs: ["fBev9We2HME", "_iwaj2wFYDc"] },
    Strategy { name: "before and after", hook: "he transforms through training", scene: "a split screen with a white line down the middle: the weak version and the monstrous one", labels: "BEFORE / AFTER, DAY 1 / DAY 359", refs: ["hNJ3vF0A3YQ", "j2ezQRfWoHA"] },
    Strategy { name: "arrival", hook: "he arrives and everyone reacts", scene: "the hero in the foreground ignoring a crowd of pointing, shouting people", labels: "1ST DAY, NEW STUDENT", refs: ["LioJPryMe40", "G3CpSi-WfNk"] },
    Strategy { name: "hero and admirers", hook: "nothing above fits; the draw is the cast", scene: "the hero large at one side, two or three adult women filling the frame, each with a different attitude", labels: "a role tag on him, one or two small tags", refs: ["ZUnHllU0Tk0", "r453L33Cads"] },
];

/// The strategy called `name` (any case), if it is one.
pub fn strategy(name: &str) -> Option<&'static Strategy> {
    let n = name.trim().to_lowercase();
    STRATEGIES.iter().find(|s| s.name == n)
}

fn strategy_table() -> String {
    STRATEGIES.iter().enumerate().map(|(k, s)| format!("{}. \"{}\": when {}. Scene: {}. Labels: {}.", k + 1, s.name, s.hook, s.scene, s.labels)).collect::<Vec<_>>().join("\n")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Gallery {
    /// Newest first.
    pub thumbs: Vec<Thumb>,
    pub concepts: Vec<Concept>,
    /// The AI's read of what the manga's best thumbnails should do.
    pub analysis: String,
}

impl Gallery {
    pub fn dir(manga_id: &str) -> PathBuf {
        mv_recap::Brain::dir(manga_id).join("thumbnails")
    }
    pub fn image(manga_id: &str, id: &str) -> PathBuf {
        Self::dir(manga_id).join(format!("{id}.png"))
    }
    /// The picture without its title, kept beside it (in its own folder, so it never shows as a
    /// thumbnail): what an edit starts from, and what a changed title is lettered on.
    pub fn art(manga_id: &str, id: &str) -> PathBuf {
        Self::dir(manga_id).join("art").join(format!("{id}.png"))
    }
    pub fn load(manga_id: &str) -> Gallery {
        let mut g: Gallery = std::fs::read(Self::dir(manga_id).join("gallery.json")).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        g.thumbs.retain(|t| Self::image(manga_id, &t.id).is_file());
        g
    }
    pub fn save(&self, manga_id: &str) {
        let dir = Self::dir(manga_id);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(d) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(dir.join("gallery.json"), d);
        }
    }
}

// ---------------------------------------------------------------- mentions

/// What a character can be called with `@`: the name without spaces, and its first word.
pub fn handle(name: &str) -> String {
    name.split_whitespace().collect::<String>()
}

fn is_handle_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '\''
}

/// Which character `@word` means: the full handle, an alias, the first name, or `@character2`.
fn lookup(word: &str, cast: &[Cast]) -> Option<usize> {
    let w = word.to_lowercase();
    if let Some(n) = w.strip_prefix("character").and_then(|n| n.parse::<usize>().ok()) {
        return (n >= 1 && n <= cast.len()).then(|| n - 1);
    }
    let names = |c: &Cast| std::iter::once(c.name.clone()).chain(c.aliases.iter().cloned()).collect::<Vec<_>>();
    cast.iter().position(|c| names(c).iter().any(|n| handle(n).to_lowercase() == w))
        .or_else(|| cast.iter().position(|c| names(c).iter().any(|n| n.split_whitespace().next().is_some_and(|f| f.to_lowercase() == w))))
}

/// The prompt with each `@mention` replaced by the character's name and its reference image
/// ("Image N", counted after `first` images already attached), and the characters to attach in
/// that order. Unknown mentions stay as they are.
pub fn resolve(prompt: &str, cast: &[Cast], first: usize) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut refs: Vec<usize> = vec![];
    let mut rest = prompt;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after.find(|c: char| !is_handle_char(c)).unwrap_or(after.len());
        let word = after[..end].trim_end_matches(['\'', '-']);
        match lookup(word, cast) {
            Some(i) if !word.is_empty() => {
                let slot = match refs.iter().position(|r| *r == i) {
                    Some(k) => k,
                    None => {
                        refs.push(i);
                        refs.len() - 1
                    }
                };
                out.push_str(&format!("{} (the character in Image {})", cast[i].name, first + slot + 1));
                rest = &after[word.len()..];
            }
            _ => {
                out.push('@');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, refs)
}

// ---------------------------------------------------------------- the labels line

/// What a prompt asks for besides the picture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labels {
    /// The big label first, then the small tags.
    pub words: Vec<String>,
    /// The hero is on the left of the picture (the default places are laid out from his side).
    pub hero_left: bool,
    /// The part of the big label drawn red.
    pub key: String,
    /// The strategy's name (empty: none given).
    pub strategy: String,
}

impl Labels {
    pub fn label(&self) -> &str {
        self.words.first().map(String::as_str).unwrap_or("")
    }
    pub fn tags(&self) -> &[String] {
        self.words.get(1..).unwrap_or(&[])
    }
}

/// `prompt` with its strategy and labels as its last lines:
/// `Strategy: awakening comparison` and `Labels: *SSS*-RANK | D-RANK | C-RANK (hero left)`, the red
/// key between stars. The picture is drawn without any words (image models misspell them); the app
/// letters them on.
pub fn with_labels(prompt: &str, l: &Labels) -> String {
    let (art, _) = split_labels(prompt);
    let mut out = art;
    if let Some(s) = strategy(&l.strategy) {
        out.push_str(&format!("\nStrategy: {}", s.name));
    }
    let words: Vec<String> = l.words.iter().map(|w| w.trim().trim_matches('"').to_string()).filter(|w| !w.is_empty()).collect();
    if let Some(first) = words.first() {
        let key = l.key.trim().trim_matches('*');
        let big = match (!key.is_empty()).then(|| first.to_lowercase().find(&key.to_lowercase())).flatten() {
            Some(i) => format!("{}*{}*{}", &first[..i], &first[i..i + key.len()], &first[i + key.len()..]),
            None => first.clone(),
        };
        let rest: Vec<String> = std::iter::once(big).chain(words[1..].iter().cloned()).collect();
        out.push_str(&format!("\nLabels: {} (hero {})", rest.join(" | "), if l.hero_left { "left" } else { "right" }));
    }
    out
}

fn quoted(s: &str) -> Option<(usize, usize)> {
    let open = s.find(['"', '\u{201c}'])?;
    let from = open + s[open..].chars().next().map_or(1, char::len_utf8);
    let len = s[from..].find(['"', '\u{201d}'])?;
    Some((from, from + len))
}

/// A prompt as (the art to draw, what goes with it). The labels are the `Labels:` line (`Title:
/// "..."` is read as one label), the strategy the `Strategy:` line; a prompt written the older way,
/// with the words inside it (`bold text: "..."`), gives them up the same way, so the image model is
/// never asked to write.
pub fn split_labels(prompt: &str) -> (String, Labels) {
    let mut art: Vec<&str> = vec![];
    let mut out = Labels { hero_left: true, ..Default::default() };
    for line in prompt.lines() {
        let t = line.trim_start();
        let low = t.to_lowercase();
        if let Some(rest) = low.starts_with("labels:").then(|| &t["labels:".len()..]) {
            let (words, side) = match rest.rfind('(') {
                Some(i) if rest.trim_end().ends_with(')') => (&rest[..i], rest[i..].to_lowercase()),
                _ => (rest, String::new()),
            };
            out.hero_left = !side.contains("right");
            out.words = words.split('|').map(|w| w.trim().trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}').trim().to_string()).filter(|w| !w.is_empty()).take(3).collect();
            // The red key: between stars in the big label.
            if let Some(first) = out.words.first_mut() {
                if let (Some(a), Some(b)) = (first.find('*'), first.rfind('*')) {
                    if b > a {
                        out.key = first[a + 1..b].trim().to_string();
                    }
                }
                *first = first.replace('*', "");
            }
        } else if let Some(rest) = low.starts_with("strategy:").then(|| &t["strategy:".len()..]) {
            out.strategy = strategy(rest).map(|s| s.name.to_string()).unwrap_or_default();
        } else if low.starts_with("title:") {
            let rest = &t["title:".len()..];
            let text = quoted(rest).map(|(a, b)| rest[a..b].trim().to_string()).unwrap_or_else(|| rest.trim().to_string());
            if out.words.is_empty() && !text.is_empty() {
                out.words = vec![text];
            }
        } else {
            art.push(line);
        }
    }
    let mut art = art.join("\n").trim().to_string();
    // The older way: `bold text: "WORDS"` (also "yellow text", "text:") inside the paragraph.
    let low = art.to_lowercase();
    if let Some(at) = low.find("text:").or_else(|| low.find("text \"")) {
        if let Some((a, b)) = quoted(&art[at..]) {
            if out.words.is_empty() {
                out.words = vec![art[at + a..at + b].trim().to_string()];
            }
            // From the adjective before "text" ("bold yellow text:") to the closing quote.
            let start = art[..at].rfind(['.', ',', ';', '\n']).map(|i| i + 1).unwrap_or(0);
            let end = at + b + art[at + b..].chars().next().map_or(0, char::len_utf8);
            art = format!("{}{}", art[..start].trim_end_matches([',', ';']), &art[end..]).replace("  ", " ").trim().to_string();
        }
    }
    (art, out)
}

/// The first paragraph when style reference pictures are attached: the style comes from them and
/// from nothing else. Describing a channel's look in words fails ("glossy" gives over-polished game
/// art, "flat" a cheap cartoon); "the same artist as these pictures" is what matches it.
pub const SAME_ARTIST: &str = "Create a new illustration that looks like it was drawn by the same artist as the art style reference images. Copy their art style exactly: the same high-quality modern anime key-visual look, the same clean line art, the same detailed anime eyes and faces, the same shading on hair, skin and clothes, the same vivid saturated colours and bright daylight, the same body proportions and the same level of background detail. Use those references for style only: new characters, new poses, new layout. Do not reproduce any of their text.";

/// What a style reference picture is called in the request.
pub const STYLE_CAPTION: &str = "art style reference: draw the new image in this exact style. Ignore the text on it.";

/// The full prompt sent to the image model: where the style comes from, then the scene, then what
/// keeps it from looking machine-made. With `style` reference pictures attached (Images 1..=style)
/// the style is theirs and no style words are added; without any, it is described. `refs` are the
/// characters' names and looks, from Image `first + 1` on.
pub fn image_prompt(title: &str, resolved: &str, refs: &[(String, String)], first: usize, style: usize) -> String {
    let (scene, _) = split_labels(resolved);
    let look = if style > 0 {
        format!("{SAME_ARTIST} {} the art style reference{}.", if style == 1 { "Image 1 is".to_string() } else { format!("Images 1 to {style} are") }, if style == 1 { "" } else { "s" })
    } else {
        "Extremely eye-catching YouTube thumbnail art in a high-quality modern anime key-visual style: clean line art, detailed anime eyes and faces, vivid saturated colours, bright daylight, a crisp detailed anime background.".to_string()
    };
    let mut p = format!(
        "{look}\n\n16:9 YouTube thumbnail for a manhwa recap video of \"{}\". {}\n\nOnly the people named above are in the picture: no extra people, nobody drawn twice. Nothing floats in the air: every object is held by someone or rests on the ground. Hands have five fingers. Clean image: no sparkles, no hearts, no floating particles.",
        title.trim(),
        scene.trim()
    );
    if !refs.is_empty() {
        p.push_str("\n\nCharacter references (keep each face, hair and outfit exactly like its image, drawn in the art style above):");
        for (k, (name, look)) in refs.iter().enumerate() {
            p.push_str(&format!("\n- Image {}: {name}{}", first + k + 1, if look.is_empty() { String::new() } else { format!(", {look}") }));
        }
    }
    p.push_str("\n\nAbsolutely no text anywhere (an empty window or box the scene asks for stays blank): no letters, no numbers, no labels, no arrows, no speech bubbles, no sound effects, no signs with writing, no signature, no watermark, no logo, no border.");
    p
}

/// The edit request for a made thumbnail (Image 1): only the change asked for.
pub fn edit_prompt(change: &str) -> String {
    let (change, _) = split_labels(change);
    format!("Edit Image 1. Keep everything exactly as it is: the same characters, faces, hair, poses and positions, the same background and the same art style. Make only this change: {}\nDo not add or remove anyone or anything else. No text, no letters, no arrows, no watermark, no border.", change.trim())
}

// ---------------------------------------------------------------- the crop and where the labels go

/// Asks a model that can see the finished picture (attached, 1280x720) how to crop it tighter.
pub fn crop_prompt() -> (String, String) {
    let system = "You crop a finished YouTube thumbnail so it works in a feed, where it is seen at 320x180 for under a second. You are shown the picture; it is 1280 wide and 720 high, x from the left, y from the top.\n\
Choose ONE 16:9 rectangle that keeps the main character's face, the key object of the scene (what the story is about: a glowing orb, a raised hand, a pile of gold) and one or two reacting faces as large as possible, and cuts bystanders, empty sky, floor and anything small. Never cut a face in half; keep the main character's whole head. The rectangle is at least 820 wide (the picture must stay sharp) and fits inside the picture; when the picture is already tight, give the whole picture.\n\
Reply with JSON only: {\"crop\":[x,y,width,height]}";
    (system.to_string(), "Look at the picture and give the crop.".to_string())
}

/// The model's crop, or `None` when it is unusable or barely changes anything (then none is made).
pub fn parse_crop(raw: &str) -> Option<[f32; 4]> {
    let v = mv_recap::extract_json(raw)?;
    let a = v.get("crop")?.as_array()?;
    let n: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
    let [x, y, w, _] = <[f32; 4]>::try_from(n).ok()?;
    // 16:9 from its width, moved inside the frame.
    let w = w.clamp(820., 1280.);
    let h = w * 9. / 16.;
    let c = [x.clamp(0., 1280. - w), y.clamp(0., 720. - h), w, h];
    (mv_recap::thumb::crop_ok(&c) && w < 1250.).then_some(c)
}

/// Asks a model that can see the finished picture (attached, 1280x720) where its labels go.
pub fn placement_prompt(labels: &Labels) -> (String, String) {
    let system = "You place the labels on a finished YouTube thumbnail, the way the big manhwa recap channels do. You are shown the picture; it is 1280 wide and 720 high, x from the left, y from the top. Labels are heavy yellow capitals with a black outline.\n\
- The BIG label (letters about 128 high, about 60 wide each) sits beside the key object of the scene or the main character's head, over background, never over a face. An arrow runs from its near end to the thing it names (the object, or the edge of his hair): give that point as \"points_at\".\n\
- Each SMALL tag (letters about 50 high, about 24 wide each) sits beside the person or object it names, over background, hair or clothes, never over a face or a hand.\n\
- When the picture has a big empty glowing window or box, the BIG label goes INSIDE it, as large as fits, and \"points_at\" is the hero's head.\n\
- Nothing in the bottom-right corner (YouTube puts the video length there), nothing cut by the frame, labels never overlap.\n\
Reply with JSON only: {\"points_at\":[x,y],\"label\":{\"x\":0,\"y\":0,\"size\":128},\"tags\":[{\"x\":0,\"y\":0,\"size\":50}]} where each x,y is the label's top-left corner. Give one entry in \"tags\" per small tag, in the order given.";
    let mut user = format!("The big label (the main character is on the {} of the picture): \"{}\"\n", if labels.hero_left { "left" } else { "right" }, labels.label().to_uppercase());
    for (k, t) in labels.tags().iter().enumerate() {
        user.push_str(&format!("Small tag {}: \"{}\"\n", k + 1, t.to_uppercase()));
    }
    user.push_str("Look at the picture and place them.");
    (system.to_string(), user)
}

/// The model's placement as a lettering, or `None` when it is unusable (then the default places
/// are kept): every label inside the frame, a readable size, and no two labels overlapping.
pub fn parse_placement(raw: &str, labels: &Labels) -> Option<mv_recap::thumb::Lettering> {
    use mv_recap::thumb::{label_width, Arrow, Label, Lettering, LABEL_SIZE, LETTER_H, LETTER_W, TAG_SIZE};
    let v = mv_recap::extract_json(raw)?;
    let num = |o: &Value, k: &str| o.get(k).and_then(Value::as_f64).map(|n| n as f32);
    let place = |o: &Value, text: &str, lo: f32, hi: f32, base: f32, key: &str| -> Option<Label> {
        let mut size = num(o, "size").unwrap_or(base).clamp(lo, hi);
        let (x, y) = (num(o, "x")?, num(o, "y")?);
        // Too wide for where it was put: smaller, down to the smallest readable.
        let room = LETTER_W - 8. - x;
        let w = label_width(text, size);
        if w > room {
            size = (size * room / w).max(lo * 0.8);
        }
        let l = Label { text: text.to_string(), x, y, size, key: key.to_string() };
        (x >= 0. && y >= -4. && x + label_width(text, size) <= LETTER_W && y + size * 1.1 <= LETTER_H && !(x + label_width(text, size) > 1040. && y + size > 600.)).then_some(l)
    };
    let label = labels.label();
    if label.is_empty() {
        return None;
    }
    let big = place(v.get("label")?, label, 100., 140., LABEL_SIZE, &labels.key)?;
    let mut out = Lettering { labels: vec![big.clone()], ..Default::default() };
    let target = v.get("points_at").or_else(|| v.get("hero_head")).and_then(Value::as_array).filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32]));
    if let Some(to) = target.filter(|h| (0. ..=LETTER_W).contains(&h[0]) && (0. ..=LETTER_H).contains(&h[1])) {
        // From the label's end nearest the target, under its middle or at mid height.
        let (left, right) = (big.x - 6., big.x + label_width(label, big.size) + 6.);
        let below = to[1] > big.y + big.size * 1.3 && (left..=right).contains(&to[0]);
        let from = if below { [to[0].clamp(left + 20., right - 20.), big.y + big.size * 1.12] } else { [if (to[0] - left).abs() <= (to[0] - right).abs() { left } else { right }, big.y + big.size * 0.6] };
        let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
        if len >= 30. {
            out.arrows.push(Arrow { from, to, width: 30. });
        }
    }
    let tags = v.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
    for (text, o) in labels.tags().iter().zip(&tags) {
        if let Some(l) = place(o, text, 38., 60., TAG_SIZE, "") {
            out.labels.push(l);
        }
    }
    let rect = |l: &Label| (l.x, l.y, l.x + label_width(&l.text, l.size), l.y + l.size * 1.1);
    let clash = out.labels.iter().enumerate().any(|(i, a)| out.labels.iter().skip(i + 1).any(|b| {
        let (a, b) = (rect(a), rect(b));
        a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
    }));
    (!clash).then_some(out)
}

/// The look line for a character reference.
pub fn look_of(c: &Cast) -> String {
    looks(&c.description)
}

// ---------------------------------------------------------------- the AI's click analysis

pub struct Brief<'a> {
    pub title: &'a str,
    pub cast: &'a [Cast],
    pub story: &'a [String],
    pub comparable: &'a [Video],
    pub reference: &'a [Video],
    pub hook: &'a str,
}

const CLICK_GUIDE: &str = "What the biggest recap channels' thumbnails do (study, then adapt to THIS story):\n\
- Reaction scene: the hero does something absurd or impressive in a specific, bright, story-true place, and the story's other characters (heroines, rivals, classmates) react big in the frame: shock, blush, disbelief. Their faces tell the viewer how huge it is.\n\
- Label and arrow: a yellow arrow at the hero with a 1-2 word yellow label ('MC', 'NEWBIE', 'S-RANK?'), or one short line a character says.\n\
- Romance beat, when the story has one: a heroine blushing at the oblivious hero.\n\
- The hook as a scene: show the premise happening (what he does, to whom, where) instead of a generic pose.\n\
Overused by small channels, and below average in the data: the generic aura / lightning power pose, the hero alone holding a weapon, split or multi-panel frames, dark caves and night voids, small status windows and stat numbers (a system window only works BIG: a quarter of the frame), kneeling crowds, tiny-hero-versus-giant shots. Use one only when it IS this story's hook, and even then put people reacting in the frame.\n\
Rules: one clear scene, readable faces, 2-3 dominant colours, nothing small. Never invent powers, weapons, places or events that are not in the story: exaggerate what is there.";

/// How a concept is designed and its prompt written: the system the channel's thumbnails are made
/// with, tested by putting them in feeds of competing thumbnails (each rule is there because a
/// thumbnail that broke it lost the click).
const RECIPE: &str = "HOW TO DESIGN EACH ONE\n\
- A STORY, NOT A POSE: one situation with a visible gap the viewer reads in a second (a giant blazing orb beside three small dull ones; a mountain of gold beside two coins; a bored hero with a hand on a beaten champion). A group portrait of people standing around loses to a story scene.\n\
- THE STRATEGY: pick it from the story's hook, from the STRATEGY TABLE below, preferring the higher ones (they won more often in the feed). \"hero and admirers\" only when nothing above fits the hook.\n\
- THE CAST IS REAL: only this series' own characters, by their handles, each with their real hair, eyes, accessories and outfit from the character list. A fan must recognise everyone at a glance.\n\
- THREE OR FOUR BIG SHAPES: the hero, the hook object, one or two reacting faces; frame them tight and cut bystanders. At feed size anything smaller than a face is noise.\n\
- ONE FOCAL POINT: the brightest, most saturated thing in the picture is the hook object or the hero's face.\n\
- REACTIONS: every face but the hero's reacts (shock, disbelief, a blush, anger). A calm bystander is wasted space. The hero stays calm, bored or smirking while the thing he does is huge.\n\
- COLOUR: bright, high-key and flat, the way the channels the owner clicks most colour theirs: a story-true place in sunny daylight (a blue sky with big clouds, a colourful street, an arena with banners) or one strong saturated colour pair (gold on deep violet, red on teal), clean flat shading, never dark and painterly. NEVER a white, grey or pale background: it catches the eye in a feed and loses the click.\n\
- FUNNY AND FLIRTY: a gag alone sits in the middle; a gag WITH an adult woman of the story reacting big to it (shocked, flustered, blushing) wins. When the story has one, let her be large in the frame.\n\
- WOMEN, when the story has adult women around the hero: glamorous grown adults with a mature beautiful face and a voluptuous anime hourglass figure, in their own outfit cut tight and fashionable, fully clothed, in a confident pose. Limits that always hold: adults only, clothed, nothing explicit or see-through; a character who is a child or teenager in the story is drawn plainly or left out.\n\
HOW TO WRITE ITS PROMPT (short paragraphs for an image model; no style words at all, the style comes from reference pictures)\n\
1. \"Exactly N people and nobody else: ... No extra people, no duplicates.\"\n\
2. The scene of the strategy, framed tight: where the hero is, how large and cropped by which edges, his expression, the hook object and how it shows the gap.\n\
3. Each other character, one line each: who they are in a few words, hair, eyes, accessories, outfit with colours, where they are and how they react.\n\
4. \"BACKGROUND: ...\" with its colour pair and its one glow.\n\
Simple poses, nothing in mid-air, and never any words, numbers, labels, arrows or bubbles to draw: the app letters them on afterwards, so they are always spelled right.\n\
THE LABELS (not a title; the picture carries the thumbnail and the words are tags)\n\
- `text`: ONE big label of one to three words for the hook object or the hero (SSS-RANK on his orb, JOBLESS on the hero, 100K XP on his counter). Irony wins: the weakest-sounding tag on the strongest person. Never the manga's or a character's name.\n\
- `key`: the part of `text` drawn red, the shocking part (\"SSS\" of \"SSS-RANK\", \"100K\").\n\
- `tags`: AT MOST two small tags of one to three words for other characters or objects (D-RANK, C-RANK; #1-RANK on the beaten one), or one short line of dialogue.\n\
- `side`: \"left\" or \"right\", the side the hero is on.\n\
- `strategy`: the strategy's name exactly as in the table.\n\
STRATEGY TABLE (best first)\n";

/// System and user prompt for 4 thumbnail concepts; the answer is JSON only.
pub fn concepts_prompt(b: &Brief) -> (String, String) {
    let names: Vec<String> = b.cast.iter().map(|c| format!("@{}", handle(&c.name))).collect();
    let learned = super::click::model().thumbnail_digest();
    let system = format!(
        "You are a thumbnail art director for a top manhwa recap channel. You know why viewers click and you design thumbnails that are clickbait but honest to the story.\n\n{CLICK_GUIDE}\n\n{learned}\n\
{RECIPE}{}\n\n\
First find this story's hook: its premise in one sentence and its single most surprising gap (a weak person holds absurd power, a nobody's reward dwarfs everyone's, the liar is believed), said in under ten words. Then write 6 different concepts that sell that hook, the strongest first, using at least three different strategies from the table (the best fit first). Where this system and the older craft guide above disagree, this system wins. In each prompt, name characters ONLY with these handles so their face references get attached: {}.\n\
Reply with JSON only: {{\"analysis\":\"2-3 sentences: the hook in under ten words, then the strategy that shows it best and why\",\"concepts\":[{{\"name\":\"short name\",\"strategy\":\"awakening comparison\",\"why\":\"one sentence: the gap it shows and why it gets the click\",\"prompt\":\"\",\"text\":\"SSS-RANK\",\"key\":\"SSS\",\"tags\":[\"D-RANK\",\"C-RANK\"],\"side\":\"left\"}}]}}",
        strategy_table(),
        if names.is_empty() { "(no characters known yet; describe the hero instead)".to_string() } else { names.join(", ") }
    );
    let mut u = format!("Manga: {}\n", b.title);
    if !b.cast.is_empty() {
        u.push_str("\nCharacters (handle: look):\n");
        for c in b.cast {
            u.push_str(&format!("- @{}: {}\n", handle(&c.name), looks(&c.description)));
            if !c.traits.is_empty() {
                u.push_str(&format!("  traits: {}\n", c.traits.join(", ")));
            }
        }
    }
    if !b.story.is_empty() {
        u.push_str(&format!("\nStory so far:\n{}\n", b.story.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")));
    }
    let mut top: Vec<&Video> = b.comparable.iter().chain(b.reference.iter()).collect();
    top.sort_by_key(|v| std::cmp::Reverse(v.views));
    if !top.is_empty() {
        u.push_str("\nBest-performing recap videos (their titles hint at what their thumbnails sell):\n");
        for v in top.iter().take(10) {
            u.push_str(&format!("- \"{}\" — {} views\n", v.title, short_count(v.views)));
        }
    }
    if !b.hook.trim().is_empty() {
        u.push_str(&format!("\nThe planned thumbnail text: \"{}\"\n", b.hook.trim()));
    }
    u.push_str(&super::logic::brain_notes(&[mv_core::brain::Area::Thumbnails, mv_core::brain::Area::Niche]));
    (system, u)
}

/// System and user prompt for rewriting the prompt box (`draft`, maybe empty) into one strong
/// image prompt; the answer is the prompt text only.
pub fn improve_prompt(b: &Brief, draft: &str) -> (String, String) {
    let (_, context) = concepts_prompt(b);
    let names: Vec<String> = b.cast.iter().map(|c| format!("@{}", handle(&c.name))).collect();
    let learned = super::click::model().thumbnail_digest();
    let system = format!(
        "You are a thumbnail art director for a top manhwa recap channel. You turn a rough idea into one image prompt that an image model (Nano Banana, GPT image, Seedream, Flux) renders into a thumbnail people click, while staying honest to the story.\n\n{CLICK_GUIDE}\n\n{learned}\n\
{RECIPE}{}\n\n\
Write the prompt's paragraphs, then on their own last two lines the strategy and the labels, exactly like this (the red part of the big label between stars):\nStrategy: awakening comparison\nLabels: *SSS*-RANK | D-RANK | C-RANK (hero left)\nName characters ONLY with these handles so their face references get attached: {}. Keep every handle the draft used, and the draft's strategy and labels when it has them. Reply with those things only: no heading, no quotes, no markdown.",
        strategy_table(),
        if names.is_empty() { "(no characters known yet)".to_string() } else { names.join(", ") }
    );
    let ask = if draft.trim().is_empty() {
        "Write the strongest thumbnail prompt for this manga's recap.".to_string()
    } else {
        format!("Improve this draft; keep its idea and characters:\n{}", draft.trim())
    };
    (system, format!("{context}\n{ask}"))
}

/// The AI's prompt without code fences, surrounding quotes, a "Prompt:" label or a JSON wrapper.
pub fn clean_prompt(raw: &str) -> Option<String> {
    let mut t = raw.trim();
    if let Some(v) = mv_recap::extract_json(t).filter(|_| t.starts_with('{') || t.starts_with("```")) {
        if let Some(p) = v.get("prompt").and_then(Value::as_str) {
            return Some(p.trim().to_string()).filter(|p| !p.is_empty());
        }
    }
    if t.starts_with("```") {
        t = t.trim_start_matches('`').trim_start_matches(|c: char| c.is_alphabetic()).trim_end_matches('`').trim();
    }
    for label in ["Prompt:", "prompt:", "**Prompt:**"] {
        t = t.strip_prefix(label).unwrap_or(t).trim();
    }
    let t = t.trim_matches(|c| c == '"' || c == '“' || c == '”').trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The AI's concepts and analysis, or `None` without a usable concept.
pub fn parse_concepts(raw: &str) -> Option<(String, Vec<Concept>)> {
    let v = mv_recap::extract_json(raw)?;
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let concepts: Vec<Concept> = v
        .get("concepts")
        .and_then(Value::as_array)?
        .iter()
        .map(|c| {
            // Words the model still wrote into the prompt come out of it and become the title.
            let (prompt, inside) = split_labels(&s(c, "prompt"));
            let text = Some(s(c, "text")).filter(|t| !t.is_empty()).unwrap_or_else(|| inside.label().to_string());
            let tags: Vec<String> = c.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).take(2).collect();
            let strategy = strategy(&s(c, "strategy")).map(|x| x.name.to_string()).unwrap_or_default();
            Concept { name: s(c, "name"), why: s(c, "why"), prompt, text, tags, side: s(c, "side"), key: s(c, "key").trim_matches('*').to_string(), strategy }
        })
        .filter(|c| !c.prompt.is_empty())
        .take(6)
        .collect();
    (!concepts.is_empty()).then(|| (s(&v, "analysis"), concepts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast() -> Vec<Cast> {
        vec![
            Cast { name: "Arhan Karavan".into(), aliases: vec!["Arhan".into()], ..Default::default() },
            Cast { name: "Ruko".into(), ..Default::default() },
        ]
    }

    #[test]
    fn mentions_map_to_reference_slots_in_order() {
        let (p, refs) = resolve("@Ruko watches @ArhanKaravan swing; @ruko gasps", &cast(), 0);
        assert_eq!(refs, vec![1, 0]);
        assert_eq!(p, "Ruko (the character in Image 1) watches Arhan Karavan (the character in Image 2) swing; Ruko (the character in Image 1) gasps");
    }

    #[test]
    fn first_names_numbers_and_unknowns() {
        let (p, refs) = resolve("@Arhan and @character2 meet @nobody at test@mail", &cast(), 1);
        assert_eq!(refs, vec![0, 1]);
        assert!(p.starts_with("Arhan Karavan (the character in Image 2)"));
        assert!(p.contains("@nobody") && p.contains("test@mail"));
    }

    #[test]
    fn cleans_the_ai_prompt() {
        assert_eq!(clean_prompt("```\n@Arhan walks\n```").as_deref(), Some("@Arhan walks"));
        assert_eq!(clean_prompt("Prompt: \"@Arhan walks\"").as_deref(), Some("@Arhan walks"));
        assert_eq!(clean_prompt(r#"{"prompt":"@Arhan walks"}"#).as_deref(), Some("@Arhan walks"));
        assert_eq!(clean_prompt("  "), None);
    }

    #[test]
    fn the_labels_ride_on_the_prompt_and_never_reach_the_image_model() {
        let c = Concept { prompt: "Exactly 2 people and nobody else: @Ruko, left third.".into(), text: "sss-rank".into(), key: "sss".into(), tags: vec!["d-rank".into()], side: "Left".into(), strategy: "Awakening Comparison".into(), ..Default::default() };
        let full = c.full_prompt();
        assert_eq!(full, "Exactly 2 people and nobody else: @Ruko, left third.\nStrategy: awakening comparison\nLabels: *sss*-rank | d-rank (hero left)");
        let (art, l) = split_labels(&full);
        assert_eq!((art.as_str(), l.label(), l.tags(), l.hero_left, l.key.as_str(), l.strategy.as_str()), (c.prompt.as_str(), "sss-rank", &["d-rank".to_string()][..], true, "sss", "awakening comparison"));
        // The hero's side, at most three words, an unknown strategy, setting labels again, and none.
        let r = split_labels("A scene.\nstrategy: dance battle\nlabels: \"F-RANK\" | TOP 1 | a | too many (hero right)").1;
        assert_eq!((r.hero_left, r.words.clone(), r.strategy.as_str(), r.key.as_str()), (false, vec!["F-RANK".to_string(), "TOP 1".into(), "a".into()], "", ""));
        let again = Labels { words: vec!["BORED".into()], hero_left: false, ..Default::default() };
        assert_eq!(with_labels(&full, &again), "Exactly 2 people and nobody else: @Ruko, left third.\nLabels: BORED (hero right)");
        assert_eq!(with_labels(&full, &Labels { words: vec![" ".into()], ..Default::default() }), c.prompt);
        // A title line, and a prompt written the older way: one label each.
        assert_eq!(split_labels("A scene.\ntitle: \"He Wins\" key: wins").1.words, vec!["He Wins"]);
        let (art, l) = split_labels("@Ruko grins, right third, bold yellow text: \"HE EATS SWORDS\". Onlookers gasp.");
        assert_eq!((art.as_str(), l.label()), ("@Ruko grins, right third. Onlookers gasp.", "HE EATS SWORDS"));
        assert_eq!(split_labels("Just a scene."), ("Just a scene.".to_string(), Labels { hero_left: true, ..Default::default() }));

        // With style pictures attached the style is theirs: no style words, the characters after them.
        let p = image_prompt("Sword Eater", &full, &[("Ruko".into(), "black hair".into())], 2, 2);
        assert!(p.starts_with(SAME_ARTIST) && p.contains("Images 1 to 2 are the art style references.") && p.contains("Image 3: Ruko, black hair"), "{p}");
        assert!(p.contains("Exactly 2 people") && p.contains("Absolutely no text") && p.contains("no sparkles"), "{p}");
        assert!(!p.to_lowercase().contains("sss-rank") && !p.contains("Labels:") && !p.contains("Strategy:") && !p.contains("glossy"), "{p}");
        // Without any, the style is described.
        let plain = image_prompt("Sword Eater", &full, &[], 0, 0);
        assert!(plain.contains("modern anime key-visual style") && !plain.contains("same artist"), "{plain}");
        assert!(edit_prompt("make her jacket red\nLabels: X").contains("Make only this change: make her jacket red\n"));
        // The AI is told the same, with every strategy.
        let b = Brief { title: "T", cast: &cast(), story: &[], comparable: &[], reference: &[], hook: "" };
        let (system, _) = concepts_prompt(&b);
        assert!(system.contains("A STORY, NOT A POSE") && system.contains("NEVER a white") && system.contains("AT MOST two small tags") && system.contains("adults only") && !system.contains("bold text:"), "{system}");
        assert!(STRATEGIES.iter().all(|s| system.contains(&format!("\"{}\"", s.name))), "{system}");
        assert!(improve_prompt(&b, "").0.contains("Labels: *SSS*-RANK | D-RANK | C-RANK (hero left)"));
        // Every strategy has its style pair; a name finds its strategy whatever its case.
        assert!(STRATEGIES.iter().all(|s| s.refs.iter().all(|r| super::super::magnific::youtube_id(r).as_deref() == Some(*r))));
        assert_eq!(strategy(" Beaten Champion ").map(|s| s.refs[1]), Some("QxZWEjXxdjc"));
        assert_eq!(STRATEGIES.last().map(|s| s.name), Some("hero and admirers"));
        // What the owner's own clicks taught comes first.
        assert_eq!(STRATEGIES[0].name, "glamorous reaction");
        assert!(system.contains("FUNNY AND FLIRTY") && system.contains("high-key") && system.contains("a quarter of the frame"), "{system}");
        assert!(placement_prompt(&Labels::default()).0.contains("INSIDE it"));
    }

    #[test]
    fn a_model_that_sees_the_picture_crops_it_and_places_the_labels() {
        // The crop: 16:9 from its width, moved inside the frame; none when it is barely a crop.
        assert!(crop_prompt().0.contains("at least 820 wide"));
        assert_eq!(parse_crop(r#"{"crop":[200,100,1000,500]}"#), Some([200., 100., 1000., 562.5]));
        assert_eq!(parse_crop(r#"{"crop":[600,400,1000,562]}"#), Some([280., 157.5, 1000., 562.5]));
        assert_eq!(parse_crop(r#"{"crop":[0,0,1280,720]}"#), None);
        assert_eq!(parse_crop(r#"{"crop":[0,0]}"#), None);
        assert_eq!(parse_crop("no"), None);

        let labels = Labels { words: vec!["sss-rank".into(), "d-rank".into(), "c-rank".into()], hero_left: true, key: "sss".into(), ..Default::default() };
        let (system, user) = placement_prompt(&labels);
        assert!(system.contains("1280 wide and 720 high") && system.contains("about 128 high") && user.contains("\"SSS-RANK\"") && user.contains("Small tag 2: \"C-RANK\"") && user.contains("on the left"));
        let l = parse_placement(r#"Here: {"points_at":[312,84],"label":{"x":372,"y":4,"size":128},"tags":[{"x":384,"y":200,"size":50},{"x":900,"y":22,"size":46}]}"#, &labels).unwrap();
        assert_eq!(l.labels.iter().map(|x| (x.text.as_str(), x.x, x.y, x.size, x.key.as_str())).collect::<Vec<_>>(), vec![("sss-rank", 372., 4., 128., "sss"), ("d-rank", 384., 200., 50., ""), ("c-rank", 900., 22., 46., "")]);
        // The arrow runs from the label's near end to what it names.
        assert_eq!((l.arrows.len(), l.arrows[0].to), (1, [312., 84.]));
        assert!(l.arrows[0].from[0] < 372. && l.arrows[0].from[0] > 312.);
        // A target under the label: the arrow drops from under it. The older key still works.
        let down = parse_placement(r#"{"hero_head":[520,436],"label":{"x":400,"y":0,"size":128}}"#, &labels).unwrap();
        assert!(down.arrows[0].from[1] > 128. && (down.arrows[0].from[0] - 520.).abs() < 1.);
        // Off the frame, over the length badge, or on top of each other: not used.
        assert!(parse_placement(r#"{"label":{"x":1250,"y":700,"size":128}}"#, &labels).is_none());
        assert!(parse_placement(r#"{"label":{"x":372,"y":4},"tags":[{"x":380,"y":10}]}"#, &labels).is_none());
        assert!(parse_placement("no json", &labels).is_none());
        // A tag that cannot be placed is left out; a label put too far right is made smaller.
        let l = parse_placement(r#"{"label":{"x":800,"y":10,"size":140},"tags":[{"x":9000,"y":1}]}"#, &labels).unwrap();
        assert_eq!(l.labels.len(), 1);
        assert!(l.labels[0].size < 140. && l.arrows.is_empty());
    }

    #[test]
    fn concepts_parse() {
        let raw = r#"Here: {"analysis":"Sell the sword.","concepts":[{"name":"Onlookers","why":"status","prompt":"@Arhan walks","text":"HE EATS SWORDS"},{"name":"x","prompt":""}]}"#;
        let (a, c) = parse_concepts(raw).unwrap();
        assert_eq!(a, "Sell the sword.");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "HE EATS SWORDS");
        // Words left inside a prompt become the title; the picture is asked for without them.
        let (_, c) = parse_concepts(r#"{"concepts":[{"name":"n","strategy":"Number Gap","prompt":"@Arhan grins, bold text: \"LEVEL 999\".","key":"*999*","tags":["TOP 1"," ","S-RANK","x"],"side":"right"}]}"#).unwrap();
        assert_eq!((c[0].prompt.as_str(), c[0].text.as_str(), c[0].tags.clone(), c[0].side.as_str(), c[0].key.as_str(), c[0].strategy.as_str()), ("@Arhan grins.", "LEVEL 999", vec!["TOP 1".to_string(), "S-RANK".to_string()], "right", "999", "number gap"));
        assert!(c[0].full_prompt().ends_with("Strategy: number gap\nLabels: LEVEL *999* | TOP 1 | S-RANK (hero right)"));
    }
}
