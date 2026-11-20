//! The writing steps of an anime chapter: the cast and places gathered from the pages, the
//! scenes, each scene's storyboard, and its revision after the episode director's check. Each
//! step is a prompt (system, user) and a function that tidies the AI's answer into the project:
//! our vocabulary, the cast's own names, panels that exist, nothing made taken from the answer.
use std::collections::HashMap;
use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::Value;

use super::{lenient_names, norm_angle, norm_camera, norm_grade, norm_kind, norm_lens, norm_motion, norm_size, norm_transition, pacing, panel_digest, recipes, said_aloud, Cut, FxElement, Plate, Scene, GRADES, MOTIONS, MOVES, SIZES, TRANSITIONS};
use crate::project::{lenient_text, lenient_u32, Bible, CastMember, Location, Outfit, Project};

/// The most story panels one scene is boarded from at once (a longer one is split into parts).
pub const SCENE_PANELS: usize = 26;

/// How much the director may add to the manga.
pub const EXPANSIONS: [&str; 3] = ["faithful", "cinematic", "expanded"];

pub fn expansion_rule(level: &str) -> &'static str {
    match level.trim() {
        "cinematic" => "You may also show, briefly, what the manga skips between its panels (how a wound was got, the walk to a place), up to a fifth more length. No new dialogue beyond a word, a breath or a cry.",
        "expanded" => "You may add short original beats in the manga's spirit that deepen the world or a side character (a glimpse of someone's family, the place seen from outside), up to a third more length.",
        _ => "Add only what a film needs to tell the same story: establishing shots, reactions, inserts and stills of what is spoken of. No new events and no new lines.",
    }
}

// ------------------------------------------------------------------ the cast, from the pages

/// Names in the chapter's panels that the cast does not know (people to design before the
/// board): who is drawn and who speaks aloud.
pub fn unknown_people(p: &Project, chapter: u32) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let Some(ch) = p.chapter(chapter) else { return out };
    let mut add = |name: &str| {
        let n = name.trim();
        let skip = n.is_empty() || matches!(n.to_lowercase().as_str(), "narrator" | "system" | "?" | "unknown" | "sfx" | "crowd" | "everyone" | "all");
        if !skip && p.character(n).is_none() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            out.push(n.to_string());
        }
    };
    for sp in ch.anime.panels.iter().filter(|sp| sp.story()) {
        for c in &sp.characters {
            add(&c.name);
        }
        for l in sp.lines.iter().filter(|l| said_aloud(l)) {
            add(&l.speaker);
        }
    }
    out
}

const CAST_SHAPE: &str = r#"{"title": "chapter title, if the pages give one", "summary": "what happens in this chapter, one paragraph", "bible": {"logline": "the series in one sentence", "genre": "", "tone": "", "themes": "", "art_style": "the anime's look: character design line, colour design, background art, light", "world": "the rules of this world as the pages show them"}, "cast": [{"name": "the name the dialogue uses; if never named, a short handle such as 'Scarred hunter'", "aliases": ["every other label the panel notes use for this same person"], "role": "protagonist|deuteragonist|antagonist|main|supporting|minor", "gender": "male|female", "age": "", "personality": "", "appearance": "face, hair and its colour, eyes and their colour, build, height, marks: what never changes", "outfits": [{"name": "short name", "look": "what they wear in this chapter, every piece with its colour"}], "signature": "items and tells that identify them"}], "locations": [{"name": "", "look": "architecture, materials, colours, light: enough to paint a background of it"}]}"#;

/// The cast and places of a chapter, gathered from its panels as they were read (and from what
/// Studio knows of the manga, `notes`, when it has read it).
pub fn cast_from_pages(p: &Project, chapter: u32, notes: &str) -> (String, String) {
    let system = format!(
        "You are the character designer and setting researcher (設定) of a TV-anime adaptation. From the manga's own pages, as your assistant read them panel by panel, you write the character and place bible the whole production draws from.\nAnswer with ONE JSON object, no other text:\n{CAST_SHAPE}\nRules:\n- One entry per distinct person. Merge labels that are clearly the same person (the same look, outfit and part in neighbouring panels), and list every such label under \"aliases\".\n- Keep the names of the cast that already exists exactly as given, and do not repeat those people unless you add something (a new outfit, a look that was missing); add the people who are new.\n- At most two people are protagonist or deuteragonist. List only people who speak or act; a crowd is one \"minor\" entry at most.\n- Write looks concretely and only from what the panel notes show (the pages are in colour: use the colours noted). Leave a field empty rather than invent it.\n- Appearance is an animator's design note: shapes and colours someone could draw the same way a thousand times.\n- Locations: every place this chapter plays in, once each."
    );
    let ch = p.chapter(chapter).cloned().unwrap_or_default();
    let mut user = format!("Manga: {}. Chapter {chapter}.\n", p.adapts.as_deref().unwrap_or(&p.title));
    if !p.cast.is_empty() {
        user.push_str("\nThe cast so far (keep these names):\n");
        for c in &p.cast {
            let _ = writeln!(user, "- {} ({}): {} | outfits: {}", c.name, c.role, c.appearance.chars().take(200).collect::<String>(), c.outfits.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    if !p.locations.is_empty() {
        let _ = writeln!(user, "\nPlaces so far: {}", p.locations.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", "));
    }
    let unknown = unknown_people(p, chapter);
    if !unknown.is_empty() && !p.cast.is_empty() {
        let _ = writeln!(user, "\nLabels in the panel notes that the cast does not know yet: {}", unknown.join("; "));
    }
    if !notes.trim().is_empty() {
        let _ = write!(user, "\nWhat is already known of this manga (use it for names and roles; the pages decide the looks):\n{}\n", notes.trim());
    }
    let _ = write!(user, "\nThe chapter's panels (#number [page] what is drawn | who | where | lines | sfx | mood):\n{}", panel_digest(&ch.anime.panels, 0, ch.anime.panels.len()));
    (system, user)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CastEntry {
    #[serde(deserialize_with = "lenient_text")]
    name: String,
    #[serde(deserialize_with = "lenient_names")]
    aliases: Vec<String>,
    #[serde(deserialize_with = "lenient_text")]
    role: String,
    #[serde(deserialize_with = "lenient_text")]
    gender: String,
    #[serde(deserialize_with = "lenient_text")]
    age: String,
    #[serde(deserialize_with = "lenient_text")]
    personality: String,
    #[serde(deserialize_with = "lenient_text")]
    appearance: String,
    outfits: Vec<Outfit>,
    #[serde(deserialize_with = "lenient_text")]
    signature: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CastAnswer {
    #[serde(deserialize_with = "lenient_text")]
    title: String,
    #[serde(deserialize_with = "lenient_text")]
    summary: String,
    bible: Bible,
    cast: Vec<CastEntry>,
    locations: Vec<Location>,
}

/// Adds what the pages showed to the project: new people (their sheets are painted before the
/// cuts that need them), looks and outfits that were missing, places, the chapter's summary;
/// then every panel's people get their cast names. Returns how many people were added.
pub fn apply_cast(p: &mut Project, chapter: u32, v: Value) -> anyhow::Result<usize> {
    let a: CastAnswer = serde_json::from_value(v)?;
    if a.cast.is_empty() && p.cast.is_empty() {
        anyhow::bail!("the AI found no characters in the pages");
    }
    let mut alias: HashMap<String, String> = HashMap::new();
    let mut added = 0;
    for e in a.cast {
        let name = e.name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        // The same person under the name we have, or under one of the aliases.
        // (By name: people added a moment ago have no id until the cast is normalised.)
        let known = p.character(&name).map(|c| c.name.clone()).or_else(|| e.aliases.iter().find_map(|x| p.character(x).map(|c| c.name.clone())));
        let canon = match known.and_then(|n| p.cast.iter_mut().find(|c| c.name == n)) {
            Some(c) => {
                let fill = |have: &mut String, new: &str| {
                    if have.trim().is_empty() && !new.trim().is_empty() {
                        *have = new.trim().to_string();
                    }
                };
                fill(&mut c.role, &e.role);
                fill(&mut c.gender, &e.gender);
                fill(&mut c.age, &e.age);
                fill(&mut c.personality, &e.personality);
                fill(&mut c.appearance, &e.appearance);
                fill(&mut c.signature, &e.signature);
                for o in e.outfits {
                    if !o.name.trim().is_empty() && !o.look.trim().is_empty() && !c.outfits.iter().any(|x| x.name.eq_ignore_ascii_case(o.name.trim())) {
                        c.outfits.push(o);
                    }
                }
                c.name.clone()
            }
            None => {
                p.cast.push(CastMember { name: name.clone(), role: if e.role.trim().is_empty() { "supporting".into() } else { e.role }, gender: e.gender, age: e.age, intro_chapter: chapter, personality: e.personality, appearance: e.appearance, outfits: e.outfits.into_iter().filter(|o| !o.look.trim().is_empty()).collect(), signature: e.signature, ..Default::default() });
                added += 1;
                name.clone()
            }
        };
        alias.insert(name.to_lowercase(), canon.clone());
        for x in e.aliases {
            alias.insert(x.trim().to_lowercase(), canon.clone());
        }
    }
    for l in a.locations {
        if !l.name.trim().is_empty() && p.location(&l.name).is_none() {
            p.locations.push(Location { name: l.name.trim().to_string(), look: l.look, ..Default::default() });
        }
    }
    let fill = |have: &mut String, new: String| {
        if have.trim().is_empty() {
            *have = new;
        }
    };
    fill(&mut p.bible.logline, a.bible.logline);
    fill(&mut p.bible.genre, a.bible.genre);
    fill(&mut p.bible.tone, a.bible.tone);
    fill(&mut p.bible.themes, a.bible.themes);
    fill(&mut p.bible.art_style, a.bible.art_style);
    fill(&mut p.bible.world, a.bible.world);
    p.normalize();
    // Every panel's people under their cast names.
    let names: Vec<(String, String)> = p.cast.iter().map(|c| (c.name.to_lowercase(), c.name.clone())).collect();
    let resolve = |p: &Project, n: &str| -> Option<String> {
        let key = n.trim().to_lowercase();
        names.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).or_else(|| alias.get(&key).cloned()).or_else(|| p.character(n).map(|c| c.name.clone()))
    };
    let snapshot = p.clone();
    let ch = p.chapter_mut(chapter);
    fill(&mut ch.title, a.title);
    fill(&mut ch.summary, a.summary);
    ch.anime.cast_done = true;
    for sp in &mut ch.anime.panels {
        for c in &mut sp.characters {
            if let Some(n) = resolve(&snapshot, &c.name) {
                c.name = n;
            }
        }
        for l in &mut sp.lines {
            if let Some(n) = resolve(&snapshot, &l.speaker) {
                l.speaker = n;
            }
        }
    }
    Ok(added)
}

// ------------------------------------------------------------------ scenes

const SCENES_SHAPE: &str = r#"{"treatment": "how this chapter is told as an episode: its hook, its arc, its colour script and its last image, in a few sentences", "scenes": [{"title": "", "kind": "dialogue|action|dread|narration|flashback|montage", "location": "place name", "light": "the lighting state: time of day, the light source and its colour", "grade": "one colour grade from the list", "from": 1, "to": 9, "summary": "what happens, beat by beat", "characters": ["cast names in it"], "plan": "how it is told: what is kept, cut and added", "blocking": "the staging map: where each character and landmark stands in the place (left / right / near / far of each other), where they move, the objective or threat and which way it travels, and the side of the line the camera keeps", "music": "none, or the music's mood in a few words", "ambience": "the room tone: wind, crowd murmur, dripping cave"}]}"#;

/// The scenes of a chapter: its panels grouped by place, time and purpose, each with a kind
/// that sets its pacing and how much of it is animated.
pub fn scenes(p: &Project, chapter: u32) -> (String, String) {
    let system = format!(
        "You are the series-composition writer and director (シリーズ構成・監督) of a high-end TV anime adapting a manga. You divide one chapter into SCENES and decide how each is told, before any storyboard is drawn.\nAnswer with ONE JSON object, no other text:\n{SCENES_SHAPE}\nRules:\n- A scene is a run of consecutive panels in one place and time with one dramatic purpose. A change of place, a jump in time, a memory, a switch from talk to a fight (or back), or a strong change of light (a fire is lit, the power dies) starts a new scene.\n- \"from\" and \"to\" are panel numbers (as given, inclusive). Scenes follow the panels' order and together cover every panel exactly once.\n- kind: dialogue (people talk; daily life), action (a fight, a chase, physical danger happening now), dread (suspense, horror, a threat building), narration (the narrator or captions explain the world), flashback (a memory), montage (time passing in a series of images).\n- grade: the scene's single colour key, one of: {}. Safe daily life is warm_home or daylight; a dungeon is cold_dungeon; night scenes night; a hospital hospital; memories flashback; pure menace horror; bloodshed blood_red; the System's presence system_blue.\n- plan: a director's note for the storyboard. Say which beats to keep word for word, which gag or chibi drawings, logos and repeats to drop, and what to add. {}\n- An in-medias-res opening, a title logo or credits are not scenes of their own unless they tell story.\n- blocking: plan the staging BEFORE any shot, as a key animator does: a floor plan in words. Every cut of the scene will be framed from it, so the audience always knows where everyone is in relation to everything else.\n- The treatment names the episode's hook (its first image), where its tension peaks, and the one image it ends on.",
        GRADES.join(", "),
        expansion_rule(&p.settings.expansion),
    );
    let ch = p.chapter(chapter).cloned().unwrap_or_default();
    let mut user = format!("Anime of \"{}\", chapter {chapter}{}.\n", p.adapts.as_deref().unwrap_or(&p.title), if ch.title.is_empty() { String::new() } else { format!(": {}", ch.title) });
    if !ch.summary.trim().is_empty() {
        let _ = writeln!(user, "What happens: {}", ch.summary.trim());
    }
    let prev: Vec<String> = p.chapters.iter().filter(|c| c.number < chapter && !c.summary.trim().is_empty()).map(|c| format!("{}. {}", c.number, c.summary.trim())).collect();
    if !prev.is_empty() {
        let _ = writeln!(user, "Story so far: {}", prev.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" "));
    }
    if !p.locations.is_empty() {
        let _ = writeln!(user, "Places: {}", p.locations.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join("; "));
    }
    let _ = write!(user, "\nThe chapter's panels (#number [page] what is drawn | who | where | lines | sfx | mood):\n{}", panel_digest(&ch.anime.panels, 0, ch.anime.panels.len()));
    (system, user)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SceneIn {
    #[serde(deserialize_with = "lenient_text")]
    title: String,
    #[serde(deserialize_with = "lenient_text")]
    kind: String,
    #[serde(deserialize_with = "lenient_text")]
    location: String,
    #[serde(deserialize_with = "lenient_text")]
    light: String,
    #[serde(deserialize_with = "lenient_text")]
    grade: String,
    #[serde(deserialize_with = "lenient_u32")]
    from: u32,
    #[serde(deserialize_with = "lenient_u32")]
    to: u32,
    #[serde(deserialize_with = "lenient_text")]
    summary: String,
    #[serde(deserialize_with = "lenient_names")]
    characters: Vec<String>,
    #[serde(deserialize_with = "lenient_text")]
    plan: String,
    #[serde(deserialize_with = "lenient_text")]
    music: String,
    #[serde(deserialize_with = "lenient_text")]
    ambience: String,
    #[serde(deserialize_with = "lenient_text")]
    blocking: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ScenesAnswer {
    #[serde(deserialize_with = "lenient_text")]
    treatment: String,
    scenes: Vec<SceneIn>,
}

/// Stores the scenes: in panel order, covering every panel once (a gap goes to the scene
/// before it), each with its length worked out from the measured pacing. The old board goes.
pub fn apply_scenes(p: &mut Project, chapter: u32, v: Value) -> anyhow::Result<usize> {
    let a: ScenesAnswer = serde_json::from_value(v)?;
    let panels = p.chapter(chapter).map(|c| c.anime.panels.clone()).unwrap_or_default();
    let total = panels.len();
    if total == 0 {
        anyhow::bail!("the chapter has no panels yet");
    }
    let mut ins: Vec<SceneIn> = a.scenes.into_iter().filter(|s| s.from >= 1 && (s.from as usize) <= total).collect();
    if ins.is_empty() {
        anyhow::bail!("the AI named no scenes");
    }
    ins.sort_by_key(|s| s.from);
    let mut scenes: Vec<Scene> = vec![];
    for s in ins {
        let from = (s.from as usize - 1).max(scenes.last().map(|x| x.to).unwrap_or(0));
        let to = (s.to as usize).clamp(from, total);
        if to <= from {
            continue;
        }
        // Whatever lies between the scene before and this one belongs to the one before.
        if let Some(prev) = scenes.last_mut() {
            prev.to = from;
        }
        let cast: Vec<String> = s.characters.iter().map(|n| p.character(n).map(|c| c.name.clone()).unwrap_or_else(|| n.clone())).collect();
        scenes.push(Scene { number: 0, title: s.title, kind: norm_kind(&s.kind).into(), location: s.location, light: s.light, grade: norm_grade(&s.grade).into(), from, to, summary: s.summary, characters: cast, plan: s.plan, music: s.music, ambience: s.ambience, seconds: 0., blocking: s.blocking });
    }
    if let Some(first) = scenes.first_mut() {
        first.from = 0;
    }
    if let Some(last) = scenes.last_mut() {
        last.to = total;
    }
    // A scene with no story in it (only logos, blanks) joins its neighbour.
    let story = |s: &Scene| panels[s.from..s.to].iter().filter(|x| x.story()).count();
    let mut kept: Vec<Scene> = vec![];
    for s in scenes {
        if story(&s) == 0 {
            match kept.last_mut() {
                Some(prev) => prev.to = s.to,
                None => kept.push(s),
            }
        } else if kept.last().is_some_and(|k| story(k) == 0) {
            let lead = kept.pop().map(|k| k.from).unwrap_or(s.from);
            kept.push(Scene { from: lead, ..s });
        } else {
            kept.push(s);
        }
    }
    // A scene of very many panels is boarded in parts, so no board is too long to write whole.
    let mut parts: Vec<Scene> = vec![];
    for s in kept {
        let idx: Vec<usize> = (s.from..s.to).filter(|i| panels[*i].story()).collect();
        let k = idx.len().div_ceil(SCENE_PANELS);
        if k <= 1 {
            parts.push(s);
            continue;
        }
        let each = idx.len().div_ceil(k);
        for part in 0..k {
            let from = if part == 0 { s.from } else { idx[part * each] };
            let to = if part + 1 == k { s.to } else { idx[(part + 1) * each] };
            parts.push(Scene { title: format!("{} ({} of {k})", s.title, part + 1), from, to, ..s.clone() });
        }
    }
    let mut kept = parts;
    for (i, s) in kept.iter_mut().enumerate() {
        s.number = i as u32 + 1;
        let lines: Vec<_> = panels[s.from..s.to].iter().filter(|x| x.story()).flat_map(|x| x.lines.clone()).collect();
        // Each line gets its own lead here: in the board they are spread over several cuts.
        let spoken: f32 = lines.iter().map(|l| pacing::line_seconds(&l.text) + pacing::LINE_GAP).sum();
        s.seconds = (pacing::scene_seconds(&s.kind, story(s), spoken) * 2.).round() / 2.;
    }
    let n = kept.len();
    let anime = &mut p.chapter_mut(chapter).anime;
    anime.scenes = kept;
    if !a.treatment.trim().is_empty() {
        anime.treatment = a.treatment.trim().to_string();
    }
    for c in &mut anime.cuts {
        c.clear_made();
    }
    anime.cuts.clear();
    anime.issues.clear();
    anime.animatic = None;
    anime.approved = false;
    anime.video = None;
    Ok(n)
}

// ------------------------------------------------------------------ the storyboard

/// How a director adapts manga to anime: the craft every storyboard is written with.
pub const DIRECTING: &str = "HOW YOU DIRECT. These rules come from measuring real TV-anime episodes cut by cut against the manga they adapt, and from film craft. Follow them.

ADAPTING
1. A panel is a story beat, not a shot. Keep the story, the characters, their looks and their words (lines nearly word for word); you are free in how it is filmed.
2. Talk: about one cut per panel. Fights and horror: several small panels (a blow, a sound effect, a reaction) fuse into ONE cut, about one cut for every two panels.
3. A panel with several balloons is usually one longer take, not a cut per balloon. A tall panel is a tilt over its subject, or two or three cuts (wide, nearer, then the eyes or hands). A panel that is only a balloon or a caption gives its line to the cut beside it.
4. Add what a film needs and a page skips: an establishing shot when the place changes, a reaction, an insert of the thing talked about, a still of what an inner monologue speaks of.
5. Drop chibi and gag drawings (play the beat in the normal style, or cut it), logos and credits.
6. Narration boxes become narrator voice-over on wide, calm pictures joined by dissolves. Thought balloons become inner monologue (kind \"thought\") over the thinker's face or hands, or over what they look at. A system window is narration by \"System\".

COVERAGE AND CONTINUITY
7. Open every new place wide enough to show where everyone is, within its first three cuts; go closer as the scene tightens; go wide again when people move or the mood breaks.
8. Two people talking stand on a line. Keep the camera on ONE side of it for the whole scene: whoever is on the left of the frame stays on the left in every cut, and their eyelines meet (one faces right, the other left). Give every character their side and facing. To change sides, first cut to a shot on the line itself (straight on one of them, or from behind them both).
9. Never cut between two shots of the same subject with the same size and the same angle: change the size by two steps (medium to close-up) or the angle clearly.
10. Do not sit on the speaker for every line: cut to the listener, to an insert, to the thing they look at.
11. Mostly hard cuts. Dissolve for time passing and for narration; black to end a scene; white for a flash or entering a memory; smash for a sudden loud change.

COMPOSITION
12. Give every frame depth: something near the lens, the subject, the place behind. Faces sit on the upper third with room in front of where they look. Dead centre and symmetry are for the sacred, the ominous and the final image. Diagonals and a tilted horizon are for danger and action; level frames for calm.
13. Low angle: power, threat, resolve. High angle: weakness, being small, being watched. Bird's eye: the layout of a place, a death, a ritual. Extreme close-up of the eyes: realisation, terror. Of the mouth with the eyes cut off, of a fist, of feet: what the character hides.
14. Wide lens: scale, speed, things looming at the lens (awe, action, horror). Long lens: intimacy, isolation, soft backgrounds (talk, close-ups). Write the composition as one line an illustrator can draw from.

CAMERA
15. About half the screen time has a camera move, and it is SLOW: a drift of a tenth of the frame over the whole cut, a push so gentle it is felt rather than seen. Fast moves (whip, track_in, shake) belong to action and to shocks. Every move has a reason: to reveal, to follow, to tighten, to unsettle.
16. \"amount\" scales a move: 1 is the usual slow drift, 2 to 4 a real travel (a pan across a group, a tilt up a tall figure), under 1 the slightest creep.

PACING (measured)
17. Cut lengths: action 1.5-3 s on average, some under 1 s; dread 3-5 s; dialogue 3.5-6 s, with takes of 8-16 s that carry three to five lines; narration 5-8 s. A cut must hold its lines: about 2.6 words a second, the first line half a second in, a breath between lines. An insert needs 1.5 s to be read (less only in a fight).
18. Vary the rhythm: short cuts beside long ones; tighten toward the scene's peak; after a shock, one long quiet cut.
19. End the scene on its point. The chapter ends on ONE held image (the eyes, the object that stands for the cliffhanger) of 4 s or more.

ECONOMY: HOW MUCH MOVES (\"motion\")
20. hold = a frozen drawing. camera = a still drawing under a moving camera. lip = mouth flaps and blinks on a held drawing. pose = two or three key poses with short moves between. full = continuous animation. Choose the CHEAPEST that sells the cut: real TV anime does.
21. In talk and dread, about a third of the time is hold or camera, about half lip or pose, and at most a fifth full. Action is half or more full, but its close-ups against speed lines are lip or hold.
22. Hide mouths where you can: lines over the backs of a group, over tiny figures in a wide shot, over the listener's face, over an insert. \"mouth\" is true only when a speaker's mouth is seen moving; such a cut is lip, pose or full.
23. Reuse like a studio. One plate per place, view and lighting state, shared by every cut set there; a reverse angle is a second plate; close-ups reuse a plate (it is blurred behind them). Come back to the same framing of a \"clock\" object instead of inventing new ones. Effects come from the small bank.

COLOUR AND SOUND
24. The scene has ONE colour grade. A cut takes another only when it shows a memory or another place.
25. Write the sounds the cut needs in \"sfx\" (footsteps on gravel, a blade drawn, the crowd's murmur).

MORE CRAFT (from film and anime production sources)
26. A moving character keeps one screen direction for the whole journey: leaving frame left, they enter the next cut from the right. To reverse it, show the turn inside a cut or put a cut of them coming straight at the lens between.
27. A pov cut always sits next to a cut of the person whose eyes it is: the looker, what they see, the looker again.
28. A close-up holds one face. A cut with three or more lines, or two speakers, is a two-shot, over-the-shoulder or wider.
29. A shot and its reverse match: the same size, angle and lens on both sides; when the talk turns, move both one step closer. Save the tightest size for the scene's peak.
30. After eight close cuts in a row, or when anyone moves, enters or leaves, go back to a cut that shows where everyone is. In a fight, show the whole arena again every ten cuts.
31. Build fights in bursts: three to six cuts under a second, one clear payoff, then a rest of a second or more (the fallen body, the stance, the dust). Keep a third of a fight's cuts cowboy or wider, attacker and target together.
32. A blow is a held wind-up, the strike as one smear, and the reaction; on the contact cut flash once, shake briefly, write the sound.
33. Shake the camera only for something physical (a hit, a landing, an explosion, a scare): never in talk, narration or memory, never on more than two cuts in a row.
34. Never more than three flashes in any second (flash, white transitions): broadcast rules forbid it.
35. Over a still drawing never pan faster than a full frame in seven seconds. Give every move at least 1.5 s (a push 3 s) and never push in on more than two cuts in a row.
36. A thought or narration line never moves the thinker's mouth.
37. Before the decisive blow, reveal or line, put one still cut of one to three seconds with a single sound; the next cut comes in hard.
38. In dread, show the audience the danger before the characters see it; keep the threat to a part, a shadow or a soft shape until one full reveal.
39. Enter and leave a memory with white or a dissolve, never a plain cut.
40. End every scene on its button (a line, a look, an image), with at most one cut after the last line.
41. Dutch on at most one cut in twenty, never three in a row. Eye level is the rule; extreme angles and the fisheye are accents.

STAGING AND PERFORMANCE (a key animator's review of AI anime)
42. Spatial continuity first. Frame every cut from the scene's staging map: the audience must always know where each character is in relation to the others, to their objective and to the threat. Things that travel keep travelling the same way.
43. Nothing appears without its source: an explosion, a spell, a light, a sound is shown with (or right after) who or what makes it.
44. Progress the shots: show a person whole (where they stand, what they face) before a big close-up of them, unless the close-up is a deliberate introduction in pieces.
45. Not everything is faces. Carry emotion through poses, hands, the body, silhouettes and what the framing leaves out; hiding the eyes can say more than showing them.
46. Build a reveal in three: the preparation (a pose, a breath, held), a partial or abstract detail that makes the audience ask what it is, then the full reveal. A stunned character can be shown in slow motion, then a hard cut back to full sound and a wide shot.
47. Reactions are in character and in tone: what would this person really think and do here? No quips in a scene of horror or grief.
48. A turning point can turn the colour: the grade may shift at the moment of a decisive act and hold for the rest of the scene.";

const BOARD_SHAPE: &str = r#"{"plates": [{"id": "short-slug", "view": "what the camera sees of this place from here"}], "fx": [{"id": "short-slug", "what": "the effect, e.g. dark purple aura flames", "blend": "screen|over"}], "cuts": [{"recipe": "a recipe id, or empty", "seconds": 3.5, "from": [12, 13], "size": "", "angle": "", "lens": "", "camera": "", "amount": 1, "shake": 0, "composition": "how the frame is built: where the subject sits, what is in front, the depth, the light", "picture": "what we see: who is where, their acting", "action": "what moves during the cut", "motion": "hold|camera|lip|pose|full", "characters": [{"name": "exact cast name", "expression": "", "outfit": "what they wear here", "pose": "", "side": "left|centre|right", "facing": "left|right|camera|away"}], "plate": "plate id", "plate_only": false, "fx": [{"id": "fx id", "x": 0.5, "y": 0.5, "scale": 0.6, "front": true}], "lines": [{"speaker": "name, Narrator or System", "text": "", "kind": "speech|thought|shout|whisper|narration"}], "mouth": false, "sfx": "", "transition": "cut", "grade": "", "dust": 0, "rays": null, "speed": 0, "flash": 0}]}"#;

fn board_system(kind: &str) -> String {
    format!(
        "You are the series director and storyboard artist (絵コンテ) of a high-end TV anime. You draw up the storyboard of ONE scene from the manga's panels, as your assistant read them.\n\n{DIRECTING}\n\nSHOT RECIPES for this kind of scene (use an id in \"recipe\" when a cut follows one; you may change any of its values, or use none):\n{}\n\nAnswer with ONE JSON object, no other text:\n{BOARD_SHAPE}\nVocabulary (use exactly these words):\n- size: {}\n- angle: eye level, low, high, bird's eye, worm's eye, ground level, and \"dutch\" alone or added to any (\"low dutch\")\n- lens: wide, normal, tele, fisheye\n- camera: {}\n- motion: {}\n- transition (into the cut): {}\n- grade: leave empty for the scene's own; else one of {}\nOutput rules: \"from\" lists the panel numbers a cut adapts (empty for a cut you add). List under \"plates\" only views that are NEW (reuse the existing ids wherever they fit); every cut names a plate. List under \"fx\" only new effects. Characters only from the cast, spelled exactly; every line of the panels that you keep is spoken in exactly one cut. Photography extras: dust (0-60 motes in the light), rays [x, y] (light shafts from that point of the frame, 0-1), speed (0-1 speed lines), flash (0-1 white flash on an impact); effects are placed with x, y (0-1), scale (0.2-1.5) and front.",
        recipes::listing(kind),
        SIZES.join(", "),
        MOVES.join(", "),
        MOTIONS.join(", "),
        TRANSITIONS.join(", "),
        GRADES.join(", "),
    )
}

/// A cut as the director wrote it (nothing that was made), for showing it its own board again.
fn cut_script(c: &Cut) -> Value {
    serde_json::json!({
        "cut": c.number, "recipe": c.recipe, "seconds": c.seconds, "from": c.from.iter().map(|i| i + 1).collect::<Vec<_>>(),
        "size": c.size, "angle": c.angle, "lens": c.lens, "camera": c.camera, "amount": c.amount, "shake": c.shake,
        "composition": c.composition, "picture": c.picture, "action": c.action, "motion": c.motion,
        "characters": c.characters.iter().map(|x| serde_json::json!({ "name": x.name, "expression": x.expression, "outfit": x.outfit, "pose": x.pose, "side": x.side, "facing": x.facing })).collect::<Vec<_>>(),
        "plate": c.plate, "plate_only": c.plate_only,
        "fx": c.fx.iter().map(|f| serde_json::json!({ "id": f.id, "x": f.x, "y": f.y, "scale": f.scale, "front": f.front })).collect::<Vec<_>>(),
        "lines": c.lines.iter().map(|l| serde_json::json!({ "speaker": l.speaker, "text": l.text, "kind": l.kind })).collect::<Vec<_>>(),
        "mouth": c.mouth, "sfx": c.sfx, "transition": c.transition, "grade": c.grade, "dust": c.dust, "rays": c.rays, "speed": c.speed, "flash": c.flash,
    })
}

/// One line per cut, for the scene after it.
fn cut_line(c: &Cut) -> String {
    let who: Vec<String> = c.characters.iter().map(|x| if x.side.is_empty() { x.name.clone() } else { format!("{} ({}, facing {})", x.name, x.side, if x.facing.is_empty() { "?" } else { &x.facing }) }).collect();
    format!("cut {} ({:.1}s, {} / {} / {}, {}): {}{}", c.number, c.seconds, c.size, c.angle, c.camera, c.motion, c.picture.chars().take(110).collect::<String>(), if who.is_empty() { String::new() } else { format!(" | {}", who.join(", ")) })
}

/// What the director is told about a scene before it boards (or revises) it.
fn scene_brief(p: &Project, chapter: u32, s: &Scene, note: &str) -> String {
    let ch = p.chapter(chapter).cloned().unwrap_or_default();
    let a = &ch.anime;
    let pace = pacing::pace(&s.kind);
    let (lo, hi) = pacing::scene_cuts(&s.kind, s.seconds);
    let first = a.scenes.first().is_some_and(|x| x.number == s.number);
    let last = a.scenes.last().is_some_and(|x| x.number == s.number);
    let mut user = format!("Anime of \"{}\", chapter {chapter}{}.\n", p.adapts.as_deref().unwrap_or(&p.title), if ch.title.is_empty() { String::new() } else { format!(": {}", ch.title) });
    if !a.treatment.is_empty() {
        let _ = writeln!(user, "Your treatment of the episode: {}", a.treatment);
    }
    let names: Vec<String> = a.scenes.iter().map(|x| format!("{}. {} ({})", x.number, x.title, x.kind)).collect();
    let _ = writeln!(user, "Its scenes: {}", names.join("; "));
    user.push_str("\nCast (name: look | outfits):\n");
    let in_scene = |name: &str| s.characters.iter().any(|n| n.eq_ignore_ascii_case(name)) || a.panels[s.from.min(a.panels.len())..s.to.min(a.panels.len())].iter().any(|x| x.characters.iter().any(|c| c.name.eq_ignore_ascii_case(name)));
    for c in p.cast.iter().filter(|c| in_scene(&c.name) || c.tier() == 0) {
        let _ = writeln!(user, "- {}: {} | {}", c.name, c.appearance.chars().take(140).collect::<String>(), c.outfits.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", "));
    }
    let here: Vec<&Plate> = p.plates.iter().filter(|x| x.location.eq_ignore_ascii_case(s.location.trim())).collect();
    if !here.is_empty() {
        user.push_str("\nPlates that exist for this place (reuse them):\n");
        for pl in here {
            let _ = writeln!(user, "- {}: {} — {}", pl.id, pl.view, pl.light);
        }
    }
    if !p.fx.is_empty() {
        let _ = writeln!(user, "\nThe effects bank: {}", p.fx.iter().map(|f| format!("{} ({})", f.id, f.what)).collect::<Vec<_>>().join("; "));
    }
    let _ = write!(
        user,
        "\nTHE SCENE TO BOARD — scene {} of {}: \"{}\"\n- kind: {} (cuts average {:.1}-{:.1} s; a take over {:.0} s is a deliberate long take)\n- place: {}\n- light: {}\n- colour grade: {}\n- what happens: {}\n- how to tell it: {}\n- STAGING MAP (frame every cut from it; give each character the side and facing it implies): {}\n- length: about {:.0} s, in {lo} to {hi} cuts\n- music: {} · room tone: {}\n",
        s.number,
        a.scenes.len(),
        s.title,
        s.kind,
        pace.mean_cut.0,
        pace.mean_cut.1,
        pace.long_take,
        if s.location.is_empty() { "(as in the panels)" } else { &s.location },
        if s.light.is_empty() { "(as in the panels)" } else { &s.light },
        s.grade,
        s.summary,
        if s.plan.is_empty() { "keep it as the manga tells it" } else { &s.plan },
        if s.blocking.is_empty() { "(work it out from the panels first, then keep it)" } else { &s.blocking },
        s.seconds,
        if s.music.is_empty() { "none" } else { &s.music },
        if s.ambience.is_empty() { "as the place needs" } else { &s.ambience },
    );
    let at = a.scenes.iter().position(|x| x.number == s.number);
    let earlier: Vec<&Cut> = a.cuts.iter().filter(|c| c.scene != s.number && a.scenes.iter().position(|x| x.number == c.scene) < at).collect();
    let before: Vec<String> = earlier[earlier.len().saturating_sub(3)..].iter().map(|c| cut_line(c)).collect();
    if !before.is_empty() {
        let _ = write!(user, "\nThe last cuts of the scene before (do not repeat their framing on your first cut):\n{}\n", before.join("\n"));
    }
    let _ = write!(user, "\nThe scene's panels (#number [page, tall / wide] what is drawn | who | where | lines | sfx | mood):\n{}", panel_digest(&a.panels, s.from, s.to));
    if first {
        user.push_str("\nThis is the FIRST scene of the episode: its first cut is the hook.");
    }
    if last {
        user.push_str("\nThis is the LAST scene of the episode: end on one held image of 4 s or more.");
    }
    if !note.trim().is_empty() {
        let _ = write!(user, "\nThe user wants: {}", note.trim());
    }
    user
}

/// The storyboard of one scene.
pub fn storyboard_scene(p: &Project, chapter: u32, scene: u32, note: &str) -> Option<(String, String)> {
    let s = p.chapter(chapter)?.anime.scene(scene)?.clone();
    Some((board_system(&s.kind), scene_brief(p, chapter, &s, note)))
}

/// The same scene again, with what the episode director's check found in the board.
pub fn revise_scene(p: &Project, chapter: u32, scene: u32, issues: &[String]) -> Option<(String, String)> {
    let a = &p.chapter(chapter)?.anime;
    let s = a.scene(scene)?.clone();
    let mine: Vec<Value> = a.cuts.iter().filter(|c| c.scene == scene).map(cut_script).collect();
    let mut user = scene_brief(p, chapter, &s, "");
    let _ = write!(
        user,
        "\n\nYOUR STORYBOARD OF THIS SCENE (cut numbers are the episode's):\n{}\n\nTHE EPISODE DIRECTOR CHECKED IT AND FOUND:\n{}\n\nReturn the WHOLE scene again in the same JSON shape with these problems fixed. Change only what the notes require; keep every line of dialogue and every cut that was fine.",
        serde_json::to_string(&mine).unwrap_or_default(),
        issues.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n"),
    );
    Some((board_system(&s.kind), user))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PlateIn {
    #[serde(deserialize_with = "lenient_text")]
    id: String,
    #[serde(deserialize_with = "lenient_text")]
    view: String,
    #[serde(deserialize_with = "lenient_text")]
    light: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BoardAnswer {
    plates: Vec<PlateIn>,
    fx: Vec<FxElement>,
    cuts: Vec<Cut>,
}

/// A number the AI may have left out: the default then.
fn or(v: f32, default: f32) -> f32 {
    if v.is_finite() && v > 0. {
        v
    } else {
        default
    }
}

/// Stores a scene's storyboard in place of the one it had: new plates (of the scene's place and
/// light) and effects join the project's, the cuts are tidied (our vocabulary, the recipe's
/// values where the director left them out, the cast's names, panels of this chapter, sane
/// lengths) and nothing made is taken from the answer. Returns how many cuts the scene has.
pub fn apply_scene_board(p: &mut Project, chapter: u32, scene: u32, v: Value) -> anyhow::Result<usize> {
    let a: BoardAnswer = serde_json::from_value(v)?;
    if a.cuts.is_empty() {
        anyhow::bail!("the AI boarded no cuts");
    }
    let s = p.chapter(chapter).and_then(|c| c.anime.scene(scene)).cloned().ok_or_else(|| anyhow::anyhow!("chapter {chapter} has no scene {scene}"))?;
    let n_panels = p.chapter(chapter).map(|c| c.anime.panels.len()).unwrap_or(0);
    let place = if s.location.trim().is_empty() { s.title.clone() } else { s.location.clone() };
    for pl in a.plates {
        let named = if pl.id.trim().is_empty() { format!("{place} {}", pl.view) } else { pl.id.clone() };
        let id = crate::slug(&named);
        if !p.plates.iter().any(|x| x.id == id) {
            p.plates.push(Plate { id, location: place.clone(), view: pl.view, light: if pl.light.trim().is_empty() { s.light.clone() } else { pl.light }, image: None });
        }
    }
    for mut f in a.fx {
        f.id = crate::slug(if f.id.trim().is_empty() { &f.what } else { &f.id });
        f.image = None;
        f.clip = None;
        f.blend = if f.blend.trim() == "over" { "over".into() } else { "screen".into() };
        if !f.what.trim().is_empty() && !p.fx.iter().any(|x| x.id == f.id) {
            p.fx.push(f);
        }
    }
    let mut cuts = a.cuts;
    for c in cuts.iter_mut() {
        c.number = 0;
        c.scene = scene;
        c.clear_made();
        c.notes.clear();
        // Panel numbers come 1-based from the board; stored 0-based.
        c.from = c.from.iter().filter(|i| **i >= 1 && **i <= n_panels).map(|i| i - 1).collect();
        c.from.dedup();
        let r = recipes::recipe(&c.recipe);
        c.recipe = r.map(|r| r.id.to_string()).unwrap_or_default();
        let pick = |have: &str, of: Option<&'static str>| if have.trim().is_empty() { of.unwrap_or("").to_string() } else { have.to_string() };
        c.size = norm_size(&pick(&c.size, r.map(|r| r.size))).into();
        c.angle = norm_angle(&pick(&c.angle, r.map(|r| r.angle)));
        c.lens = norm_lens(&pick(&c.lens, r.map(|r| r.lens))).into();
        c.camera = norm_camera(&pick(&c.camera, r.map(|r| r.camera))).into();
        c.motion = norm_motion(&pick(&c.motion, r.map(|r| r.motion)), &c.camera).into();
        if c.composition.trim().is_empty() {
            c.composition = r.map(|r| r.composition.to_string()).unwrap_or_default();
        }
        c.seconds = if c.seconds.is_finite() && c.seconds > 0. { c.seconds } else { r.map(|r| (r.seconds.0 + r.seconds.1) / 2.).unwrap_or(3.) }.clamp(pacing::CUT_MIN, pacing::CUT_MAX);
        c.amount = or(c.amount, 1.).clamp(0.25, 6.);
        c.shake = if c.shake.is_finite() { c.shake.clamp(0., 2.) } else { 0. };
        c.transition = norm_transition(&c.transition).into();
        // The scene's key, unless the cut names another real one (a memory, another place).
        let named = norm_grade(&c.grade);
        let unknown = named == "neutral" && !c.grade.trim().eq_ignore_ascii_case("neutral");
        c.grade = if c.grade.trim().is_empty() || unknown { s.grade.clone() } else { named.into() };
        if c.location.trim().is_empty() {
            c.location = place.clone();
        }
        c.dust = c.dust.min(80);
        c.speed = if c.speed.is_finite() { c.speed.clamp(0., 1.) } else { 0. };
        c.flash = if c.flash.is_finite() { c.flash.clamp(0., 1.) } else { 0. };
        c.lines.retain(|l| !l.text.trim().is_empty());
        c.lines.truncate(8);
        c.characters.retain(|x| !x.name.trim().is_empty());
        for x in &mut c.characters {
            if let Some(m) = p.character(&x.name) {
                x.name = m.name.clone();
            }
            x.side = match x.side.trim().to_lowercase().as_str() {
                t if t.contains("left") => "left".into(),
                t if t.contains("right") => "right".into(),
                t if t.contains("cent") || t.contains("mid") => "centre".into(),
                _ => String::new(),
            };
            x.facing = match x.facing.trim().to_lowercase().as_str() {
                t if t.contains("left") => "left".into(),
                t if t.contains("right") => "right".into(),
                t if t.contains("away") || t.contains("back") => "away".into(),
                t if t.contains("cam") || t.contains("front") || t.contains("viewer") => "camera".into(),
                _ => String::new(),
            };
        }
        for l in &mut c.lines {
            if let Some(m) = p.character(&l.speaker) {
                l.speaker = m.name.clone();
            }
        }
        // A plate id the board did not define: a plate of this place, else a new one for it.
        c.plate = crate::slug(&c.plate);
        if !p.plates.iter().any(|x| x.id == c.plate) {
            match p.plates.iter().find(|x| x.location.eq_ignore_ascii_case(c.location.trim()) && x.light == s.light).or_else(|| p.plates.iter().find(|x| x.location.eq_ignore_ascii_case(c.location.trim()))) {
                Some(x) => c.plate = x.id.clone(),
                None => {
                    let id = crate::slug(&c.location);
                    if !p.plates.iter().any(|x| x.id == id) {
                        p.plates.push(Plate { id: id.clone(), location: c.location.clone(), view: format!("{} view of {}", c.size, c.location), light: s.light.clone(), image: None });
                    }
                    c.plate = id;
                }
            }
        }
        c.fx.retain(|f| p.fx.iter().any(|x| x.id == crate::slug(&f.id)));
        for f in &mut c.fx {
            f.id = crate::slug(&f.id);
            f.x = if f.x.is_finite() { f.x.clamp(-0.2, 1.2) } else { 0.5 };
            f.y = if f.y.is_finite() { f.y.clamp(-0.2, 1.2) } else { 0.5 };
            f.scale = or(f.scale, 0.6).clamp(0.1, 2.);
            f.opacity = or(f.opacity, 1.).min(1.);
        }
        if let Some(r) = c.rays.as_mut() {
            r[0] = r[0].clamp(0., 1.);
            r[1] = r[1].clamp(0., 1.);
        }
    }
    let n = cuts.len();
    let anime = &mut p.chapter_mut(chapter).anime;
    anime.cuts.retain(|c| c.scene != scene);
    anime.cuts.extend(cuts);
    anime.renumber();
    anime.animatic = None;
    anime.video = None;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anime::tests::proj;
    use crate::anime::SourcePanel;
    use crate::project::{Line, PanelChar};

    fn panel(desc: &str, who: &[&str], lines: &[(&str, &str, &str)]) -> SourcePanel {
        SourcePanel {
            description: desc.into(),
            characters: who.iter().map(|n| PanelChar { name: n.to_string(), ..Default::default() }).collect(),
            lines: lines.iter().map(|(s, t, k)| Line { speaker: s.to_string(), text: t.to_string(), kind: k.to_string() }).collect(),
            read: true,
            ..Default::default()
        }
    }

    fn chapter() -> Project {
        let mut p = proj();
        p.chapter_mut(1).anime.panels = vec![
            SourcePanel { read: true, skip: true, ..Default::default() },
            panel("the gate site from above", &[], &[("Narrator", "A construction site in Seoul.", "narration")]),
            panel("two hunters by a truck", &["Kim", "Park"], &[("Kim", "Hey, Park!", "speech"), ("Park", "Been a while.", "speech")]),
            panel("Jin Woo arrives", &["Jin Woo"], &[("jin", "Good to be working with you again today.", "speech")]),
            panel("a goblin leaps", &["Jin Woo", "Goblin"], &[]),
            panel("the blade lands", &["Jin Woo"], &[]),
        ];
        p
    }

    #[test]
    fn the_cast_is_gathered_from_the_pages() {
        let mut p = chapter();
        assert_eq!(unknown_people(&p, 1), vec!["Kim", "Park", "Goblin"]);
        let (_, user) = cast_from_pages(&p, 1, "");
        assert!(user.contains("Jin Woo") && user.contains("Labels in the panel notes that the cast does not know yet: Kim; Park; Goblin") && user.contains("#3 [p0] two hunters by a truck"));
        let v = serde_json::json!({
            "title": "The weakest hunter", "summary": "A raid begins.",
            "bible": { "art_style": "clean anime", "world": "gates" },
            "cast": [
                { "name": "Kim Sangshik", "aliases": ["Kim", "Mr. Kim"], "role": "supporting", "gender": "male", "appearance": "grey hair, navy jacket", "outfits": [{ "name": "raid", "look": "navy jacket" }] },
                { "name": "Park", "role": "", "appearance": "green padded jacket" },
                { "name": "Jin Woo", "aliases": ["jin"], "appearance": "IGNORED: already known", "outfits": [{ "name": "Hoodie", "look": "x" }, { "name": "hospital", "look": "white gown" }] },
                { "name": " " }
            ],
            "locations": [{ "name": "Gate site", "look": "fenced construction site" }]
        });
        assert_eq!(apply_cast(&mut p, 1, v).unwrap(), 2);
        // The known lead keeps his look and gains only the outfit he did not have.
        let jin = p.character("Jin Woo").unwrap();
        assert_eq!(jin.appearance, "black hair, grey eyes");
        assert_eq!(jin.outfits.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), vec!["hoodie", "hospital"]);
        let kim = p.character("Kim Sangshik").unwrap();
        assert_eq!((kim.intro_chapter, kim.role.as_str()), (1, "supporting"));
        assert_eq!(p.character("Park").unwrap().role, "supporting");
        assert!(p.location("gate site").is_some() && p.bible.world == "gates");
        let ch = p.chapter(1).unwrap();
        assert_eq!((ch.title.as_str(), ch.summary.as_str()), ("The weakest hunter", "A raid begins."));
        // The panels now speak of the cast by name.
        assert_eq!(ch.anime.panels[2].characters[0].name, "Kim Sangshik");
        assert_eq!(ch.anime.panels[2].lines[0].speaker, "Kim Sangshik");
        assert_eq!(ch.anime.panels[3].lines[0].speaker, "Jin Woo");
        assert_eq!(unknown_people(&p, 1), vec!["Goblin"]);
    }

    #[test]
    fn scenes_cover_every_panel_once_and_get_their_length() {
        let mut p = chapter();
        p.chapter_mut(1).anime.cuts = vec![Cut { number: 1, comp: Some("cuts/a.mp4".into()), ..Default::default() }];
        let v = serde_json::json!({
            "treatment": "Open on the site.",
            "scenes": [
                { "title": "The fight", "kind": "fight", "from": "5", "to": 9, "grade": "cold dungeon", "characters": "Jin woo, Goblin" },
                { "title": "At the gate", "kind": "Dialogue", "location": "Gate site", "light": "overcast noon", "grade": "daylight", "from": 2, "to": 3, "characters": ["jin woo"] },
                { "title": "junk", "from": 0, "to": 0 },
                { "title": "past the end", "from": 40, "to": 50 }
            ]
        });
        assert_eq!(apply_scenes(&mut p, 1, v).unwrap(), 2);
        let a = &p.chapter(1).unwrap().anime;
        assert!(a.cuts.is_empty() && a.treatment == "Open on the site.");
        let s = &a.scenes;
        // In panel order; the logo before the first scene and the panel between them are covered.
        assert_eq!((s[0].number, s[0].title.as_str(), s[0].from, s[0].to, s[0].kind.as_str(), s[0].grade.as_str()), (1, "At the gate", 0, 4, "dialogue", "daylight"));
        assert_eq!((s[1].number, s[1].from, s[1].to, s[1].kind.as_str(), s[1].grade.as_str()), (2, 4, 6, "action", "cold_dungeon"));
        assert_eq!(s[1].characters, vec!["Jin Woo", "Goblin"]);
        // Three panels of talk at 4 s each; two of action at 2 s each.
        assert_eq!((s[0].seconds, s[1].seconds), (12., 4.));
        assert!(apply_scenes(&mut p, 1, serde_json::json!({ "scenes": [] })).is_err());
    }

    fn boarded() -> Project {
        let mut p = chapter();
        apply_scenes(&mut p, 1, serde_json::json!({ "scenes": [
            { "title": "At the gate", "kind": "dialogue", "location": "Gate site", "light": "overcast noon", "grade": "daylight", "from": 1, "to": 4 },
            { "title": "The fight", "kind": "action", "location": "Cave", "light": "torchlight", "grade": "cold_dungeon", "from": 5, "to": 6 }
        ]})).unwrap();
        p
    }

    #[test]
    fn a_scene_s_board_is_tidied_and_can_be_replaced() {
        let mut p = boarded();
        let (system, user) = storyboard_scene(&p, 1, 1, "more tension").unwrap();
        assert!(system.contains("HOW YOU DIRECT") && system.contains("group-backs-pan") && !system.contains("lunge-at-lens"));
        assert!(user.contains("scene 1 of 2: \"At the gate\"") && user.contains("length: about 12 s, in 2 to 4 cuts") && user.contains("FIRST scene") && !user.contains("LAST scene") && user.contains("more tension"));
        assert!(user.contains("#3 [p0] two hunters by a truck") && !user.contains("a goblin leaps"));
        assert!(storyboard_scene(&p, 1, 9, "").is_none());
        let v = serde_json::json!({
            "plates": [{ "id": "Gate Wide", "view": "the fenced site from above" }],
            "fx": [{ "id": "aura", "what": "purple aura", "blend": "glow" }, { "id": "", "what": " " }],
            "cuts": [
                { "recipe": "site high dutch", "from": [2, 99], "plate": "gate-wide", "plate_only": "true", "lines": [{ "speaker": "Narrator", "text": "A construction site in Seoul.", "kind": "narration" }], "comp": "C:/evil.mp4" },
                { "recipe": "nope", "seconds": "4.5s", "shot": "MCU", "angle": "low angle", "camera": "slow push in", "motion": "", "grade": "teal", "transition": "O.L",
                  "characters": [{ "name": "jin woo", "side": "Left of frame", "facing": "to the right" }, { "name": "" }], "lines": [{ "speaker": "jin", "text": "Good to be working with you again today.", "kind": "speech" }], "mouth": "yes",
                  "fx": [{ "id": "aura", "scale": 0 }, { "id": "nope" }], "rays": [3, -1], "plate": "somewhere else" },
                { "seconds": 90, "camera": "handheld shake", "grade": "flashback", "plate": "gate-wide" }
            ]
        });
        assert_eq!(apply_scene_board(&mut p, 1, 1, v).unwrap(), 3);
        let a = &p.chapter(1).unwrap().anime;
        let c = &a.cuts[0];
        // The recipe filled in what the director left out; nothing made came from the answer.
        assert_eq!((c.number, c.scene, c.recipe.as_str(), c.size.as_str(), c.angle.as_str(), c.lens.as_str(), c.camera.as_str(), c.motion.as_str()), (1, 1, "site-high-dutch", "establishing", "high dutch", "wide", "tilt_down", "camera"));
        assert_eq!((c.from.clone(), c.seconds, c.plate.as_str(), c.plate_only, c.grade.as_str(), c.comp.clone()), (vec![1], 5.5, "gate-wide", true, "daylight", None));
        assert!(c.composition.contains("looking steeply down"));
        let d = &a.cuts[1];
        assert_eq!((d.recipe.as_str(), d.seconds, d.size.as_str(), d.angle.as_str(), d.camera.as_str(), d.motion.as_str(), d.grade.as_str(), d.transition.as_str(), d.mouth), ("", 4.5, "medium close-up", "low", "push_in", "camera", "daylight", "dissolve", true));
        assert_eq!((d.characters.len(), d.characters[0].name.as_str(), d.characters[0].side.as_str(), d.characters[0].facing.as_str(), d.lines[0].speaker.as_str()), (1, "Jin Woo", "left", "right", "Jin Woo"));
        assert_eq!((d.fx.len(), d.fx[0].scale, d.rays, d.plate.as_str()), (1, 0.6, Some([1., 0.]), "gate-wide"));
        // A memory keeps its own grade; a take is capped.
        let e = &a.cuts[2];
        assert_eq!((e.seconds, e.camera.as_str(), e.grade.as_str()), (20., "shake", "flashback"));
        assert_eq!(p.plates.iter().map(|x| (x.id.as_str(), x.location.as_str(), x.light.as_str())).collect::<Vec<_>>(), vec![("gate-wide", "Gate site", "overcast noon")]);
        assert_eq!(p.fx.iter().map(|f| (f.id.as_str(), f.blend.as_str())).collect::<Vec<_>>(), vec![("aura", "screen")]);

        // The second scene's cuts follow; boarding the first again replaces only its own.
        apply_scene_board(&mut p, 1, 2, serde_json::json!({ "cuts": [{ "recipe": "lunge-at-lens", "from": [5] }, { "recipe": "impact", "from": [6] }] })).unwrap();
        let a = &p.chapter(1).unwrap().anime;
        assert_eq!(a.cuts.iter().map(|c| (c.number, c.scene)).collect::<Vec<_>>(), vec![(1, 1), (2, 1), (3, 1), (4, 2), (5, 2)]);
        assert_eq!((a.cuts[3].motion.as_str(), a.cuts[3].plate.as_str(), a.cuts[3].grade.as_str()), ("full", "cave", "cold_dungeon"));
        assert!(a.boarded());
        let (_, again) = revise_scene(&p, 1, 2, &["cut 4: too long".to_string()]).unwrap();
        assert!(again.contains("YOUR STORYBOARD OF THIS SCENE") && again.contains("- cut 4: too long") && again.contains("\"recipe\":\"lunge-at-lens\"") && again.contains("The last cuts of the scene before"));
        apply_scene_board(&mut p, 1, 1, serde_json::json!({ "cuts": [{ "recipe": "master-two-shot", "from": [3] }] })).unwrap();
        let a = &p.chapter(1).unwrap().anime;
        assert_eq!(a.cuts.iter().map(|c| (c.number, c.scene, c.recipe.as_str())).collect::<Vec<_>>(), vec![(1, 1, "master-two-shot"), (2, 2, "lunge-at-lens"), (3, 2, "impact")]);
        assert!(apply_scene_board(&mut p, 1, 1, serde_json::json!({ "cuts": [] })).is_err());
        assert!(apply_scene_board(&mut p, 1, 7, serde_json::json!({ "cuts": [{}] })).is_err());
    }
}
