//! The assistant's creative tools: custom motion-graphic videos described as a JSON spec
//! (`motion_compose`, `motion_preview`, `motion_assets`), separate from the recap pipeline.
//!
//! The spec is checked strictly here (every unknown key, wrong type or out-of-range value comes
//! back with its path, so the model can fix it), image sources are resolved to files (library
//! pages and panels, covers, Studio files), then `python/motion_compose.py` renders it (NVENC
//! with a libx264 fallback) into `<Studio>/motion/`.
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use futures::FutureExt as _;
use serde_json::{json, Map, Value};
use ui_assistant::tools::{bool_arg, num_arg, str_arg, Phrases, ProgressFn, Risk, ToolDef, ToolOutput, ToolResult};

use crate::setup::Env;

pub const FORMATS: &[(&str, u32, u32)] = &[("landscape", 1920, 1080), ("reels", 1080, 1920), ("square", 1080, 1080)];
pub const TRANSITIONS: &[&str] = &["none", "cut", "fade", "fadeblack", "fadewhite", "slide_left", "slide_right", "slide_up", "slide_down", "zoom_in", "zoom_out", "wipe_left", "wipe_right", "flash", "whip"];
pub const EASES: &[&str] = &["linear", "in", "out", "in_out", "back", "bounce", "elastic", "step", "ease_in", "ease_out", "ease_in_out", "back_out", "bounce_out", "elastic_out"];
pub const LAYER_TYPES: &[&str] = &["image", "text", "shape", "particles"];
pub const LAYER_EFFECTS: &[&str] = &["glow", "edge_glow", "shadow", "blur", "outline", "shake", "pulse", "float"];
pub const SCENE_EFFECTS: &[&str] = &["grade", "vignette", "grain", "flash", "shake", "letterbox", "zoom", "chromatic", "particles"];
pub const GRADES: &[&str] = &["none", "warm", "cool", "noir", "vivid", "teal_orange", "sepia"];
pub const PARTICLES: &[&str] = &["embers", "snow", "dust", "sparks", "stars", "rain"];
pub const FONTS: &[&str] = &["bold", "black", "regular", "impact", "serif", "mono", "condensed"];
pub const FITS: &[&str] = &["contain", "cover", "width", "height", "none"];
pub const BLENDS: &[&str] = &["normal", "add", "screen", "multiply"];
pub const SHAPES: &[&str] = &["rect", "circle", "line"];
pub const ALIGNS: &[&str] = &["left", "center", "right"];
/// Built-in sound effects of `python/sfx.py`.
pub const SFX: &[&str] = &["impact", "whoosh", "riser", "boom", "sword_clang", "magic", "heartbeat", "glass", "crowd_gasp", "thunder", "footsteps_run", "door_slam"];
pub const ANIMATED: &[&str] = &["x", "y", "scale", "rotation", "opacity"];
pub const MAX_SECONDS: f64 = 180.0;
pub const PREVIEW_SECONDS: f64 = 3.0;
const MAX_SCENES: usize = 60;
const MAX_LAYERS: usize = 30;

/// The schema in the tool description: compact enough for small models, with two examples.
pub const SCHEMA: &str = r##"Spec (JSON): {"format":"landscape|reels|square" (or "width","height"), "fps":30, "background":"#000000", "title":"name",
 "scenes":[{"duration":s, "background":"#hex" or {"image":SRC,"blur":0-40,"dim":0-1}, "transition":{"type":T,"duration":s} (into this scene),
   "layers":[LAYER...], "effects":[{"type":"grade","preset":"warm|cool|noir|vivid|teal_orange|sepia"}|{"type":"vignette","strength":0-1}|{"type":"grain","amount":0-1}|{"type":"flash","at":s,"duration":s}|{"type":"shake","at":s,"duration":s,"strength":0-1}|{"type":"zoom","from":1,"to":1.1}|{"type":"letterbox","size":0-0.2}|{"type":"chromatic","amount":0-1}|{"type":"particles","kind":K}]}],
 "audio":{"narration":[{"text":"…","at":s,"voice":id}], "music":{"src":path,"volume":0.35,"fade_out":2}, "sfx":[{"name":SFX,"at":s,"volume":1}]}}
LAYER common: "type":"image|text|shape|particles", "x","y" (0-1 of the frame, anchor point), "scale", "rotation" (deg), "opacity" (0-1), "anchor":[0.5,0.5], "start","end" (s in scene), "blend":"normal|add|screen|multiply",
 "keyframes":[{"t":s,"x"?,"y"?,"scale"?,"rotation"?,"opacity"?,"ease":"linear|in|out|in_out|back|bounce|elastic|step"}], "effects":[{"type":"glow|edge_glow|shadow|blur|outline|shake|pulse|float", "strength"?, "color"?}]
 image: "src":SRC, "fit":"contain|cover|width|height|none", "width" (0-1 of frame width), "layered":true (2.5D panel motion when the Layered motion pack is installed)
 text: "text", "size" (0-0.5 of frame height), "color", "font":"bold|black|regular|impact|serif|mono|condensed", "stroke":{"color","width"}, "align", "max_width" (0-1), "shadow":true, "typewriter":true
 shape: "shape":"rect|circle|line", "w","h" (0-1), "color", "radius"; particles: "kind":"embers|snow|dust|sparks|stars|rain", "count", "color", "speed"
SRC: {"manga":title,"chapter":"12","page":3} (+"panel":1 for one panel), {"cover":title}, {"studio":"relative/path.png"}, or an absolute file path.
T: none|cut|fade|fadeblack|fadewhite|slide_left|slide_right|slide_up|slide_down|zoom_in|zoom_out|wipe_left|wipe_right|flash|whip. SFX: impact|whoosh|riser|boom|sword_clang|magic|heartbeat|glass|crowd_gasp|thunder|footsteps_run|door_slam.
Example 1 (title card): {"format":"landscape","scenes":[{"duration":4,"background":"#0b0b12","layers":[{"type":"text","text":"SOLO LEVELING","size":0.12,"font":"black","color":"#ffffff","x":0.5,"y":0.5,"effects":[{"type":"glow","color":"#4aa3ff","strength":0.8}],"keyframes":[{"t":0,"scale":0.8,"opacity":0},{"t":0.8,"scale":1,"opacity":1,"ease":"back"},{"t":4,"scale":1.08}]}],"effects":[{"type":"vignette","strength":0.5}]}],"audio":{"sfx":[{"name":"boom","at":0.7}]}}
Example 2 (panel spotlight): {"format":"reels","scenes":[{"duration":3,"background":{"image":{"manga":"Solo Leveling","chapter":"1","page":2},"blur":24,"dim":0.5},"layers":[{"type":"image","src":{"manga":"Solo Leveling","chapter":"1","page":2,"panel":1},"fit":"width","width":0.9,"x":0.5,"y":0.45,"effects":[{"type":"edge_glow","color":"#ff3355"}],"keyframes":[{"t":0,"scale":1},{"t":3,"scale":1.12,"ease":"in_out"}]},{"type":"particles","kind":"embers","count":60}]},{"duration":3,"transition":{"type":"whip","duration":0.4},"layers":[{"type":"text","text":"The weakest hunter…","size":0.06,"y":0.8,"typewriter":true}]}],"audio":{"narration":[{"text":"The weakest hunter of all mankind.","at":0.5}],"sfx":[{"name":"whoosh","at":2.8}]}}"##;

// ── Validation ────────────────────────────────────────────────────────────────────

/// Resolves an image source (`SRC`) to `{"path", "crop"?, "panel"?, "output_dir"?}`.
pub type Resolve<'a> = dyn Fn(&Value) -> Result<Value, String> + 'a;

struct Check<'a> {
    errors: Vec<String>,
    warnings: Vec<String>,
    resolve: &'a Resolve<'a>,
    voice_ok: &'a dyn Fn(&str) -> bool,
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// " (did you mean \"x\"?)" for a near miss among `allowed`.
fn near(word: &str, allowed: &[&str]) -> String {
    allowed.iter().map(|a| (distance(&word.to_lowercase(), a), *a)).filter(|(d, _)| *d <= 2.max(word.len() / 3)).min().map(|(_, a)| format!(" (did you mean \"{a}\"?)")).unwrap_or_default()
}

fn is_color(s: &str) -> bool {
    let h = s.trim().trim_start_matches('#');
    s.trim().starts_with('#') && matches!(h.len(), 3 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

impl Check<'_> {
    fn err(&mut self, path: &str, msg: impl AsRef<str>) {
        self.errors.push(format!("{path}: {}", msg.as_ref()));
    }

    fn keys(&mut self, path: &str, o: &Map<String, Value>, allowed: &[&str]) {
        for k in o.keys() {
            if !allowed.contains(&k.as_str()) {
                let hint = near(k, allowed);
                self.err(path, format!("unknown key \"{k}\"{hint}; allowed: {}", allowed.join(", ")));
            }
        }
    }

    fn num(&mut self, path: &str, o: &Map<String, Value>, k: &str, lo: f64, hi: f64, default: f64) -> f64 {
        match o.get(k) {
            None | Some(Value::Null) => default,
            Some(Value::Number(n)) => {
                let v = n.as_f64().unwrap_or(default);
                if !(lo..=hi).contains(&v) {
                    self.err(&format!("{path}.{k}"), format!("{v} is out of range ({lo} to {hi})"));
                }
                v.clamp(lo, hi)
            }
            Some(other) => {
                self.err(&format!("{path}.{k}"), format!("must be a number, got {other}"));
                default
            }
        }
    }

    fn pick(&mut self, path: &str, o: &Map<String, Value>, k: &str, allowed: &[&str], default: &str) -> String {
        match o.get(k) {
            None | Some(Value::Null) => default.to_string(),
            Some(Value::String(s)) if allowed.contains(&s.trim().to_lowercase().as_str()) => s.trim().to_lowercase(),
            Some(Value::String(s)) => {
                let hint = near(s, allowed);
                self.err(&format!("{path}.{k}"), format!("\"{s}\" is not one of {}{hint}", allowed.join(", ")));
                default.to_string()
            }
            Some(other) => {
                self.err(&format!("{path}.{k}"), format!("must be a string, got {other}"));
                default.to_string()
            }
        }
    }

    fn color(&mut self, path: &str, o: &Map<String, Value>, k: &str, default: &str) -> String {
        match o.get(k) {
            None | Some(Value::Null) => default.to_string(),
            Some(Value::String(s)) if is_color(s) => s.trim().to_string(),
            Some(other) => {
                self.err(&format!("{path}.{k}"), format!("must be a colour like \"#ff3355\", got {other}"));
                default.to_string()
            }
        }
    }

    fn boolean(&mut self, path: &str, o: &Map<String, Value>, k: &str) -> bool {
        match o.get(k) {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(other) => {
                self.err(&format!("{path}.{k}"), format!("must be true or false, got {other}"));
                false
            }
        }
    }

    fn obj<'v>(&mut self, path: &str, v: &'v Value) -> Option<&'v Map<String, Value>> {
        match v.as_object() {
            Some(o) => Some(o),
            None => {
                self.err(path, format!("must be an object, got {v}"));
                None
            }
        }
    }

    fn list<'v>(&mut self, path: &str, o: &'v Map<String, Value>, k: &str) -> &'v [Value] {
        match o.get(k) {
            None | Some(Value::Null) => &[],
            Some(Value::Array(a)) => a,
            Some(other) => {
                self.err(&format!("{path}.{k}"), format!("must be a list, got {other}"));
                &[]
            }
        }
    }

    fn src(&mut self, path: &str, v: &Value) -> Value {
        match (self.resolve)(v) {
            Ok(r) => r,
            Err(e) => {
                self.err(path, e);
                Value::Null
            }
        }
    }

    fn effects(&mut self, path: &str, list: &[Value], allowed: &[&str], dur: f64) -> Vec<Value> {
        let mut out = vec![];
        for (i, e) in list.iter().enumerate() {
            let p = format!("{path}[{i}]");
            let Some(o) = self.obj(&p, e) else { continue };
            let ty = self.pick(&p, o, "type", allowed, "");
            if ty.is_empty() {
                if !o.contains_key("type") {
                    self.err(&p, format!("needs a \"type\": {}", allowed.join(", ")));
                }
                continue;
            }
            let mut n = Map::new();
            n.insert("type".into(), json!(ty));
            let keys: &[&str] = match ty.as_str() {
                "glow" => &["type", "strength", "color"],
                "edge_glow" => &["type", "color", "width", "strength"],
                "shadow" => &["type", "opacity", "offset", "blur", "color"],
                "blur" => &["type", "radius"],
                "outline" => &["type", "color", "width"],
                "shake" => &["type", "strength", "at", "duration"],
                "pulse" | "float" => &["type", "amount", "period"],
                "grade" => &["type", "preset"],
                "vignette" => &["type", "strength"],
                "grain" => &["type", "amount"],
                "flash" => &["type", "at", "duration", "color"],
                "letterbox" => &["type", "size"],
                "zoom" => &["type", "from", "to", "ease"],
                "chromatic" => &["type", "amount"],
                "particles" => &["type", "kind", "count", "color", "speed", "size"],
                _ => &["type"],
            };
            self.keys(&p, o, keys);
            let mut put = |k: &str, v: Value| {
                n.insert(k.into(), v);
            };
            match ty.as_str() {
                "glow" => {
                    put("strength", json!(self.num(&p, o, "strength", 0., 2., 0.6)));
                    put("color", json!(self.color(&p, o, "color", "#ffffff")));
                }
                "edge_glow" => {
                    put("color", json!(self.color(&p, o, "color", "#ff3355")));
                    put("width", json!(self.num(&p, o, "width", 0.001, 0.1, 0.012)));
                    put("strength", json!(self.num(&p, o, "strength", 0., 2., 1.)));
                }
                "shadow" => {
                    put("opacity", json!(self.num(&p, o, "opacity", 0., 1., 0.6)));
                    put("offset", json!(self.num(&p, o, "offset", 0., 0.1, 0.01)));
                    put("blur", json!(self.num(&p, o, "blur", 0., 0.1, 0.012)));
                    put("color", json!(self.color(&p, o, "color", "#000000")));
                }
                "blur" => put("radius", json!(self.num(&p, o, "radius", 0., 0.1, 0.01))),
                "outline" => {
                    put("color", json!(self.color(&p, o, "color", "#ffffff")));
                    put("width", json!(self.num(&p, o, "width", 0.001, 0.05, 0.004)));
                }
                "shake" => {
                    put("strength", json!(self.num(&p, o, "strength", 0., 1., 0.4)));
                    let at = self.num(&p, o, "at", 0., dur, 0.);
                    put("at", json!(at));
                    put("duration", json!(self.num(&p, o, "duration", 0.01, dur.max(0.01), (dur - at).max(0.01))));
                }
                "pulse" | "float" => {
                    put("amount", json!(self.num(&p, o, "amount", 0., 1., if ty == "pulse" { 0.05 } else { 0.02 })));
                    put("period", json!(self.num(&p, o, "period", 0.1, 20., 2.)));
                }
                "grade" => put("preset", json!(self.pick(&p, o, "preset", GRADES, "warm"))),
                "vignette" => put("strength", json!(self.num(&p, o, "strength", 0., 1., 0.4))),
                "grain" => put("amount", json!(self.num(&p, o, "amount", 0., 1., 0.25))),
                "flash" => {
                    put("at", json!(self.num(&p, o, "at", 0., dur, 0.)));
                    put("duration", json!(self.num(&p, o, "duration", 0.02, 3., 0.25)));
                    put("color", json!(self.color(&p, o, "color", "#ffffff")));
                }
                "letterbox" => put("size", json!(self.num(&p, o, "size", 0., 0.25, 0.1))),
                "zoom" => {
                    put("from", json!(self.num(&p, o, "from", 0.2, 5., 1.)));
                    put("to", json!(self.num(&p, o, "to", 0.2, 5., 1.1)));
                    put("ease", json!(self.pick(&p, o, "ease", EASES, "in_out")));
                }
                "chromatic" => put("amount", json!(self.num(&p, o, "amount", 0., 1., 0.3))),
                "particles" => {
                    put("kind", json!(self.pick(&p, o, "kind", PARTICLES, "embers")));
                    put("count", json!(self.num(&p, o, "count", 1., 400., 60.).round()));
                    put("color", json!(self.color(&p, o, "color", "")));
                    put("speed", json!(self.num(&p, o, "speed", 0., 5., 1.)));
                    put("size", json!(self.num(&p, o, "size", 0.1, 5., 1.)));
                }
                _ => {}
            }
            out.push(Value::Object(n));
        }
        out
    }

    fn keyframes(&mut self, path: &str, list: &[Value], dur: f64) -> Vec<Value> {
        let mut out: Vec<Value> = vec![];
        let mut last = -1.0;
        for (i, k) in list.iter().enumerate() {
            let p = format!("{path}[{i}]");
            let Some(o) = self.obj(&p, k) else { continue };
            self.keys(&p, o, &["t", "x", "y", "scale", "rotation", "opacity", "ease"]);
            if !o.contains_key("t") {
                self.err(&p, "needs \"t\" (seconds into the scene)");
                continue;
            }
            let t = self.num(&p, o, "t", 0., dur, 0.);
            if t < last {
                self.err(&format!("{p}.t"), format!("{t} comes before the previous keyframe ({last}); keep keyframes in time order"));
            }
            last = t;
            let mut n = Map::new();
            n.insert("t".into(), json!(t));
            n.insert("ease".into(), json!(self.pick(&p, o, "ease", EASES, "in_out")));
            let mut any = false;
            for prop in ANIMATED {
                if o.contains_key(*prop) {
                    let (lo, hi) = prop_range(prop);
                    n.insert((*prop).into(), json!(self.num(&p, o, prop, lo, hi, 0.)));
                    any = true;
                }
            }
            if !any {
                self.err(&p, format!("animates nothing: give at least one of {}", ANIMATED.join(", ")));
            }
            out.push(Value::Object(n));
        }
        out
    }

    fn layer(&mut self, path: &str, v: &Value, dur: f64) -> Option<Value> {
        let o = self.obj(path, v)?;
        let ty = match o.get("type").and_then(Value::as_str) {
            Some(t) if LAYER_TYPES.contains(&t) => t.to_string(),
            Some(t) => {
                let hint = near(t, LAYER_TYPES);
                self.err(&format!("{path}.type"), format!("\"{t}\" is not one of {}{hint}", LAYER_TYPES.join(", ")));
                return None;
            }
            None => {
                self.err(path, format!("needs a \"type\": {}", LAYER_TYPES.join(", ")));
                return None;
            }
        };
        const COMMON: &[&str] = &["type", "x", "y", "scale", "rotation", "opacity", "anchor", "start", "end", "keyframes", "effects", "blend", "name"];
        let own: &[&str] = match ty.as_str() {
            "image" => &["src", "fit", "width", "layered", "crop"],
            "text" => &["text", "size", "color", "font", "stroke", "align", "max_width", "shadow", "typewriter", "line_height"],
            "shape" => &["shape", "w", "h", "color", "radius", "outline"],
            _ => &["kind", "count", "color", "speed", "size"],
        };
        let allowed: Vec<&str> = COMMON.iter().chain(own).copied().collect();
        self.keys(path, o, &allowed);
        let mut n = Map::new();
        n.insert("type".into(), json!(ty));
        for prop in ANIMATED {
            let (lo, hi) = prop_range(prop);
            let default = match *prop {
                "x" | "y" => 0.5,
                "scale" | "opacity" => 1.0,
                _ => 0.0,
            };
            n.insert((*prop).into(), json!(self.num(path, o, prop, lo, hi, default)));
        }
        let anchor = match o.get("anchor") {
            None | Some(Value::Null) => vec![0.5, 0.5],
            Some(Value::Array(a)) if a.len() == 2 && a.iter().all(|x| x.as_f64().is_some_and(|f| (0. ..=1.).contains(&f))) => a.iter().filter_map(Value::as_f64).collect(),
            Some(other) => {
                self.err(&format!("{path}.anchor"), format!("must be [x, y] with values 0 to 1, got {other}"));
                vec![0.5, 0.5]
            }
        };
        n.insert("anchor".into(), json!(anchor));
        let start = self.num(path, o, "start", 0., dur, 0.);
        let end = self.num(path, o, "end", 0., dur, dur);
        if end <= start {
            self.err(path, format!("\"end\" ({end}) must be after \"start\" ({start})"));
        }
        n.insert("start".into(), json!(start));
        n.insert("end".into(), json!(end));
        n.insert("blend".into(), json!(self.pick(path, o, "blend", BLENDS, "normal")));
        let kf = self.list(path, o, "keyframes");
        n.insert("keyframes".into(), json!(self.keyframes(&format!("{path}.keyframes"), kf, dur)));
        let fx = self.list(path, o, "effects");
        n.insert("effects".into(), json!(self.effects(&format!("{path}.effects"), fx, LAYER_EFFECTS, dur)));
        match ty.as_str() {
            "image" => {
                match o.get("src") {
                    Some(s) => {
                        let r = self.src(&format!("{path}.src"), s);
                        n.insert("src".into(), r);
                    }
                    None => self.err(path, "an image layer needs \"src\""),
                }
                n.insert("fit".into(), json!(self.pick(path, o, "fit", FITS, "contain")));
                if o.contains_key("width") {
                    n.insert("width".into(), json!(self.num(path, o, "width", 0.01, 4., 1.)));
                }
                let layered = self.boolean(path, o, "layered");
                n.insert("layered".into(), json!(layered));
                if let Some(c) = o.get("crop") {
                    match c.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()) {
                        Some(b) if b.len() == 4 && b[2] > b[0] && b[3] > b[1] && b.iter().all(|x| (0. ..=1.).contains(x)) => {
                            n.insert("crop".into(), json!(b));
                        }
                        _ => self.err(&format!("{path}.crop"), "must be [x0, y0, x1, y1] fractions with x1 > x0 and y1 > y0"),
                    }
                }
            }
            "text" => {
                let text = o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                if text.trim().is_empty() {
                    self.err(path, "a text layer needs non-empty \"text\"");
                } else if text.chars().count() > 400 {
                    self.err(&format!("{path}.text"), "is longer than 400 characters; split it over layers or scenes");
                }
                n.insert("text".into(), json!(text));
                n.insert("size".into(), json!(self.num(path, o, "size", 0.01, 0.5, 0.07)));
                n.insert("color".into(), json!(self.color(path, o, "color", "#ffffff")));
                n.insert("font".into(), json!(self.pick(path, o, "font", FONTS, "bold")));
                n.insert("align".into(), json!(self.pick(path, o, "align", ALIGNS, "center")));
                n.insert("max_width".into(), json!(self.num(path, o, "max_width", 0.05, 1., 0.9)));
                n.insert("line_height".into(), json!(self.num(path, o, "line_height", 0.8, 3., 1.15)));
                n.insert("shadow".into(), json!(self.boolean(path, o, "shadow")));
                n.insert("typewriter".into(), json!(self.boolean(path, o, "typewriter")));
                if let Some(s) = o.get("stroke") {
                    let p = format!("{path}.stroke");
                    if let Some(so) = self.obj(&p, s) {
                        self.keys(&p, so, &["color", "width"]);
                        let c = self.color(&p, so, "color", "#000000");
                        let w = self.num(&p, so, "width", 0., 0.3, 0.08);
                        n.insert("stroke".into(), json!({ "color": c, "width": w }));
                    }
                }
            }
            "shape" => {
                n.insert("shape".into(), json!(self.pick(path, o, "shape", SHAPES, "rect")));
                n.insert("w".into(), json!(self.num(path, o, "w", 0.001, 4., 0.3)));
                n.insert("h".into(), json!(self.num(path, o, "h", 0.001, 4., 0.1)));
                n.insert("color".into(), json!(self.color(path, o, "color", "#ffffff")));
                n.insert("radius".into(), json!(self.num(path, o, "radius", 0., 0.5, 0.)));
                if let Some(s) = o.get("outline") {
                    let p = format!("{path}.outline");
                    if let Some(so) = self.obj(&p, s) {
                        self.keys(&p, so, &["color", "width"]);
                        let c = self.color(&p, so, "color", "#ffffff");
                        let w = self.num(&p, so, "width", 0.001, 0.05, 0.004);
                        n.insert("outline".into(), json!({ "color": c, "width": w }));
                    }
                }
            }
            _ => {
                n.insert("kind".into(), json!(self.pick(path, o, "kind", PARTICLES, "embers")));
                n.insert("count".into(), json!(self.num(path, o, "count", 1., 400., 60.).round()));
                n.insert("color".into(), json!(self.color(path, o, "color", "")));
                n.insert("speed".into(), json!(self.num(path, o, "speed", 0., 5., 1.)));
                n.insert("size".into(), json!(self.num(path, o, "size", 0.1, 5., 1.)));
            }
        }
        Some(Value::Object(n))
    }

    fn audio(&mut self, v: &Value, total: f64) -> Value {
        let Some(o) = self.obj("audio", v) else { return json!({}) };
        self.keys("audio", o, &["narration", "music", "sfx", "volume"]);
        let mut narration = vec![];
        let items: Vec<Value> = match o.get("narration") {
            None | Some(Value::Null) => vec![],
            Some(Value::String(s)) => vec![json!({ "text": s })],
            Some(Value::Object(m)) => vec![Value::Object(m.clone())],
            Some(Value::Array(a)) => a.clone(),
            Some(other) => {
                self.err("audio.narration", format!("must be a list of {{\"text\", \"at\", \"voice\"}}, got {other}"));
                vec![]
            }
        };
        for (i, it) in items.iter().enumerate() {
            let p = format!("audio.narration[{i}]");
            let Some(no) = self.obj(&p, it) else { continue };
            self.keys(&p, no, &["text", "at", "voice", "speed"]);
            let text = no.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if text.is_empty() {
                self.err(&p, "needs \"text\"");
                continue;
            }
            if text.chars().count() > 1500 {
                self.err(&format!("{p}.text"), "is longer than 1500 characters; split it");
            }
            let at = self.num(&p, no, "at", 0., total, 0.);
            let voice = no.get("voice").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if !voice.is_empty() && !(self.voice_ok)(&voice) {
                self.err(&format!("{p}.voice"), format!("unknown voice \"{voice}\" (studio_voices lists them)"));
            }
            let speed = self.num(&p, no, "speed", 0.5, 2., 1.);
            narration.push(json!({ "text": text, "at": at, "voice": voice, "speed": speed }));
        }
        let music = match o.get("music") {
            None | Some(Value::Null) => Value::Null,
            Some(m) => match self.obj("audio.music", m) {
                Some(mo) => {
                    self.keys("audio.music", mo, &["src", "volume", "start", "fade_in", "fade_out", "loop"]);
                    let src = match mo.get("src") {
                        Some(s) => self.src("audio.music.src", s),
                        None => {
                            self.err("audio.music", "needs \"src\" (a music file; motion_assets lists the music library)");
                            Value::Null
                        }
                    };
                    json!({
                        "src": src,
                        "volume": self.num("audio.music", mo, "volume", 0., 2., 0.35),
                        "start": self.num("audio.music", mo, "start", 0., 3600., 0.),
                        "fade_in": self.num("audio.music", mo, "fade_in", 0., 30., 0.5),
                        "fade_out": self.num("audio.music", mo, "fade_out", 0., 30., 2.),
                        "loop": self.boolean("audio.music", mo, "loop"),
                    })
                }
                None => Value::Null,
            },
        };
        let mut sfx = vec![];
        let list = self.list("audio", o, "sfx").to_vec();
        for (i, it) in list.iter().enumerate() {
            let p = format!("audio.sfx[{i}]");
            let Some(so) = self.obj(&p, it) else { continue };
            self.keys(&p, so, &["name", "src", "at", "volume"]);
            let at = self.num(&p, so, "at", 0., total, 0.);
            let volume = self.num(&p, so, "volume", 0., 3., 1.);
            match (so.get("name").and_then(Value::as_str), so.get("src")) {
                (Some(n), _) if SFX.contains(&n) => sfx.push(json!({ "name": n, "at": at, "volume": volume })),
                (Some(n), _) => {
                    let hint = near(n, SFX);
                    self.err(&format!("{p}.name"), format!("\"{n}\" is not a built-in sound ({}){hint}", SFX.join(", ")));
                }
                (None, Some(s)) => {
                    let r = self.src(&format!("{p}.src"), s);
                    sfx.push(json!({ "src": r, "at": at, "volume": volume }));
                }
                (None, None) => self.err(&p, "needs \"name\" (a built-in sound) or \"src\" (a file)"),
            }
        }
        json!({ "narration": narration, "music": music, "sfx": sfx, "volume": self.num("audio", o, "volume", 0., 2., 1.) })
    }
}

fn prop_range(prop: &str) -> (f64, f64) {
    match prop {
        "x" | "y" => (-2., 3.),
        "scale" => (0., 20.),
        "rotation" => (-3600., 3600.),
        _ => (0., 1.),
    }
}

/// Checks a spec and returns it normalised (every default filled in, sources resolved, scene
/// start times), with warnings; or every problem found, each with its path.
pub fn validate(spec: &Value, resolve: &Resolve, voice_ok: &dyn Fn(&str) -> bool) -> Result<(Value, Vec<String>), Vec<String>> {
    let mut c = Check { errors: vec![], warnings: vec![], resolve, voice_ok };
    let Some(o) = spec.as_object() else {
        return Err(vec![format!("the spec must be a JSON object with \"scenes\", got {}", short(spec))]);
    };
    c.keys("spec", o, &["format", "width", "height", "fps", "background", "title", "scenes", "audio"]);
    let format = c.pick("spec", o, "format", &FORMATS.iter().map(|f| f.0).collect::<Vec<_>>(), "landscape");
    let (fw, fh) = FORMATS.iter().find(|f| f.0 == format).map(|f| (f.1, f.2)).unwrap_or((1920, 1080));
    let mut w = c.num("spec", o, "width", 160., 3840., fw as f64).round() as u32;
    let mut h = c.num("spec", o, "height", 160., 3840., fh as f64).round() as u32;
    w -= w % 2;
    h -= h % 2;
    let fps = c.num("spec", o, "fps", 12., 60., 30.).round();
    let background = c.color("spec", o, "background", "#000000");
    let scenes_in = match o.get("scenes") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        Some(Value::Array(_)) | None => {
            c.err("spec", "needs \"scenes\": a list of at least one scene");
            vec![]
        }
        Some(other) => {
            c.err("spec.scenes", format!("must be a list, got {}", short(other)));
            vec![]
        }
    };
    if scenes_in.len() > MAX_SCENES {
        c.err("spec.scenes", format!("has {} scenes; at most {MAX_SCENES}", scenes_in.len()));
    }
    let mut scenes: Vec<Value> = vec![];
    let mut start: f64 = 0.0;
    let mut prev_dur: f64 = 0.0;
    for (i, sv) in scenes_in.iter().take(MAX_SCENES).enumerate() {
        let p = format!("scenes[{i}]");
        let Some(so) = c.obj(&p, sv) else { continue };
        c.keys(&p, so, &["duration", "background", "transition", "layers", "effects", "name"]);
        if !so.contains_key("duration") {
            c.err(&p, "needs \"duration\" in seconds");
        }
        let dur = c.num(&p, so, "duration", 0.2, 60., 3.);
        // The transition into this scene overlaps the end of the previous one.
        let transition = match so.get("transition") {
            None | Some(Value::Null) => Value::Null,
            Some(t) => {
                let tobj: Map<String, Value> = match t {
                    Value::String(s) => [("type".to_string(), json!(s))].into_iter().collect(),
                    Value::Object(m) => m.clone(),
                    other => {
                        c.err(&format!("{p}.transition"), format!("must be {{\"type\", \"duration\"}}, got {}", short(other)));
                        Map::new()
                    }
                };
                c.keys(&format!("{p}.transition"), &tobj, &["type", "duration"]);
                let ty = c.pick(&format!("{p}.transition"), &tobj, "type", TRANSITIONS, "fade");
                let d = c.num(&format!("{p}.transition"), &tobj, "duration", 0.05, 3., 0.5);
                if i == 0 || matches!(ty.as_str(), "none" | "cut") {
                    Value::Null
                } else {
                    let limit = (prev_dur.min(dur) * 0.9_f64).max(0.05);
                    if d > limit {
                        c.err(&format!("{p}.transition.duration"), format!("{d} is longer than the scenes it joins allow ({limit:.2})"));
                    }
                    json!({ "type": ty, "duration": d.min(limit) })
                }
            }
        };
        if i > 0 {
            start += prev_dur - transition.get("duration").and_then(Value::as_f64).unwrap_or(0.);
        }
        let bg = match so.get("background") {
            None | Some(Value::Null) => json!({ "color": background, "image": null, "blur": 0, "dim": 0 }),
            Some(Value::String(s)) if is_color(s) => json!({ "color": s, "image": null, "blur": 0, "dim": 0 }),
            Some(Value::Object(m)) => {
                let bp = format!("{p}.background");
                c.keys(&bp, m, &["color", "image", "blur", "dim", "fit"]);
                let color = c.color(&bp, m, "color", &background);
                let image = m.get("image").map(|s| c.src(&format!("{bp}.image"), s)).unwrap_or(Value::Null);
                json!({ "color": color, "image": image, "blur": c.num(&bp, m, "blur", 0., 60., 0.), "dim": c.num(&bp, m, "dim", 0., 1., 0.), "fit": c.pick(&bp, m, "fit", FITS, "cover") })
            }
            Some(other) => {
                c.err(&format!("{p}.background"), format!("must be a colour \"#hex\" or {{\"image\", \"blur\", \"dim\"}}, got {}", short(other)));
                Value::Null
            }
        };
        let layers_in = c.list(&p, so, "layers").to_vec();
        if layers_in.len() > MAX_LAYERS {
            c.err(&format!("{p}.layers"), format!("has {} layers; at most {MAX_LAYERS}", layers_in.len()));
        }
        let layers: Vec<Value> = layers_in.iter().take(MAX_LAYERS).enumerate().filter_map(|(j, l)| c.layer(&format!("{p}.layers[{j}]"), l, dur)).collect();
        let fx_in = c.list(&p, so, "effects").to_vec();
        let effects = c.effects(&format!("{p}.effects"), &fx_in, SCENE_EFFECTS, dur);
        if layers.is_empty() && bg.get("image").is_none_or(Value::is_null) && effects.is_empty() {
            c.warnings.push(format!("{p} shows only a plain background"));
        }
        scenes.push(json!({ "start": round3(start), "duration": dur, "background": bg, "transition": transition, "layers": layers, "effects": effects }));
        prev_dur = dur;
    }
    let total = round3(start + prev_dur);
    if total > MAX_SECONDS {
        c.err("spec.scenes", format!("the video would be {total:.1} s long; at most {MAX_SECONDS} s"));
    }
    let audio = match o.get("audio") {
        None | Some(Value::Null) => json!({ "narration": [], "music": null, "sfx": [], "volume": 1.0 }),
        Some(a) => c.audio(a, total.max(0.)),
    };
    if !c.errors.is_empty() {
        return Err(c.errors);
    }
    let title = o.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
    Ok((json!({ "width": w, "height": h, "fps": fps, "background": background, "title": title, "duration": total, "scenes": scenes, "audio": audio }), c.warnings))
}

fn round3(v: f64) -> f64 {
    (v * 1000.).round() / 1000.
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 60 {
        s.chars().take(60).collect::<String>() + "…"
    } else {
        s
    }
}

/// The `spec` argument: an object, or a JSON string (repaired when a small model got the JSON
/// slightly wrong).
pub fn spec_arg(a: &Map<String, Value>) -> Result<Value, String> {
    match a.get("spec") {
        Some(Value::Object(_)) => Ok(a["spec"].clone()),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).or_else(|_| serde_json::from_str::<Value>(&ui_assistant::textcall::repair(s))).map_err(|e| format!("spec is not valid JSON: {e}")),
        Some(other) => Err(format!("spec must be a JSON object, got {}", short(other))),
        None if a.contains_key("scenes") => Ok(Value::Object(a.iter().filter(|(k, _)| !matches!(k.as_str(), "name" | "overwrite" | "quality" | "at" | "clip" | "start")).map(|(k, v)| (k.clone(), v.clone())).collect())),
        None => Err("Give the video as \"spec\": {\"scenes\": [...]} (see the schema in the tool description).".into()),
    }
}

/// A safe file name for the render ("Solo intro" → "Solo intro.mp4").
pub fn output_name(name: &str, title: &str) -> String {
    let raw = if !name.trim().is_empty() { name } else if !title.trim().is_empty() { title } else { "" };
    let clean: String = raw.trim().trim_end_matches(".mp4").chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean.chars().take(80).collect();
    // Windows drops trailing dots and spaces; "." / ".." would name a folder, not a file.
    let clean = clean.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    let stem = clean.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if clean.is_empty() || reserved {
        format!("motion-{}", chrono_stamp())
    } else {
        clean
    }
}

/// A fresh work folder for one render job (`<motion>/.work/job-<pid>-<ms>`): never derived from
/// the user's name, so no two jobs share one and no name can point it outside `.work`.
fn job_work_dir(dir: &Path) -> PathBuf {
    dir.join(".work").join(format!("job-{}-{}", std::process::id(), chrono_stamp_ms()))
}

// ── Path confinement ──────────────────────────────────────────────────────────────

/// True for a UNC / network / device path (`\\host\share`, `//host/share`, `\\?\…`): Windows
/// sends the user's credentials just to probe one, so the assistant never touches them.
fn is_network_path(s: &str) -> bool {
    let s = s.trim();
    if s.starts_with("\\\\") || s.starts_with("//") || s.starts_with("\\/") || s.starts_with("/\\") {
        return true;
    }
    Path::new(s).components().any(|c| match c {
        std::path::Component::Prefix(p) => !matches!(p.kind(), std::path::Prefix::Disk(_)),
        _ => false,
    })
}

fn has_parent_dir(p: &Path) -> bool {
    p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// An absolute local path the assistant may read: no network share, no `..`.
pub fn local_path(s: &str) -> Result<PathBuf, String> {
    let s = s.trim();
    if is_network_path(s) {
        return Err(format!("\"{s}\" is a network path; only files on this computer can be used"));
    }
    let p = PathBuf::from(s);
    if has_parent_dir(&p) {
        return Err(format!("\"{s}\" contains \"..\"; give the file's full path"));
    }
    Ok(p)
}

/// `rel` inside `root`, refusing anything that would land outside it (`..`, an absolute path, a
/// drive or share prefix, or a link that resolves elsewhere).
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_s = rel.trim().trim_start_matches(['/', '\\']);
    let r = Path::new(rel_s);
    if rel_s.is_empty() || is_network_path(rel) || has_parent_dir(r) || r.components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
        return Err(format!("\"{rel}\" must be a path inside {}", root.display()));
    }
    let p = root.join(r);
    if let (Ok(full), Ok(base)) = (p.canonicalize(), root.canonicalize()) {
        if !full.starts_with(&base) {
            return Err(format!("\"{rel}\" points outside {}", root.display()));
        }
    }
    Ok(p)
}

fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}")
}

pub fn motion_dir() -> PathBuf {
    mv_core::paths::studio().join("motion")
}

// ── Source resolution (the library, Studio files) ─────────────────────────────────

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "flac", "m4a", "aac", "opus"];

fn is_media(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).is_some_and(|e| IMAGE_EXTS.contains(&e.as_str()) || AUDIO_EXTS.contains(&e.as_str()) || e == "mp4")
}

/// A chapter's page images in reading order.
pub fn page_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))).collect()).unwrap_or_default();
    files.sort();
    files
}

/// `<chapter output>/studio/read.json` panels of page `index` (0-based), each a read.json panel.
pub fn page_panels(output_dir: &Path, index: usize) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(output_dir.join("studio").join("read.json")) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return vec![] };
    v.get("pages").and_then(Value::as_array).into_iter().flatten().enumerate().find(|(i, p)| p.get("index").and_then(Value::as_u64).map_or(*i == index, |x| x as usize == index)).and_then(|(_, p)| p.get("panels").and_then(Value::as_array).cloned()).unwrap_or_default()
}

fn panel_box(panel: &Value) -> Option<Vec<f64>> {
    let b: Vec<f64> = panel.get("bbox").or_else(|| panel.get("box"))?.as_array()?.iter().filter_map(Value::as_f64).collect();
    (b.len() == 4 && b[2] > b[0] && b[3] > b[1]).then_some(b)
}

/// The resolver used by the tools: library pages/panels, covers, Studio files, absolute paths.
pub fn resolve_src(v: &Value, groups: &[crate::data::MangaGroup]) -> Result<Value, String> {
    let file = |p: PathBuf| -> Result<Value, String> {
        if p.is_file() {
            Ok(json!({ "path": p.display().to_string() }))
        } else {
            Err(format!("file not found: {}", p.display()))
        }
    };
    match v {
        Value::String(s) if !s.trim().is_empty() => {
            let p = PathBuf::from(s.trim());
            if p.is_absolute() || is_network_path(s) {
                file(local_path(s)?)
            } else {
                // Relative: inside the Studio folder (motion renders, assets).
                let in_studio = inside(&mv_core::paths::studio(), s)?;
                if in_studio.is_file() {
                    file(in_studio)
                } else {
                    Err(format!("\"{s}\" is not a file; give an absolute path, {{\"studio\": \"relative/path\"}} or {{\"manga\", \"chapter\", \"page\"}}"))
                }
            }
        }
        Value::Object(o) => {
            if let Some(p) = o.get("path").and_then(Value::as_str) {
                return resolve_src(&json!(p), groups).map(|mut r| {
                    if let Some(c) = o.get("crop") {
                        r["crop"] = c.clone();
                    }
                    r
                });
            }
            if let Some(rel) = o.get("studio").and_then(Value::as_str) {
                return file(inside(&mv_core::paths::studio(), rel)?);
            }
            if let Some(t) = o.get("cover").and_then(Value::as_str) {
                let g = super::find_manga(groups, t)?;
                return match g.cover.clone().filter(|p| p.is_file()) {
                    Some(p) => file(p),
                    None => Err(format!("{} has no cover image on disk", g.title)),
                };
            }
            if let Some(t) = o.get("manga").and_then(Value::as_str) {
                let g = super::find_manga(groups, t)?;
                let ch = o.get("chapter").map(|c| match c {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
                let Some(ch) = ch else { return Err(format!("say which chapter of {} (\"chapter\": \"12\")", g.title)) };
                let picked = super::pick_chapters(g, &[ch])?;
                let c = picked.first().ok_or("no such chapter")?;
                let pages = page_files(&c.pages_dir);
                let page = o.get("page").and_then(Value::as_f64).unwrap_or(1.).round().max(1.) as usize;
                let Some(path) = pages.get(page - 1) else {
                    return Err(format!("{} {} has {} pages; page {page} does not exist", g.title, c.label(), pages.len()));
                };
                let mut out = json!({ "path": path.display().to_string(), "output_dir": c.output_dir.display().to_string(), "page_index": page - 1 });
                if let Some(k) = o.get("panel").and_then(Value::as_f64) {
                    let k = k.round().max(1.) as usize;
                    let panels = page_panels(&c.output_dir, page - 1);
                    if panels.is_empty() {
                        return Err(format!("page {page} of {} {} has no panels yet (Studio's Read step finds them); use the whole page or run studio_run_steps until read", g.title, c.label()));
                    }
                    let Some(panel) = panels.get(k - 1) else {
                        return Err(format!("page {page} has {} panels; panel {k} does not exist", panels.len()));
                    };
                    if let Some(b) = panel_box(panel) {
                        out["crop"] = json!(b);
                    }
                    out["panel"] = panel.clone();
                }
                return Ok(out);
            }
            Err(format!("unknown image source {}: use {{\"manga\", \"chapter\", \"page\"}}, {{\"cover\"}}, {{\"studio\"}} or a file path", short(v)))
        }
        other => Err(format!("an image source must be a path or an object, got {}", short(other))),
    }
}

// ── Running the renderer ──────────────────────────────────────────────────────────

/// Runs `motion_compose.py <args>`; progress lines go to `progress`; returns the final
/// `{"type": "done", ...}` line.
fn run_python(env: &Env, args: Vec<String>, progress: Option<ProgressFn>) -> Result<Value, String> {
    let py = env.python.as_ref().map_err(|e| format!("Studio's Python is not set up ({e}). Finish Studio → Setup first."))?;
    let layout = env.scripts.as_ref().map_err(|e| format!("Studio's scripts are missing ({e})"))?;
    let script = layout.script("motion_compose.py").map_err(|e| format!("{e}"))?;
    let mut cmd = mv_studio::python::quiet_command(&py.program);
    cmd.args(&py.prefix_args).arg(&script).args(&args).current_dir(&layout.dir);
    cmd.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Could not start Python ({}): {e}", py.command_line()))?;
    let stderr = child.stderr.take();
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let mut last: Option<Value> = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            match v.get("type").and_then(Value::as_str) {
                Some("progress") => {
                    if let Some(p) = &progress {
                        let pct = v.get("fraction").and_then(Value::as_f64).map(|f| format!("{:.0}% ", f * 100.)).unwrap_or_default();
                        p(format!("{pct}{}", v.get("message").and_then(Value::as_str).unwrap_or("")).trim().to_string());
                    }
                }
                Some("done") | Some("error") => last = Some(v),
                _ => {}
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errs.join().unwrap_or_default();
    match last {
        Some(v) if v["type"] == "done" => Ok(v),
        Some(v) => {
            let mut msg = v.get("message").and_then(Value::as_str).unwrap_or("the render failed").to_string();
            if let Some(list) = v.get("errors").and_then(Value::as_array).filter(|l| !l.is_empty()) {
                msg = format!("{msg}:\n- {}", list.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n- "));
            }
            Err(msg)
        }
        None => {
            let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
            Err(format!("The renderer stopped ({status}): {}", tail.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | ")))
        }
    }
}

/// Checks the spec against the library and returns the normalised spec, warnings and default voice.
async fn checked(a: &Map<String, Value>) -> Result<(Value, Vec<String>), String> {
    let spec = spec_arg(a)?;
    let groups = super::load_groups().await.unwrap_or_default();
    let default_voice = mv_studio::voices::default_voice();
    let r = mv_ui::bg::blocking(move || {
        let resolve = |v: &Value| resolve_src(v, &groups);
        let voice_ok = |v: &str| mv_studio::tts::find_voice(v).is_some();
        validate(&spec, &resolve, &voice_ok)
    })
    .await
    .map_err(|e| e.to_string())?;
    let (mut spec, warnings) = r.map_err(|errs| format!("The spec has {} problem{}; fix {} and call again:\n- {}", errs.len(), if errs.len() == 1 { "" } else { "s" }, if errs.len() == 1 { "it" } else { "them" }, errs.join("\n- ")))?;
    // Fill each narration's voice and its engine (Kokoro, Piper, Chatterbox…).
    if let Some(list) = spec["audio"]["narration"].as_array_mut() {
        for n in list {
            let v = n["voice"].as_str().filter(|v| !v.is_empty()).map(str::to_string).unwrap_or_else(|| default_voice.clone());
            n["engine"] = json!(mv_studio::tts::engine_hint(&v));
            n["voice"] = json!(v);
        }
    }
    Ok((spec, warnings))
}

async fn env_for_render() -> Result<Env, String> {
    let env = super::studio_env().await.ok_or("Could not check the Studio setup")?;
    if !env.runnable() {
        return Err("Rendering needs Studio's Python and scripts. Open Studio and finish Setup first.".into());
    }
    if !env.ffmpeg {
        return Err("FFmpeg is missing; install it (Studio → Setup) to render videos.".into());
    }
    Ok(env)
}

fn job_file(dir: &Path, job: &Value) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let path = dir.join(format!("job-{}-{}.json", std::process::id(), chrono_stamp_ms()));
    std::fs::write(&path, serde_json::to_vec_pretty(job).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(path)
}

fn chrono_stamp_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn config_json() -> Value {
    json!({ "vfx_python": mv_studio::vfx_pack::python().map(|p| p.display().to_string()) })
}

async fn compose(a: Map<String, Value>) -> ToolResult {
    super::studio_on().await?;
    let progress = ui_assistant::tools::progress_handle();
    let (spec, warnings) = checked(&a).await?;
    let env = env_for_render().await?;
    let dir = motion_dir();
    let name = output_name(str_arg(&a, "name"), spec["title"].as_str().unwrap_or(""));
    let out = dir.join(format!("{name}.mp4"));
    if out.exists() && bool_arg(&a, "overwrite") != Some(true) {
        return Err(format!("{} already exists. Ask the user whether to replace it, then call again with overwrite: true (or pick another name).", out.display()));
    }
    let frame = dir.join(format!("{name}.png"));
    let draft = str_arg(&a, "quality") == "draft";
    let job = json!({ "mode": "full", "spec": spec, "out": out, "frame": frame, "scale": if draft { 0.5 } else { 1.0 }, "work_dir": job_work_dir(&dir), "config": config_json() });
    let job_path = job_file(&dir.join(".work"), &job)?;
    if let Some(p) = &progress {
        p("Starting the renderer".into());
    }
    let jp = job_path.clone();
    let done = mv_ui::bg::blocking(move || run_python(&env, vec!["render".into(), jp.display().to_string()], progress)).await.map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&job_path);
    let done = done?;
    let shown = out.clone();
    let label = name.clone();
    ui_assistant::on_ui(move |_, cx| {
        mv_ui::toast::show(cx, mv_ui::toast::Toast::success(format!("Rendered “{label}”")).icon(mv_ui::ph::CHECK_CIRCLE).action("Show in folder", move |_, cx| cx.reveal_path(&shown)));
    })
    .await
    .ok();
    let mut notes: Vec<String> = warnings;
    notes.extend(done.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    Ok(ToolOutput::Value(json!({
        "path": out.display().to_string(),
        "preview": frame.is_file().then(|| frame.display().to_string()),
        "duration": done.get("duration"),
        "size": format!("{}x{}", done["width"].as_u64().unwrap_or(0), done["height"].as_u64().unwrap_or(0)),
        "encoder": done.get("encoder"),
        "warnings": notes,
        "note": "Play it with studio_play {\"path\": …}.",
    })))
}

async fn preview(a: Map<String, Value>) -> ToolResult {
    let progress = ui_assistant::tools::progress_handle();
    let (spec, warnings) = checked(&a).await?;
    let env = env_for_render().await?;
    let total = spec["duration"].as_f64().unwrap_or(0.);
    let dir = motion_dir().join(".preview");
    let clip = bool_arg(&a, "clip") == Some(true);
    let stamp = chrono_stamp_ms();
    let first_mid = spec["scenes"][0]["duration"].as_f64().unwrap_or(1.) / 2.;
    let at = num_arg(&a, "at").unwrap_or(first_mid).clamp(0., total.max(0.));
    let start = num_arg(&a, "start").unwrap_or(0.).clamp(0., total.max(0.));
    let (out, frame) = if clip { (Some(dir.join(format!("preview-{stamp}.mp4"))), dir.join(format!("preview-{stamp}.png"))) } else { (None, dir.join(format!("frame-{stamp}.png"))) };
    let job = json!({ "mode": if clip { "clip" } else { "frame" }, "spec": spec, "out": out, "frame": frame, "frame_at": if clip { start + PREVIEW_SECONDS.min(total - start) / 2. } else { at }, "start": start, "seconds": PREVIEW_SECONDS, "scale": 0.5, "work_dir": dir.join(format!("work-{stamp}")), "config": config_json() });
    let job_path = job_file(&dir, &job)?;
    // Old previews are dropped (keeps the folder small).
    prune(&dir, 24);
    let jp = job_path.clone();
    let done = mv_ui::bg::blocking(move || run_python(&env, vec!["render".into(), jp.display().to_string()], progress)).await.map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&job_path);
    let done = done?;
    let mut notes = warnings;
    notes.extend(done.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    Ok(ToolOutput::Value(json!({
        "ok": true,
        "frame": frame.display().to_string(),
        "clip": out.map(|p| p.display().to_string()),
        "at": if clip { start } else { at },
        "duration": total,
        "scenes": spec["scenes"].as_array().map(|s| s.iter().map(|x| json!({ "start": x["start"], "duration": x["duration"], "layers": x["layers"].as_array().map_or(0, Vec::len) })).collect::<Vec<_>>()),
        "warnings": notes,
        "note": "The spec is valid. Render it with motion_compose (same spec).",
    })))
}

fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).filter(|(_, p)| p.is_file()).collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

async fn assets(a: Map<String, Value>) -> ToolResult {
    let groups = super::load_groups().await?;
    let manga = str_arg(&a, "manga").to_string();
    if !manga.is_empty() {
        let g = super::find_manga(&groups, &manga)?.clone();
        let chapter = str_arg(&a, "chapter").to_string();
        if chapter.is_empty() {
            return Ok(ToolOutput::Value(json!({
                "manga": g.title,
                "cover": g.cover.as_ref().filter(|p| p.is_file()).map(|p| p.display().to_string()),
                "chapters": ui_assistant::chapters::compress(&g.chapters.iter().filter_map(|c| c.number).collect::<Vec<_>>()),
                "note": "Pass chapter for its pages and panels.",
            })));
        }
        let c = super::pick_chapters(&g, &[chapter])?.remove(0);
        let rows = mv_ui::bg::blocking(move || {
            page_files(&c.pages_dir)
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let panels = page_panels(&c.output_dir, i).len();
                    json!({ "page": i + 1, "panels": panels, "path": p.display().to_string() })
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| e.to_string())?;
        let (offset, limit) = ui_assistant::tools::page_args(&a, 40);
        let total = rows.len();
        let items: Vec<Value> = rows.into_iter().skip(offset).take(limit).collect();
        let mut v = ui_assistant::tools::paged(items, offset, total);
        v["manga"] = json!(g.title);
        v["use"] = json!(format!("{{\"manga\": \"{}\", \"chapter\": \"{}\", \"page\": N}} (+ \"panel\": K)", g.title, str_arg(&a, "chapter")));
        return Ok(ToolOutput::Value(v));
    }
    let music = mv_ui::bg::blocking(|| list_files(&mv_core::paths::music_library(), 40)).await.unwrap_or_default();
    let renders = mv_ui::bg::blocking(|| list_files(&motion_dir(), 20)).await.unwrap_or_default();
    let voices: Vec<Value> = mv_studio::tts::VOICES.iter().filter(|v| v.lang == "en").take(12).map(|v| json!({ "id": v.id, "name": v.label })).collect();
    Ok(ToolOutput::Value(json!({
        "manga": groups.iter().map(|g| json!({ "title": g.title, "chapters": ui_assistant::chapters::compress(&g.chapters.iter().filter_map(|c| c.number).collect::<Vec<_>>()), "cover": g.cover.is_some() })).collect::<Vec<_>>(),
        "music": music,
        "sfx": SFX,
        "voices": voices,
        "defaultVoice": mv_studio::voices::default_voice(),
        "motionRenders": renders,
        "layeredMotion": mv_studio::vfx_pack::installed(),
        "note": "Pages: motion_assets {manga, chapter}. Image sources: {\"manga\", \"chapter\", \"page\"[, \"panel\"]}, {\"cover\": title}, {\"studio\": \"relative/path\"} or an absolute path.",
    })))
}

fn list_files(dir: &Path, max: usize) -> Vec<String> {
    let mut out: Vec<(std::time::SystemTime, String)> = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_file() && is_media(&p) {
            out.push((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p.display().to_string()));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().take(max).map(|(_, p)| p).collect()
}

pub(super) fn tools() -> Vec<ToolDef> {
    let spec_param = json!({ "type": "object", "description": "The video spec (see the schema above)" });
    vec![
        ToolDef::new(
            "motion_assets",
            "What can go into a motion_compose spec: downloaded manga (chapters, covers), the music library, built-in sound effects, voices and earlier renders. With manga + chapter: that chapter's pages (path, number of panels) to use as image sources.",
            json!({ "type": "object", "properties": {
                "manga": { "type": "string", "description": "Manga title or id (optional)" },
                "chapter": { "type": "string", "description": "Chapter number (with manga): list its pages" },
                "offset": { "type": "number" }, "limit": { "type": "number" },
            }, "required": [] }),
            Risk::Read,
        )
        .phrases(|a| Phrases::new("List", "Listing", "Listed", &if str_arg(a, "chapter").is_empty() { "motion assets".to_string() } else { format!("pages of {} Ch. {}", str_arg(a, "manga"), str_arg(a, "chapter")) }))
        .run(|a| async move { assets(a).await }.boxed()),
        ToolDef::new(
            "motion_preview",
            format!("Checks a motion spec and renders a preview: one frame at `at` seconds (PNG), or with clip: true a {PREVIEW_SECONDS:.0}-second low-resolution clip from `start`. Returns the file paths, or every problem in the spec with its path so you can fix it. Same spec as motion_compose."),
            json!({ "type": "object", "properties": {
                "spec": spec_param.clone(),
                "at": { "type": "number", "description": "Seconds into the video for the frame (default: middle of the first scene)" },
                "clip": { "type": "boolean", "description": "Render a short clip instead of a frame" },
                "start": { "type": "number", "description": "Clip start in seconds" },
            }, "required": ["spec"] }),
            // It renders (Python, FFmpeg, maybe the GPU effects pack), so Ask mode asks first.
            Risk::Act,
        )
        .phrases(|_| Phrases::new("Preview", "Previewing", "Previewed", "the motion video"))
        .run(|a| async move { preview(a).await }.boxed()),
        ToolDef::new(
            "motion_compose",
            format!("Renders a custom motion-graphic video (intro, trailer, character spotlight, text animation) from a JSON spec, separate from the recap pipeline: scenes of image / text / shape / particle layers with keyframed position, scale, rotation and opacity (eased), transitions, effects, and audio (narration in the app's voices, music, sound effects). Saved to Studio's motion folder as `name`.mp4 with a preview frame; existing files need overwrite: true (ask the user first). Check specs with motion_preview first.\n{SCHEMA}"),
            json!({ "type": "object", "properties": {
                "spec": spec_param,
                "name": { "type": "string", "description": "File name (default: the spec's title)" },
                "quality": { "type": "string", "enum": ["full", "draft"], "description": "draft renders at half size, faster" },
                "overwrite": { "type": "boolean", "description": "Replace a render of the same name" },
            }, "required": ["spec"] }),
            Risk::Act,
        )
        .risk_for(|a| if bool_arg(a, "overwrite") == Some(true) { Risk::Destructive } else { Risk::Act })
        .phrases(|a| {
            let n = str_arg(a, "name");
            Phrases::new("Render", "Rendering", "Rendered", &if n.is_empty() { "the motion video".to_string() } else { format!("“{n}”") })
        })
        .run(|a| async move { compose(a).await }.boxed()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_resolve(v: &Value) -> Result<Value, String> {
        match v {
            Value::String(s) if s.starts_with("missing") => Err(format!("file not found: {s}")),
            Value::String(s) => Ok(json!({ "path": s })),
            Value::Object(o) if o.contains_key("manga") => Ok(json!({ "path": "C:/pages/002.jpg", "crop": [0.1, 0.1, 0.9, 0.5] })),
            other => Err(format!("unknown image source {other}")),
        }
    }

    fn check(spec: Value) -> Result<(Value, Vec<String>), Vec<String>> {
        validate(&spec, &ok_resolve, &|v| v == "af_heart")
    }

    fn examples() -> Vec<Value> {
        SCHEMA.lines().filter(|l| l.starts_with("Example")).filter_map(|l| l.split_once("): ").map(|(_, j)| serde_json::from_str::<Value>(j).unwrap_or_else(|e| panic!("example is not JSON: {e}\n{j}")))).collect()
    }

    #[test]
    fn the_schema_examples_are_valid_specs() {
        let ex = examples();
        assert_eq!(ex.len(), 2);
        for e in ex {
            let (n, warnings) = check(e).unwrap();
            assert!(n["duration"].as_f64().unwrap() > 0.);
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    #[test]
    fn a_good_spec_is_normalised() {
        let spec = json!({
            "format": "reels",
            "scenes": [
                { "duration": 2, "layers": [{ "type": "text", "text": "Hi", "keyframes": [{ "t": 0, "opacity": 0 }, { "t": 1, "opacity": 1, "ease": "out" }] }] },
                { "duration": 3, "transition": { "type": "fade", "duration": 0.5 }, "layers": [{ "type": "image", "src": "C:/a.png" }] },
                { "duration": 2, "transition": "slide_left", "layers": [{ "type": "particles" }] },
            ],
            "audio": { "narration": "Hello there", "sfx": [{ "name": "whoosh", "at": 1.5 }] },
        });
        let (n, _) = check(spec).unwrap();
        assert_eq!((n["width"].as_u64(), n["height"].as_u64(), n["fps"].as_f64()), (Some(1080), Some(1920), Some(30.)));
        // Transitions overlap: 2 + 3 + 2 - 0.5 - 0.5.
        assert_eq!(n["duration"], json!(6.0));
        assert_eq!(n["scenes"][1]["start"], json!(1.5));
        assert_eq!(n["scenes"][2]["start"], json!(4.0));
        assert_eq!(n["scenes"][0]["transition"], Value::Null);
        let text = &n["scenes"][0]["layers"][0];
        assert_eq!((text["x"].as_f64(), text["scale"].as_f64(), text["font"].as_str()), (Some(0.5), Some(1.0), Some("bold")));
        assert_eq!(n["scenes"][1]["layers"][0]["src"]["path"], "C:/a.png");
        assert_eq!(n["audio"]["narration"][0]["text"], "Hello there");
        assert_eq!(n["audio"]["sfx"][0]["name"], "whoosh");
    }

    #[test]
    fn bad_specs_say_what_and_where() {
        let errs = check(json!({ "scenes": [] })).unwrap_err();
        assert!(errs[0].contains("at least one scene"), "{errs:?}");
        assert!(check(json!("nope")).unwrap_err()[0].contains("JSON object"));
        let errs = check(json!({
            "fromat": "landscape",
            "scenes": [{
                "duration": 3,
                "layers": [
                    { "type": "txt", "text": "x" },
                    { "type": "text", "text": "", "color": "red", "keyframes": [{ "t": 2, "scale": 2 }, { "t": 1, "scale": 1 }, { "t": 9, "opacity": 1 }, { "t": 0.5 }] },
                    { "type": "image", "src": "missing.png", "fit": "stretch", "opacity": 3 },
                ],
                "transition": { "type": "dissolve" },
                "effects": [{ "type": "grade", "preset": "neon" }, { "strength": 1 }],
            }],
            "audio": { "sfx": [{ "name": "explosion", "at": 1 }], "narration": [{ "text": "a", "voice": "nobody" }] },
        }))
        .unwrap_err();
        let all = errs.join("\n");
        for want in [
            "spec: unknown key \"fromat\" (did you mean \"format\"?)",
            "scenes[0].layers[0].type: \"txt\" is not one of image, text, shape, particles (did you mean \"text\"?)",
            "scenes[0].layers[1]: a text layer needs non-empty \"text\"",
            "scenes[0].layers[1].color: must be a colour",
            "scenes[0].layers[1].keyframes[1].t: 1 comes before the previous keyframe",
            "scenes[0].layers[1].keyframes[2].t: 9 is out of range (0 to 3)",
            "scenes[0].layers[1].keyframes[3]: animates nothing",
            "scenes[0].layers[2].src: file not found: missing.png",
            "scenes[0].layers[2].fit: \"stretch\" is not one of",
            "scenes[0].layers[2].opacity: 3 is out of range (0 to 1)",
            "scenes[0].transition.type: \"dissolve\" is not one of",
            "scenes[0].effects[0].preset: \"neon\" is not one of",
            "scenes[0].effects[1]: needs a \"type\"",
            "audio.sfx[0].name: \"explosion\" is not a built-in sound",
            "audio.narration[0].voice: unknown voice \"nobody\"",
        ] {
            assert!(all.contains(want), "missing «{want}» in:\n{all}");
        }
    }

    #[test]
    fn limits_hold() {
        let long: Vec<Value> = (0..4).map(|_| json!({ "duration": 50 })).collect();
        assert!(check(json!({ "scenes": long })).unwrap_err().iter().any(|e| e.contains("at most 180")));
        let errs = check(json!({ "scenes": [{ "duration": 1 }, { "duration": 1, "transition": { "type": "fade", "duration": 2 } }] })).unwrap_err();
        assert!(errs[0].contains("longer than the scenes it joins allow"), "{errs:?}");
        let errs = check(json!({ "scenes": [{ "duration": 2, "layers": [{ "type": "shape", "start": 1.5, "end": 1 }] }] })).unwrap_err();
        assert!(errs[0].contains("\"end\" (1) must be after \"start\" (1.5)"), "{errs:?}");
    }

    #[test]
    fn spec_args_and_names() {
        let mut a = Map::new();
        a.insert("spec".into(), json!("{scenes: [{duration: 2,}]}"));
        assert_eq!(spec_arg(&a).unwrap()["scenes"][0]["duration"], 2);
        let mut b = Map::new();
        b.insert("scenes".into(), json!([]));
        b.insert("name".into(), json!("x"));
        let s = spec_arg(&b).unwrap();
        assert!(s.get("scenes").is_some() && s.get("name").is_none(), "a spec given inline without the wrapper");
        assert!(spec_arg(&Map::new()).is_err());
        assert_eq!(output_name("My: intro?", ""), "My intro");
        assert_eq!(output_name("", "Solo trailer.mp4"), "Solo trailer");
        assert!(output_name("", "").starts_with("motion-"));
        // Dot-only / space-only names would name the motion folder itself.
        for bad in ["..", ".", " . ", "...", "  ", ".mp4", "...mp4", "CON", "nul.mp4", "com1"] {
            assert!(output_name(bad, "").starts_with("motion-"), "{bad:?} → {}", output_name(bad, ""));
        }
        assert_eq!(output_name("..intro..", ""), "intro");
        assert_eq!(output_name("Vol. 2 intro", ""), "Vol. 2 intro");
    }

    #[test]
    fn every_job_gets_its_own_work_folder_inside_work() {
        let dir = Path::new("C:/studio/motion");
        let a = job_work_dir(dir);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = job_work_dir(dir);
        assert_ne!(a, b);
        for w in [a, b] {
            assert_eq!(w.parent(), Some(dir.join(".work").as_path()));
            assert!(w.file_name().unwrap().to_str().unwrap().starts_with("job-"));
        }
    }

    #[test]
    fn paths_stay_local_and_inside_their_root() {
        for bad in [r"\\host\share\a.png", "//host/share/a.png", r"\\?\C:\a.png", r"\\.\pipe\x"] {
            assert!(is_network_path(bad), "{bad}");
            assert!(local_path(bad).unwrap_err().contains("network"), "{bad}");
        }
        assert!(local_path(r"C:\a\..\b.png").unwrap_err().contains(".."));
        assert_eq!(local_path(r"C:\a\b.png").unwrap(), PathBuf::from(r"C:\a\b.png"));
        let root = std::env::temp_dir().join(format!("mv-motion-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("a.png"), b"x").unwrap();
        assert_eq!(inside(&root, "sub/a.png").unwrap(), root.join("sub").join("a.png"));
        assert_eq!(inside(&root, "/sub/a.png").unwrap(), root.join("sub").join("a.png"), "a leading slash means the root");
        for bad in ["../..", "sub/../../x.png", "..", r"C:\Windows\x.png", r"\\host\share\x.png", "", "  "] {
            assert!(inside(&root, bad).is_err(), "{bad:?} escapes");
        }
        // The resolver refuses the same escapes.
        assert!(resolve_src(&json!({ "studio": "../.." }), &[]).is_err());
        assert!(resolve_src(&json!(r"\\host\share\a.png"), &[]).unwrap_err().contains("network"));
        assert!(resolve_src(&json!({ "path": "//host/share/a.png" }), &[]).unwrap_err().contains("network"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn panels_come_from_read_json() {
        let dir = std::env::temp_dir().join(format!("mv-motion-read-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("studio")).unwrap();
        std::fs::write(dir.join("studio").join("read.json"), r#"{"pages":[{"index":0,"panels":[{"bbox":[0.1,0.2,0.5,0.6]}]},{"index":1,"panels":[]}]}"#).unwrap();
        let p = page_panels(&dir, 0);
        assert_eq!(panel_box(&p[0]), Some(vec![0.1, 0.2, 0.5, 0.6]));
        assert!(page_panels(&dir, 1).is_empty() && page_panels(&dir, 7).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_studio_tool_has_a_valid_schema_and_a_group() {
        super::super::register();
        let specs = ui_assistant::tools::tool_specs();
        for want in ["studio_run_steps", "studio_play", "studio_thumbnails", "studio_thumbnail_make", "motion_compose", "motion_preview", "motion_assets"] {
            assert!(specs.iter().any(|s| s.name == want), "{want} is registered");
            assert_eq!(ui_assistant::tools::group_of(want), ui_assistant::tools::Group::Studio, "{want}");
        }
        let mut names = std::collections::HashSet::new();
        for s in &specs {
            assert!(names.insert(s.name.clone()), "duplicate {}", s.name);
            assert_eq!(s.parameters["type"], "object", "{}", s.name);
            for r in s.parameters["required"].as_array().unwrap() {
                assert!(s.parameters["properties"].get(r.as_str().unwrap()).is_some(), "{}: {r}", s.name);
            }
        }
        // Spending Magnific credits always asks, whatever the arguments.
        assert_eq!(ui_assistant::tools::risk_for("studio_thumbnail_make", &Map::new()), Some(Risk::Destructive));
        // find_tools finds the creative tool from plain words.
        let hits = ui_assistant::tools::rank_tools("make an animated intro video", &specs);
        assert!(hits.iter().take(3).any(|(n, _)| n == "motion_compose"), "{hits:?}");
    }

    #[test]
    fn tools_are_well_formed() {
        let t = tools();
        assert_eq!(t.iter().map(|t| t.name).collect::<Vec<_>>(), ["motion_assets", "motion_preview", "motion_compose"]);
        let compose = t.iter().find(|t| t.name == "motion_compose").unwrap();
        assert!(compose.description.contains("Example 1") && compose.description.contains("Example 2"));
        let mut a = Map::new();
        assert_eq!(compose.risk_for.as_ref().unwrap()(&a), Risk::Act);
        a.insert("overwrite".into(), json!(true));
        assert_eq!(compose.risk_for.as_ref().unwrap()(&a), Risk::Destructive, "overwriting asks first");
        // Previewing renders, so Ask mode asks; listing assets only reads.
        assert_eq!(t.iter().find(|t| t.name == "motion_preview").unwrap().risk, Risk::Act);
        assert_eq!(t.iter().find(|t| t.name == "motion_assets").unwrap().risk, Risk::Read);
    }
}
