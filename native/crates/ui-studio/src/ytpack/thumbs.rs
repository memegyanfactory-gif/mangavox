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
    /// Words printed on the thumbnail.
    pub text: String,
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

/// The full prompt sent to the image model: the look of the thumbnail, then the user's prompt.
/// `refs` are the characters' names and looks, from Image `first + 1` on.
pub fn image_prompt(title: &str, resolved: &str, refs: &[(String, String)], first: usize) -> String {
    let mut p = format!(
        "YouTube thumbnail (16:9) for a manhwa recap video of \"{}\". Korean webtoon art style, bold clean line art, cinematic rim light, high contrast, saturated colours, sharp focus, readable at a small size.\n\n{}",
        title.trim(),
        resolved.trim()
    );
    if !refs.is_empty() {
        p.push_str("\n\nCharacter references (keep each face, hair and outfit exactly like its image):");
        for (k, (name, look)) in refs.iter().enumerate() {
            p.push_str(&format!("\n- Image {}: {name}{}", first + k + 1, if look.is_empty() { String::new() } else { format!(", {look}") }));
        }
    }
    p.push_str("\n\nNo watermark, no logo, no small text, no extra fingers, no blur.");
    p
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
Rules: one clear scene, readable faces, 2-3 dominant colours, yellow text with a thick black outline, 1-4 big words or one short line of dialogue, nothing small. Never invent powers, weapons, places or events that are not in the story: exaggerate what is there.";

/// System and user prompt for 4 thumbnail concepts; the answer is JSON only.
pub fn concepts_prompt(b: &Brief) -> (String, String) {
    let names: Vec<String> = b.cast.iter().map(|c| format!("@{}", handle(&c.name))).collect();
    let learned = super::click::model().thumbnail_digest();
    let system = format!(
        "You are a thumbnail art director for a top manhwa recap channel. You know why viewers click and you design thumbnails that are clickbait but honest to the story.\n\n{CLICK_GUIDE}\n\n{learned}\n\
Write 6 different concepts, each from a different pattern that fits this manga; where the craft guide and the learned rules disagree, the learned rules win (they are measured). Name the visual features plainly in each prompt (close-up, onlookers, aura, arrow, the text colour). In each prompt, name characters ONLY with these handles so their face references get attached: {}. Describe pose, expression, what they hold, the background, the lighting and the colours concretely, in one paragraph an image model can follow. Put the words for the thumbnail in `text` and also in the prompt as \"bold text: ...\".\n\
Reply with JSON only: {{\"analysis\":\"2-3 sentences: what this manga's thumbnails should sell and why\",\"concepts\":[{{\"name\":\"short name\",\"why\":\"one sentence on why it gets the click\",\"prompt\":\"\",\"text\":\"\"}}]}}",
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
Write ONE paragraph, 60-140 words: the focal character's pose and expression, what they hold or do, who reacts behind them and how, the background, the lighting and the 2-3 colours, and the huge text as `bold text: \"...\"` (2-4 words). Name characters ONLY with these handles so their face references get attached: {}. Keep every handle the draft used. Reply with the prompt only: no title, no quotes, no markdown.",
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
        .map(|c| Concept { name: s(c, "name"), why: s(c, "why"), prompt: s(c, "prompt"), text: s(c, "text") })
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
    fn concepts_parse() {
        let raw = r#"Here: {"analysis":"Sell the sword.","concepts":[{"name":"Onlookers","why":"status","prompt":"@Arhan walks","text":"HE EATS SWORDS"},{"name":"x","prompt":""}]}"#;
        let (a, c) = parse_concepts(raw).unwrap();
        assert_eq!(a, "Sell the sword.");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "HE EATS SWORDS");
    }
}
