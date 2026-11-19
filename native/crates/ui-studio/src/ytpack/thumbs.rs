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
}

impl Concept {
    /// The prompt as the prompt box holds it: the art, then its labels line (see [`split_labels`]).
    pub fn full_prompt(&self) -> String {
        let mut labels = vec![self.text.clone()];
        labels.extend(self.tags.iter().cloned());
        with_labels(&self.prompt, &labels, self.side.trim().to_lowercase() != "right")
    }
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
    /// The big label on the hero first, then the small tags.
    pub words: Vec<String>,
    /// The hero is on the left of the picture (the labels are laid out from his side).
    pub hero_left: bool,
}

impl Labels {
    pub fn label(&self) -> &str {
        self.words.first().map(String::as_str).unwrap_or("")
    }
    pub fn tags(&self) -> &[String] {
        self.words.get(1..).unwrap_or(&[])
    }
}

/// `prompt` with its labels as the last line: `Labels: JOBLESS | BLANK CHECK | SUSPICIOUS... (hero left)`.
/// The picture is drawn without any words (image models misspell them); the app letters them on.
pub fn with_labels(prompt: &str, words: &[String], hero_left: bool) -> String {
    let (art, _) = split_labels(prompt);
    let words: Vec<&str> = words.iter().map(|w| w.trim().trim_matches('"')).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return art;
    }
    format!("{art}\nLabels: {} (hero {})", words.join(" | "), if hero_left { "left" } else { "right" })
}

fn quoted(s: &str) -> Option<(usize, usize)> {
    let open = s.find(['"', '\u{201c}'])?;
    let from = open + s[open..].chars().next().map_or(1, char::len_utf8);
    let len = s[from..].find(['"', '\u{201d}'])?;
    Some((from, from + len))
}

/// A prompt as (the art to draw, the labels to letter on it). The labels are the `Labels:` line
/// (`Title: "..."` is read as one label); a prompt written the older way, with the words inside it
/// (`bold text: "..."`), gives them up the same way, so the image model is never asked to write.
pub fn split_labels(prompt: &str) -> (String, Labels) {
    let mut art: Vec<&str> = vec![];
    let mut out = Labels { words: vec![], hero_left: true };
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
    p.push_str("\n\nAbsolutely no text anywhere: no letters, no numbers, no labels, no arrows, no speech bubbles, no sound effects, no signs with writing, no signature, no watermark, no logo, no border.");
    p
}

/// The edit request for a made thumbnail (Image 1): only the change asked for.
pub fn edit_prompt(change: &str) -> String {
    let (change, _) = split_labels(change);
    format!("Edit Image 1. Keep everything exactly as it is: the same characters, faces, hair, poses and positions, the same background and the same art style. Make only this change: {}\nDo not add or remove anyone or anything else. No text, no letters, no arrows, no watermark, no border.", change.trim())
}

// ---------------------------------------------------------------- where the labels go

/// Asks a model that can see the finished picture (attached, 1280x720) where its labels go.
pub fn placement_prompt(labels: &Labels) -> (String, String) {
    let system = "You place the labels on a finished YouTube thumbnail, the way the big manhwa recap channels do. You are shown the picture; it is 1280 wide and 720 high, x from the left, y from the top. Labels are heavy yellow capitals with a black outline.\n\
- The BIG label (letters about 96 high, about 44 wide each) sits in the top part of the picture right beside the main character's head, on the side away from the frame edge, over background, never over a face. An arrow runs from its near end to the edge of his hair.\n\
- Each SMALL tag (letters about 42 high, about 20 wide each) sits beside the person or object it names, over background, hair or clothes, never over a face or a hand.\n\
- Nothing in the bottom-right corner (YouTube puts the video length there), nothing cut by the frame, labels never overlap.\n\
Reply with JSON only: {\"hero_head\":[x,y],\"label\":{\"x\":0,\"y\":0,\"size\":96},\"tags\":[{\"x\":0,\"y\":0,\"size\":42}]} where each x,y is the label's top-left corner and hero_head is the point on the edge of his hair the arrow points at. Give one entry in \"tags\" per small tag, in the order given.";
    let mut user = format!("The big label, on the main character (he is on the {} of the picture): \"{}\"\n", if labels.hero_left { "left" } else { "right" }, labels.label().to_uppercase());
    for (k, t) in labels.tags().iter().enumerate() {
        user.push_str(&format!("Small tag {}: \"{}\"\n", k + 1, t.to_uppercase()));
    }
    user.push_str("Look at the picture and place them.");
    (system.to_string(), user)
}

/// The model's placement as a lettering, or `None` when it is unusable (then the default places
/// are kept): every label inside the frame, a readable size, and no two labels overlapping.
pub fn parse_placement(raw: &str, labels: &Labels) -> Option<mv_recap::thumb::Lettering> {
    use mv_recap::thumb::{label_width, Arrow, Label, Lettering, LETTER_H, LETTER_W};
    let v = mv_recap::extract_json(raw)?;
    let num = |o: &Value, k: &str| o.get(k).and_then(Value::as_f64).map(|n| n as f32);
    let place = |o: &Value, text: &str, lo: f32, hi: f32, base: f32| -> Option<Label> {
        let mut size = num(o, "size").unwrap_or(base).clamp(lo, hi);
        let (x, y) = (num(o, "x")?, num(o, "y")?);
        // Too wide for where it was put: smaller, down to the smallest readable.
        let room = LETTER_W - 8. - x;
        let w = label_width(text, size);
        if w > room {
            size = (size * room / w).max(lo * 0.8);
        }
        let l = Label { text: text.to_string(), x, y, size };
        (x >= 0. && y >= -4. && x + label_width(text, size) <= LETTER_W && y + size * 1.1 <= LETTER_H && !(x + label_width(text, size) > 1040. && y + size > 600.)).then_some(l)
    };
    let label = labels.label();
    if label.is_empty() {
        return None;
    }
    let big = place(v.get("label")?, label, 64., 110., 96.)?;
    let mut out = Lettering { labels: vec![big.clone()], arrows: vec![] };
    let head = v.get("hero_head").and_then(Value::as_array).filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32]));
    if let Some(to) = head.filter(|h| (0. ..=LETTER_W).contains(&h[0]) && (0. ..=LETTER_H).contains(&h[1])) {
        // From the label's end nearest the head, at mid height, stopping short of the hair.
        let (left, right) = (big.x - 6., big.x + label_width(label, big.size) + 6.);
        let from = [if (to[0] - left).abs() <= (to[0] - right).abs() { left } else { right }, big.y + big.size * 0.6];
        let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
        if len >= 30. {
            out.arrows.push(Arrow { from, to, width: 22. });
        }
    }
    let tags = v.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
    for (text, o) in labels.tags().iter().zip(&tags) {
        if let Some(l) = place(o, text, 30., 52., 42.) {
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
Overused by small channels, and below average in the data: the generic aura / lightning power pose, the hero alone holding a weapon, split or multi-panel frames, dark caves and night voids, status windows and stat numbers, kneeling crowds, tiny-hero-versus-giant shots. Use one only when it IS this story's hook, and even then put people reacting in the frame.\n\
Rules: one clear scene, readable faces, 2-3 dominant colours, nothing small. Never invent powers, weapons, places or events that are not in the story: exaggerate what is there.";

/// How a concept is designed and its prompt written: the recipe the channel's own accepted
/// thumbnails are made with (each rule is there because breaking it gave a rejected picture).
const RECIPE: &str = "HOW TO DESIGN EACH ONE\n\
- THE CAST IS REAL: only this series' own characters, by their handles, each with their real hair, eyes, accessories and outfit from the character list. A fan must recognise everyone at a glance.\n\
- THE HERO: very large in the foreground at one side (the left or right third), from the chest up, cropped by the frame. A cold, unimpressed, closed-mouth face, his fringe throwing a shadow across his eyes; turned three-quarters, looking down or away and ignoring everyone. ONE power cue only that is true to the story (one glowing eye with a flame trail, an orb in his hand, an aura). He never grins at the viewer.\n\
- THE OTHERS: two or three of the series' other main characters filling the rest of the frame at slightly different depths, large: each from the top of the head near the top edge down to the thighs. Each has a DIFFERENT attitude towards the hero (smug, annoyed with arms crossed, flirty, mocking, suspicious): several are judging or underestimating him, and that is the hook. When the story has adult women around the hero, they are the ones to draw, as the main attraction: glamorous grown adults with a mature beautiful face and a very voluptuous anime hourglass figure (a full bust, a narrow waist, wide hips, thick thighs), in their own outfit cut tight and fashionable, fully clothed, in a casual confident pose (hand on hip, arms crossed with a hip cocked, twirling hair, looking over a shoulder). Limits that always hold: adults only, clothed, nothing explicit or see-through; a character who is a child or teenager in the story is drawn plainly or left out, and a story without adult women gets rivals, mentors or enemies instead.\n\
- THE PLACE: a crisp, detailed anime background from the story with bright daylight and a vivid blue sky with clouds (tall windows over a skyline, a courtyard, an arena, a classroom). Never a plain wall, never night without a strong glow.\n\
- CLEAN: no sparkles, hearts, particles or wind effects. One glow, everything else crisp.\n\
HOW TO WRITE ITS PROMPT (four short paragraphs for an image model; no style words at all, the style comes from reference pictures)\n\
1. \"Exactly N people and nobody else: one young man and three adult women. No extra people, no duplicates.\"\n\
2. \"LEFT THIRD, foreground, very large, from the chest up and cropped by the left and bottom edges: ...\" the hero: age, hair, the shadow over his eyes, expression, where he looks, the one power cue, his clothes with colours.\n\
3. \"CENTRE AND RIGHT, ...\" the others, one line each (\"- Centre: ...\", \"- Far right, nearest the camera, cropped by the right edge: ...\"): who they are in a few words, hair, eyes, accessories, outfit with colours, pose, expression, and where they look.\n\
4. \"BACKGROUND: ...\".\n\
Simple poses, nothing in mid-air, and never any words, labels, arrows or bubbles to draw: the app letters them on afterwards, so they are always spelled right.\n\
THE LABELS (not a title; the picture carries the thumbnail and the words are tags)\n\
- `text`: ONE big label of one or two words that names the hero with irony: the weakest-sounding tag on the strongest person (JOBLESS, F-RANK, NEW STUDENT, BETRAYED, BORED, TRASH?). Never the manga's or a character's name.\n\
- `tags`: one or two small tags of one to three words, each for another character or an object in the picture (BLANK CHECK on the cheque she holds, SUSPICIOUS... on the one glaring, S-RANK, TOP 1).\n\
- `side`: \"left\" or \"right\", the side the hero is on.";

/// System and user prompt for 4 thumbnail concepts; the answer is JSON only.
pub fn concepts_prompt(b: &Brief) -> (String, String) {
    let names: Vec<String> = b.cast.iter().map(|c| format!("@{}", handle(&c.name))).collect();
    let learned = super::click::model().thumbnail_digest();
    let system = format!(
        "You are a thumbnail art director for a top manhwa recap channel. You know why viewers click and you design thumbnails that are clickbait but honest to the story.\n\n{CLICK_GUIDE}\n\n{learned}\n\
{RECIPE}\n\n\
First find this story's hook: its premise in one sentence and what everyone around the hero gets wrong about him. Then write 6 different concepts that sell that hook, the strongest first: vary who stands with the hero, their attitudes, the place, his power cue and the labels. Where this recipe and the older craft guide above disagree, the recipe wins (it is what the channel's accepted thumbnails are made with). In each prompt, name characters ONLY with these handles so their face references get attached: {}.\n\
Reply with JSON only: {{\"analysis\":\"2-3 sentences: the hook and what this manga's thumbnails should sell\",\"concepts\":[{{\"name\":\"short name\",\"why\":\"one sentence: who is in it, what each thinks of him, and why it gets the click\",\"prompt\":\"\",\"text\":\"JOBLESS\",\"tags\":[\"BLANK CHECK\",\"SUSPICIOUS...\"],\"side\":\"left\"}}]}}",
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
{RECIPE}\n\n\
Write the prompt's paragraphs, then on its own last line the labels, exactly like this: Labels: JOBLESS | BLANK CHECK | SUSPICIOUS... (hero left)\nName characters ONLY with these handles so their face references get attached: {}. Keep every handle the draft used, and the draft's labels when it has them. Reply with those two things only: no heading, no quotes, no markdown.",
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
            Concept { name: s(c, "name"), why: s(c, "why"), prompt, text, tags, side: s(c, "side") }
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
        let c = Concept { prompt: "Exactly 2 people and nobody else: @Ruko, left third.".into(), text: "jobless".into(), tags: vec!["blank check".into()], side: "Left".into(), ..Default::default() };
        let full = c.full_prompt();
        assert_eq!(full, "Exactly 2 people and nobody else: @Ruko, left third.\nLabels: jobless | blank check (hero left)");
        let (art, l) = split_labels(&full);
        assert_eq!((art.as_str(), l.label(), l.tags(), l.hero_left), (c.prompt.as_str(), "jobless", &["blank check".to_string()][..], true));
        // The hero's side, setting labels again, and no labels.
        assert!(!split_labels("A scene.\nlabels: \"F-RANK\" | TOP 1 | a | too many (hero right)").1.hero_left);
        assert_eq!(split_labels("A scene.\nlabels: \"F-RANK\" | TOP 1 | a | too many (hero right)").1.words, vec!["F-RANK", "TOP 1", "a"]);
        assert_eq!(with_labels(&full, &["BORED".into()], false), "Exactly 2 people and nobody else: @Ruko, left third.\nLabels: BORED (hero right)");
        assert_eq!(with_labels(&full, &[" ".into()], true), c.prompt);
        // A title line, and a prompt written the older way: one label each.
        assert_eq!(split_labels("A scene.\ntitle: \"He Wins\" key: wins").1.words, vec!["He Wins"]);
        let (art, l) = split_labels("@Ruko grins, right third, bold yellow text: \"HE EATS SWORDS\". Onlookers gasp.");
        assert_eq!((art.as_str(), l.label()), ("@Ruko grins, right third. Onlookers gasp.", "HE EATS SWORDS"));
        assert_eq!(split_labels("Just a scene."), ("Just a scene.".to_string(), Labels { words: vec![], hero_left: true }));

        // With style pictures attached the style is theirs: no style words, the characters after them.
        let p = image_prompt("Sword Eater", &full, &[("Ruko".into(), "black hair".into())], 2, 2);
        assert!(p.starts_with(SAME_ARTIST) && p.contains("Images 1 to 2 are the art style references.") && p.contains("Image 3: Ruko, black hair"), "{p}");
        assert!(p.contains("Exactly 2 people") && p.contains("Absolutely no text") && p.contains("no sparkles"), "{p}");
        assert!(!p.to_lowercase().contains("jobless") && !p.contains("Labels:") && !p.contains("glossy"), "{p}");
        // Without any, the style is described.
        let plain = image_prompt("Sword Eater", &full, &[], 0, 0);
        assert!(plain.contains("modern anime key-visual style") && !plain.contains("same artist"), "{plain}");
        assert!(edit_prompt("make her jacket red\nLabels: X").contains("Make only this change: make her jacket red\n"));
        // The AI is told the same.
        let b = Brief { title: "T", cast: &cast(), story: &[], comparable: &[], reference: &[], hook: "" };
        let (system, _) = concepts_prompt(&b);
        assert!(system.contains("THE CAST IS REAL") && system.contains("Exactly N people") && system.contains("\"tags\"") && system.contains("adults only") && !system.contains("bold text:"), "{system}");
        assert!(improve_prompt(&b, "").0.contains("Labels: JOBLESS | BLANK CHECK | SUSPICIOUS... (hero left)"));
    }

    #[test]
    fn a_model_that_sees_the_picture_places_the_labels() {
        let labels = Labels { words: vec!["jobless".into(), "blank check".into(), "suspicious...".into()], hero_left: true };
        let (system, user) = placement_prompt(&labels);
        assert!(system.contains("1280 wide and 720 high") && user.contains("\"JOBLESS\"") && user.contains("Small tag 2: \"SUSPICIOUS...\"") && user.contains("on the left"));
        let l = parse_placement(r#"Here: {"hero_head":[312,84],"label":{"x":372,"y":4,"size":96},"tags":[{"x":384,"y":146,"size":42},{"x":884,"y":22,"size":38}]}"#, &labels).unwrap();
        assert_eq!(l.labels.iter().map(|x| (x.text.as_str(), x.x, x.y, x.size)).collect::<Vec<_>>(), vec![("jobless", 372., 4., 96.), ("blank check", 384., 146., 42.), ("suspicious...", 884., 22., 38.)]);
        // The arrow runs from the label's near end to his hair.
        assert_eq!((l.arrows.len(), l.arrows[0].to), (1, [312., 84.]));
        assert!(l.arrows[0].from[0] < 372. && l.arrows[0].from[0] > 312.);
        // Off the frame, over the length badge, or on top of each other: not used.
        assert!(parse_placement(r#"{"label":{"x":1250,"y":700,"size":96}}"#, &labels).is_none());
        assert!(parse_placement(r#"{"label":{"x":372,"y":4},"tags":[{"x":380,"y":10}]}"#, &labels).is_none());
        assert!(parse_placement("no json", &labels).is_none());
        // A tag that cannot be placed is left out; a label put too far right is made smaller.
        let l = parse_placement(r#"{"label":{"x":1040,"y":10,"size":110},"tags":[{"x":9000,"y":1}]}"#, &labels).unwrap();
        assert_eq!(l.labels.len(), 1);
        assert!(l.labels[0].size < 110. && l.arrows.is_empty());
    }

    #[test]
    fn concepts_parse() {
        let raw = r#"Here: {"analysis":"Sell the sword.","concepts":[{"name":"Onlookers","why":"status","prompt":"@Arhan walks","text":"HE EATS SWORDS"},{"name":"x","prompt":""}]}"#;
        let (a, c) = parse_concepts(raw).unwrap();
        assert_eq!(a, "Sell the sword.");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "HE EATS SWORDS");
        // Words left inside a prompt become the title; the picture is asked for without them.
        let (_, c) = parse_concepts(r#"{"concepts":[{"name":"n","prompt":"@Arhan grins, bold text: \"LEVEL 999\".","tags":["TOP 1"," ","S-RANK","x"],"side":"right"}]}"#).unwrap();
        assert_eq!((c[0].prompt.as_str(), c[0].text.as_str(), c[0].tags.clone(), c[0].side.as_str()), ("@Arhan grins.", "LEVEL 999", vec!["TOP 1".to_string(), "S-RANK".to_string()], "right"));
        assert!(c[0].full_prompt().ends_with("Labels: LEVEL 999 | TOP 1 | S-RANK (hero right)"));
    }
}
