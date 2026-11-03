//! STAGE: the AI blocks one scene (where everyone stands, the shots in panel order, what each
//! actor does in each shot), then Blender assembles it from library assets, renders a previz
//! still per shot and the final frames, and ffmpeg joins them into the scene's clip.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Result};
use mv_ai::CancellationToken;
use mv_recap::{Brain, ChapterRead, ModelChoice};
use serde_json::{json, Value};

use crate::ai::{ask_json, request, Llm};
use crate::library::{Category, Library};
use crate::plan::{Action, Blocking, Episode, Placement, ScenePlan, SceneRender, Shot, ShotLine};
use crate::util::truncate;

/// Actions the assembler knows how to animate.
pub(crate) const ACTIONS: &str = "idle, walk_to, run_to, turn_to, talk, nod, shake_head, point, raise_arm, wave, guard, punch, kick, lift, bow, jump, fall, collapse, get_up, sit";
const FRAMINGS: [&str; 9] = ["establishing", "wide", "full", "medium", "medium_close", "close", "extreme_close", "two_shot", "over_shoulder"];
const RENDER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// One thing standing in the scene.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActorInfo {
    pub key: String,
    pub name: String,
    pub kind: &'static str,
    pub dims: [f32; 3],
    pub blend: PathBuf,
}

pub(crate) fn actors(ep: &Episode, scene: &ScenePlan, lib: &Library) -> Vec<ActorInfo> {
    let mut v = vec![];
    for key in scene.characters.iter().chain(scene.props.iter()) {
        let Some(need) = ep.need(key) else { continue };
        let Some(a) = need.asset_id.as_deref().and_then(|id| lib.get(id)) else { continue };
        if !lib.ready(a) {
            continue;
        }
        let kind = match a.category {
            Category::Characters => "character",
            Category::Vehicles => "vehicle",
            Category::Fx => "fx",
            _ => "prop",
        };
        v.push(ActorInfo { key: key.clone(), name: need.name.clone(), kind, dims: a.dims, blend: lib.blend_of(a) });
    }
    v
}

/// Half the usable stage: the location's size minus a margin, between 3 and 8 m.
fn stage_half(env_dims: [f32; 3]) -> f32 {
    let w = env_dims[0].min(env_dims[1]);
    if w <= 0.1 {
        return 5.0;
    }
    (w / 2.0 - 1.5).clamp(3.0, 8.0)
}

const SYSTEM: &str = "You are the director of a 3D anime adaptation of a manga. You block one scene for the 3D animators: \
where everyone stands, the shots in the order of the manga panels, and what each actor does. Reply with one JSON object only.";

pub(crate) fn prompt(ep: &Episode, scene: &ScenePlan, read: &ChapterRead, brain: &Brain, actors: &[ActorInfo], env: Option<(&str, [f32; 3])>) -> String {
    let half = stage_half(env.map(|e| e.1).unwrap_or([0.0; 3]));
    let mut s = format!("Episode: {} ({}, {}).\nScene {}: {}\nMood: {}. Time: {}.\nWhat happens: {}\n", ep.title, ep.manga_title, ep.label, scene.index + 1, scene.title, scene.mood, scene.time_of_day, scene.summary);
    if let Some((name, d)) = env {
        s.push_str(&format!("Location: {name}, {:.0} x {:.0} m. The stage is x and y from -{half:.1} to {half:.1} m; the cameras are on the -Y side.\n", d[0], d[1]));
    }
    s.push_str("\nActors (key, what, height m):\n");
    for a in actors {
        s.push_str(&format!("- {} ({}, {}): {:.2}\n", a.key, a.name, a.kind, a.dims[2]));
    }
    if let Some(note) = scene.note.as_deref().filter(|n| !n.trim().is_empty()) {
        s.push_str(&format!("\nThe viewer asked for this change from the last take: {note}\n"));
    }
    s.push_str("\nManga panels of this scene (page, panel):\n");
    let mut n = 0;
    for p in read.pages.iter().filter(|p| p.index >= scene.pages[0] && p.index <= scene.pages[1]) {
        for (i, panel) in p.panels.iter().enumerate() {
            if n >= 40 {
                break;
            }
            n += 1;
            let who: Vec<String> = panel.characters.iter().map(|c| brain.canonical(&c.name)).collect();
            s.push_str(&format!("({}, {i}) {}", p.index, truncate(&panel.description, 200)));
            if !who.is_empty() {
                s.push_str(&format!(" | who: {}", who.join(", ")));
            }
            for l in panel.lines.iter().filter(|l| l.kind != "sfx").take(3) {
                s.push_str(&format!(" | {}: \"{}\"", l.speaker.as_deref().unwrap_or("narration"), truncate(&l.text, 120)));
            }
            s.push('\n');
        }
    }
    s.push_str(&format!(
        r#"
Answer with this JSON:
{{"placements": [{{"actor": "key", "pos": [x, y], "facing": "other actor key or [x, y] or degrees"}}],
 "shots": [{{"panel": [page, panel], "description": "what the shot shows", "framing": "{}",
   "subject": "actor key", "secondary": "actor key or null", "angle": "front|side|back|left|right|low|high",
   "move": "static|push_in|pull_out|pan_left|pan_right|orbit|shake", "duration": seconds,
   "actions": [{{"actor": "key", "type": "action", "target": "actor key or [x, y] or null", "start": seconds into the shot, "duration": seconds}}],
   "line": {{"speaker": "actor key or null", "text": "the line spoken in this shot"}} }}]}}
Rules:
- 3 to 10 shots following the panels in order; together 15 to 45 seconds. A shot is 1.5 to 7 seconds.
- Every actor gets a placement inside the stage; characters who talk face each other.
- Action types: {ACTIONS}. Actions run in story order; an actor keeps where the last action left them.
- Act out what the panels show: fights get guard, punch, kick, fall; talks get talk, nod, point; arrivals walk_to or run_to.
- Use over_shoulder and two_shot for conversations, close and extreme_close for emotion and reveals, wide or establishing to open the scene.
"#,
        FRAMINGS.join("|")
    ));
    s
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|x| x as f32).filter(|x| x.is_finite())
}

/// Reads the AI's blocking, keeping only actors that exist and clamping positions and times.
pub(crate) fn parse_blocking(v: &Value, actors: &[ActorInfo], half: f32) -> Result<Blocking> {
    let known = |k: &str| actors.iter().any(|a| a.key == k);
    let clamp_pt = |p: &Value| -> Value {
        match p.as_array() {
            Some(a) if a.len() >= 2 => json!([num(&a[0]).unwrap_or(0.0).clamp(-half, half), num(&a[1]).unwrap_or(0.0).clamp(-half, half)]),
            _ => p.clone(),
        }
    };
    let target = |t: &Value| -> Value {
        match t {
            Value::String(k) if known(k) => t.clone(),
            Value::Array(_) => clamp_pt(t),
            Value::Number(_) => t.clone(),
            _ => Value::Null,
        }
    };
    let mut placements = vec![];
    for p in v.get("placements").and_then(|p| p.as_array()).into_iter().flatten() {
        let actor = p.get("actor").and_then(|a| a.as_str()).unwrap_or_default();
        if !known(actor) || placements.iter().any(|x: &Placement| x.actor == actor) {
            continue;
        }
        let pos = p.get("pos").and_then(|a| a.as_array()).map(|a| [a.first().and_then(num).unwrap_or(0.0), a.get(1).and_then(num).unwrap_or(0.0)]).unwrap_or([0.0, 0.0]);
        placements.push(Placement { actor: actor.into(), pos: [pos[0].clamp(-half, half), pos[1].clamp(-half, half)], facing: target(p.get("facing").unwrap_or(&Value::Null)) });
    }
    // Anyone the AI forgot stands in a row at the front of the stage.
    for (i, a) in actors.iter().enumerate() {
        if !placements.iter().any(|p| p.actor == a.key) {
            let x = (i as f32 - (actors.len() as f32 - 1.0) / 2.0) * 1.6;
            placements.push(Placement { actor: a.key.clone(), pos: [x.clamp(-half, half), 0.0], facing: json!(0) });
        }
    }
    let mut shots = vec![];
    for s in v.get("shots").and_then(|s| s.as_array()).into_iter().flatten().take(12) {
        let duration = s.get("duration").and_then(num).unwrap_or(3.0).clamp(1.5, 7.0);
        let subject = s.get("subject").and_then(|x| x.as_str()).filter(|k| known(k)).map(String::from).or_else(|| actors.iter().find(|a| a.kind == "character").or(actors.first()).map(|a| a.key.clone())).unwrap_or_default();
        let secondary = s.get("secondary").and_then(|x| x.as_str()).filter(|k| known(k) && *k != subject).map(String::from);
        let mut framing = s.get("framing").and_then(|x| x.as_str()).unwrap_or("medium").to_lowercase().replace([' ', '-'], "_");
        if !FRAMINGS.contains(&framing.as_str()) {
            framing = "medium".into();
        }
        let actions = s
            .get("actions")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let actor = a.get("actor").and_then(|x| x.as_str()).filter(|k| known(k))?;
                let start = a.get("start").and_then(num).unwrap_or(0.0).clamp(0.0, duration - 0.5);
                let len = a.get("duration").and_then(num).unwrap_or(duration - start).clamp(0.4, duration - start);
                Some(Action { actor: actor.into(), kind: a.get("type").and_then(|x| x.as_str()).unwrap_or("idle").to_lowercase(), target: target(a.get("target").unwrap_or(&Value::Null)), start, duration: len })
            })
            .collect();
        let panel = s.get("panel").and_then(|p| p.as_array()).and_then(|p| Some([p.first()?.as_u64()? as usize, p.get(1)?.as_u64()? as usize]));
        let line = s.get("line").filter(|l| l.is_object()).and_then(|l| {
            let text = l.get("text").and_then(|t| t.as_str()).unwrap_or_default().trim().to_string();
            (!text.is_empty()).then(|| ShotLine { speaker: l.get("speaker").and_then(|x| x.as_str()).map(String::from), text })
        });
        shots.push(Shot {
            panel,
            description: s.get("description").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            framing,
            subject,
            secondary,
            angle: s.get("angle").and_then(|x| x.as_str()).unwrap_or("front").to_lowercase(),
            camera_move: s.get("move").and_then(|x| x.as_str()).unwrap_or("static").to_lowercase(),
            duration,
            actions,
            line,
        });
    }
    if shots.is_empty() {
        bail!("the blocking had no shots");
    }
    Ok(Blocking { placements, shots })
}

/// A plain blocking when the AI's answer is unusable: open wide, then each character talks.
pub(crate) fn fallback_blocking(actors: &[ActorInfo], scene: &ScenePlan) -> Blocking {
    let chars: Vec<&ActorInfo> = actors.iter().filter(|a| a.kind == "character").collect();
    let placements = actors
        .iter()
        .enumerate()
        .map(|(i, a)| Placement { actor: a.key.clone(), pos: [(i as f32 - (actors.len() as f32 - 1.0) / 2.0) * 1.6, if a.kind == "character" { 0.0 } else { 1.2 }], facing: json!(0) })
        .collect();
    let first = chars.first().copied().or(actors.first()).map(|a| a.key.clone()).unwrap_or_default();
    let mut shots = vec![Shot { framing: "wide".into(), subject: first.clone(), angle: "front".into(), camera_move: "push_in".into(), duration: 4.0, description: scene.title.clone(), ..Default::default() }];
    for c in chars.iter().take(4) {
        shots.push(Shot {
            framing: "medium".into(),
            subject: c.key.clone(),
            angle: "front".into(),
            camera_move: "static".into(),
            duration: 3.0,
            actions: vec![Action { actor: c.key.clone(), kind: "talk".into(), target: Value::Null, start: 0.3, duration: 2.4 }],
            ..Default::default()
        });
    }
    Blocking { placements, shots }
}

pub(crate) async fn block(
    llm: &dyn Llm,
    model: &ModelChoice,
    ep: &Episode,
    scene: &ScenePlan,
    read: &ChapterRead,
    brain: &Brain,
    actors: &[ActorInfo],
    env: Option<(&str, [f32; 3])>,
    cancel: &CancellationToken,
) -> Result<Blocking> {
    let user = prompt(ep, scene, read, brain, actors, env);
    let v = ask_json(llm, request(model, SYSTEM, user, vec![]), cancel).await?;
    parse_blocking(&v, actors, stage_half(env.map(|e| e.1).unwrap_or([0.0; 3])))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub samples: u32,
}

/// The scene file for `assemble.py`: seconds become frames, shots run back to back.
pub(crate) fn scene_spec(b: &Blocking, actors: &[ActorInfo], env_blend: Option<&Path>, sky: Option<&Path>, scene: &ScenePlan, rs: &RenderSettings, out: &Path) -> Value {
    let fps = rs.fps as f32;
    let mut shots = vec![];
    let mut actions = vec![];
    let mut start = 1u32;
    for (i, s) in b.shots.iter().enumerate() {
        let len = (s.duration * fps).round().max(12.0) as u32;
        let end = start + len - 1;
        shots.push(json!({ "id": format!("shot_{i:02}"), "frames": [start, end], "framing": s.framing, "subject": s.subject, "secondary": s.secondary, "angle": s.angle, "move": s.camera_move }));
        for a in &s.actions {
            let a0 = start + (a.start * fps).round() as u32;
            let a1 = (a0 + (a.duration * fps).round().max(6.0) as u32).min(end);
            actions.push(json!({ "actor": a.actor, "type": a.kind, "frames": [a0, a1.max(a0 + 4)], "target": a.target }));
        }
        start = end + 1;
    }
    let actors_json: Vec<Value> = actors
        .iter()
        .map(|a| {
            let p = b.placements.iter().find(|p| p.actor == a.key);
            json!({ "id": a.key, "blend": a.blend, "kind": a.kind, "pos": p.map(|p| p.pos).unwrap_or([0.0, 0.0]), "facing": p.map(|p| p.facing.clone()).unwrap_or(Value::Null), "scale": 1.0 })
        })
        .collect();
    let tod = if scene.time_of_day.is_empty() { "day" } else { scene.time_of_day.as_str() };
    json!({
        "fps": rs.fps, "width": rs.width, "height": rs.height, "samples": rs.samples, "mode": "both",
        "time_of_day": tod, "mood": scene.mood,
        "sky": { "hdri": sky },
        "environment": env_blend.map(|p| json!({ "blend": p })),
        "actors": actors_json, "actions": actions, "shots": shots,
        "previz_dir": out.join("previz"), "frames_dir": out.join("frames"), "blend_out": out.join("scene.blend"),
        "previz_width": 640, "previz_height": 360,
    })
}

pub(crate) async fn render(blender: &Path, scripts: &Path, ffmpeg: Option<&Path>, spec: &Value, out: &Path, fps: u32, cancel: &CancellationToken) -> Result<SceneRender> {
    for sub in ["frames", "previz"] {
        let _ = std::fs::remove_dir_all(out.join(sub));
    }
    std::fs::create_dir_all(out)?;
    let job = out.join("scene.json");
    std::fs::write(&job, serde_json::to_vec_pretty(spec)?)?;
    let v = crate::blender::run(blender, &scripts.join("assemble.py"), &job, &out.join("blender.log"), RENDER_TIMEOUT, cancel).await?;
    let frames = v.get("frames").and_then(|f| f.as_u64()).unwrap_or(0) as u32;
    let shots: Vec<[u32; 2]> = v
        .get("shots")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let f = s.get("frames")?.as_array()?;
            Some([f.first()?.as_u64()? as u32, f.get(1)?.as_u64()? as u32])
        })
        .collect();
    let previz: Vec<PathBuf> = v.get("previz").and_then(|p| p.as_array()).into_iter().flatten().filter_map(|p| p.as_str().map(PathBuf::from)).collect();
    let frames_dir = out.join("frames");
    let stills: Vec<PathBuf> = shots.iter().map(|[a, b]| frames_dir.join(format!("f_{:04}.png", (a + b) / 2))).filter(|p| p.is_file()).collect();
    let clip = match ffmpeg {
        Some(ff) => {
            let clip = out.join("scene.mp4");
            crate::blender::encode_clip(ff, &frames_dir, fps, &clip).await?;
            Some(clip)
        }
        None => None,
    };
    Ok(SceneRender { clip, previz, stills, frames_dir, blend: out.join("scene.blend"), seconds: frames as f32 / fps as f32, shots, rendered_at: crate::util::now() })
}
