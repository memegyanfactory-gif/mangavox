"""
Video Generator for MangaVox
Combines manga panels/pages + TTS narration into a narrated recap video: every panel sits
centred (with a soft shadow), fully visible, on a still, zoomed-in, blurred copy of itself; only
the panel moves, with a slow eased sub-pixel Ken Burns, and it crossfades into the next clip
exactly when that clip's narration starts.
Narration is assembled sample-accurately, mixed with the score and loudness-normalised
in two passes.
Requires: ffmpeg, Pillow
"""
import os
import sys
import json
import subprocess
import shutil
import wave
import math
import re
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

try:
    import layered_plan
except ImportError:  # imported as python.video_generator (the tests)
    from . import layered_plan

FPS = 30
AUDIO_RATE = 48000
# Panel inset from the frame edge (fraction of the frame) when the blurred fill is used.
FG_MARGIN = 0.05
# The thinner margin of the "fill" framing (single panels drawn larger, still whole).
FILL_MARGIN = 0.015
# Ken Burns: push-in / pull-out amount and the fixed zoom used while panning.
FG_PUSH = 0.05
FG_PAN_ZOOM = 1.04
# The still blurred background is the panel zoomed in this much past "cover".
BG_ZOOM = 1.15
# Transition into the next beat of the same page, and the rotation used on a page change.
SAME_PAGE_TRANSITION = ("fade", 0.35)
PAGE_TRANSITIONS = ["smoothleft", "fade", "smoothup", "fadeblack"]
# Loudness target for the final mix (LUFS / dBTP).
LOUD_I = -16.0
LOUD_TP = -1.5

FG_MOTIONS = {
    "tall": ["push", "pan_down", "pull", "pan_up"],
    "wide": ["push", "pan_right", "pull", "pan_left"],
    "even": ["push", "pull", "pan_right", "push", "pull", "pan_down"],
}
# Where the visible window sits in the slack left by the zoom (0..1 per axis; "e" is the
# eased 0..1 progress) for each panel move.
FG_WINDOW = {
    "push": ("0.5", "0.5"), "pull": ("0.5", "0.5"), "still": ("0.5", "0.5"),
    "pan_down": ("0.5", "{e}"), "pan_up": ("0.5", "(1-{e})"),
    "pan_right": ("{e}", "0.5"), "pan_left": ("(1-{e})", "0.5"),
}
# Bumped whenever the look of a rendered clip changes, so cached clips are not reused.
CLIP_VERSION = 7

# Looks (Studio → Video → Look): how panels are laid out and move, which transitions turn the
# page, and what the finished picture gets. "duo" puts tall panels side by side like the big
# manhwa recap channels: each new one slides into the older one's place while the other stays.
# "punch" snaps action beats in with a zoom, a shake and a white flash. "ramp" times the moves
# to the beat (a quick snap on a short beat, a slow drift on a long one). The effect switches
# are listed in LOOK_EFFECTS; "color" is one of COLOR_PRESETS. "mood" lets the story drive the
# edit (plan_moods: a grade, atmosphere particles and transitions per scene, punches only on
# intense beats); "edge_glow" runs a thin line of light along the frame's borders.
_FX_OFF = {"leaks": False, "chroma": False, "parallax": False, "particles": False, "letterbox": False, "mood": False, "edge_glow": False}
LOOKS = {
    "clean": {
        "duo": False, "punch": False, "push": FG_PUSH, "transitions": PAGE_TRANSITIONS, "ramp": False,
        "grade": False, "vignette": False, "grain": False, "glow": False, "impact": False,
        **_FX_OFF, "color": "none",
    },
    "cinematic": {
        "duo": False, "punch": False, "push": 0.06, "transitions": ["fade", "fadeblack", "smoothleft", "fade"], "ramp": False,
        "grade": True, "vignette": True, "grain": True, "glow": True, "impact": False,
        **_FX_OFF, "leaks": True, "letterbox": True, "mood": True, "color": "warm",
    },
    "recap": {
        "duo": True, "punch": False, "push": FG_PUSH, "transitions": ["smoothleft", "smoothup", "fade", "smoothright"], "ramp": False,
        "grade": True, "vignette": True, "grain": False, "glow": False, "impact": False,
        **_FX_OFF, "color": "none",
    },
    "dynamic": {
        "duo": True, "punch": True, "push": 0.08, "ramp": True,
        # Smooth turns with the odd accent (a burn, a zoom blur) rather than a rotation of them.
        "transitions": ["smoothleft", "fade", "smoothup", "zoomblur", "smoothright", "fade", "slidedown", "burn"],
        "grade": True, "vignette": True, "grain": False, "glow": True, "impact": True,
        **_FX_OFF, "leaks": True, "chroma": True, "parallax": True, "mood": True, "edge_glow": True, "color": "none",
    },
}
DEFAULT_LOOK = "dynamic"
# Effects the user can switch on or off over a look ("chapter_card": on in every look unless switched off).
LOOK_EFFECTS = ("grade", "vignette", "grain", "glow", "impact", "leaks", "chroma", "parallax", "particles", "letterbox", "mood", "edge_glow", "chapter_card")
# Colour presets (one lookup table, merged with the grade): Y / U / V expressions of `val`.
COLOR_PRESETS = {
    "none": None,
    # Golden highlights, a touch less blue.
    "warm": ("clip(val*1.02+2,0,255)", "clip(124+(val-128)*1.04,0,255)", "clip(134+(val-128)*1.06,0,255)"),
    # Steel blue, slightly cooler shadows.
    "cool": ("clip((val-16)*1.03+14,0,255)", "clip(135+(val-128)*1.05,0,255)", "clip(123+(val-128)*0.98,0,255)"),
    # Black and white with an S-curve: inked blacks, clean paper whites.
    "noir": ("clip(0.35*val+0.65*(16+219*(0.5-0.5*cos(PI*clip((val-16)/219,0,1)))),0,255)", "128", "128"),
    # Punchier colour and contrast.
    "vivid": ("clip((val-110)*1.08+110,0,255)", "clip(128+(val-128)*1.35,0,255)", "clip(128+(val-128)*1.35,0,255)"),
}
# Transitions drawn by the clips themselves over a plain dissolve: a film burn (a hot flare
# that swells over the cut) and a zoom blur (the old page rushes in and blurs, the new one
# lands out of it).
CLIP_TRANSITIONS = ("burn", "zoomblur", "particles")
# Transitions the clips help draw, and the xfade under them: the two above, a glitch (pixelate
# with an RGB tear and noise on both sides) and whips (a fast slide, both sides motion-blurred).
CLIP_DRAWN = {
    "burn": "fade", "zoomblur": "fade", "glitch": "pixelize", "particles": "pixelize",
    "whipleft": "slideleft", "whipright": "slideright", "whipup": "slideup", "whipdown": "slidedown",
}
# The Video editor's per-clip transitions: name -> (transition as planned, default seconds).
EDIT_TRANSITIONS = {
    "fade": ("fade", 0.5), "dissolve": ("dissolve", 0.5), "black": ("fadeblack", 0.7), "flash": ("fadewhite", 0.3),
    "whip_left": ("whipleft", 0.22), "whip_right": ("whipright", 0.22), "whip_up": ("whipup", 0.22), "whip_down": ("whipdown", 0.22),
    "slide_left": ("slideleft", 0.45), "slide_right": ("slideright", 0.45), "slide_up": ("slideup", 0.45), "slide_down": ("slidedown", 0.45),
    "wipe": ("wipeleft", 0.45), "circle": ("circleopen", 0.5), "zoom": ("zoomblur", 0.35), "burn": ("burn", 0.9),
    "glitch": ("glitch", 0.3),
}
# Effect strengths the editor can scale (multipliers 0..2 over the look's own).
INTENSITIES = ("motion", "grain", "vignette", "parallax", "chroma")
# Letterbox: the picture area of a landscape frame with bars (width / height).
LETTERBOX_ASPECT = 2.39
# Parallax: how far the background drifts (share of the frame width) while the panel moves.
PARALLAX = 0.03
# Light leak, film burn and zoom blur timing (seconds). A leak blooms as a new page lands; a
# burn peaks in the middle of the cut; a zoom blur spans the cut.
LEAK_SEC = 1.1
LEAK_GAP = 10.0
BURN_SEC = 0.9
BURN_PEAK = 0.32
# A big scene change bursts into particles: sparks and pixel shards in the new scene's colour fly
# out from the middle while the old page breaks into blocks that settle into the new one (xfade's
# pixelize, under the burst; peaking BURST_PEAK in), at
# most one every SCENE_GAP seconds; the join takes SCENE_TRANSITION of the base length.
BURST_SEC = 1.0
BURST_PEAK = 0.4
SCENE_GAP = 30.0
SCENE_TRANSITION = 1.6
# Scenes that burst as they begin; an action scene keeps its whips, a horror or sad one its slow
# fades (a burst would break them).
BURST_MOODS = ("neutral", "calm", "mystery", "reveal", "romance", "comedy")
ZOOM_BLUR = 0.14
# Particle loop length (seconds): one seamless loop, made once per render.
PARTICLE_SEC = 4.0
# Duo layout: panels narrower than this (w/h) pair up; frame edge and middle gap (of the
# width) and the room kept above and below (of the height). A panel is always drawn whole in
# its half (fitted, never wider than the frame allows): a strip too tall to fill its half shows
# narrower over the blurred fill rather than being cut and panned.
DUO_MAX_ASPECT = 0.9
DUO_OUTER = 0.04
DUO_GAP = 0.025
DUO_OUTER_Y = 0.03
# How the new panel arrives (seconds, and how far it slides as a share of the frame width)
# and how long the background takes to turn into the new panel's.
DUO_SLIDE = 0.45
DUO_SLIDE_DIST = 0.06
DUO_BG_FADE = 0.5
# Action beats: at most one in this many clips gets the punch, which lasts this long.
IMPACT_GAP = 4
IMPACT_SEC = 0.4
IMPACT_WORDS = re.compile(
    r"\b(explo\w*|blast\w*|strik\w*|slam\w*|smash\w*|punch\w*|crash\w*|roar\w*|scream\w*|"
    r"shatter\w*|clash\w*|slash\w*|attack\w*|erupt\w*|thunder\w*|kill\w*|destroy\w*|rage|charg\w*)\b",
    re.I,
)


def resolve_look(value):
    """The look a render uses: a preset name, or {"preset": ..., <effect>: bool, ...} with
    the user's effect switches over the preset's own. Unknown presets fall back to the default."""
    if isinstance(value, str):
        value = {"preset": value}
    value = value if isinstance(value, dict) else {}
    name = value.get("preset") if value.get("preset") in LOOKS else DEFAULT_LOOK
    look = dict(LOOKS[name], preset=name)
    for k in LOOK_EFFECTS:
        if isinstance(value.get(k), bool):
            look[k] = value[k]
    if value.get("color") in COLOR_PRESETS:
        look["color"] = value["color"]
    # Layered motion (the optional pack, see layered_motion.py): "off", "key" moments or "all".
    look["layered"] = value.get("layered") if value.get("layered") in layered_plan.MODES else layered_plan.DEFAULT_MODE
    # The Studio's framing choices (absent: the look's own): tall panels side by side or not,
    # "fill" draws single panels larger (a thinner margin, still whole: never cut), and the
    # camera motion amount (a multiplier on the look's moves, under the editor's own).
    if isinstance(value.get("duo"), bool):
        look["duo"] = value["duo"]
    look["framing"] = "fill" if value.get("framing") == "fill" else "fit"
    look["motion_amount"] = _num(value.get("motion"), 1.0, 0.0, 2.0)
    return look


def apply_intensity(look, intensity):
    """`look` with the editor's effect strengths (INTENSITIES -> multiplier 0..2) applied:
    "motion" scales the auto moves (push and pan zoom), the rest are kept as "<name>_k" for the
    grain, vignette, parallax drift and RGB split to scale by."""
    look = dict(look)
    for k in INTENSITIES:
        look[f"{k}_k"] = 1.0
    if isinstance(intensity, dict):
        for k in INTENSITIES:
            if k in intensity:
                look[f"{k}_k"] = _num(intensity.get(k), 1.0, 0.0, 2.0)
    look["motion_k"] = round(look["motion_k"] * look.get("motion_amount", 1.0), 4)
    look["push"] = round(look.get("push", FG_PUSH) * look["motion_k"], 4)
    return look


def letterbox_bar(w, h):
    """Height of each letterbox bar (even pixels) for a w x h frame; 0 for an upright frame."""
    if w <= h:
        return 0
    return max(0, round((h - w / LETTERBOX_ASPECT) / 4) * 2)


def color_lut(grade, color):
    """lutyuv expressions (y, u, v) for the grade and a colour preset applied after it, or None
    when neither is on. Both go through one table, so a preset costs nothing over the grade."""
    stages = []
    if grade:
        stages.append((
            "clip((val-100)*1.08+100,0,255)",
            "clip(126+(val-128)*1.18,0,255)",
            "clip(130+(val-128)*1.18,0,255)",
        ))
    if COLOR_PRESETS.get(color):
        stages.append(COLOR_PRESETS[color])
    if not stages:
        return None
    out = ["val", "val", "val"]
    for stage in stages:
        out = [s.replace("val", f"({o})") if o != "val" else s for s, o in zip(stage, out)]
    return tuple(out)


def is_impact(text):
    """An action beat: an exclamation or a word of violence or force in its narration."""
    return bool(text) and ("!" in text or IMPACT_WORDS.search(text) is not None)


# ── Story mood: each page's scene (read.json) drives the grade, atmosphere and transitions ──

# A scene kind that says the mood by itself (read.json scene.kind).
KIND_MOODS = {
    "fight": "action", "action": "action", "horror": "horror", "comedy": "comedy",
    "emotional": "sad", "flashback": "sad", "calm": "calm", "reveal": "reveal",
}
# Words of scene.mood for the kinds that do not (dialogue, other, ...), checked in this order.
MOOD_WORDS = (
    ("horror", re.compile(r"\b(eerie|creep\w*|terrif\w*|dread\w*|grotesque|sinister|unsettl\w*|horrif\w*|macabre|ominous|menac\w*|haunt\w*)\b", re.I)),
    ("action", re.compile(r"\b(explosive|violent|brutal|savage|feroc\w*|furious|chaotic|rage|frantic|feral|battle|intense|heroic)\b", re.I)),
    ("sad", re.compile(r"\b(sad\w*|melanchol\w*|somber|sombre|wistful|tear\w*|grie\w*|bitter|remorse\w*|lonely|mourn\w*|brooding|reflective|regret\w*)\b", re.I)),
    ("romance", re.compile(r"\b(romantic|romance|tender|lov\w*|heartfelt|affection\w*|sweet|intimate|warm|kiss\w*|confession)\b", re.I)),
    ("comedy", re.compile(r"\b(playful|comedic|comic\w*|absurd|cheerful|gleeful|silly|cute|awkward|wry|joyful|carefree|funny|goofy)\b", re.I)),
    ("mystery", re.compile(r"\b(tense|suspicious|myster\w*|uneasy|quiet|curious|cryptic)\b", re.I)),
)
ROMANCE_WORDS = MOOD_WORDS[3][1]
# Atmosphere particles a scene's tags call for.
SNOW_TAGS = re.compile(r"\b(snow\w*|winter|blizzard|frost\w*|frozen|icy|freez\w*)\b", re.I)
RAIN_TAGS = re.compile(r"\b(rain\w*|storm\w*|downpour|drizzl\w*|monsoon)\b", re.I)
FIRE_TAGS = re.compile(r"\b(fire|fires|flame\w*|flaming|ember\w*|burn\w*|explo\w*|blaz\w*|lava|inferno|spark\w*)\b", re.I)
GRIEF_TAGS = re.compile(r"\b(funeral|grave\w*|tears|grief|loss|death|farewell|mourning)\b", re.I)
# Tags of a hit (with an intense scene, they make a beat a punch even without a loud line).
IMPACT_TAGS = re.compile(
    r"\b(explo\w*|impact|punch\w*|clash\w*|strik\w*|slash\w*|attack\w*|ambush|blast\w*|smash\w*|first blow|surprise attack|kick\w*)\b", re.I,
)
# Smoothing: a beat's mood is the weighted majority of this many beats either side (weights
# 3, 2, 1 by distance), so a single odd beat never flips the grade; intensity is averaged over
# MOOD_INT_RADIUS beats either side.
MOOD_RADIUS = 2
MOOD_INT_RADIUS = 1
# Punch (zoom + shake + flash) rules with the mood on: only a beat whose page is at least this
# intense, at least IMPACT_MIN_SEC after the last punch (and IMPACT_GAP clips); in the first
# IMPACT_EARLY_WINDOW seconds IMPACT_EARLY_SEC (and one clip fewer) is enough.
PUNCH_INTENSITY = 7
IMPACT_MIN_SEC = 20.0
IMPACT_EARLY_WINDOW = 30.0
IMPACT_EARLY_SEC = 10.0
# Transitions by mood: the pool a page turn into a clip of that mood rotates through, and the
# share of the look's transition length it lasts (calm moods dissolve slowly, action whips).
MOOD_TRANSITIONS = {
    "action": (("whipleft", "zoomblur", "whipright", "whipup"), 0.5),
    "horror": (("fadeblack", "fadeblack", "dissolve"), 1.6),
    "sad": (("dissolve", "fade", "smoothleft"), 1.6),
    "romance": (("dissolve", "fade", "smoothright"), 1.6),
    "calm": (("dissolve", "smoothleft", "fade", "smoothup"), 1.4),
    "mystery": (("fade", "fadeblack", "dissolve"), 1.2),
    "comedy": (("smoothleft", "smoothup", "smoothright", "slideleft"), 0.8),
    "reveal": (("fade", "smoothup"), 1.0),
    "neutral": (("smoothleft", "fade", "smoothup", "smoothright"), 1.0),
}
# Accents (a flash, a burn, a glitch, a circle) are rare: entering a reveal, a peak of action (a
# page this intense) or a big comic beat, at most one every ACCENT_GAP seconds.
ACCENTS = {"reveal": (("fadewhite", 0.6), ("pixelize", 1.4), ("burn", 1.0)), "action": (("glitch", 0.6),), "comedy": (("circleopen", 1.0),),
           "mystery": (("pixelize", 1.4),)}
ACCENT_INTENSITY = 8
ACCENT_GAP = 25.0
# Grade per mood, baked into each clip's stills (free per frame): (saturation, contrast,
# brightness, shadow tint RGB, highlight tint RGB), tints as shares of full scale. Action:
# punchier with teal shadows and orange highlights; horror: cool, drained, a little green;
# comedy: warm and bright; sad: muted, a little cool; romance: warm and soft.
MOOD_GRADES = {
    "action": (1.08, 1.10, 0.0, (-0.035, 0.005, 0.035), (0.035, 0.01, -0.035)),
    "horror": (0.70, 1.04, -0.03, (-0.02, 0.015, 0.02), (-0.02, 0.012, 0.0)),
    "comedy": (1.10, 1.02, 0.03, (0.01, 0.005, -0.01), (0.03, 0.015, -0.02)),
    "sad": (0.78, 0.96, -0.01, (-0.01, 0.0, 0.02), (-0.015, 0.0, 0.015)),
    "romance": (1.02, 0.95, 0.02, (0.02, 0.0, 0.005), (0.03, 0.01, -0.01)),
    "mystery": (0.88, 1.03, -0.01, (-0.01, 0.0, 0.02), (0.0, 0.0, 0.01)),
    "reveal": (1.0, 1.08, 0.0, (-0.01, 0.0, 0.02), (0.01, 0.005, 0.0)),
    "calm": (0.95, 0.98, 0.01, (0.0, 0.0, 0.005), (0.01, 0.005, 0.0)),
}
# Edge glow colour per mood (the halo and the travelling lights; the core line stays near white).
EDGE_COLORS = {
    "action": (255, 122, 52), "horror": (96, 226, 170), "comedy": (255, 208, 84), "sad": (112, 162, 255),
    "romance": (255, 124, 190), "mystery": (172, 126, 255), "reveal": (255, 214, 140), "calm": (124, 218, 255),
    "neutral": (255, 188, 108),
}


def load_scenes(output_dir):
    """{page index: scene {kind, mood, intensity, tags}} from <output_dir>/studio/read.json
    (the chapter's AI reading), or {} when there is none."""
    try:
        with open(os.path.join(output_dir, "studio", "read.json"), "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    out = {}
    for i, p in enumerate(data.get("pages") or [] if isinstance(data, dict) else []):
        if not isinstance(p, dict) or not isinstance(p.get("scene"), dict):
            continue
        idx = p.get("index", i)
        if isinstance(idx, int):
            out[idx] = p["scene"]
    return out


def scene_mood(scene):
    """The mood (a MOOD_TRANSITIONS key) of a read.json scene."""
    if not isinstance(scene, dict):
        return "neutral"
    kind = str(scene.get("kind") or "").strip().lower()
    words = str(scene.get("mood") or "")
    tags = " ".join(str(t) for t in scene.get("tags") or [] if isinstance(t, (str, int, float)))
    if kind == "emotional" and (ROMANCE_WORDS.search(words) or ROMANCE_WORDS.search(tags)):
        return "romance"
    if kind in KIND_MOODS:
        return KIND_MOODS[kind]
    for name, rx in MOOD_WORDS:
        if rx.search(words):
            return name
    return "romance" if ROMANCE_WORDS.search(tags) else "neutral"


def smooth_moods(values, radius=MOOD_RADIUS):
    """Each value replaced by the weighted majority of its neighbourhood (weight radius + 1 -
    distance), so a lone beat between two others takes theirs while a run of two holds. A tie
    keeps the beat's own value, else the one before it."""
    out = []
    n = len(values)
    for k in range(n):
        score = {}
        for j in range(max(0, k - radius), min(n, k + radius + 1)):
            score[values[j]] = score.get(values[j], 0) + radius + 1 - abs(j - k)
        best = max(score.values())
        tied = [v for v, s in score.items() if s == best]
        if values[k] in tied:
            out.append(values[k])
        elif out and out[-1] in tied:
            out.append(out[-1])
        else:
            out.append(tied[0])
    return out


# Atmosphere is a full-frame layer (it costs every frame it shows on), so it is kept for the
# scenes that call for it: embers for fire or a fight at least this intense (smoothed), dust only
# for quiet scenes at most DUST_INTENSITY.
EMBER_INTENSITY = 7.5
DUST_INTENSITY = 6


def particle_kind(mood, intensity, tags):
    """The atmosphere a scene gets: "snow" / "rain" for its weather tags, "embers" for fire or
    an intense fight, "rain" for grief, floating "dust" for a quiet mystery, calm or romance
    scene; None for the rest."""
    tags = " ".join(str(t) for t in tags or [])
    if SNOW_TAGS.search(tags):
        return "snow"
    if RAIN_TAGS.search(tags):
        return "rain"
    if FIRE_TAGS.search(tags) or (mood == "action" and intensity >= EMBER_INTENSITY):
        return "embers"
    if mood == "sad" and GRIEF_TAGS.search(tags):
        return "rain"
    if mood in ("mystery", "calm", "romance") and intensity <= DUST_INTENSITY:
        return "dust"
    return None


def mood_grade(mood, intensity):
    """The grade of MOOD_GRADES for `mood`, scaled by how intense the scene is (60% at 0, all
    of it from 10), rounded so it names the clip's stills stably; None for no grade."""
    g = MOOD_GRADES.get(mood)
    if not g:
        return None
    s = min(1.0, max(0.6, 0.6 + 0.04 * (intensity or 0)))
    sat, con, bri, sh, hi = g
    lerp = lambda v, one=0.0: round(one + (v - one) * s, 3)
    return (lerp(sat, 1.0), lerp(con, 1.0), lerp(bri), tuple(lerp(v) for v in sh), tuple(lerp(v) for v in hi))


def grade_lut(grade):
    """An RGB point table (768 entries, for PIL's Image.point) of a mood grade's contrast,
    brightness and shadow / highlight tints."""
    _, con, bri, sh, hi = grade
    lut = []
    for c in range(3):
        for v in range(256):
            x = v / 255
            y = (x - 0.5) * con + 0.5 + bri + sh[c] * (1 - x) ** 2 + hi[c] * x * x
            lut.append(max(0, min(255, round(y * 255))))
    return lut


def apply_grade(img, grade):
    """`img` (RGB) through a mood grade (mood_grade's), or as it is for None."""
    if not grade:
        return img
    from PIL import ImageEnhance
    if abs(grade[0] - 1.0) > 1e-3:
        img = ImageEnhance.Color(img).enhance(grade[0])
    return img.point(grade_lut(grade))


def plan_moods(items, scenes):
    """The mood plan of a chapter: one entry per render item (beat), or all None without
    scenes. Each beat takes its page's scene (a page read.json has nothing for takes the
    nearest beat's that does); moods are smoothed over MOOD_RADIUS beats and intensity over
    MOOD_INT_RADIUS, and from them come:
    {"mood", "kind", "intensity" (smoothed), "raw" (the page's own), "tags",
     "grade" (mood_grade), "particles" (particle_kind, smoothed like the mood),
     "punch" (an intense enough page and a loud line or a hit in its tags or a fight at 8+),
     "edge" (the edge glow's colour)}."""
    if not scenes or not items:
        return [None] * len(items)
    raw = [scenes.get(it.get("page")) if isinstance(it, dict) else None for it in items]
    if not any(raw):
        return [None] * len(items)
    known = [k for k, s in enumerate(raw) if s]
    # Pages without a scene borrow the nearest beat's that has one.
    for k in range(len(raw)):
        if not raw[k]:
            raw[k] = raw[min(known, key=lambda j: (abs(j - k), j))]
    moods = smooth_moods([scene_mood(s) for s in raw])
    ints = [_num(s.get("intensity"), 5.0, 0.0, 10.0) for s in raw]
    smooth_i = []
    for k in range(len(ints)):
        part = ints[max(0, k - MOOD_INT_RADIUS): k + MOOD_INT_RADIUS + 1]
        smooth_i.append(round(sum(part) / len(part), 2))
    tags = [[str(t) for t in s.get("tags") or [] if isinstance(t, (str, int, float))] for s in raw]
    parts = smooth_moods([particle_kind(m, i, t) for m, i, t in zip(moods, smooth_i, tags)])
    out = []
    for k, it in enumerate(items):
        kind = str(raw[k].get("kind") or "").strip().lower()
        text = it.get("text", "") if isinstance(it, dict) else ""
        hit = is_impact(text) or bool(IMPACT_TAGS.search(" ".join(tags[k]))) or (kind in ("fight", "action") and ints[k] >= 8)
        out.append({
            "mood": moods[k], "kind": kind, "intensity": smooth_i[k], "raw": ints[k], "tags": tags[k],
            "grade": mood_grade(moods[k], smooth_i[k]), "particles": parts[k],
            "punch": ints[k] >= PUNCH_INTENSITY and hit, "edge": EDGE_COLORS.get(moods[k], EDGE_COLORS["neutral"]),
        })
    return out


def punch_ok(t, n, last_t, last_n):
    """May clip `n` starting at `t` seconds punch, the last punch being clip `last_n` at
    `last_t`? IMPACT_MIN_SEC and IMPACT_GAP clips apart; IMPACT_EARLY_SEC and a clip fewer in
    the first IMPACT_EARLY_WINDOW seconds, where the edit is busiest."""
    early = t < IMPACT_EARLY_WINDOW
    return n - last_n >= (IMPACT_GAP - 1 if early else IMPACT_GAP) and t - last_t >= (IMPACT_EARLY_SEC if early else IMPACT_MIN_SEC)


def plan_impacts(starts, candidates, overrides=None):
    """Which clips punch: candidates[k] wants it, punch_ok spaces them. overrides[k] (the
    editor's: True for its shake, False for any other move it picked, None for the look's)
    wins either way; an override punch counts for the spacing too."""
    overrides = overrides or [None] * len(starts)
    out = []
    last_t, last_n = -1e9, -IMPACT_GAP
    for k, t in enumerate(starts):
        hit = overrides[k] if overrides[k] is not None else bool(candidates[k]) and punch_ok(t, k, last_t, last_n)
        if hit:
            last_t, last_n = t, k
        out.append(bool(hit))
    return out


def mood_transition(mood, intensity, turn, base, accent=False):
    """(transition, seconds) for a page turn into a clip of `mood` (MOOD_TRANSITIONS, the
    `turn`th of its pool); with `accent` allowed, an intense enough reveal, action or comic beat
    gets one of its ACCENTS instead."""
    if accent and mood in ACCENTS and (mood == "reveal" or (intensity or 0) >= ACCENT_INTENSITY):
        name, k = ACCENTS[mood][turn % len(ACCENTS[mood])]
        return name, round(base * k, 3)
    names, k = MOOD_TRANSITIONS.get(mood, MOOD_TRANSITIONS["neutral"])
    return names[turn % len(names)], round(base * k, 3)


def edge_segments(colors, t0=0.0, t1=float("inf")):
    """The edge glow's colour over time as [(start, end, rgb)] covering t0..t1: `colors` is
    [(clip start seconds, rgb)] (each clip's mood colour), runs of one colour merged; the glow
    turns with the clip it belongs to."""
    runs = []
    for t, rgb in colors:
        rgb = tuple(rgb)
        if not runs or rgb != runs[-1][1]:
            runs.append((t, rgb))
    out = []
    for i, (t, rgb) in enumerate(runs):
        end = runs[i + 1][0] if i + 1 < len(runs) else float("inf")
        a, b = max(t, t0) if i else t0, min(end, t1)
        if b > a:
            out.append((a, b, rgb))
    return out


def duo_geometry(iw, ih, w, h, slot):
    """Where a panel of iw x ih sits in a w x h frame's left (slot 0) or right (slot 1) half:
    drawn whole at `size`, `vis` rows visible from `pos` (all of them: `slack`, the rows a panel
    taller than the frame would pan by, is always 0 so nothing narrated is ever cut)."""
    outer = round(w * DUO_OUTER)
    slot_w = (w - 2 * outer - round(w * DUO_GAP)) // 2
    avail = h - 2 * round(h * DUO_OUTER_Y)
    dw = max(2, min(slot_w, round(iw * avail / ih)) // 2 * 2)
    dh = max(2, min(avail, round(ih * dw / iw)) // 2 * 2)
    vis = min(dh, h)
    cx = outer + slot_w / 2 if slot == 0 else w - outer - slot_w / 2
    return {"size": (dw, dh), "vis": vis, "pos": (int(cx - dw / 2) // 2 * 2, (h - vis) // 4 * 2), "slack": dh - vis}


def vignette_angle(look):
    """The vignette's angle for `look` (its strength scaled by the editor), or None when off."""
    vk = look.get("vignette_k", 1.0)
    if not look.get("vignette") or vk <= 0:
        return None
    return math.pi / 6 if vk == 1.0 else round(min(1.4, math.pi / 6 * vk), 4)


def make_vignette_mask(w, h, angle, work_dir):
    """The vignette as one still (yuva420p, FFV1): luma 0 under an alpha of 1 - the vignette
    filter's own factor (cos^4 of the angle times the distance from the centre over the
    half-diagonal), so laying it over a frame multiplies the luma and pulls the chroma to grey
    exactly as `vignette` does, at a fraction of the cost (the filter works every pixel out in
    floating point on every frame, single-threaded). Made once per size and angle, cached."""
    root = fx_cache_dir() or work_dir
    path = os.path.join(root, f"vignette_{w}x{h}_{angle:.4f}_v{FX_VERSION}.mkv")
    if os.path.exists(path) and os.path.getsize(path) > 100:
        return path
    tmp = f"{path}.part{os.getpid()}_{threading.get_ident()}.mkv"
    # (Commas inside an option value of the lavfi graph are escaped.)
    alpha = f"255*(1-pow(cos({angle:.6f}*min(1\\,hypot(trunc(X-W/2)\\,trunc(Y-H/2))/hypot(W/2\\,H/2))),4))"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-f", "lavfi", "-i", f"color=c=black:s={w}x{h}:r=1:d=1,format=yuva420p,geq=lum=0:cb=128:cr=128:a='{alpha}'",
        "-frames:v", "1", "-c:v", "ffv1", tmp,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if result.returncode != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"vignette mask failed: {result.stderr[:300]}")
    try:
        os.replace(tmp, path)
    except OSError:
        if not os.path.exists(path):
            return tmp
        os.unlink(tmp)
    return path


def edge_thickness(w, h):
    """Rows of light (even) the edge glow spans inward from the frame's edge."""
    return max(8, round(min(w, h) * 0.02) // 2 * 2)


# The edge glow: a thin soft line of the mood's colour just inside the picture's border, and one
# soft light gliding clockwise round it once every EDGE_LAP seconds, brightest at its head and
# fading along its tail. Quiet by design (it must never pull the eye off the picture). Both are
# stills, drawn once per size and colour (make_edge_glow) and laid over the borders only
# (edge_glow_graph): the line as four strips, the light as one sprite per border that the overlay
# moves along it every frame. (A moving picture as an overlay input costs far more: four renders
# side by side spent more time passing its frames along than drawing.)
EDGE_VERSION = 4
EDGE_LAP = 12.0
EDGE_CORE = (255, 242, 222)
# The light's tail: it fades over EDGE_TAIL of the way round (to 5% at three times that); ahead
# of its head it rises over EDGE_RISE.
EDGE_TAIL = 1 / 12
EDGE_RISE = 1 / 160


def _hex(rgb):
    return "0x%02x%02x%02x" % tuple(max(0, min(255, int(v))) for v in rgb)


def edge_geometry(w, ih):
    """The edge glow's sizes in a picture w x ih (between any letterbox bars): strip thickness,
    side strip length (the sides fit between the top and bottom strips), the light's sprite
    length and where its head sits in it (px from its leading end)."""
    th = edge_thickness(w, ih)
    perim = 2 * (w + ih)
    ahead = math.ceil(3 * EDGE_RISE * perim)
    # Even, like every place in the atlas: a 4:2:0 crop snaps to even pixels.
    length = (math.ceil(3 * EDGE_TAIL * perim) + ahead + 1) // 2 * 2
    return th, ih - 2 * th, length, ahead


def _edge_weights(d, k, th):
    """The line's and the halo's weight at distance d (px) in from the border."""
    import numpy as np
    fade = np.clip((th - d) / (0.35 * th), 0, 1)
    # A soft thread 3 px in (at 1080 rows), with a faint glow inward.
    return np.exp(-0.5 * ((d - 3 * k) / (1.1 * k)) ** 2) * fade, np.exp(-d / (6 * k)) * fade


def _edge_rgba(rgb, alpha, white):
    import numpy as np
    col, core = np.array(rgb, np.float32), np.array(EDGE_CORE, np.float32)
    out = np.empty(alpha.shape + (4,), np.float32)
    out[..., :3] = col + (core - col) * np.clip(white, 0, 1)[..., None]
    out[..., 3] = np.clip(alpha, 0, 1) * 255
    return (out + 0.5).clip(0, 255).astype(np.uint8)


def edge_pieces(w, ih, rgb):
    """The edge glow's stills (RGBA arrays): the line's strips "top", "bottom", "left", "right"
    (each as laid: top and bottom w x th, the sides th x side, their outer edge at the border;
    corners belong to the top and bottom strips, the line turning in them) and the light's sprite
    per border, "light_top" etc., pointing the way it travels (clockwise)."""
    import numpy as np
    th, side, length, ahead = edge_geometry(w, ih)
    k = min(w, ih) / 1080
    perim = 2 * (w + ih)
    pieces = {}
    # The line: each strip pixel's distance in from its nearest border.
    for name, (y0, x0, hh, ww) in (("top", (0, 0, th, w)), ("bottom", (ih - th, 0, th, w)),
                                   ("left", (th, 0, side, th)), ("right", (th, w - th, side, th))):
        y, x = np.mgrid[y0:y0 + hh, x0:x0 + ww].astype(np.float32)
        d = np.minimum(np.minimum(y, ih - 1 - y), np.minimum(x, w - 1 - x))
        line, halo = _edge_weights(d, k, th)
        pieces[name] = _edge_rgba(rgb, line * 0.26 + halo * 0.05, line * 0.25)
    # The light along the top border, head at `length - ahead` (travelling right): rows are the
    # distance in, columns the way along.
    d, c = np.mgrid[0:th, 0:length].astype(np.float32)
    behind = (length - ahead) - c
    light = np.where(behind >= 0, np.exp(-np.maximum(behind, 0) / (EDGE_TAIL * perim)), np.exp(-0.5 * (behind / (EDGE_RISE * perim)) ** 2))
    line, halo = _edge_weights(d, k, th)
    top = _edge_rgba(rgb, line * 0.5 * light + halo * 0.12 * light, line * (0.25 + 0.55 * light))
    pieces["light_top"] = top
    pieces["light_bottom"] = top[::-1, ::-1]
    pieces["light_right"] = top.transpose(1, 0, 2)[:, ::-1]
    pieces["light_left"] = top.transpose(1, 0, 2)[::-1, :]
    return pieces


def edge_atlas_layout(w, ih):
    """Where each of edge_pieces' stills sits in their atlas: {name: (x, y, w, h)}, and its size.
    The across-the-frame pieces stack at the left, the upright ones stand side by side right of them."""
    th, side, length, _ = edge_geometry(w, ih)
    across = max(w, length)
    places = {"top": (0, 0, w, th), "bottom": (0, th, w, th), "light_top": (0, 2 * th, length, th), "light_bottom": (0, 3 * th, length, th),
              "left": (across, 0, th, side), "right": (across + th, 0, th, side),
              "light_left": (across + 2 * th, 0, th, length), "light_right": (across + 3 * th, 0, th, length)}
    return places, (across + 4 * th, max(4 * th, side, length))


def make_edge_glow(w, ih, rgb, work_dir):
    """The edge glow's stills in colour `rgb` for a picture w x ih, packed in one PNG (see
    edge_atlas_layout), made once per size and colour and kept in the fx cache. Returns its path."""
    import numpy as np
    from PIL import Image
    root = fx_cache_dir() or work_dir
    out = os.path.join(root, f"edge_{w}x{ih}_{_hex(rgb)[2:]}_v{EDGE_VERSION}.png")
    if os.path.exists(out) and os.path.getsize(out) > 100:
        return out
    places, (aw, ah) = edge_atlas_layout(w, ih)
    atlas = np.zeros((ah, aw, 4), np.uint8)
    for name, img in edge_pieces(w, ih, rgb).items():
        x, y, pw, ph = places[name]
        atlas[y:y + ph, x:x + pw] = img
    tmp = f"{out}.part{os.getpid()}_{threading.get_ident()}.png"
    Image.fromarray(atlas, "RGBA").save(tmp, compress_level=1)
    try:
        os.replace(tmp, out)
    except OSError:
        if not os.path.exists(out):
            return tmp
        os.unlink(tmp)
    return out


def edge_moves(w, h, bar):
    """Where the edge glow's pieces go in a w x h picture with letterbox bars `bar` high: their
    places in the atlas ({name: (x, y, w, h)}) and where each is laid ({name: (x, y)}, numbers or
    overlay expressions of the video's time t: the lights move round the frame)."""
    ih = h - 2 * bar
    th, side, length, ahead = edge_geometry(w, ih)
    places, _ = edge_atlas_layout(w, ih)
    perim = 2 * (w + ih)
    speed = perim / EDGE_LAP

    def along(s0):
        # How far the head is along the border that starts s0 of the way round: from `ahead`
        # before its start (so the light's rise reaches round a corner before its head does) to
        # perim - ahead after it (the tail long gone by then).
        return f"(mod(t*{speed:.4f}+{perim - s0 + ahead},{perim})-{ahead})"

    lead = length - ahead
    at = {
        "top": (0, bar), "bottom": (0, bar + ih - th), "left": (0, bar + th), "right": (w - th, bar + th),
        "light_top": (f"{along(0)}-{lead}", bar),
        "light_right": (w - th, f"{bar - lead}+{along(w)}"),
        "light_bottom": (f"{w - ahead}-{along(w + ih)}", bar + ih - th),
        "light_left": (0, f"{bar + ih - ahead}-{along(2 * w + ih)}"),
    }
    return places, at


def edge_glow_graph(w, h, bar, src, out, segments):
    """The edge glow over [src] (a w x h picture, letterbox bars `bar` high) into [out].
    `segments` is [(start, end, label)]: each label make_edge_glow's still in that stretch's
    colour (see edge_segments), on for its time only. The line's strips are laid on their borders;
    each light sprite moves along its border with the light's head, round the frame on the whole
    video's clock (t), so the light runs on across the joins of the parts."""
    places, at = edge_moves(w, h, bar)
    parts = []
    cur = src
    for n, (a, b, label) in enumerate(segments):
        # Each colour from half a frame before its first frame to half a frame before the next
        # colour's (the first open before, the last after), so every frame is in exactly one.
        bounds = ([f"gte(t,{a - 0.5 / FPS:.6f})"] if n else []) + ([f"lt(t,{b - 0.5 / FPS:.6f})"] if n + 1 < len(segments) else [])
        on = f":enable='{'*'.join(bounds)}'" if bounds else ""
        e = f"{out}_e{n}"
        names = list(at)
        parts.append(f"[{label}]format=yuva420p,setparams=colorspace=bt470bg:range=tv,split={len(names)}" + "".join(f"[{e}{i}]" for i in range(len(names))))
        for i, name in enumerate(names):
            x, y, pw, ph = places[name]
            parts.append(f"[{e}{i}]crop={pw}:{ph}:{x}:{y}[{e}{name}]")
            nxt = f"{e}{name}o"
            ox, oy = at[name]
            parts.append(f"[{cur}][{e}{name}]overlay=x='{ox}':y='{oy}':format=yuv420{on}[{nxt}]")
            cur = nxt
    parts.append(f"[{cur}]null[{out}]")
    return ";".join(parts)


def look_filters(look, w, h, src, out, particles=None, vignette=None, edge=None):
    """Finishing filters for `look` on [src] (a w x h picture), labelled [out]: a gentle grade
    and colour preset (one lookup table), floating particles (the looped [particles] stream,
    when given), a vignette (the [vignette] mask of make_vignette_mask laid over, when given,
    else the vignette filter), fine film grain, letterbox bars and the edge glow (`edge`:
    edge_glow_graph's [(start, end, label)] streams; none drawn when not given), each only when
    the look has it on. (The panel glow is baked into each still instead: it would cost more than
    the rest together per frame.)"""
    parts = []
    cur = src
    lut = color_lut(look.get("grade"), look.get("color"))
    if lut:
        # One lookup table (fast): a little contrast around the mid-darks, richer and warmer colour.
        parts.append(f"[{cur}]lutyuv=y='{lut[0]}':u='{lut[1]}':v='{lut[2]}'[{out}_g]")
        cur = f"{out}_g"
    if particles and look.get("particles"):
        parts.append(f"[{cur}][{particles}]overlay=0:0:shortest=1:format=yuv420[{out}_p]")
        cur = f"{out}_p"
    tail = []
    vk, gk = look.get("vignette_k", 1.0), look.get("grain_k", 1.0)
    if look.get("vignette") and vk > 0 and vignette:
        parts.append(f"[{cur}][{vignette}]overlay=0:0:format=yuv420[{out}_v]")
        cur = f"{out}_v"
    elif look.get("vignette") and vk > 0:
        tail.append("vignette=angle=PI/6" if vk == 1.0 else f"vignette=angle={min(1.4, math.pi / 6 * vk):.4f}")
    if look.get("grain") and round(4 * gk) > 0:
        tail.append(f"noise=alls={round(4 * gk)}:allf=t")
    bar = letterbox_bar(w, h) if look.get("letterbox") else 0
    bars = [f"drawbox=x=0:y=0:w={w}:h={bar}:color=black:t=fill", f"drawbox=x=0:y={h - bar}:w={w}:h={bar}:color=black:t=fill"] if bar else []
    if look.get("edge_glow") and edge:
        # The glow before the bars, so a light running on past a corner never shows on them.
        parts.append(f"[{cur}]{','.join(tail) if tail else 'null'}[{out}_t]")
        parts.append(edge_glow_graph(w, h, bar, f"{out}_t", f"{out}_e" if bars else out, edge))
        if bars:
            parts.append(f"[{out}_e]{','.join(bars)}[{out}]")
    else:
        tail += bars
        parts.append(f"[{cur}]{','.join(tail) if tail else 'null'}[{out}]")
    return ";".join(parts)


# A filter graph longer than this goes to ffmpeg in a file (Windows caps a command line at 32k
# characters; the edge glow and the editor's layers can make a part's graph long).
GRAPH_INLINE = 12000


def graph_args(graph, path):
    """ffmpeg's arguments for the filter graph `graph`: inline, or when long written to `path`
    and read from there (-/filter_complex on ffmpeg 7+, -filter_complex_script before)."""
    if len(graph) <= GRAPH_INLINE:
        return ["-filter_complex", graph]
    with open(path, "w", encoding="utf-8") as f:
        f.write(graph)
    return ["-/filter_complex" if ffmpeg_major() >= 7 else "-filter_complex_script", path]


_FFMPEG_MAJOR = None


def ffmpeg_major():
    """ffmpeg's major version ("ffmpeg version 6.1.1" -> 6). Development builds ("N-125365-…")
    count as new; an unreadable answer counts as old (the option every version knows)."""
    global _FFMPEG_MAJOR
    if _FFMPEG_MAJOR is None:
        try:
            out = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True, timeout=10).stdout
            m = re.match(r"ffmpeg version (?:n?(\d+)|N-)", out)
            _FFMPEG_MAJOR = (int(m.group(1)) if m.group(1) else 99) if m else 0
        except Exception:
            _FFMPEG_MAJOR = 0
    return _FFMPEG_MAJOR


def check_ffmpeg():
    """Verify ffmpeg is available."""
    if shutil.which("ffmpeg") is None:
        raise RuntimeError(
            "ffmpeg not found. Install it with: brew install ffmpeg"
        )

def get_audio_duration_sec(audio_path):
    """Get audio duration in seconds using ffprobe, falling back to WAV headers."""
    try:
        result = subprocess.run(
            [
                "ffprobe", "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                audio_path,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            duration = float((result.stdout or "").strip())
            if math.isfinite(duration) and duration > 0:
                return duration
    except Exception:
        pass
    try:
        with wave.open(audio_path, 'r') as wf:
            return wf.getnframes() / wf.getframerate()
    except Exception:
        return 5.0

def get_resolution(preset, fmt="landscape"):
    """Return (width, height) for a resolution preset; "reels" is the same size turned upright (9:16)."""
    presets = {
        "720p":  (1280, 720),
        "1080p": (1920, 1080),
        "1440p": (2560, 1440),
        "4k":    (3840, 2160),
        "2160p": (3840, 2160),
    }
    w, h = presets.get(preset, (1920, 1080))
    return (h, w) if fmt == "reels" else (w, h)


REEL_TARGET_SEC = 60.0
# A reel may end this far from its ideal cut point to land on a narration boundary.
REEL_SNAP_SEC = 15.0


def plan_reel_cuts(boundaries, total, target=REEL_TARGET_SEC, snap=REEL_SNAP_SEC):
    """Split `total` seconds into consecutive ~`target`-second reels.

    `boundaries` are the times (seconds) where one narration beat ends and the next
    begins. The reel count is `round(total / target)` so every reel stays close to
    the target, and each cut moves to the nearest boundary within `snap` seconds of
    its ideal spot (only a beat longer than that gets cut mid-sentence).
    Returns [(start, end), ...] covering 0..total.
    """
    if total <= 0:
        return []
    n = max(1, int(round(total / target)))
    cuts = []
    prev = 0.0
    for k in range(1, n):
        ideal = total * k / n
        near = [b for b in boundaries if abs(b - ideal) <= snap and prev + 1.0 < b < total - 1.0]
        cut = min(near, key=lambda b: abs(b - ideal)) if near else ideal
        if cut <= prev + 1.0:
            continue
        cuts.append(cut)
        prev = cut
    edges = [0.0, *cuts, float(total)]
    return [(round(a, 3), round(b, 3)) for a, b in zip(edges, edges[1:])]


def split_reels(final_video, cuts, reels_dir):
    """Cut `final_video` into reels_dir/reel_01.mp4 ... at `cuts` (re-encoded so every
    cut is frame-exact). Writes reels.json and returns the reel paths."""
    if os.path.isdir(reels_dir):
        shutil.rmtree(reels_dir, ignore_errors=True)
    os.makedirs(reels_dir, exist_ok=True)
    out = []
    manifest = []
    for i, (start, end) in enumerate(cuts):
        dur = end - start
        if dur < 0.2:
            continue
        path = os.path.join(reels_dir, f"reel_{i + 1:02d}.mp4")
        fade = min(0.15, dur / 4)
        cmd = [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
            "-ss", f"{start:.3f}", "-i", final_video, "-t", f"{dur:.3f}",
            # Every audio track (with music, "Narration only" rides along).
            "-map", "0:v:0", "-map", "0:a",
            "-af", f"afade=t=in:d=0.03,afade=t=out:st={max(0.0, dur - fade):.3f}:d={fade:.3f}",
            "-c:v", "libx264", "-preset", "fast", "-crf", "20", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "160k",
            "-movflags", "+faststart",
            path,
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        if result.returncode != 0 or not os.path.exists(path):
            raise RuntimeError(f"Failed to cut reel {i + 1}: {result.stderr[:300]}")
        out.append(path)
        manifest.append({"file": os.path.basename(path), "start": start, "end": end})
    with open(os.path.join(reels_dir, "reels.json"), "w", encoding="utf-8") as f:
        json.dump({"reels": manifest}, f, indent=2)
    return out


def combine_audio_files(audio_paths, output_path):
    """Concatenate WAV files into a single WAV using ffmpeg."""
    if os.path.exists(output_path):
        return

    list_file = output_path + ".txt"
    with open(list_file, 'w') as f:
        for p in audio_paths:
            f.write(f"file '{p}'\n")

    subprocess.run([
        "ffmpeg", "-y", "-f", "concat", "-safe", "0",
        "-i", list_file,
        "-c:a", "pcm_s16le", "-ar", "22050", "-ac", "1",
        output_path
    ], capture_output=True, timeout=300)

    os.unlink(list_file)


# ── Timeline planning (pure helpers) ──

def plan_transitions(durations, pages, kind="crossfade", base=0.5, names=None, joins=None, forced=None, moods=None):
    """Plan crossfades between clips of `durations` seconds without moving the narration.

    Returns (starts, trans): starts[k] is when clip k's narration begins (running sum)
    and trans[k] = (xfade transition, seconds) for the change into clip k (trans[0] is
    None). Each clip's video runs on for trans[k + 1] seconds past its narration, so the
    fade into clip k begins exactly at starts[k] and the total stays sum(durations).
    Same-page beats dissolve quickly, page changes rotate through `names` (default
    PAGE_TRANSITIONS), short clips get shorter fades, and kind "none" gives one-frame (hard)
    cuts. joins[k] "cut" cuts into clip k (it animates its own entrance: a duo slide) and
    "impact" makes it a quick dissolve (the clip flashes in itself). forced[k] (name, seconds),
    the editor's pick for clip k, wins over all of that. With `moods` (plan_moods entries per
    clip) a page turn follows the mood of the clip it turns to (mood_transition), with an accent
    at most every ACCENT_GAP seconds.
    """
    names = names or PAGE_TRANSITIONS
    joins = joins or [None] * len(durations)
    forced = forced or [None] * len(durations)
    moods = moods or [None] * len(durations)
    starts = []
    t = 0.0
    for d in durations:
        starts.append(t)
        t += d
    frame = 1.0 / FPS
    trans = [None]
    turn = 0
    last_accent = -ACCENT_GAP
    last_scene = -SCENE_GAP
    for k in range(1, len(durations)):
        if forced[k]:
            name, dur = forced[k]
        elif kind == "none" or joins[k] == "cut":
            name, dur = "fade", frame
        elif joins[k] == "impact":
            name, dur = "fade", 0.12
        elif pages[k] == pages[k - 1]:
            name, dur = SAME_PAGE_TRANSITION[0], min(SAME_PAGE_TRANSITION[1], base)
        elif moods[k]:
            m = moods[k]
            # An accent marks entering a reveal (or a peak), not every page of one.
            entering = not moods[k - 1] or moods[k - 1]["mood"] != m["mood"] or m["mood"] != "reveal"
            name, dur = mood_transition(m["mood"], m.get("raw"), turn, base, entering and starts[k] - last_accent >= ACCENT_GAP)
            if any(name == a for v in ACCENTS.values() for a, _ in v):
                last_accent = starts[k]
            elif m["mood"] in BURST_MOODS and moods[k - 1] and (moods[k - 1]["mood"] != m["mood"] or moods[k - 1].get("kind") != m.get("kind")) and starts[k] - last_scene >= SCENE_GAP:
                # A new scene bursts into particles (an accent keeps its own).
                name, dur = "particles", round(base * SCENE_TRANSITION, 3)
                last_scene = starts[k]
            turn += 1
        else:
            name, dur = names[turn % len(names)], base
            turn += 1
        dur = max(frame, min(dur, 0.45 * durations[k - 1], 0.45 * durations[k]))
        trans.append((name, round(dur, 3)))
    return starts, trans


# Panels of one page come and go: the panel leaving slides PANEL_SLIDE of the frame width out
# as the dissolve takes it, and the next slides in the same way and settles, a little after it
# (PANEL_SETTLE seconds longer), so the swap moves instead of standing still.
PANEL_SLIDE = 0.05
PANEL_SETTLE = 0.3


def plan_panel_moves(starts, trans, pages, joins=None, duo=None, forced=None, holds=None):
    """Panel entrances and exits for clips on the same page as the one before: clip k-1 gets
    "panel_out": (when the change starts, seconds, direction) and clip k "panel_in": (seconds,
    direction), in their own seconds; the direction (+1: towards the left, -1: the right)
    alternates. A cut, an impact, a duo clip (it slides in by itself) or a transition the editor
    picked (`forced`) keeps its own entrance, and a hold (`holds`: the same panel again) stays put."""
    joins = joins or [None] * len(starts)
    holds = holds or [False] * len(starts)
    duo = duo or [False] * len(starts)
    forced = forced or [None] * len(starts)
    out = [{} for _ in starts]
    turn = 0
    for k in range(1, len(starts)):
        if pages[k] != pages[k - 1] or joins[k] or forced[k] or holds[k] or duo[k] or duo[k - 1] or not trans[k] or trans[k][1] < 1.5 / FPS:
            continue
        d = 1 if turn % 2 == 0 else -1
        turn += 1
        dur = trans[k][1]
        out[k - 1]["panel_out"] = (round(starts[k] - starts[k - 1], 3), dur, d)
        out[k]["panel_in"] = (round(dur + PANEL_SETTLE, 3), d)
    return out


def _panel_move(fx, w, axis):
    """Overlay offset (pixels, of overlay time t) of a panel's entrance and exit (plan_panel_moves)."""
    if axis != "x":
        return "0"
    terms = []
    if "panel_in" in fx:
        dur, d = fx["panel_in"]
        terms.append(f"({round(d * PANEL_SLIDE * w)})*pow(max(0,1-t/{dur}),3)")
    if "panel_out" in fx:
        t0, dur, d = fx["panel_out"]
        terms.append(f"({-round(d * PANEL_SLIDE * w)})*pow(min(1,max(0,(t-{t0})/{dur})),2)")
    return "+".join(terms) if terms else "0"


def scene_bursts(starts, trans, colors):
    """The particle bursts the finish draws over the joins: [(first frame on the video's clock,
    rgb)] for every "particles" join (plan_transitions), peaking halfway through it. `colors`:
    each clip's rgb (the scene it bursts into gives the colour)."""
    out = []
    for k in range(1, len(starts)):
        if trans[k] and trans[k][0] == "particles":
            out.append((round((starts[k] - starts[0] + trans[k][1] / 2 - BURST_PEAK) * FPS), tuple(colors[k])))
    return out


def plan_clip_fx(starts, trans, pages, joins=None, leaks=False, impacts=None, gap=None, forced=None, quiet=None):
    """The page-turn effects each clip draws itself, in its own seconds. A burn, zoom blur,
    glitch or whip into clip k is drawn by both clips (the same frames at the same moment, under
    the join, so it cannot show): clip k-1 gets "<name>_out": (when the change starts, seconds)
    and clip k "<name>_in": seconds. With `leaks`, a clip that turns to a new page some other
    way (and the opening clip) gets "leak": a flare blooming as it lands, at most one every
    `gap` seconds (a burn counts) so it stays an accent; an action beat (`impacts`) flashes
    instead, and a `quiet` clip (a horror or sad scene: a warm flare would break it) gets none.
    A transition the editor picked (`forced[k]`) is drawn wherever it is."""
    joins = joins or [None] * len(starts)
    impacts = impacts or [False] * len(starts)
    forced = forced or [None] * len(starts)
    quiet = quiet or [False] * len(starts)
    gap = LEAK_GAP if gap is None else gap
    fx = [{} for _ in starts]
    last = -gap
    if leaks and fx and not impacts[0] and not quiet[0]:
        fx[0]["leak"] = True
        last = 0.0
    for k in range(1, len(starts)):
        if not trans[k]:
            continue
        if forced[k]:
            name, dur = trans[k]
            if name in CLIP_DRAWN and dur > 1.5 / FPS:
                if name == "burn":
                    last = starts[k]
                fx[k - 1][f"{name}_out"] = (round(starts[k] - starts[k - 1], 3), dur)
                fx[k][f"{name}_in"] = dur
                if name == "burn":
                    fx[k - 1]["burn_flip"] = fx[k]["burn_flip_in"] = k % 2 == 0
            continue
        if joins[k] == "impact" or impacts[k] or pages[k] == pages[k - 1]:
            continue
        name, dur = trans[k]
        if name == "particles":
            # Drawn over the join by the finish (scene_bursts); it counts as a page-turn effect.
            last = starts[k]
            continue
        if name in CLIP_DRAWN and dur > 1.5 / FPS:
            if name in CLIP_TRANSITIONS:
                last = starts[k]
            fx[k - 1][f"{name}_out"] = (round(starts[k] - starts[k - 1], 3), dur)
            fx[k][f"{name}_in"] = dur
            if name == "burn":
                # Burns alternate sides; both clips draw the same one.
                fx[k - 1]["burn_flip"] = fx[k]["burn_flip_in"] = k % 2 == 0
        elif leaks and not quiet[k] and starts[k] - last >= gap:
            fx[k]["leak"] = True
            last = starts[k]
    return fx


def ramp_ease(var, frames, ramp=False):
    """Eased 0..1 progress of a move over `frames` frames (ffmpeg expression of frame counter
    `var`). With `ramp`, a short beat snaps (a fast ease-out over its first half) and a long one
    drifts (mostly constant speed, so it never sits still and then rushes)."""
    if not ramp:
        return _ease(var, frames)
    if frames < 2 * FPS:
        n = max(2, round(frames * 0.5))
        return f"(1-pow(1-min(1,{var}/{n}),3))"
    lin = f"min(1,{var}/{max(1, frames - 1)})"
    return f"(0.65*{lin}+0.35*{_ease(var, frames)})"


def clip_frames(starts, trans, k, last_duration):
    """Frames clip k must render: up to the next clip's start plus that fade's tail (and a
    spare frame), or its own duration for the last clip."""
    first = round(starts[k] * FPS)
    if k + 1 < len(starts):
        return round(starts[k + 1] * FPS) - first + math.ceil(trans[k + 1][1] * FPS) + 1
    return math.ceil(last_duration * FPS) + 1


def xfade_chain(labels, starts, trans, out):
    """Filtergraph that chains video `labels` with xfade so labels[k] first appears at
    starts[k] (seconds, snapped to the frame grid, relative to starts[0]); trans[k] is
    the (transition, seconds) into labels[k]. The result is labelled `out`."""
    if len(labels) == 1:
        return f"[{labels[0]}]null[{out}]"
    base = round(starts[0] * FPS)
    parts = []
    prev = labels[0]
    for k in range(1, len(labels)):
        name, dur = trans[k]
        # A hair before the frame, so xfade (which starts at the first frame with
        # pts >= offset) never slips a frame through decimal rounding.
        offset = max(0.0, (round(starts[k] * FPS) - base) / FPS - 0.001)
        nxt = out if k == len(labels) - 1 else f"{out}_{k}"
        # A burn, zoom blur, glitch or whip is drawn by the clips on both sides over a plain join.
        name = CLIP_DRAWN.get(name, name)
        parts.append(
            f"[{prev}][{labels[k]}]xfade=transition={name}:duration={dur:.3f}:offset={offset:.6f}[{nxt}]"
        )
        prev = nxt
    return ";".join(parts)


def pick_motion(aspect, box_aspect, turn, prev):
    """Ken Burns move for a panel of `aspect` (w/h) in a box of `box_aspect`: tall panels pan
    vertically, wide ones horizontally, all push/pull; never the same move twice running."""
    if aspect < box_aspect * 0.75:
        options = FG_MOTIONS["tall"]
    elif aspect > box_aspect * 1.33:
        options = FG_MOTIONS["wide"]
    else:
        options = FG_MOTIONS["even"]
    motion = options[turn % len(options)]
    if motion == prev:
        motion = options[(turn + 1) % len(options)]
    return motion


def _ease(var, frames):
    """Cosine ease 0..1 over `frames` frames, as an ffmpeg expression of frame counter `var`."""
    return f"(0.5-0.5*cos(PI*{var}/{max(1, frames - 1)}))"


def _motion_zoom(motion, amount, pan_zoom):
    """Zoom expression (of the eased progress {e}) for a Ken Burns move."""
    if motion == "push":
        return f"1+{amount}*{{e}}"
    if motion == "pull":
        return f"{1 + amount:.4f}-{amount}*{{e}}"
    if motion in ("pan_down", "pan_up", "pan_right", "pan_left"):
        return f"{pan_zoom}"
    return "1"


# A punch-in snaps to its zoom over this long, then drifts on.
PUNCH_SEC = 0.25


def edit_move(move, push):
    """Zoom (an expression of the eased progress {e} and frame counter `on`) and window (x, y
    fractions of the slack the zoom leaves, of {e}) for a Video editor move: {"kind", "amount",
    "from", "to"} with from / to (x, y) in 0..1 of the panel (where the camera looks; 0.5 is the
    centre) and amount the share zoomed in. `push` (the look's push for the clip) stands in for a
    missing amount on zooms.

    zoom pushes into "to" (else "from", else the centre); zoom_out pulls back from "from";
    pan_* travels the whole slack of a 1 + amount zoom; ken_burns drifts from "from" to "to" while
    pushing from half the amount to all of it; punch snaps to the amount in PUNCH_SEC, then drifts
    in a little more; still holds."""
    kind = move.get("kind")
    a = move.get("amount")
    f0, f1 = move.get("from"), move.get("to")

    def aim(f):
        return (f"{f[0]:.4f}", f"{f[1]:.4f}") if f else ("0.5", "0.5")

    if kind == "zoom":
        a = push if a is None else a
        return f"1+{a:.4f}*{{e}}", aim(f1 or f0)
    if kind == "zoom_out":
        a = push if a is None else a
        return f"{1 + a:.4f}-{a:.4f}*{{e}}", aim(f0 or f1)
    if kind in ("pan_left", "pan_right", "pan_up", "pan_down"):
        a = max(0.02, FG_PAN_ZOOM - 1 if a is None else a)
        return f"{1 + a:.4f}", FG_WINDOW[kind]
    if kind == "ken_burns":
        a = 0.08 if a is None else a
        p0, p1 = f0 or (0.35, 0.4), f1 or (0.65, 0.6)
        window = (f"({p0[0]:.4f}+{p1[0] - p0[0]:.4f}*{{e}})", f"({p0[1]:.4f}+{p1[1] - p0[1]:.4f}*{{e}})")
        return f"{1 + a / 2:.4f}+{a / 2:.4f}*{{e}}", window
    if kind == "punch":
        a = 0.15 if a is None else a
        n = max(2, round(PUNCH_SEC * FPS))
        return f"1+{a:.4f}*(1-pow(1-min(1,on/{n}),3))+0.02*{{e}}", aim(f1 or f0)
    return "1", FG_WINDOW["still"]


def move_dirs(move):
    """Which way the blurred background drifts under an editor move (see PARALLAX_DIRS)."""
    kind = move.get("kind")
    if kind == "ken_burns":
        p0, p1 = move.get("from") or (0.35, 0.4), move.get("to") or (0.65, 0.6)
        sign = lambda v: (v > 0.01) - (v < -0.01)
        return (sign(p1[0] - p0[0]) * 0.8, sign(p1[1] - p0[1]) * 0.6)
    return PARALLAX_DIRS.get({"zoom": "push", "zoom_out": "pull", "punch": "push"}.get(kind, kind), (0.6, 0))


def _warp(window, zoom, frames, interp, ease=None, held=True):
    """One still held for `frames` frames with an eased move: each frame shows the
    sub-rectangle 1/zoom the size of the image, placed by the `window` fractions, stretched
    over the whole frame. perspective resamples with subpixel precision, so slow moves glide
    (zoompan rounds its crop to whole, chroma-aligned pixels and visibly steps). With `held`
    False the input is already a moving picture of `frames` frames (a layered panel)."""
    e = ease or _ease("on", frames)
    # A camera that does not move (no term of the eased progress): the corners are worked out
    # once, not for every frame (perspective then also keeps its sampling table, the bulk of its
    # cost per frame).
    still = not any(re.search(r"\{e\}|\b(?:on|in|t)\b", v) for v in (zoom, *window))
    z = "(" + zoom.format(e=e) + ")"
    px, py = (v.format(e=e) for v in window)
    cw, ch = f"W/{z}", f"H/{z}"
    x, y = f"(W-{cw})*{px}", f"(H-{ch})*{py}"
    clock = _hold(frames) if held else f"settb=1/{FPS},setpts=N"
    return (
        f"{clock},"
        f"perspective=x0='{x}':y0='{y}':x1='{x}+{cw}':y1='{y}':"
        f"x2='{x}':y2='{y}+{ch}':x3='{x}+{cw}':y3='{y}+{ch}':"
        f"interpolation={interp}:eval={'init' if still else 'frame'}"
    )


# ── Per-clip images ──

def _blurred_fill(img, w, h, grade=None):
    """The frame-sized background: `img` zoomed in past "cover", blurred, desaturated, darkened
    (and through the mood `grade`, at the small size where it is cheap)."""
    from PIL import Image, ImageEnhance, ImageFilter
    iw, ih = img.size
    # Blur at 1/8 size (much cheaper, and smoother once scaled back up).
    sw, sh = max(16, w // 8), max(16, h // 8)
    cover = max(sw / iw, sh / ih) * BG_ZOOM
    small = img.resize((max(sw, round(iw * cover)), max(sh, round(ih * cover))), Image.BOX)
    left = (small.width - sw) // 2
    top = (small.height - sh) // 2
    small = small.crop((left, top, left + sw, top + sh))
    small = small.filter(ImageFilter.GaussianBlur(radius=max(2.0, sw * 0.03)))
    small = ImageEnhance.Color(small).enhance(0.6)
    small = apply_grade(ImageEnhance.Brightness(small).enhance(0.55), grade)
    return small.resize((w, h), Image.BICUBIC).filter(ImageFilter.GaussianBlur(radius=max(2, w // 400)))


def _bloom(img):
    """A soft glow on a still: a blurred quarter-size copy screened over it, the highlights most."""
    from PIL import Image, ImageChops, ImageEnhance, ImageFilter
    small = img.resize((max(1, img.width // 4), max(1, img.height // 4)), Image.BOX)
    small = small.filter(ImageFilter.GaussianBlur(radius=max(2.0, small.width / 60)))
    glow = ImageEnhance.Brightness(small.resize(img.size, Image.BICUBIC)).enhance(0.14)
    return ImageChops.screen(img, glow)


# ── Light leaks, film burns and particles (made once, small, then scaled) ──

# Bumped whenever the drawn flare frames change (they are cached by size between renders).
FX_VERSION = 1


def fx_cache_dir():
    """Folder the size-only effect frames are kept in between renders (MANGAVOX_FX_CACHE, else
    the system temp folder), or None when it cannot be made."""
    import tempfile
    path = os.environ.get("MANGAVOX_FX_CACHE") or os.path.join(tempfile.gettempdir(), "mangavox-fx")
    try:
        os.makedirs(path, exist_ok=True)
        return path
    except OSError:
        return None


def _glow_sprite(size, color, alpha):
    """A soft round light: `color` with a radial falloff, `alpha` (0..1) at its centre."""
    from PIL import Image, ImageChops
    w, h = max(2, int(size[0])), max(2, int(size[1]))
    falloff = ImageChops.invert(Image.radial_gradient("L")).resize((w, h), Image.BILINEAR)
    falloff = falloff.point(lambda v: int(v * v / 255 * max(0.0, min(1.0, alpha))))
    sprite = Image.new("RGBA", (w, h), color + (0,))
    sprite.putalpha(falloff)
    return sprite


def _paint(canvas, sprite, cx, cy):
    """Alpha-composite `sprite` centred on (cx, cy) of `canvas`, clipped to it."""
    x, y = int(cx - sprite.width / 2), int(cy - sprite.height / 2)
    left, top = max(0, -x), max(0, -y)
    right, bottom = min(sprite.width, canvas.width - x), min(sprite.height, canvas.height - y)
    if right > left and bottom > top:
        canvas.alpha_composite(sprite.crop((left, top, right, bottom)), (x + left, y + top))


def make_flare_frames(kind, w, h, work_dir, color=None):
    """PNG frames (a quarter of the frame size, RGBA) of a light leak ("leak": a warm flare with
    a lens streak sweeping across as a page lands) or a film burn ("burn": a hot orange-to-white
    bloom that swells over a cut and dies away), or (half the frame size, they are finer) a
    particle burst ("burst": sparks and pixel shards in `color` flying out from the middle over
    a soft bloom as one scene dissolves into the next). The clips scale them up: they are soft,
    so the low resolution never shows, and they cost a few milliseconds on the frames they touch
    only. Returns (ffmpeg input pattern, frame count)."""
    from PIL import Image, ImageDraw
    import random
    div = 2 if kind == "burst" else 4
    sw, sh = max(32, w // div), max(32, h // div)
    seconds = {"leak": LEAK_SEC, "burst": BURST_SEC}.get(kind, BURN_SEC)
    count = max(2, round(seconds * FPS))
    # The frames depend on the size only, so they are kept between renders (they take seconds
    # to draw): in a shared cache when there is one, else next to the clips.
    root = fx_cache_dir() or work_dir
    tint = f"_{_hex(color)[2:]}" if color else ""
    folder = os.path.join(root, f"fx_{kind}{tint}_{sw}x{sh}_v{FX_VERSION}")
    pattern = os.path.join(folder, "f_%03d.png")
    if os.path.exists(os.path.join(folder, f"f_{count - 1:03d}.png")):
        return pattern, count
    # Drawn into a folder of its own and moved into place whole, so a render running at the
    # same time never reads half a set.
    tmp = f"{folder}.part{os.getpid()}_{id(work_dir)}"
    os.makedirs(tmp, exist_ok=True)
    rng = random.Random(7)
    # The burn's flicker, drawn in frame order so every frame gets the same value as before.
    flicker = [0.88 + 0.12 * rng.random() for _ in range(count)]
    if kind == "burst":
        shards = _burst_shards(random.Random(23), sw, sh)

    def draw(i):
        u = i / (count - 1)
        img = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
        if kind == "burst":
            _draw_burst(img, ImageDraw.Draw(img, "RGBA"), shards, u, sw, sh, color or (255, 214, 160))
        elif kind == "leak":
            # Quick bloom, slow fade; the flare drifts left to right and rises a little.
            env = math.sin(math.pi * min(1.0, u ** 0.7)) ** 1.6
            cx = (-0.15 + 1.25 * u) * sw
            cy = (0.42 - 0.12 * u) * sh
            _paint(img, _glow_sprite((sh * 1.9, sh * 1.6), (255, 146, 58), 0.62 * env), cx, cy)
            _paint(img, _glow_sprite((sh * 1.2, sh * 1.0), (255, 84, 120), 0.32 * env), cx - 0.32 * sw, cy + 0.22 * sh)
            _paint(img, _glow_sprite((sh * 0.55, sh * 0.55), (255, 236, 200), 0.7 * env), cx, cy)
            # The anamorphic streak through the flare.
            _paint(img, _glow_sprite((sw * 1.5, max(4, sh * 0.07)), (255, 214, 170), 0.55 * env), cx, cy)
        else:
            # Swells to cover the frame at BURN_PEAK, then burns out; a little flicker.
            peak = BURN_PEAK / BURN_SEC
            env = (u / peak) ** 1.5 if u < peak else (1 - (u - peak) / (1 - peak)) ** 1.3
            env *= flicker[i]
            grow = 0.55 + 1.9 * min(1.0, u / peak) ** 0.8
            cx, cy = sw * 0.12, sh * 0.62
            _paint(img, _glow_sprite((sh * 2.6 * grow, sh * 2.2 * grow), (232, 64, 18), 0.95 * env), cx, cy)
            _paint(img, _glow_sprite((sh * 1.9 * grow, sh * 1.6 * grow), (255, 150, 48), 0.95 * env), cx + 0.06 * sw, cy)
            _paint(img, _glow_sprite((sh * 1.2 * grow, sh * 1.0 * grow), (255, 244, 220), 0.98 * env), cx + 0.1 * sw, cy - 0.04 * sh)
            # A second, smaller burn from the far edge.
            _paint(img, _glow_sprite((sh * 1.4 * grow, sh * 1.2 * grow), (255, 120, 30), 0.7 * env), sw * 0.95, sh * 0.2)
        img.save(os.path.join(tmp, f"f_{i:03d}.png"), compress_level=1)

    with ThreadPoolExecutor(max_workers=max(2, min(16, cpus()))) as pool:
        list(pool.map(draw, range(count)))
    try:
        os.replace(tmp, folder)
    except OSError:
        # Another render got there first (or the cache cannot be renamed into): use what is there.
        if not os.path.exists(os.path.join(folder, f"f_{count - 1:03d}.png")):
            return os.path.join(tmp, "f_%03d.png"), count
        shutil.rmtree(tmp, ignore_errors=True)
    return pattern, count


def _burst_shards(rng, sw, sh):
    """The particle burst's shards: where each starts (around the middle, wider than high), where
    it flies, when it lights and how big it is; a few in three are square pixel shards, the rest
    sparks with a streak."""
    out = []
    for _ in range(240):
        a = rng.uniform(0, 2 * math.pi)
        r0 = rng.uniform(0.0, 0.22) ** 0.8
        x0, y0 = 0.5 + r0 * math.cos(a) * 0.62, 0.5 + r0 * math.sin(a) * 0.5
        speed = rng.uniform(0.25, 0.9)
        out.append({"x0": x0, "y0": y0, "dx": math.cos(a) * speed * 0.75, "dy": math.sin(a) * speed * 0.55 - rng.uniform(0, 0.05),
                    "on": rng.uniform(0.0, 0.3), "life": rng.uniform(0.45, 0.8), "size": rng.uniform(0.8, 2.6),
                    "square": rng.random() < 0.5, "white": rng.random() < 0.4})
    return out


def _draw_burst(img, draw, shards, u, sw, sh, color):
    """Frame u (0..1) of the particle burst on `img` (RGBA, sw x sh): a soft bloom swelling to
    BURST_PEAK and dying, the shards flying out on an ease-out, each fading over its life."""
    peak = BURST_PEAK / BURST_SEC
    bloom = (u / peak) ** 1.3 if u < peak else (1 - (u - peak) / (1 - peak)) ** 1.6
    unit = min(sw, sh)
    _paint(img, _glow_sprite((unit * 1.6, unit * 1.1), color, 0.14 * bloom), sw / 2, sh / 2)
    _paint(img, _glow_sprite((unit * 0.7, unit * 0.5), (255, 246, 232), 0.22 * bloom), sw / 2, sh / 2)
    core = tuple(round(c + (255 - c) * 0.6) for c in color)
    for p in shards:
        v = (u - p["on"]) / p["life"]
        if not 0 < v < 1:
            continue
        # Out fast, then slowing; bright as it lights, fading as it goes.
        go = 1 - (1 - v) ** 2.2
        x = (p["x0"] + p["dx"] * go) * sw
        y = (p["y0"] + p["dy"] * go) * sh
        a = (min(1.0, v / 0.12) * (1 - v) ** 1.4)
        s = max(1.0, p["size"] * unit / 260)
        col = core if p["white"] else color
        if p["square"]:
            # A pixel shard: solid enough to read on a white page as well as a dark one, with a
            # darker rim.
            q = s * 1.6
            rim = tuple(round(c * 0.45) for c in col)
            draw.rectangle([x - q - 1, y - q - 1, x + q + 1, y + q + 1], fill=rim + (round(150 * a),))
            draw.rectangle([x - q, y - q, x + q, y + q], fill=col + (round(235 * a),))
        else:
            # A streak behind the spark, along its flight, shorter as it slows.
            tail = (1 - v) ** 1.5 * 0.05
            draw.line([(x - p["dx"] * tail * sw, y - p["dy"] * tail * sh), (x, y)], fill=col + (round(150 * a),), width=max(1, round(s)))
            _paint(img, _glow_sprite((s * 7, s * 7), col, 0.65 * a), x, y)


def make_particle_loop(w, h, work_dir):
    """A PARTICLE_SEC loop of floating embers and dust (rising, swaying, twinkling; seamless) as a
    full-size video with alpha, rendered once per render and laid over the whole picture."""
    from PIL import Image
    import random
    out = os.path.join(work_dir, f"particles_{w}x{h}.mkv")
    if os.path.exists(out) and os.path.getsize(out) > 1000:
        return out
    sw, sh = max(64, w // 2), max(64, h // 2)
    count = round(PARTICLE_SEC * FPS)
    folder = os.path.join(work_dir, "fx_particles")
    os.makedirs(folder, exist_ok=True)
    rng = random.Random(11)
    unit = min(sw, sh)
    motes = []
    for _ in range(46):
        ember = rng.random() < 0.55
        size = unit * (rng.uniform(0.012, 0.024) if ember else rng.uniform(0.008, 0.016))
        motes.append({
            "x": rng.random(), "y": rng.random(),
            # Whole laps per loop, so it repeats seamlessly.
            "laps": rng.choice((1, 1, 2)) if ember else 1,
            "sway": rng.uniform(0.01, 0.04), "sway_n": rng.choice((1, 2)), "phase": rng.random(),
            "tw_n": rng.choice((2, 3, 4)),
            "sprite": _glow_sprite((size * 2.4, size * 2.4), (255, 150, 60) if ember else (255, 240, 220), 1.0),
            "alpha": rng.uniform(0.45, 0.9) if ember else rng.uniform(0.25, 0.5),
            "rise": 1 if ember else rng.choice((-1, 1)) * 0.5,
        })
    for i in range(count):
        u = i / count
        img = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
        for m in motes:
            y = (m["y"] - m["rise"] * m["laps"] * u) % 1.0
            x = (m["x"] + m["sway"] * math.sin(2 * math.pi * (m["sway_n"] * u + m["phase"]))) % 1.0
            tw = 0.6 + 0.4 * math.sin(2 * math.pi * (m["tw_n"] * u + m["phase"]))
            spr = m["sprite"].copy()
            spr.putalpha(spr.getchannel("A").point(lambda v, a=m["alpha"] * tw: int(v * a)))
            _paint(img, spr, x * sw, y * sh)
        img.save(os.path.join(folder, f"p_{i:03d}.png"))
    tmp = out + ".part.mkv"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-framerate", str(FPS), "-i", os.path.join(folder, "p_%03d.png"),
        "-vf", f"scale={w}:{h}:flags=bicubic,format=yuva420p",
        "-c:v", "ffv1", "-level", "3", "-slices", "16", tmp,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    if result.returncode != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"particle loop failed: {result.stderr[:300]}")
    os.replace(tmp, out)
    return out


# Bumped whenever the mood atmosphere loops change (cached by size).
ATMOS_VERSION = 1
# Atmosphere loop length (seconds).
ATMOS_SEC = 8.0
ATMOS_KINDS = ("embers", "snow", "rain", "dust")


def _encode_loop(folder, pattern, w, h, out, scale=True):
    """PNG frames -> a seamless loop with alpha (FFV1, yuva420p) at `out`, moved into place whole."""
    tmp = f"{out}.part{os.getpid()}_{threading.get_ident()}.mkv"
    vf = (f"scale={w}:{h}:flags=bicubic," if scale else "") + "format=yuva420p"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-framerate", str(FPS), "-i", os.path.join(folder, pattern),
        "-vf", vf, "-c:v", "ffv1", "-level", "3", "-slices", "16", tmp,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    shutil.rmtree(folder, ignore_errors=True)
    if result.returncode != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"loop encode failed: {result.stderr[:300]}")
    try:
        os.replace(tmp, out)
    except OSError:
        if not os.path.exists(out):
            return tmp
        os.unlink(tmp)
    return out


def make_atmosphere_loop(kind, w, h, work_dir):
    """An ATMOS_SEC seamless loop (full size, with alpha) of a mood's atmosphere, laid over the
    clips of that mood: "embers" (warm sparks rising and twinkling), "snow" (flakes drifting
    down, the bigger ones faster), "rain" (thin slanted streaks falling fast) or "dust" (soft
    bokeh motes circling slowly). Kept subtle (sprites peak at 0.2..0.75 alpha and fall off
    softly, so they read at about a quarter to a third) and drawn at half size, then scaled: they
    are soft. Made once per size and kept in the fx cache. Returns (path, frames)."""
    from PIL import Image
    import random
    count = round(ATMOS_SEC * FPS)
    root = fx_cache_dir() or work_dir
    out = os.path.join(root, f"atmos_{kind}_{w}x{h}_v{ATMOS_VERSION}.mkv")
    if os.path.exists(out) and os.path.getsize(out) > 1000:
        return out, count
    sw, sh = max(64, w // 2), max(64, h // 2)
    unit = min(sw, sh)
    rng = random.Random(20 + (ATMOS_KINDS.index(kind) if kind in ATMOS_KINDS else 9))
    levels = 12
    motes = []

    def mote(sprite, alpha, **kw):
        # The sprite at `levels` brightnesses (picked per frame by its twinkle).
        a = sprite.getchannel("A")
        variants = []
        for i in range(levels):
            s = sprite.copy()
            s.putalpha(a.point(lambda v, f=alpha * (i + 1) / levels: int(v * f)))
            variants.append(s)
        m = {"x": rng.random(), "y": rng.random(), "dir": 0, "laps": 0, "sway": 0.0, "sway_n": 1, "orbit": None,
             "phase": rng.random(), "tw_n": 2, "tw": 0.0, "variants": variants}
        m.update(kw)
        motes.append(m)

    if kind == "embers":
        for _ in range(36):
            size = unit * rng.uniform(0.008, 0.018)
            color = rng.choice(((255, 140, 50), (255, 178, 90), (255, 210, 140)))
            mote(_glow_sprite((size * 2.4, size * 2.4), color, 1.0), rng.uniform(0.35, 0.7), dir=-1, laps=rng.choice((1, 1, 2)),
                 sway=rng.uniform(0.01, 0.04), sway_n=rng.choice((1, 2)), tw_n=rng.choice((3, 4, 5)), tw=0.45)
    elif kind == "snow":
        for _ in range(80):
            big = rng.random() < 0.3
            size = unit * (rng.uniform(0.009, 0.014) if big else rng.uniform(0.004, 0.008))
            mote(_glow_sprite((size * 2.2, size * 2.2), (255, 255, 255), 1.0), rng.uniform(0.45, 0.75) if big else rng.uniform(0.3, 0.55),
                 dir=1, laps=2 if big else 1, sway=rng.uniform(0.01, 0.03), sway_n=rng.choice((1, 2)), tw_n=2, tw=0.1)
    elif kind == "rain":
        for _ in range(110):
            streak = _glow_sprite((max(2.0, unit * 0.004), unit * rng.uniform(0.05, 0.09)), (205, 220, 240), 1.0)
            streak = streak.rotate(10, resample=Image.BICUBIC, expand=True)
            mote(streak, rng.uniform(0.22, 0.4), dir=1, laps=rng.choice((5, 6, 7, 8)))
    else:
        for _ in range(26):
            size = unit * rng.uniform(0.018, 0.045)
            mote(_glow_sprite((size * 2.0, size * 2.0), (255, 238, 210), 1.0), rng.uniform(0.16, 0.32),
                 orbit=(rng.uniform(0.01, 0.04), rng.uniform(0.01, 0.04), rng.choice((1, 2)), rng.choice((1, 2)), rng.random()),
                 tw_n=rng.choice((1, 2)), tw=0.5)

    folder = os.path.join(work_dir, f"fx_atmos_{kind}_{os.getpid()}_{threading.get_ident()}")
    os.makedirs(folder, exist_ok=True)

    def draw(i):
        u = i / count
        img = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
        for m in motes:
            # Whole laps / whole periods per loop, so it repeats seamlessly.
            if m["orbit"]:
                ax, ay, nx, ny, p2 = m["orbit"]
                x = m["x"] + ax * math.sin(2 * math.pi * (nx * u + m["phase"]))
                y = m["y"] + ay * math.sin(2 * math.pi * (ny * u + p2))
            else:
                y = (m["y"] + m["dir"] * m["laps"] * u) % 1.0
                x = (m["x"] + m["sway"] * math.sin(2 * math.pi * (m["sway_n"] * u + m["phase"]))) % 1.0
            tw = 1.0 - m["tw"] * (0.5 + 0.5 * math.sin(2 * math.pi * (m["tw_n"] * u + m["phase"])))
            spr = m["variants"][max(0, min(levels - 1, round(tw * levels) - 1))]
            # Drawn again across the edge it is leaving, so it never pops.
            for dy in (-1, 0, 1):
                py = (y + dy) * sh
                if -spr.height < py < sh + spr.height:
                    _paint(img, spr, x * sw, py)
        img.save(os.path.join(folder, f"a_{i:03d}.png"), compress_level=1)

    with ThreadPoolExecutor(max_workers=max(2, min(16, cpus()))) as pool:
        list(pool.map(draw, range(count)))
    return _encode_loop(folder, "a_%03d.png", w, h, out), count


def _prepare_duo_images(src, idx, w, h, slot, work_dir, glow=False, inset=0, drift=0, grade=None):
    """Stills for a duo clip: the panel at its drawn size for `slot` (between letterbox bars
    `inset` pixels high), and the blurred fill (`drift` pixels wider each side, for parallax),
    both through the mood `grade` (mood_grade's) when given."""
    from PIL import Image
    img = Image.open(src)
    img.load()
    img = img.convert("RGB")
    geo = duo_slot(img.size, w, h, slot, inset)
    fg_path = os.path.join(work_dir, f"dfg_{idx:04d}.jpg")
    fg = apply_grade(img.resize(geo["size"], Image.LANCZOS), grade)
    (_bloom(fg) if glow else fg).save(fg_path, "JPEG", quality=95)
    bg_path = os.path.join(work_dir, f"dbg_{idx:04d}{'_d' if drift else ''}.jpg")
    _blurred_fill(img, w + 2 * drift, h, grade).save(bg_path, "JPEG", quality=92)
    # The stills are the same for either half (only where they go differs): see duo_slot.
    return {"fg": fg_path, "bg": bg_path, "geo": geo, "drift": drift, "src_size": img.size}


def duo_slot(src_size, w, h, slot, inset=0):
    """duo_geometry for a panel of `src_size` in `slot`, between letterbox bars `inset` high."""
    geo = duo_geometry(src_size[0], src_size[1], w, h - 2 * inset, slot)
    geo["pos"] = (geo["pos"][0], geo["pos"][1] + inset)
    return geo


def panel_margins(w, h, fill, inset=0, margin=FG_MARGIN):
    """Space (x, y pixels) left around a single panel: a margin (`margin` of the frame) on the
    blurred fill, and room for letterbox bars `inset` pixels high so the panel sits whole between them."""
    mx, my = (round(w * margin), round(h * margin)) if fill == "blur" else (0, 0)
    if inset:
        my = max(my, inset + (round(h * 0.025) if fill == "blur" else 0))
    return mx, my


def _prepare_clip_images(src, idx, w, h, fill, work_dir, glow=False, inset=0, drift=0, grade=None, margin=FG_MARGIN):
    """Write this clip's stills: the upscaled panel and the frame-sized background it sits on.
    For the blurred fill the background is the panel zoomed in past "cover", blurred, darkened
    and desaturated, with the panel's soft drop shadow baked in; otherwise black. The
    background is composed once here; with `drift` (pixels) it is that much larger on every
    side, for a parallax drift. Both go through the mood `grade` when given (baked in, so it
    costs nothing per frame). Returns where everything goes."""
    from PIL import Image, ImageDraw, ImageFilter

    img = Image.open(src)
    img.load()
    img = img.convert("RGB")
    iw, ih = img.size
    blur = fill == "blur"
    drift = drift if blur else 0
    mx, my = panel_margins(w, h, fill, inset, margin)
    bw, bh = w - 2 * mx, h - 2 * my
    scale = min(bw / iw, bh / ih)
    fw = max(2, int(iw * scale) // 2 * 2)
    fh = max(2, int(ih * scale) // 2 * 2)
    fx = (w - fw) // 4 * 2
    fy = (h - fh) // 4 * 2

    fg_path = os.path.join(work_dir, f"fg_{idx:04d}.jpg")
    fg = apply_grade(img.resize((fw, fh), Image.LANCZOS), grade)
    (_bloom(fg) if glow else fg).save(fg_path, "JPEG", quality=95)

    bg_path = os.path.join(work_dir, f"bg_{idx:04d}{'_d' if drift else ''}.jpg")
    if blur:
        bg = _blurred_fill(img, w + 2 * drift, h + 2 * drift, grade)
        # Soft drop shadow where the panel sits.
        pad = max(8, round(min(w, h) * 0.04))
        mask = Image.new("L", (fw + 2 * pad, fh + 2 * pad), 0)
        ImageDraw.Draw(mask).rectangle([pad, pad, pad + fw - 1, pad + fh - 1], fill=210)
        mask = mask.filter(ImageFilter.GaussianBlur(radius=pad / 2.5))
        bg.paste((0, 0, 0), (drift + fx - pad, drift + fy - pad + max(2, round(h * 0.008))), mask)
    else:
        bg = Image.new("RGB", (w, h), (0, 0, 0))
    bg.save(bg_path, "JPEG", quality=92)

    return {
        "fg": fg_path, "fg_pos": (fx, fy), "fg_size": (fw, fh), "bg": bg_path, "drift": drift,
        "aspect": iw / ih, "box_aspect": bw / bh,
    }


def clip_key(src, focus, motion, frames, w, h, fill):
    """Short hash naming a rendered clip, so a re-run (after Pause / Resume) reuses the clips
    it already made and renders only what changed."""
    import hashlib
    try:
        st = os.stat(src)
        ident = [os.path.abspath(src), st.st_size, int(st.st_mtime)]
    except OSError:
        ident = [src]
    raw = json.dumps([CLIP_VERSION, ident, focus, motion, frames, w, h, fill], default=str)
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:12]


def _shake(amount, axis):
    """Overlay offset (pixels, of overlay time t) for a camera jolt that dies out in IMPACT_SEC."""
    if amount <= 0:
        return "0"
    f = "sin" if axis == "x" else "cos"
    return f"{amount}*{f}(2*PI*t*{13 if axis == 'x' else 11})*exp(-t*{round(3.5 / IMPACT_SEC, 2)})"


def _flash(impact):
    """A white flash fading into the picture at an action beat."""
    return f",eq=brightness='0.35*pow(max(0,1-t/{IMPACT_SEC}),2)':eval=frame" if impact else ""


def _encode_clip(inputs, graph, frames, out_path):
    tmp = out_path + ".part.mp4"
    threads = ffmpeg_threads(render_jobs())
    # Each input's decoder held to this run's share of the cores too (left alone, every one starts
    # a thread per core, and buffers a frame for each: the atmosphere's 1440p frames add up).
    capped = []
    for arg in inputs:
        if arg == "-i":
            capped += ["-threads", str(threads)]
        capped.append(arg)

    def run(codec):
        cmd = [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-filter_complex_threads", str(threads),
            *capped,
            "-filter_complex", ";".join(graph),
            "-map", "[v]", "-frames:v", str(frames), "-r", str(FPS),
            *codec, "-an", tmp,
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        return result.returncode == 0 and os.path.exists(tmp), result.stderr

    ok, stderr = run_codecs(clip_codecs(threads), run)
    if not ok:
        raise RuntimeError(f"clip render failed: {stderr[:300]}")
    # On disk before it gets its name: a power cut right after the rename otherwise leaves a
    # clip of the right size full of nothing, which the next run would reuse (clip_usable).
    with open(tmp, "rb+") as f:
        os.fsync(f.fileno())
    os.replace(tmp, out_path)


def clip_usable(path, frames):
    """Whether a clip an earlier run left can be reused: it opens and holds `frames` frames.
    (A clip cut short by a crash or a power cut is rendered again instead of failing the
    finish.) Without ffprobe, any non-empty file counts."""
    if not os.path.exists(path) or os.path.getsize(path) <= 100:
        return False
    try:
        r = subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=nb_frames", "-of", "csv=p=0", path],
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return True
    if r.returncode != 0 or r.stderr.strip():
        return False
    try:
        return int(r.stdout.strip().splitlines()[0]) >= frames
    except (ValueError, IndexError):
        return False


# Stills (full-range JPEG) are turned into video (limited) range once, before they are held,
# so every clip, the grade, the colour presets and the letterbox black agree on what black is.
TV = "scale=out_range=tv,"


def _hold(frames):
    """Filters that hold a still for `frames` frames on an exact 1/FPS clock (a still's own
    25 fps timebase would round N/FPS so that frames collide and the output stutters)."""
    return f"loop=loop={max(0, frames - 1)}:size=1:start=0,settb=1/{FPS},setpts=N"


# Which way the blurred background drifts (x, y; +1 = its content moves left / up) under each
# panel move: the same way the panel's picture goes, slower, so the panel floats over it.
PARALLAX_DIRS = {
    "pan_right": (1, 0), "pan_left": (-1, 0), "pan_down": (0, 1), "pan_up": (0, -1),
    "push": (0.7, -0.5), "pull": (-0.7, 0.5), "still": (0.6, 0),
}


def _window(graph, src, a, b, total, chain, tag):
    """Apply filter `chain` to frames [a, b) of [src] (`total` frames) only, labelled [tag]:
    the stream is cut there and joined back, so the frames outside cost nothing. Inside the
    chain t and the frame counters start at 0."""
    a, b = max(0, min(a, total)), max(0, min(b, total))
    if b <= a:
        return src
    pieces = [p for p in ((0, a, None), (a, b, chain), (b, total, None)) if p[1] > p[0]]
    if len(pieces) == 1:
        graph.append(f"[{src}]{chain}[{tag}]")
        return tag
    graph.append(f"[{src}]split={len(pieces)}" + "".join(f"[{tag}s{i}]" for i in range(len(pieces))))
    for i, (s, e, ch) in enumerate(pieces):
        graph.append(f"[{tag}s{i}]trim=start_frame={s}:end_frame={e},setpts=PTS-STARTPTS{',' + ch if ch else ''}[{tag}p{i}]")
    graph.append("".join(f"[{tag}p{i}]" for i in range(len(pieces))) + f"concat=n={len(pieces)}:v=1:a=0[{tag}]")
    return tag


def _zoom_blur(n, dur, w, out):
    """Filters for the zoom-blur transition over `n` frames: the old page rushes forward and
    blurs (`out`), or the new one lands out of a blur over the first `dur` seconds and more."""
    span = max(1, round(dur * FPS))
    p = f"min(1,on/{span})"
    z = f"(1+{ZOOM_BLUR}*pow({p},2))" if out else f"(1+{ZOOM_BLUR}*pow(1-{p},3))"
    lo, hi = f"(W-W/{z})/2", f"(W+W/{z})/2"
    top, bot = f"(H-H/{z})/2", f"(H+H/{z})/2"
    chain = [
        f"perspective=x0='{lo}':y0='{top}':x1='{hi}':y1='{top}':x2='{lo}':y2='{bot}':x3='{hi}':y3='{bot}':"
        "interpolation=linear:eval=frame"
    ]
    s = w / 1280
    steps = ((0.3, 0.55, 2), (0.55, 0.8, 5), (0.8, 99, 10)) if out else ((0, 0.25, 10), (0.25, 0.5, 5), (0.5, 0.75, 2))
    for a, b, sigma in steps:
        chain.append(f"gblur=sigma={sigma * s:.1f}:enable='gte(t,{a * dur:.3f})*lt(t,{b * dur:.3f})'")
    return ",".join(chain)


def _chroma(w, span=IMPACT_SEC, strength=1.0, grow=False):
    """RGB split for an action beat: red and blue torn apart at the hit, settling back as the
    punch dies out over `span` seconds (only its frames go through RGB). `grow` tears them
    apart towards the end instead (the clip a glitch leaves)."""
    s = max(2, round(w * 0.006 * strength))
    steps = ((0, 0.5, 0.3), (0.5, 0.8, 0.6), (0.8, 99, 1.0)) if grow else ((0, 0.12, 1.0), (0.12, 0.24, 0.6), (0.24, 99, 0.3))
    chain = ["format=gbrp"]
    for a, b, k in steps:
        amt = max(1, round(s * k))
        chain.append(
            f"rgbashift=rh=-{amt}:bh={amt}:rv={-max(0, amt // 3)}:bv={max(0, amt // 3)}:"
            f"enable='gte(t,{a * span:.3f})*lt(t,{b * span:.3f})'"
        )
    chain.append("format=yuv420p")
    return ",".join(chain)


def _glitch(w, span, strength, out):
    """A glitch over `span` seconds: a hard RGB tear with noise (growing into the cut on the
    clip it leaves, dying out on the one it lands on)."""
    return _chroma(w, span, max(0.5, strength) * 2.2, grow=out) + ",noise=alls=18:allf=t+u"


def _whip_blur(name, w, h):
    """Motion blur along a whip's direction (frames under the slide only)."""
    if name in ("whipleft", "whipright"):
        return f"avgblur=sizeX={max(2, round(w * 0.012))}:sizeY=1"
    return f"avgblur=sizeX=1:sizeY={max(2, round(h * 0.02))}"


def _finish_clip(clip, frames, w, h, inputs, graph, cur):
    """The clip's own effects over its composed picture [cur], ending in [v]: the zoom blur and
    film burn of a page change (on both clips around it), the RGB split of an action beat and
    the light leak of a new page. Each touches only the frames it shows on."""
    fx = clip.get("fx") or {}
    assets = clip.get("assets") or {}
    if "zoomblur_out" in fx:
        t0, dur = fx["zoomblur_out"]
        a = round(t0 * FPS)
        cur = _window(graph, cur, a, frames, frames, _zoom_blur(frames - a, dur, w, True), "zo")
    if "zoomblur_in" in fx:
        dur = fx["zoomblur_in"]
        n = max(2, round(dur * 1.6 * FPS))
        cur = _window(graph, cur, 0, n, frames, _zoom_blur(n, dur * 1.6, w, False), "zi")
    ck = clip.get("chroma_k", 1.0)
    if "glitch_out" in fx:
        t0, dur = fx["glitch_out"]
        cur = _window(graph, cur, round(t0 * FPS), frames, frames, _glitch(w, dur, ck, True), "go")
    if "glitch_in" in fx:
        dur = fx["glitch_in"] * 1.5
        cur = _window(graph, cur, 0, max(2, round(dur * FPS)), frames, _glitch(w, dur, ck, False), "gi")
    for whip in ("whipleft", "whipright", "whipup", "whipdown"):
        if f"{whip}_out" in fx:
            t0, dur = fx[f"{whip}_out"]
            a = round(t0 * FPS)
            cur = _window(graph, cur, a, a + max(1, round(dur * FPS)), frames, _whip_blur(whip, w, h), "wo")
        if f"{whip}_in" in fx:
            cur = _window(graph, cur, 0, max(1, round(fx[f"{whip}_in"] * FPS)), frames, _whip_blur(whip, w, h), "wi")
    if clip.get("impact") and clip.get("chroma") and ck > 0:
        cur = _window(graph, cur, 0, round(IMPACT_SEC * FPS), frames, _chroma(w, strength=ck), "ca")
    atmos = clip.get("atmos")
    if atmos:
        # The mood's particles (make_atmosphere_loop), from where the loop is at this clip's first
        # frame on the whole video's clock, so two clips with the same ones match under a join.
        path, count, at = atmos
        # Seeking (exact: every frame is a keyframe) rather than decoding up to it; the loop then
        # wraps round to its start.
        seek = ["-ss", f"{(at % count - 0.5) / FPS:.6f}"] if at % count else []
        inputs.extend(["-stream_loop", "-1", *seek, "-i", path])
        idx = inputs.count("-i") - 1
        # Tagged like the clip's own picture (BT.601, limited range: the stills' conversion), so
        # the overlay takes it as it is instead of converting every frame.
        graph.append(f"[{idx}:v]setparams=colorspace=bt470bg:range=tv,settb=1/{FPS},setpts=N[atm]")
        graph.append(f"[{cur}][atm]overlay=0:0:format=yuv420[atmo]")
        cur = "atmo"
    flares = []
    if "burn_out" in fx and assets.get("burn"):
        t0, dur = fx["burn_out"]
        flares.append((assets["burn"], t0 + dur / 2 - BURN_PEAK, fx.get("burn_flip", False)))
    if "burn_in" in fx and assets.get("burn"):
        flares.append((assets["burn"], fx["burn_in"] / 2 - BURN_PEAK, fx.get("burn_flip_in", False)))
    if fx.get("leak") and assets.get("leak"):
        flares.append((assets["leak"], 0.0, False))
    for i, ((pattern, count), start, flip) in enumerate(flares):
        first = max(0, math.ceil(-start * FPS - 1e-6))
        offset = max(0.0, start)
        if first >= count or round(offset * FPS) >= frames:
            continue
        inputs.extend(["-framerate", str(FPS), "-start_number", str(first), "-i", pattern])
        idx = inputs.count("-i") - 1
        graph.append(
            f"[{idx}:v]{'hflip,' if flip else ''}scale={w}:{h}:flags=bicubic,format=yuva420p,"
            f"settb=1/{FPS},setpts=N+{round(offset * FPS)}[fl{i}]"
        )
        graph.append(f"[{cur}][fl{i}]overlay=0:0:eof_action=pass:format=yuv420[fo{i}]")
        cur = f"fo{i}"
    graph.append(f"[{cur}]null[v]")


# ── Framing: nothing narrated is ever cut ──
#
# Every clip has a must-see region (fractions of the picture it shows) that stays whole on
# screen for the whole beat: a panel crop's own panel (less PANEL_TRIM each side: the crop's
# hair of padding and the panel's border) grown to every character's face in it (FACE_PAD of
# the face each side, for the head round it); for a beat framed on the AI's focus box, that box
# snapped out to the panel it sits in and that panel's faces (padded REGION_PAD). Moves are
# planned inside it (clip_camera) and checked afterwards (framing_worst); a clip that still
# loses any of it is re-planned (frame_clip).
PANEL_TRIM = 0.03
FACE_PAD = 0.25
REGION_PAD = 0.04
# A focus box keeps to itself (grown to the faces it touches) only inside a big panel (at least
# SPLASH_AREA of the page) this many times its area; otherwise it snaps out to the whole panel.
FOCUS_SNAP = 6.0
SPLASH_AREA = 0.5
# How much of the region must stay visible (1 = all of it, less float noise).
FULL_VIEW = 0.999
# A studio panel crop: p<page>_<panel>.jpg (mv-recap read.rs crop_panel).
PANEL_FILE = re.compile(r"(?:^|[\\/])p(\d+)_(\d+)\.\w+$", re.I)


def _box(v):
    """(x0, y0, x1, y1) of a read.json box (fractions), or None when it is not one."""
    try:
        b = tuple(float(x) for x in v)
    except (TypeError, ValueError):
        return None
    return b if len(b) == 4 and b[2] > b[0] and b[3] > b[1] else None


def _union(a, b):
    return (min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3]))


def _clamp_box(b):
    return tuple(min(1.0, max(0.0, v)) for v in b)


def _grow(b, k):
    """`b` grown by `k` of its size on every side."""
    w, h = b[2] - b[0], b[3] - b[1]
    return (b[0] - w * k, b[1] - h * k, b[2] + w * k, b[3] + h * k)


def _area(b):
    return max(0.0, b[2] - b[0]) * max(0.0, b[3] - b[1])


def _meet(a, b):
    return (max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3]))


def load_panels(output_dir):
    """{page index: [panel {bbox, characters: [{face}]}, ...]} from <output_dir>/studio/read.json
    (boxes are x0, y0, x1, y1 fractions of the page), or {}."""
    try:
        with open(os.path.join(output_dir, "studio", "read.json"), "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    out = {}
    for i, p in enumerate(data.get("pages") or [] if isinstance(data, dict) else []):
        if isinstance(p, dict) and isinstance(p.get("panels"), list) and isinstance(p.get("index", i), int):
            out[p.get("index", i)] = [x if isinstance(x, dict) else {} for x in p["panels"]]
    return out


def beat_panel(seg, panels):
    """The read.json panel a beat shows (its studio crop p<page>_<n>.jpg), or None."""
    m = PANEL_FILE.search(str(seg.get("panel") or seg.get("panel_image") or ""))
    if not m:
        return None
    page, n = int(m.group(1)), int(m.group(2))
    lst = panels.get(page) or []
    return lst[n] if n < len(lst) else None


def panel_faces(panel):
    """The face boxes (page fractions) of a read.json panel's characters."""
    faces = []
    for c in (panel or {}).get("characters") or []:
        b = _box(c.get("face")) if isinstance(c, dict) else None
        if b:
            faces.append(b)
    return faces


def crop_region(panel):
    """The must-see region of a beat that shows a studio panel crop, in fractions of the crop:
    the panel less PANEL_TRIM each side, grown to its faces (mapped through the crop's 1%
    padding, read.rs crop_panel) with FACE_PAD round each."""
    region = (PANEL_TRIM, PANEL_TRIM, 1 - PANEL_TRIM, 1 - PANEL_TRIM)
    b = _box((panel or {}).get("bbox"))
    if b:
        px, py = (b[2] - b[0]) * 0.01, (b[3] - b[1]) * 0.01
        c = _clamp_box((b[0] - px, b[1] - py, b[2] + px, b[3] + py))
        cw, ch = c[2] - c[0], c[3] - c[1]
        for f in panel_faces(panel):
            f = _grow(f, FACE_PAD)
            region = _union(region, ((f[0] - c[0]) / cw, (f[1] - c[1]) / ch, (f[2] - c[0]) / cw, (f[3] - c[1]) / ch))
    return _clamp_box(region)


def focus_region(focus_point, page_panels):
    """The page area (fractions) a beat framed on the AI's focus box (ymin, xmin, ymax, xmax of
    0..1000) must show whole: the box snapped out to the panel it overlaps most and that panel's
    faces, padded REGION_PAD; only inside a splash panel (SPLASH_AREA of the page, over FOCUS_SNAP
    times its area) it keeps to the box, grown to every face it touches. Without panels the box
    gets a wide margin instead."""
    ymin, xmin, ymax, xmax = (float(v) / 1000 for v in focus_point)
    f = _clamp_box((xmin, ymin, xmax, ymax))
    best, overlap = None, 0.0
    for p in page_panels or []:
        b = _box(p.get("bbox"))
        if b and _area(_meet(b, f)) > overlap:
            best, overlap = p, _area(_meet(b, f))
    if best is None:
        return _clamp_box(_grow(f, 0.25))
    b = _box(best["bbox"])
    region = f
    if _area(b) < SPLASH_AREA or _area(b) <= FOCUS_SNAP * max(1e-6, _area(f)):
        region = _union(f, b)
        for face in panel_faces(best):
            region = _union(region, _grow(face, FACE_PAD))
    else:
        for face in panel_faces(best):
            g = _grow(face, FACE_PAD)
            if _area(_meet(g, f)) > 0:
                region = _union(region, g)
    return _clamp_box(_grow(region, REGION_PAD))


def crop_to_region(page_image, region, aspect, out_path):
    """Crop `page_image` to `region` (page fractions), only ever widened towards `aspect` (w/h),
    never cut into. Returns (the crop's path, the region in fractions of the crop)."""
    from PIL import Image
    img = Image.open(page_image)
    iw, ih = img.size
    x0, y0, x1, y1 = region[0] * iw, region[1] * ih, region[2] * iw, region[3] * ih
    bw, bh = max(1.0, x1 - x0), max(1.0, y1 - y0)
    if bw / bh < aspect:
        grow = max(0.0, min(iw, bh * aspect) - bw)
        x0, x1 = x0 - grow / 2, x1 + grow / 2
    else:
        grow = max(0.0, min(ih, bw / aspect) - bh)
        y0, y1 = y0 - grow / 2, y1 + grow / 2
    # Slid back inside the page where widening ran over an edge.
    if x0 < 0:
        x0, x1 = 0, min(iw, x1 - x0)
    if x1 > iw:
        x0, x1 = max(0, x0 - (x1 - iw)), iw
    if y0 < 0:
        y0, y1 = 0, min(ih, y1 - y0)
    if y1 > ih:
        y0, y1 = max(0, y0 - (y1 - ih)), ih
    box = (int(x0), int(y0), max(int(x0) + 1, math.ceil(x1)), max(int(y0) + 1, math.ceil(y1)))
    img.convert("RGB").crop(box).save(out_path, "JPEG", quality=95)
    cw, ch = box[2] - box[0], box[3] - box[1]
    inner = ((region[0] * iw - box[0]) / cw, (region[1] * ih - box[1]) / ch,
             (region[2] * iw - box[0]) / cw, (region[3] * ih - box[1]) / ch)
    return out_path, _clamp_box(inner)


def region_zoom_max(region):
    """The most the camera may zoom into a picture and still show `region` whole."""
    return 1.0 / max(1e-6, region[2] - region[0], region[3] - region[1])


def region_window(r0, r1, z):
    """(lo, hi): where the window (a fraction of the slack, as FG_WINDOW) may sit along one axis
    at zoom `z` with [r0, r1] of that axis in view; the middle of it when it cannot be."""
    a = 1 - 1 / z
    if a <= 1e-9:
        return 0.0, 1.0
    lo, hi = max(0.0, 1 - (1 - r1) / a), min(1.0, r0 / a)
    if lo > hi:
        mid = min(1.0, max(0.0, ((r0 + r1) / 2 - 1 / (2 * z)) / a))
        return mid, mid
    return lo, hi


def _centre(r0, r1, z):
    lo, hi = region_window(r0, r1, z)
    a = 1 - 1 / z
    p = 0.5 if a <= 1e-9 else ((r0 + r1) / 2 - 1 / (2 * z)) / a
    return min(hi, max(lo, p))


def framed_window(motion, region, z):
    """FG_WINDOW for an auto move at (its largest) zoom `z` kept on `region`: a push, pull,
    punch or still centred on it (as near as its window allows), a pan travelling only the part
    of the slack that keeps it in view. A window that holds the region at the largest zoom holds
    it at every smaller one, so this holds for the whole move."""
    cx, cy = _centre(region[0], region[2], z), _centre(region[1], region[3], z)
    axis = {"pan_right": 0, "pan_left": 0, "pan_down": 1, "pan_up": 1}.get(motion)
    if axis is None:
        return (f"{cx:.4f}", f"{cy:.4f}")
    lo, hi = region_window(region[axis], region[axis + 2], z)
    if motion in ("pan_left", "pan_up"):
        lo, hi = hi, lo
    path = f"({lo:.4f}+{hi - lo:.4f}*{{e}})"
    return (path, f"{cy:.4f}") if axis == 0 else (f"{cx:.4f}", path)


def camera_at(clip, frames, on):
    """(zoom, x, y) of clip_camera at frame `on`, worked out from the very expressions the
    render uses."""
    zoom, window, ease = clip_camera(clip, frames)
    env = {"__builtins__": {}, "min": min, "max": max, "pow": pow, "cos": math.cos, "sin": math.sin,
           "exp": math.exp, "PI": math.pi, "on": on}
    e = f"({eval(ease, env)!r})"
    return tuple(eval(x.format(e=e), env) for x in (zoom, *window))


def visible_share(region, z, px, py):
    """The share of `region` the window at zoom `z` and position (px, py) shows."""
    v = 1 / max(1.0, z)
    x0, y0 = px * (1 - v), py * (1 - v)
    return _area(_meet(region, (x0, y0, x0 + v, y0 + v))) / max(1e-12, _area(region))


def framing_worst(clip, frames):
    """The least of the clip's must-see region on screen at any moment of it (geometry only):
    a duo panel shows its visible rows of its height, a single panel what the camera's window
    shows, checked every few frames and at both ends."""
    duo = clip.get("duo")
    if duo:
        g = duo["geo"]
        return 1.0 if g["slack"] <= 0 else g["vis"] / g["size"][1]
    region = clip.get("region") or (0.0, 0.0, 1.0, 1.0)
    step = max(1, frames // 90)
    worst = 1.0
    for on in sorted(set(range(0, frames, step)) | {frames - 1}):
        z, px, py = camera_at(clip, frames, on)
        worst = min(worst, visible_share(region, z, px, py))
    return worst


def frame_clip(clip, frames):
    """The framing QA of one clip: its worst coverage (framing_worst) and, when it loses any of
    its region, the safest re-plan: a push centred on the region, else a still panel without the
    punch zoom. The editor's own moves are left as picked (and reported). Returns the record
    written to video_framing.json."""
    worst = round(framing_worst(clip, frames), 4)
    rec = {"layout": "duo" if clip.get("duo") else "single", "region": [round(v, 4) for v in clip.get("region") or ()],
           "motion": clip.get("motion"), "worst": worst, "fix": None}
    if worst >= FULL_VIEW or clip.get("duo"):
        return rec
    if clip.get("move"):
        rec["fix"] = "editor move kept"
        return rec
    clip["motion"] = "push"
    rec["fix"] = "push centred on the region"
    worst = framing_worst(clip, frames)
    if worst < FULL_VIEW:
        clip["motion"] = "still"
        clip["impact"] = False
        rec["fix"] = "still, no punch zoom"
        worst = framing_worst(clip, frames)
    rec["worst_after"] = round(worst, 4)
    return rec


def clip_camera(clip, frames):
    """The single-panel camera of a clip `frames` frames long: (zoom, window, ease). The zoom is
    an ffmpeg expression of the eased progress {e} and the frame counter `on`; the window is the
    (x, y) fractions of the slack where the visible 1/zoom rectangle sits; ease is the progress
    expression. The Video editor's preview computes the same numbers (ui-studio vedit_frame.rs);
    tests/fixtures/frame_transforms.json pins both.

    `clip`: motion (an auto move of FG_WINDOW, "still" or "edit"), move (edit_move's), impact,
    ramp, push (the look's), motion_k, punch and region: the must-see box (x0, y0, x1, y1
    fractions of the panel, see the Framing section). With a region the look's own moves never
    cut it: the zoom is capped to what still shows it whole (region_zoom_max), pushes, pulls and
    the punch are centred on it and pans travel only as far as it stays in view (framed_window)."""
    motion = clip["motion"]
    move = clip.get("move")
    ramp = clip.get("ramp", False)
    # Short clips move proportionally less, so a half-second beat never lurches; with the speed
    # ramp a short beat snaps through more of the move instead, and quickly.
    k = min(1.0, frames / (3.0 * FPS))
    if ramp and frames < 2 * FPS:
        k = max(k, 0.7)
    push = round(clip.get("push", FG_PUSH) * k, 4)
    pan_zoom = round(1 + (FG_PAN_ZOOM - 1) * k * clip.get("motion_k", 1.0), 4)
    region = clip.get("region") if not move else None
    zmax = region_zoom_max(region) if region else None
    if zmax:
        push = round(max(0.0, min(push, zmax - 1)), 4)
        pan_zoom = round(max(1.0, min(pan_zoom, zmax)), 4)
    fg_zoom = _motion_zoom(motion, push, pan_zoom)
    window = FG_WINDOW.get(motion, FG_WINDOW["still"])
    if zmax:
        window = framed_window(motion, region, pan_zoom if motion.startswith("pan_") else 1 + push)
    if move:
        # The editor's own move (its amount as given: the editor chose it).
        fg_zoom, window = edit_move(move, push)
    # The punch zoom of an action beat (the editor's shake amount, else 12%).
    if clip.get("impact") and motion != "still":
        n = max(2, round(IMPACT_SEC * FPS * 0.6))
        punch = clip.get("punch", 0.12)
        window = FG_WINDOW["push"]
        if zmax:
            punch = round(max(0.0, min(punch, zmax - 1)), 4)
            window = framed_window("push", region, 1 + max(punch, push))
        fg_zoom = f"1+{punch}*pow(1-min(1,on/{n}),3)+{push}*{{e}}"
    return fg_zoom, window, ramp_ease("on", frames, ramp)


def _render_clip(clip, frames, w, h, out_path):
    """Render one clip's video (no audio): the still background with the Ken Burns panel on it.
    An action beat ("punch") snaps in from a zoom, jolts and flashes white. With parallax the
    background drifts the panel's way, slower; with "ramp" the move is timed to the beat."""
    if clip.get("duo"):
        return _render_duo_clip(clip, frames, w, h, out_path)
    imgs = clip["images"]
    motion = clip["motion"]
    move = clip.get("move")
    impact = clip.get("impact", False)
    ramp = clip.get("ramp", False)
    fg_zoom, window, ease = clip_camera(clip, frames)
    k = min(1.0, frames / (3.0 * FPS))
    if ramp and frames < 2 * FPS:
        k = max(k, 0.7)
    punch = clip.get("punch", 0.12)
    fx, fy = imgs["fg_pos"]
    jolt = round(h * 0.1 * punch) if impact else 0
    bg = f"[0:v]{TV}format=yuv420p,setsar=1,{_hold(frames)}"
    drift = imgs.get("drift", 0)
    if drift:
        dx, dy = move_dirs(move) if move else PARALLAX_DIRS.get(motion, (0.6, 0))
        e = f"(2*{ramp_ease('n', frames, ramp)}-1)"
        bg += f",crop={w}:{h}:x='{drift}+{round(dx * drift * k, 2)}*{e}':y='{drift}+{round(dy * drift * k, 2)}*{e}'"
    # A layered clip's panel is a moving picture (layered_motion.py); otherwise the still.
    anim = imgs.get("fg_anim")
    inputs = ["-i", imgs["bg"], "-i", anim or imgs["fg"]]
    graph = [
        f"{bg}[bg]",
        f"[1:v]{TV}format=yuv420p,{_warp(window, fg_zoom, frames, 'cubic', ease, held=not anim)},setsar=1[fg]",
        f"[bg][fg]overlay=x='{fx}+{_shake(jolt, 'x')}+{_panel_move(clip.get('fx') or {}, w, 'x')}':y='{fy}+{_shake(jolt, 'y')}':eval=frame{_flash(impact)},format=yuv420p[base]",
    ]
    _finish_clip(clip, frames, w, h, inputs, graph, "base")
    _encode_clip(inputs, graph, frames, out_path)


def _render_duo_clip(clip, frames, w, h, out_path):
    """Render a duo clip: two tall panels side by side over a blurred fill. The new panel slides
    and fades into its half (panning down if it is taller than the frame) while the background
    turns into its blur; the other half keeps the previous panel where it stopped. A "hold"
    (the same panel again) shows it still; an action beat arrives faster, jolts and flashes.
    With parallax the background drifts the way the panel slid in, slower, and turns back
    with the next one (the old background stays where it stopped while it fades)."""
    d = clip["duo"]
    impact = clip.get("impact", False)
    loop = _hold(frames)
    inputs, graph = [], []

    def add(path):
        inputs.extend(["-i", path])
        return inputs.count("-i") - 1

    hold = d.get("hold", False)
    move = max(2, d.get("move_frames", frames))
    drift = d.get("drift", 0)
    pan = ""
    if drift:
        # x(e) = drift - dir * drift * (2e - 1): dir +1 (left slot) slides content right.
        sign = d.get("bg_dir", 1)
        if hold:
            pan = f",crop={w}:{h}:{drift - sign * drift}:0"
        else:
            e = f"(0.5-0.5*cos(PI*min(n,{move - 1})/{move - 1}))"
            pan = f",crop={w}:{h}:x='{drift}-{sign * drift}*(2*{e}-1)':y=0"
    b = add(d["bg"])
    if d.get("bg_prev") and not hold:
        p = add(d["bg_prev"])
        prev_pan = f",crop={w}:{h}:{drift - d.get('bg_prev_dir', 1) * drift}:0" if drift else ""
        # The old background fades out over the new one, blended only while it shows.
        graph.append(f"[{b}:v]{TV}format=yuv420p,setsar=1,{loop}{pan}[b1]")
        graph.append(f"[{p}:v]{TV}format=yuva420p,setsar=1,{loop}{prev_pan},fade=t=out:st=0:d={DUO_BG_FADE}:alpha=1[b0]")
        graph.append(f"[b1][b0]overlay=0:0:enable='lt(t,{DUO_BG_FADE})'[bg]")
    else:
        graph.append(f"[{b}:v]{TV}format=yuv420p,setsar=1,{loop}{pan}[bg]")
    cur = "bg"
    jolt = round(h * 0.012) if impact else 0
    sx, sy = _shake(jolt, "x"), _shake(jolt, "y")

    partner = d.get("partner")
    if partner:
        g = partner["geo"]
        i = add(partner["fg"])
        dw, _ = g["size"]
        graph.append(f"[{i}:v]{TV}format=yuv420p,setsar=1,crop={dw}:{g['vis']}:0:{g['slack']},{loop}[pp]")
        px, py = g["pos"]
        graph.append(f"[{cur}][pp]overlay=x='{px}+{sx}':y='{py}+{sy}':eval=frame[c1]")
        cur = "c1"

    g = d["geo"]
    dw, _ = g["size"]
    x0, y0 = g["pos"]
    i = add(d["fg"])
    if hold or g["slack"] <= 0:
        fg = f"[{i}:v]{TV}format=yuv420p,setsar=1,crop={dw}:{g['vis']}:0:{g['slack'] if hold else 0},{loop}"
    else:
        e = f"(0.5-0.5*cos(PI*min(on,{move - 1})/{move - 1}))"
        y = f"{g['slack']}*{e}"
        fg = (
            f"[{i}:v]{TV}format=yuv420p,setsar=1,{loop},"
            f"perspective=x0=0:y0='{y}':x1=W:y1='{y}':x2=0:y2='{y}+H':x3=W:y3='{y}+H':interpolation=cubic:eval=frame,"
            f"crop={dw}:{g['vis']}:0:0"
        )
    if hold:
        graph.append(f"{fg}[fg]")
        graph.append(f"[{cur}][fg]overlay={x0}:{y0}[c2]")
    else:
        slide = DUO_SLIDE * (0.55 if impact else 1.0)
        dist = round(w * DUO_SLIDE_DIST * (1.6 if impact else 1.0)) * (-1 if d["slot"] == 0 else 1)
        graph.append(f"{fg},format=yuva420p,fade=t=in:st=0:d={round(slide * 0.7, 3)}:alpha=1[fg]")
        graph.append(
            f"[{cur}][fg]overlay=x='{x0}+({dist})*pow(1-min(1,t/{slide}),3)+{sx}':y='{y0}+{sy}':eval=frame[c2]"
        )
    graph.append(f"[c2]format=yuv420p{_flash(impact)}[base]")
    _finish_clip(clip, frames, w, h, inputs, graph, "base")
    _encode_clip(inputs, graph, frames, out_path)


def run_ffmpeg_progress(cmd, seconds, on_fraction=None, timeout=7200):
    """Run an ffmpeg command, calling on_fraction(0..1) as its output advances through
    `seconds` of media (from ffmpeg's -progress report). Returns (returncode, stderr)."""
    import tempfile
    import threading
    cmd = [cmd[0], "-progress", "pipe:1", "-nostats", *cmd[1:]]
    err = tempfile.TemporaryFile()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=err, text=True)
    timer = threading.Timer(timeout, proc.kill)
    timer.start()
    try:
        for line in proc.stdout:
            key, _, value = line.strip().partition("=")
            if key == "out_time_us" and on_fraction and seconds > 0:
                try:
                    on_fraction(min(1.0, max(0.0, int(value) / 1e6 / seconds)))
                except ValueError:
                    pass
        proc.wait()
    finally:
        timer.cancel()
    err.seek(0)
    stderr = err.read().decode("utf-8", "replace")
    err.close()
    return proc.returncode, stderr


def _env_int(name):
    try:
        v = int(os.environ.get(name, ""))
        return v if v > 0 else None
    except ValueError:
        return None


def cpus():
    """Cores heavy work may use: the app's resource limit share (MANGAVOX_THREADS, Settings ->
    System), else all of them, so the app stays usable while a video renders."""
    return _env_int("MANGAVOX_THREADS") or os.cpu_count() or 4


def render_jobs():
    """ffmpeg runs at once while rendering clips: about one per three cores, 2 to 6
    (MANGAVOX_RENDER_JOBS overrides)."""
    return _env_int("MANGAVOX_RENDER_JOBS") or max(2, min(6, cpus() // 3))


def ffmpeg_threads(jobs):
    """Threads each of `jobs` ffmpeg runs side by side may use (MANGAVOX_FFMPEG_THREADS
    overrides), so together they fill the cores without swamping them and the PC stays usable."""
    return _env_int("MANGAVOX_FFMPEG_THREADS") or max(2, cpus() // max(1, jobs))


# Video encoders. NVENC (an NVIDIA GPU) when this ffmpeg has it and it works here, checked once
# per process; libx264 otherwise, or with MANGAVOX_VIDEO_GPU=0. A run that fails on NVENC turns
# it off for the rest of the process and is retried on the CPU.
_NVENC = None
_NVENC_LOCK = threading.Lock()


def nvenc_available():
    global _NVENC
    with _NVENC_LOCK:
        if _NVENC is None:
            if os.environ.get("MANGAVOX_VIDEO_GPU", "").strip().lower() in ("0", "off", "false", "no"):
                _NVENC = False
            else:
                try:
                    r = subprocess.run(
                        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=black:s=640x360:r=30:d=0.2",
                         "-c:v", "h264_nvenc", "-preset", "p4", "-f", "null", "-"],
                        capture_output=True, timeout=60,
                    )
                    _NVENC = r.returncode == 0
                except Exception:
                    _NVENC = False
        return _NVENC


def encoder_failed(codec):
    """A run failed with `codec` (args from clip_codecs / final_codecs) and worked on the CPU:
    stop using NVENC. (A run that fails on the CPU too is broken in itself, not the encoder.)"""
    global _NVENC
    if "h264_nvenc" in codec:
        with _NVENC_LOCK:
            _NVENC = False


def run_codecs(codecs, run):
    """Calls run(codec) -> (ok, stderr) with each of `codecs` (clip_codecs / final_codecs) until
    one works; NVENC is turned off when it failed where the CPU then worked. Returns
    (ok, stderr of the last try)."""
    failed = None
    stderr = ""
    for codec in codecs:
        ok, stderr = run(codec)
        if ok:
            if failed:
                encoder_failed(failed)
            return True, stderr
        if "h264_nvenc" in codec:
            failed = codec
    return False, stderr


def clip_codecs(threads):
    """Encoder args to try in order for a clip (an intermediate, decoded once more by the
    finish): near-lossless on the GPU, else what the CPU path always used."""
    # A keyframe every second, so a part of the finish that starts inside a clip seeks to it.
    cpu = ["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-g", str(FPS), "-pix_fmt", "yuv420p", "-threads", str(threads)]
    gpu = ["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-rc", "constqp", "-qp", "14", "-bf", "0", "-g", str(FPS), "-pix_fmt", "yuv420p"]
    return [gpu, cpu] if nvenc_available() else [cpu]


def final_codecs(threads):
    """Encoder args to try in order for the finished picture."""
    cpu = ["-c:v", "libx264", "-preset", "fast", "-crf", "21", "-pix_fmt", "yuv420p", "-threads", str(threads)]
    # Constant quality with adaptive quantisation. 27 was measured against a near-lossless render
    # of the same chapter a little above libx264 -crf 21 (SSIM 0.9961 vs 0.9958) at about 1.3x the
    # size. It is 23 now (MANGAVOX_FINAL_CQ overrides): this file is not the last encode, the
    # export draws the watermark over it and YouTube encodes it again, so it keeps more than a
    # file that is only watched needs.
    cq = str(int(_num(os.environ.get("MANGAVOX_FINAL_CQ"), 23, 15, 35)))
    gpu = ["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", cq, "-b:v", "0",
           "-spatial-aq", "1", "-temporal-aq", "1", "-bf", "3", "-rc-lookahead", "20", "-pix_fmt", "yuv420p"]
    return [gpu, cpu] if nvenc_available() else [cpu]


def _build_narration(clips, work_dir, out_path):
    """Sample-exact narration track (48 kHz mono PCM): each clip's slice of its audio file,
    silence-padded if the file runs short, laid end to end on the rendered timeline."""
    sources = list(dict.fromkeys(c["audio_path"] for c in clips))
    converted = {src: os.path.join(work_dir, f"narr_{i:04d}.wav") for i, src in enumerate(sources)}
    # Convert every source to 48 kHz mono PCM, a batch of files per ffmpeg call.
    for b in range(0, len(sources), 24):
        batch = sources[b:b + 24]
        cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
        for src in batch:
            cmd += ["-i", src]
        for i, src in enumerate(batch):
            cmd += ["-map", f"{i}:a:0", "-ac", "1", "-ar", str(AUDIO_RATE), "-c:a", "pcm_s16le", converted[src]]
        subprocess.run(cmd, check=True, capture_output=True, timeout=600)
    with wave.open(out_path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(AUDIO_RATE)
        for c in clips:
            src = c["audio_path"]
            count = round((c["start"] + c["duration"]) * AUDIO_RATE) - round(c["start"] * AUDIO_RATE)
            with wave.open(converted[src], "rb") as wf:
                pos = min(wf.getnframes(), round(c["offset"] * AUDIO_RATE))
                wf.setpos(pos)
                data = wf.readframes(count)
            out.writeframes(data + b"\0\0" * (count - len(data) // 2))


def _retime_clips(clips, caption_segments, work_dir):
    """Clips the editor sped up or slowed down: their narration slice is retimed with atempo
    (the pitch kept) into a file of its own, which the narration track and the caption timing
    then read from its start. A slice that cannot be retimed plays as it is, cut to the clip."""
    for i, c in enumerate(clips):
        speed = c.get("speed", 1.0)
        if abs(speed - 1.0) < 1e-3:
            continue
        out = os.path.join(work_dir, f"retimed_{c['idx']:04d}.wav")
        cmd = [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
            "-ss", f"{c['offset']:.3f}", "-t", f"{c['src_duration']:.3f}", "-i", c["audio_path"],
            "-af", f"atempo={speed:.4f}", "-ac", "1", "-ar", str(AUDIO_RATE), "-c:a", "pcm_s16le", out,
        ]
        try:
            subprocess.run(cmd, check=True, capture_output=True, timeout=300)
        except Exception as e:
            print(f"[video] Could not retime clip {c['idx']}: {e}", file=sys.stderr)
            continue
        c["audio_path"], c["offset"] = out, 0.0
        if i < len(caption_segments):
            caption_segments[i]["audio_path"] = out
            caption_segments[i]["audio_offset_ms"] = 0


# Share of the video step's progress bar each stage takes (they add up to 1).
PROGRESS_SPLIT = {"prepare": 0.05, "clips": 0.6, "finalize": 0.35}


class _Progress:
    """Maps stage-local 0..1 fractions onto one 0..1000 bar for progress_cb("video", ...)."""

    def __init__(self, cb):
        self.cb = cb
        self.split = dict(PROGRESS_SPLIT)
        self.last = -1

    def __call__(self, stage, fraction):
        if not self.cb:
            return
        done = 0.0
        for name, share in self.split.items():
            if name == stage:
                done += share * min(1.0, max(0.0, fraction))
                break
            done += share
        value = min(1000, int(done * 1000))
        if value > self.last:
            self.last = value
            self.cb("video", value, 1000)


# Clip motions the Video editor can pick per clip (anything else: the look decides). All but
# shake are drawn by edit_move; shake is the action punch (zoom, jolt, white flash).
EDIT_MOTIONS = ("still", "zoom", "shake", "zoom_out", "pan_left", "pan_right", "pan_up", "pan_down", "ken_burns", "punch")
# Font a text overlay is drawn in (python/fonts).
OVERLAY_FONT = "Inter-ExtraBold.ttf"


def _num(v, default, lo, hi):
    try:
        x = float(v)
    except (TypeError, ValueError):
        return default
    return default if not math.isfinite(x) else min(hi, max(lo, x))


def apply_edit_clips(items, clips):
    """The render items of a Video editor edit: `clips` lists, in their new order, which item
    (`src`, an index into `items`) plays, trimmed by `trim_in_ms` / `trim_out_ms` off its audio
    slice, with an optional caption `text`, a `motion` from EDIT_MOTIONS with its `amount`
    (0..0.5) and `focus_from` / `focus_to` ([x, y] percent of the panel, kept as 0..1), a
    `speed` (0.5..2: the narration is retimed), and `cut` (a hard cut into it) or a
    `transition` {kind (EDIT_TRANSITIONS), ms}. Unknown sources and clips shorter than 0.1 s
    are dropped; unknown motions and transitions are ignored."""
    out = []
    for c in clips or []:
        if not isinstance(c, dict):
            continue
        try:
            src = int(c.get("src"))
        except (TypeError, ValueError):
            continue
        if not 0 <= src < len(items):
            continue
        seg = dict(items[src])
        dur = seg.get("duration_ms") or (int(seg.get("end_ms") or 0) - int(seg.get("start_ms") or 0))
        tin = int(_num(c.get("trim_in_ms"), 0, 0, 1e9))
        tout = int(_num(c.get("trim_out_ms"), 0, 0, 1e9))
        left = int(dur) - tin - tout
        speed = _num(c.get("speed"), 1.0, 0.5, 2.0)
        if left < 100 or left / speed < 100:
            continue
        seg["audio_offset_ms"] = int(seg.get("audio_offset_ms") or 0) + tin
        seg["duration_ms"] = left
        if speed != 1.0:
            seg["speed"] = speed
        if isinstance(c.get("text"), str):
            seg["text"] = c["text"]
        if c.get("motion") in EDIT_MOTIONS:
            seg["motion"] = c["motion"]
            amount = _num(c.get("amount"), None, 0.0, 0.5)
            if amount is not None:
                seg["amount"] = amount
            for key in ("focus_from", "focus_to"):
                f = c.get(key)
                if isinstance(f, (list, tuple)) and len(f) == 2:
                    seg[key] = (_num(f[0], 50.0, 0.0, 100.0) / 100.0, _num(f[1], 50.0, 0.0, 100.0) / 100.0)
        t = c.get("transition")
        if c.get("cut"):
            seg["cut"] = True
        elif isinstance(t, dict) and t.get("kind") in EDIT_TRANSITIONS:
            name, default = EDIT_TRANSITIONS[t["kind"]]
            seg["transition"] = (name, round(_num(t.get("ms"), default * 1000, 40, 3000) / 1000.0, 3))
        out.append(seg)
    return out


# The chapter badge: a small circle in the top right corner as a chapter starts, with the chapter's
# number in it and a thin ring around it that fills clockwise like a loading ring, so it is on
# screen for CARD_SHOW seconds and then gone (seconds on the chapter's clock). Nothing else: no
# word, no title. The look's "chapter_card" switch (Studio's Look) turns it off. A full recap
# joins the chapters as they are, so each chapter change carries its own.
CARD_IN = 0.35
CARD_SHOW = 6.0
CARD_OUT = CARD_IN + CARD_SHOW
# It pops in over CARD_POP (fading up while growing from CARD_START_SCALE) and fades over the last CARD_FADE.
CARD_POP = 0.3
CARD_FADE = 0.35
CARD_START_SCALE = 0.85
# The circle's size and its distance from the frame's top and right edges, as shares of the short side.
CARD_SIZE = 0.06
CARD_MARGIN = 0.04
# Drawn this many times larger and scaled down, so the edges and the digits come out smooth.
CARD_SUPER = 4
# Words that mark a scanlation or upload tag rather than a chapter title.
CARD_JUNK = re.compile(r"(?i)\b(scans?|scanlat\w*|translat(?:ion|ions|ed|or|ors)|fansubs?|raws?|tl|mtl)\b")
_CARD_OPEN, _CARD_CLOSE = "([{【「『（", ")]}】」』）"
_CARD_PREFIX = re.compile(
    # "Vol. 2", then "Ch. 12" / "Chapter 12" (any separator after), or a bare "12 -" / "#12:" (only
    # with a separator, so a title starting with a number, "100 Girlfriends", keeps it).
    r"(?i)^(?:(?:vol(?:ume)?|v)\.?\s*\d+(?:\.\d+)?\s*[,:.\-–—|]*\s*)?"
    r"(?:(?:chapter|chap|ch|episode|ep)\b\.?\s*#?\s*\d+(?:\.\d+)?\s*[:.\-–—|]*\s*|#?\d+(?:\.\d+)?\s*[:\-–—|]+\s*)"
)


def _card_drawable(text):
    """Whether the card's font (Inter: Latin, Greek, Cyrillic) has every letter of `text`."""
    return all(c.isspace() or ord(c) <= 0x052F or 0x1E00 <= ord(c) <= 0x1FFF or 0x2010 <= ord(c) <= 0x206F for c in text)


def clean_chapter_title(title):
    """A chapter's title as the card shows it, or "" when it is not a real title: anything in
    brackets or parentheses (a scanlation group's tag such as "(1r0n)") and "Vol. 2 Ch. 12 -"
    style prefixes are dropped, and what is left must read as a title: letters, not one stray
    tag of letters and digits ("1r0n", "v2"), no scanlation words, not just "Chapter"."""
    t = " ".join(str(title or "").split())
    out, depth = [], 0
    for c in t:
        if c in _CARD_OPEN:
            depth += 1
        elif c in _CARD_CLOSE:
            depth = max(0, depth - 1)
        elif not depth:
            out.append(c)
    t = " ".join("".join(out).split())
    t = _CARD_PREFIX.sub("", t, count=1)
    t = " ".join(t.strip(" -–—:|.,#").split())
    if not t or not _card_drawable(t) or CARD_JUNK.search(t):
        return ""
    if sum(c.isalpha() for c in t) < 2:
        return ""
    if " " not in t and re.search(r"\d", t) and re.search(r"[^\W\d_]", t):
        return ""
    if re.fullmatch(r"(?i)(chapter|chap|ch|episode|ep|vol|volume|v)\.?\s*#?\s*[\d.]*", t):
        return ""
    return t


def chapter_card_text(chapter):
    """(kicker, title) for the card from the job's `chapter` ({"number", "title"}): ("CHAPTER 12",
    "The Duel"); ("CHAPTER", title) without a number (a title "Chapter 5" gives the number); None
    with neither. The title is clean_chapter_title's: no group tags, no "Ch. 7 -" prefix, nothing
    the card's font cannot draw (Japanese, say), rather than boxes or junk."""
    if not isinstance(chapter, dict):
        return None
    num = chapter.get("number")
    raw = " ".join(str(chapter.get("title") or "").split())
    try:
        kicker = f"CHAPTER {float(num):g}"
    except (TypeError, ValueError):
        kicker = None
    if not kicker:
        m = re.search(r"(?i)\b(?:chapter|chap|ch)\b\.?\s*#?\s*(\d+(?:\.\d+)?)", raw)
        if m:
            kicker = f"CHAPTER {float(m.group(1)):g}"
    title = clean_chapter_title(raw)
    if not kicker:
        return ("CHAPTER", title) if title else None
    return kicker, title


def chapter_card_on(config, look):
    """Whether the video gets its chapter card: on unless the look's "chapter_card" switch (the
    Studio's Look) or the job's own "chapter_card" says off."""
    return config.get("chapter_card", True) is not False and (look or {}).get("chapter_card", True) is not False


def chapter_badge_number(chapter):
    """The chapter's number as the badge shows it ("2", "12", "12.5", "104"), or None when the
    chapter has none (a one-shot): then there is no badge at all. It is chapter_card_text's
    number, so a title like "Chapter 5" gives it too."""
    text = chapter_card_text(chapter)
    number = text[0][len("CHAPTER"):].strip() if text else ""
    return number if re.fullmatch(r"\d+(?:\.\d+)?", number) else None


def badge_geometry(w, h):
    """(circle diameter, shadow room around it, canvas side, margin) in pixels for a w x h frame.
    The first three are even, so the circle sits on the same pixels in the 4:2:0 picture of
    either renderer."""
    k = min(w, h)
    d = max(16, round(CARD_SIZE * k)) & ~1
    pad = (max(4, round(0.14 * d)) + 1) & ~1
    return d, pad, d + 2 * pad, round(CARD_MARGIN * k)


def badge_ring_width(d):
    """The ring's thickness in pixels: thin, about a 16th of the circle."""
    return max(2, round(d / 16))


def badge_state(i):
    """(opacity, scale, ring progress) of the badge on frame i of the chapter: nothing before
    CARD_IN, then it pops in, the ring fills until the fade starts, and it is gone by CARD_OUT."""
    t = i / FPS
    pop = min(1.0, max(0.0, (t - CARD_IN) / CARD_POP))
    ease = 1 - (1 - pop) ** 3
    fade = min(1.0, max(0.0, (CARD_OUT - t) / CARD_FADE))
    # Full when the fade starts, so the badge fades away as a completed ring.
    progress = min(1.0, max(0.0, (t - CARD_IN) / (CARD_SHOW - CARD_FADE)))
    return ease * fade, CARD_START_SCALE + (1 - CARD_START_SCALE) * ease, progress


def _badge_base(number, d, pad, size):
    """The badge's still part at CARD_SUPER times the size: a soft shadow, the dark disc, the
    ring's faint track and the number in white, fitted inside the ring with room to spare."""
    from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont
    q = CARD_SUPER
    box = (pad * q, pad * q, (pad + d) * q - 1, (pad + d) * q - 1)
    disc = Image.new("L", (size * q, size * q), 0)
    ImageDraw.Draw(disc).ellipse(box, fill=255)
    # The shadow only shows round the disc, so the translucent disc itself stays clean.
    shade = disc.filter(ImageFilter.GaussianBlur(max(2.0, 0.06 * d * q))).point(lambda v: int(v * 0.55))
    shade = ImageChops.subtract(ImageChops.offset(shade, 0, round(0.03 * d * q)), disc)
    img = Image.new("RGBA", disc.size, (0, 0, 0, 0))
    img.paste(Image.new("RGBA", disc.size, (0, 0, 0, 255)), mask=shade)
    img.paste(Image.new("RGBA", disc.size, (10, 10, 14, 199)), mask=disc)
    # The track and the number go on by compositing, so they sit over the disc and do not cut holes in it.
    marks = Image.new("RGBA", disc.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(marks)
    draw.ellipse(box, outline=(255, 255, 255, 64), width=badge_ring_width(d) * q)
    font_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fonts", "Inter-Bold.ttf")
    room = 0.54 * d * q
    px = max(8, round(0.46 * d * q))
    font = ImageFont.truetype(font_path, px)
    while px > 8 and draw.textlength(number, font=font) > room:
        px -= 2
        font = ImageFont.truetype(font_path, px)
    l, t, r, b = draw.textbbox((0, 0), number, font=font)
    mid = (pad + d / 2) * q
    draw.text((mid - (l + r) / 2, mid - (t + b) / 2), number, font=font, fill=(255, 255, 255, 255))
    return Image.alpha_composite(img, marks)


def _badge_ring(base, d, pad, progress):
    """`base` with the white progress arc on it: from 12 o'clock, clockwise, `progress` of the way round."""
    from PIL import Image, ImageDraw
    if progress <= 0:
        return base
    q = CARD_SUPER
    box = (pad * q, pad * q, (pad + d) * q - 1, (pad + d) * q - 1)
    ring = badge_ring_width(d) * q
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    white = (255, 255, 255, 255)
    if progress >= 1:
        draw.ellipse(box, outline=white, width=ring)
    else:
        end = -90 + 360 * progress
        draw.arc(box, -90, end, fill=white, width=ring)
        # Round ends, so the arc reads as a stroke and not a cut.
        mid, r = (pad + d / 2) * q, (box[2] - box[0] - ring) / 2
        for deg in (-90, end):
            x, y = mid + r * math.cos(math.radians(deg)), mid + r * math.sin(math.radians(deg))
            draw.ellipse((x - ring / 2, y - ring / 2, x + ring / 2, y + ring / 2), fill=white)
    return Image.alpha_composite(base, layer)


def make_chapter_card(w, h, chapter, work_dir):
    """The chapter badge as a PNG sequence for a w x h frame, or None when the chapter has no
    number. Returns (ffmpeg input pattern, frame count, side of the square frames): frame i is
    the chapter's i-th frame (badge_state: nothing before CARD_IN, then it pops in, the ring
    fills and it fades by CARD_OUT), with alpha, scale and ring all baked in, so both renderers
    only lay frame i at chapter_card_pos."""
    from PIL import Image
    number = chapter_badge_number(chapter)
    if not number:
        return None
    d, pad, size, _ = badge_geometry(w, h)
    base = _badge_base(number, d, pad, size)
    count = round(CARD_OUT * FPS)
    pattern = os.path.join(work_dir, "chapter_badge_%04d.png")
    blank = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    for i in range(count):
        opacity, scale, progress = badge_state(i)
        if opacity <= 0.002:
            frame = blank
        else:
            side = max(2, round(size * scale))
            small = _badge_ring(base, d, pad, progress).resize((side, side), Image.LANCZOS)
            frame = Image.new("RGBA", (size, size), (0, 0, 0, 0))
            frame.paste(small, ((size - side) // 2, (size - side) // 2))
            frame.putalpha(frame.getchannel("A").point(lambda v: round(v * opacity)))
        frame.save(pattern % i, compress_level=1)
    return pattern, count, size


def chapter_card_pos(w, h, bar):
    """Where the badge's frames go: the circle's top right corner CARD_MARGIN in from the
    frame's (below any letterbox bar), the frame itself a shadow's room wider; fixed, no glide."""
    d, pad, _, margin = badge_geometry(w, h)
    return (w - margin - d - pad) & ~1, max(0, bar + margin - pad) & ~1


def chapter_card_graph(card, w, h, bar, src, out, label, a):
    """The badge's frames (make_chapter_card's sequence, input stream `label`, read from frame
    `a` on) over [src] into [out], for a part starting at frame `a` of the chapter: the frames
    keep the chapter's clock, so it looks the same whichever part it falls in."""
    x, y = chapter_card_pos(w, h, bar)
    return (
        f"[{label}]format=yuva420p,setparams=colorspace=bt470bg:range=tv,settb=AVTB,setpts=(N+{a})/({FPS}*TB)[{out}_c];"
        f"[{src}][{out}_c]overlay=x={x}:y={y}:format=yuv420:eof_action=pass[{out}]"
    )


def overlay_graph(overlays, w, h, total, src, out, first_input, work_dir, t0=0.0):
    """Image and text layers from the Video editor drawn over [src] (a w x h picture) into
    [out]. Each overlay: kind "image" (path, scale = width share of the frame) or "text" (text,
    size = height share of the frame, color, box), centred at x/y percent, with opacity, shown
    from start to end seconds. Images become ffmpeg inputs from `first_input` on; text is read
    from files in `work_dir`. `t0` is where [src] starts on the video's clock (a part of the
    video made on its own): the images are shifted to it. Returns (extra input args, filter string)."""
    from caption_engine import FONTS_DIR, ffmpeg_filter_path
    inputs, parts = [], []
    cur = src
    font = os.path.join(FONTS_DIR, OVERLAY_FONT)
    for i, o in enumerate(overlays or []):
        if not isinstance(o, dict):
            continue
        start = _num(o.get("start"), 0.0, 0.0, total)
        end = _num(o.get("end"), total, 0.0, total)
        if end - start < 0.05:
            continue
        x = _num(o.get("x"), 50.0, 0.0, 100.0) / 100.0
        y = _num(o.get("y"), 50.0, 0.0, 100.0) / 100.0
        alpha = _num(o.get("opacity"), 1.0, 0.0, 1.0)
        enable = f"enable='between(t,{start:.3f},{end:.3f})'"
        nxt = f"{out}_o{i}"
        if o.get("kind") == "image":
            path = o.get("path")
            if not isinstance(path, str) or not os.path.isfile(path):
                continue
            ow = max(2, round(w * _num(o.get("scale"), 0.2, 0.02, 1.0)) // 2 * 2)
            idx = first_input + len(inputs) // 6
            shift = f",setpts=PTS+{t0:.6f}/TB" if t0 else ""
            inputs += ["-loop", "1", "-t", f"{total:.3f}", "-i", path]
            parts.append(
                f"[{idx}:v]scale={ow}:-2,format=rgba,colorchannelmixer=aa={alpha:.2f}{shift}[{nxt}_i];"
                f"[{cur}][{nxt}_i]overlay=x='{x:.4f}*W-w/2':y='{y:.4f}*H-h/2':{enable}[{nxt}]"
            )
        elif o.get("kind") == "text":
            text = o.get("text")
            if not isinstance(text, str) or not text.strip() or not os.path.isfile(font):
                continue
            os.makedirs(work_dir, exist_ok=True)
            tf = os.path.join(work_dir, f"overlay_{i}.txt")
            with open(tf, "w", encoding="utf-8") as f:
                f.write(text)
            fs = max(8, round(h * _num(o.get("size"), 0.06, 0.01, 0.3)))
            color = o.get("color")
            if not (isinstance(color, str) and re.fullmatch(r"#[0-9a-fA-F]{6}", color)):
                color = "#ffffff"
            box = f":box=1:boxcolor=black@{0.55 * alpha:.2f}:boxborderw={max(4, fs // 4)}" if o.get("box") else ""
            parts.append(
                f"[{cur}]drawtext=fontfile={ffmpeg_filter_path(font)}:textfile={ffmpeg_filter_path(tf)}:expansion=none:"
                f"fontsize={fs}:fontcolor=0x{color[1:]}@{alpha:.2f}:shadowcolor=black@{0.6 * alpha:.2f}:shadowx=2:shadowy=2{box}:"
                f"x='{x:.4f}*w-text_w/2':y='{y:.4f}*h-text_h/2':{enable}[{nxt}]"
            )
        else:
            continue
        cur = nxt
    parts.append(f"[{cur}]null[{out}]")
    return inputs, ";".join(parts)


# ── Layered motion (optional pack): animated 2.5D panels on key moments or every panel ──

_LAYERED_NOTES = set()


def _layered_note(msg):
    """One log line per reason per process (a missing pack is not news on every render)."""
    if msg not in _LAYERED_NOTES:
        _LAYERED_NOTES.add(msg)
        print(f"[video] Layered motion: {msg}", file=sys.stderr)


def layered_eligible(c):
    """A clip whose panel can be layered: a single studio panel crop (not side by side)."""
    return bool(c.get("images")) and not c.get("duo") and PANEL_FILE.search(str(c.get("source") or "")) is not None


def select_layered_clips(look, config, clips, items, panels):
    """{clip index} that get layered motion: none when the look has it off or the pack is not
    installed (said once in the log), else layered_plan.select_layered over the clips."""
    mode = (look or {}).get("layered", "off")
    if mode not in ("key", "all") or not clips:
        return set()
    py, why = layered_plan.pack_python(config)
    if not py:
        _layered_note(f"skipped ({why}); rendering as usual")
        return set()
    eligible = [layered_eligible(c) and beat_panel(items[c["idx"]], panels) is not None for c in clips]
    scores = [layered_plan.key_candidate(c.get("mood")) for c in clips]
    picked = set(layered_plan.select_layered(mode, eligible, scores))
    print(f"[video] Layered motion ({mode}): {len(picked)} of {len(clips)} clips", file=sys.stderr)
    return picked


def clip_ease(clip, frames):
    """The camera's eased progress (0..1) at every frame of a clip, from the very expression
    the render uses (so the panel's own parallax moves with the camera)."""
    _, _, ease = clip_camera(clip, frames)
    env = {"__builtins__": {}, "min": min, "max": max, "pow": pow, "cos": math.cos, "sin": math.sin, "exp": math.exp, "PI": math.pi}
    out = []
    for on in range(frames):
        env["on"] = on
        try:
            out.append(round(float(eval(ease, env)), 5))
        except Exception:
            out.append(round(0.5 - 0.5 * math.cos(math.pi * on / max(1, frames - 1)), 5))
    return out


# A key clip's characters can assemble from pixel blocks as it opens a new page (the layered pack
# draws it: they stand on the clean plate). Rare: at most one every PIXEL_GAP seconds.
PIXEL_GAP = 40.0
PIXEL_ENTRANCE = {"at": 0.1, "dur": 1.0, "block": 72}


def layered_job(c, frames, seg, panels, reads, look, output_dir, video_dir, entrance=None):
    """The pack's job for one clip (see layered_motion.run), or None when it has no panel.
    `entrance` (PIXEL_ENTRANCE) has its characters assemble from pixel blocks."""
    panel = beat_panel(seg, panels)
    imgs = c.get("images") or {}
    if panel is None or not imgs.get("fg_size"):
        return None
    src = c["source"]
    page = reads.get(c["page"]) or {}
    raw = page.get("scene") if isinstance(page.get("scene"), dict) else {}
    scene = dict(raw, summary=str(page.get("summary") or ""))
    if c.get("mood"):
        scene.update(kind=c["mood"]["kind"], tags=c["mood"]["tags"])
    intensity = c["mood"]["intensity"] if c.get("mood") else raw.get("intensity")
    prep = layered_plan.panel_key(src, panel)
    move = c.get("move")
    drift = move_dirs(move) if move else PARALLAX_DIRS.get(c["motion"], (0.6, 0))
    ease = clip_ease(c, frames)
    plan_in = [list(imgs["fg_size"]), frames, ease, list(drift), c.get("grade"), look.get("glow"), scene, intensity]
    if entrance:
        plan_in.append(entrance)
    hi = c.get("hi")
    if hi:
        plan_in.append(["hi", os.path.basename(hi)])
    akey = layered_plan.anim_key(prep, plan_in)
    return {
        "id": f"clip_{c['idx']:04d}", "src": src, "panel": panel, "scene": scene, "intensity": intensity,
        "cache": os.path.join(layered_plan.layers_root(output_dir), prep), "size": list(imgs["fg_size"]), "frames": frames,
        "ease": ease, "drift": list(drift), "grade": list(c["grade"]) if c.get("grade") else None, "glow": bool(look.get("glow")),
        "faces": [list(f) for _, f in layered_plan.panel_faces(panel)],
        "out": os.path.join(video_dir, f"fga_{c['idx']:04d}_{akey}.mp4"), "anim_key": akey,
        **({"entrance": dict(entrance)} if entrance else {}),
        # The panel's upscaled copy (upscale_panels): the pack draws from it, on the layers it cut
        # from the scan.
        **({"hi": hi} if hi else {}),
    }


def upscale_panels(sources, w, h, output_dir, config, work_dir):
    """{panel image: its upscaled copy} for the `sources` the video shows larger than their
    scan, made by the layered motion pack's Python (upscale.py: Real-ESRGAN's anime model) and
    kept in `<chapter>/studio/upscaled`. Empty without the pack or when it fails: the render
    then resizes as it always did."""
    try:
        import upscale
    except ImportError:
        from . import upscale
    if not upscale.wanted(config):
        return {}
    py, _ = layered_plan.pack_python(config)
    if not py:
        return {}
    t0 = time.monotonic()
    folder = os.path.join(output_dir, "studio", "upscaled")
    jobs, where = upscale.plan(sources, w, h, folder)
    todo = [j for j in jobs if not os.path.isfile(j["dst"])]
    if todo:
        path = os.path.join(work_dir, "upscale_jobs.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(todo, f)
        script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "upscale.py")
        env = os.environ.copy()
        env.update({"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8"})
        env.pop("VIRTUAL_ENV", None)
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
        done = error = None
        try:
            proc = subprocess.run([py, "-I", script, "run", path], capture_output=True, text=True, encoding="utf-8", errors="replace",
                                  env=env, creationflags=flags, timeout=300 + 20 * len(todo))
            for line in proc.stdout.splitlines():
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                if msg.get("type") == "done":
                    done = msg
                elif msg.get("type") == "error":
                    error = msg.get("message")
            if not done:
                error = error or proc.stderr.strip()[-300:] or f"exit {proc.returncode}"
        except (OSError, subprocess.TimeoutExpired) as e:
            error = str(e)
        if error:
            print(f"[video] Upscaler: {error}; those panels are resized as usual", file=sys.stderr)
        elif done.get("failed"):
            print(f"[video] Upscaler: {len(done['failed'])} panel(s) failed and are resized as usual", file=sys.stderr)
    made = sum(1 for j in todo if os.path.isfile(j["dst"]))
    where = {src: dst for src, dst in where.items() if os.path.isfile(dst)}
    # Copies of panels this chapter no longer shows (re-read, another size) go.
    try:
        keep = {os.path.normcase(os.path.abspath(d)) for d in where.values()}
        for name in os.listdir(folder):
            f = os.path.join(folder, name)
            if os.path.normcase(os.path.abspath(f)) not in keep:
                os.remove(f)
    except OSError:
        pass
    if jobs:
        print(f"[video] Timing: upscaler {time.monotonic() - t0:.1f} s ({made} panel(s) made, {len(where) - made} kept, "
              f"{len(set(sources)) - len(jobs)} shown at their own size)", file=sys.stderr)
    return where


def run_layered(jobs, config, work_dir, threads=None):
    """Runs the pack (its own Python, below normal priority) on the jobs {clip: job} with
    `threads` CPU threads (default all of the share); returns {job id: result}. Any failure
    leaves those clips to render as usual."""
    py, why = layered_plan.pack_python(config)
    if not py:
        _layered_note(f"skipped ({why})")
        return {}
    path = os.path.join(work_dir, "layered_jobs.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"threads": threads or cpus(), "jobs": list(jobs.values())}, f)
    script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "layered_motion.py")
    env = os.environ.copy()
    env.update({"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8", "HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1"})
    env.pop("VIRTUAL_ENV", None)
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
    result = {}
    try:
        proc = subprocess.Popen([py, "-I", script, "run", path], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                encoding="utf-8", errors="replace", env=env, creationflags=flags)
        timer = threading.Timer(900 + 60 * len(jobs), proc.kill)
        timer.start()
        errs = []
        pump = threading.Thread(target=lambda: errs.append(proc.stderr.read()), daemon=True)
        pump.start()
        try:
            for line in proc.stdout:
                line = line.strip()
                if not line.startswith("{"):
                    continue
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                if msg.get("type") == "info":
                    print(f"[video] Layered: {msg.get('message')}", file=sys.stderr)
                elif msg.get("type") == "done":
                    result = msg.get("results") or {}
                    ok = sum(1 for r in result.values() if r.get("ok"))
                    print(f"[video] Layered: {ok} of {len(jobs)} clips animated in {msg.get('seconds')} s "
                          f"(layers {msg.get('prep_seconds')} s, GPU memory peak {msg.get('vram_peak_mb')} MB)", file=sys.stderr)
                elif msg.get("type") == "error":
                    _layered_note(f"the pack failed ({msg.get('message')}); rendering as usual")
            proc.wait()
            pump.join(timeout=5)
        finally:
            timer.cancel()
            for pipe in (proc.stdout, proc.stderr):
                try:
                    pipe.close()
                except Exception:
                    pass
        if proc.returncode != 0 and not result:
            lines = [l for l in "".join(errs).strip().splitlines() if l.strip()]
            tail = lines[-1][-300:] if lines else ""
            _layered_note(f"the pack stopped ({tail or proc.returncode}); rendering as usual")
    except Exception as e:
        _layered_note(f"could not run the pack ({e}); rendering as usual")
    return result


def render_engine(config=None):
    """"gpu" (the GPU engine, gpu_render.py: the default) whenever the layered pack's Python
    (PyTorch with CUDA) is installed to run it, else "cpu", the clips-then-finish path.
    MANGAVOX_RENDER_ENGINE (or the config's "render_engine") = "cpu" forces the CPU path; "gpu"
    asks for the engine and says so in the log when it cannot be had. Measured on an RTX 3080:
    2.1-3x faster, about a tenth of the CPU time, the same picture (SSIM 0.996 against the CPU
    path); any failure falls back to the CPU path."""
    config = config if isinstance(config, dict) else {}
    want = str(os.environ.get("MANGAVOX_RENDER_ENGINE") or config.get("render_engine") or "auto").strip().lower()
    if want == "cpu":
        return "cpu"
    py, why = layered_plan.pack_python(config)
    if not py:
        if want == "gpu":
            _layered_note(f"GPU engine off ({why})")
        return "cpu"
    return "gpu"


# How long a render waits for another render's turn on the GPU engine (seconds; one at a time:
# the card has one video encoder and a chapter takes a minute or two) before drawing on the CPU
# path instead. MANGAVOX_GPU_LOCK_WAIT overrides.
GPU_LOCK_WAIT = 1800


class _GpuTurn:
    """The GPU engine's turn, one render at a time across every process of the app: an exclusive
    lock on a file in the temp folder (the OS lets go of it when the process ends, so a crash
    never leaves it held). `with _GpuTurn(wait) as ok:`; ok is False when the wait ran out."""

    def __init__(self, wait=None, path=None):
        self.wait = GPU_LOCK_WAIT if wait is None else wait
        self.path = path or os.path.join(__import__("tempfile").gettempdir(), "mangavox_gpu_engine.lock")
        self.f = None

    def _try(self):
        try:
            if os.name == "nt":
                import msvcrt
                self.f.seek(0)
                msvcrt.locking(self.f.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except OSError:
            return False

    def __enter__(self):
        self.f = open(self.path, "a+b")
        deadline = time.monotonic() + max(0.0, self.wait)
        noted = False
        while not self._try():
            if time.monotonic() >= deadline:
                self.f.close()
                self.f = None
                return False
            if not noted:
                print("[video] Waiting for another render's turn on the GPU engine", file=sys.stderr)
                noted = True
            time.sleep(1.0)
        return True

    def __exit__(self, *exc):
        if self.f:
            try:
                if os.name == "nt":
                    import msvcrt
                    self.f.seek(0)
                    msvcrt.locking(self.f.fileno(), msvcrt.LK_UNLCK, 1)
            except OSError:
                pass
            self.f.close()
            self.f = None
        return False


def _gpu_picture(video, clips, frame_counts, made, w, h, total, ass_path, config, out, on_fraction=None):
    """The finished picture (no audio) at `out`, drawn by the GPU engine: the plan
    (gpu_timeline) written next to the clips, then gpu_render.py run in the pack's Python.
    `made[k]` is clip k's file when an earlier run rendered it (read as it is). Raises on failure."""
    try:
        import gpu_timeline
    except ImportError:
        from . import gpu_timeline
    work = video.get("work_dir") or os.path.dirname(os.path.abspath(out))
    plans = [gpu_timeline.plan_clip(c, frame_counts[k], w, h, prerendered=made[k]) for k, c in enumerate(clips)]
    vf = None
    if ass_path and os.path.exists(ass_path):
        try:
            from caption_engine import subtitles_filter
        except ImportError:
            from .caption_engine import subtitles_filter
        vf = subtitles_filter(ass_path)
    py, why = layered_plan.pack_python(config)
    if not py:
        raise RuntimeError(why)
    # NVENC p4 here (MANGAVOX_GPU_PRESET overrides): measured on an RTX 3080 at 1440p it is ~1.35x
    # faster than the finish's p5 (149 vs 111 fps; p3 no faster, the drawing is the limit then)
    # with the same picture (SSIM 0.9961 vs 0.9962 against the CPU path's render) and size.
    preset = os.environ.get("MANGAVOX_GPU_PRESET") or "p4"
    failed = None
    last = None
    # The encoders in order (NVENC, then x264): a run the encoder failed is tried on the next,
    # and NVENC is turned off for the process when only x264 worked (as run_codecs does).
    # One render at a time on the engine (the card's one encoder; two at once only fight over it
    # and double the load): a second render waits its turn, then draws on the CPU path instead.
    wait = _num(os.environ.get("MANGAVOX_GPU_LOCK_WAIT"), GPU_LOCK_WAIT, 0.0, 86400.0)
    with _GpuTurn(wait) as turn:
        if not turn:
            raise RuntimeError(f"GPU engine: another render kept it busy for {wait:.0f} s")
        for codec in final_codecs(cpus()):
            codec = list(codec)
            if preset and "h264_nvenc" in codec and "-preset" in codec:
                codec[codec.index("-preset") + 1] = preset
            plan = gpu_timeline.plan_video(video, plans, w, h, total, ass_filter=vf, codec=codec, out=out)
            try:
                _gpu_render(plan, py, work, total, on_fraction)
            except RuntimeError as e:
                last = e
                if "ENCODER:" in str(e) and "h264_nvenc" in codec:
                    failed = codec
                    continue
                raise
            if failed:
                encoder_failed(failed)
            return
    raise last or RuntimeError("GPU engine: no encoder")


def _gpu_render(plan, py, work, total, on_fraction=None):
    """Runs gpu_render.py on `plan` in the pack's Python `py`; raises with its error."""
    path = os.path.join(work, "gpu_plan.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(plan, f)
    script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "gpu_render.py")
    env = os.environ.copy()
    env.update({"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8"})
    env.pop("VIRTUAL_ENV", None)
    # The fonts the captions need are found by caption_engine's filter (absolute paths).
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
    proc = subprocess.Popen([py, "-I", script, "run", path], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                            encoding="utf-8", errors="replace", env=env, creationflags=flags)
    cap = _num(os.environ.get("MANGAVOX_GPU_MAX_FPS"), 0.0, 0.0, 1000.0)
    timer = threading.Timer(600 + 2 * total + (total * FPS / cap if cap > 0 else 0), proc.kill)
    timer.start()
    errs = []
    pump = threading.Thread(target=lambda: errs.append(proc.stderr.read()), daemon=True)
    pump.start()
    done = error = None
    try:
        for line in proc.stdout:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("type") == "progress" and on_fraction and msg.get("total"):
                on_fraction(msg["done"] / msg["total"])
            elif msg.get("type") == "done":
                done = msg
            elif msg.get("type") == "error":
                error = msg.get("message")
        proc.wait()
        pump.join(timeout=5)
    finally:
        timer.cancel()
    if not done or proc.returncode != 0 or not os.path.isfile(plan["out"]):
        tail = error or "".join(errs).strip()[-300:] or f"exit {proc.returncode}"
        raise RuntimeError(f"GPU engine: {tail}")
    print(f"[video] GPU engine: {plan['frames']} frames in {done.get('seconds')} s ({done.get('fps')} fps, "
          f"GPU memory peak {done.get('vram_peak_mb')} MB; drawing {done.get('draw_s')} s, waiting on the GPU {done.get('gpu_wait_s')} s, on the encoder {done.get('encoder_wait_s')} s)", file=sys.stderr)
    if done.get("power"):
        print(f"[video] {done['power']} (frame-rate ceiling {done.get('fps_cap')})", file=sys.stderr)


def generate_video(page_files, sync_map, output_dir, config=None, progress_cb=None):
    """
    Generate a narrated manga video.

    Args:
        page_files: sorted list of page image paths
        sync_map: dict with 'segments' (page→timing) from sync_map.json
        output_dir: pipeline output directory
        config: dict with options:
            - transition: 'fade' | 'crossfade' | 'none' (default: 'fade'). Anything but
              'none'/'cut' crossfades consecutive clips without shifting the narration.
            - resolution: '720p' | '1080p' | '1440p' (default: '1080p')
            - transition_duration: page-change transition seconds (default: 0.5)
            - effect: 'ken_burns' | 'none' (default: 'ken_burns')
            - fill: 'blur' (still blurred copy of the panel behind it) | 'black' (default: 'blur')
            - format: 'landscape' (one 16:9 video) | 'reels' (9:16, also cut into
              ~1 minute reels in output_dir/reels/) (default: 'landscape')
            - reel_seconds: target reel length (default: 60)
            - score / bg_music / bg_music_volume / bg_music_loop / narration_volume
            - music_volume: linear multiplier on the score / music (default: 1.0, 0 mutes)
            - subtitles: caption setting (caption_engine.normalize_settings keys: style,
              size, posX, posY, wordsPerLine, lines, uppercase, color, highlightColor,
              emphasis, enabled); falls back to <output_dir>/subtitles.json, then the
              Studio default. Words are timed against each clip's audio.
            - edit: the Studio Video editor's edit: clips (see apply_edit_clips),
              overlays (see overlay_graph) and narration_mute
        progress_cb: function(stage, current, total)

    With music the video carries two audio tracks: "Narration + music" (the default) and
    "Narration only", so a player or an export can drop the music without a re-render.
    Rendered clips are kept (named by their content) until the video is finished, so a
    re-run after a pause or a crash only renders what is missing. The narration boundaries
    are saved next to the video in video_timeline.json (exports cut there).

    Returns:
        path to final video file
    """
    check_ffmpeg()
    config = config or {}

    transition = config.get("transition", "fade")
    resolution = config.get("resolution", "1080p")
    try:
        trans_dur = min(1.5, max(0.2, float(config.get("transition_duration", 0.5))))
    except (TypeError, ValueError):
        trans_dur = 0.5
    effect = config.get("effect", "ken_burns")
    if transition == "zoom-punch":
        effect = "ken_burns"
    trans_kind = "none" if transition in ("none", "cut") else "crossfade"
    force_rerender = config.get("force_rerender", False)
    fmt = "reels" if config.get("format") == "reels" else "landscape"
    w, h = get_resolution(resolution, fmt)
    fill = "black" if config.get("fill") == "black" else "blur"
    edit = config.get("edit") if isinstance(config.get("edit"), dict) else {}
    look = apply_intensity(resolve_look(config.get("look")), edit.get("intensity"))
    # Side-by-side panels need the wide frame and the blurred fill.
    duo_on = look["duo"] and fmt == "landscape" and fill == "blur" and effect != "none"
    margin = FILL_MARGIN if look.get("framing") == "fill" else FG_MARGIN
    # Letterbox bars (landscape only): panels are laid out between them. Parallax needs the
    # blurred fill to drift.
    inset = letterbox_bar(w, h) if look.get("letterbox") else 0
    drift = round(w * PARALLAX * look["parallax_k"] / 2) // 2 * 2 if look.get("parallax") and fill == "blur" and effect != "none" else 0
    reels_dir = os.path.join(output_dir, "reels")

    video_dir = os.path.join(output_dir, "video_parts")
    os.makedirs(video_dir, exist_ok=True)

    # Prefer panel-level beats (each panel image synced to its slice of the page
    # audio) so the video advances panel-by-panel with the narration. Fall back
    # to page-level segments when no beats were produced.
    beat_segments = sync_map.get("beat_segments") or []
    segments = sync_map.get("segments", [])
    use_beats = bool(beat_segments)
    render_items = beat_segments if use_beats else segments
    if isinstance(edit.get("clips"), list):
        render_items = apply_edit_clips(render_items, edit["clips"])
        use_beats = True
    if not render_items:
        raise RuntimeError("No segments in sync map — nothing to render")
    # The story's mood per beat (from the chapter's AI reading next to the sync map, or
    # config["scenes"] {page: scene}): grade, atmosphere, transitions and punches follow it.
    scenes = config.get("scenes") if isinstance(config.get("scenes"), dict) else None
    if scenes is not None:
        scenes = {int(k): v for k, v in scenes.items() if str(k).lstrip("-").isdigit() and isinstance(v, dict)}
    elif look.get("mood"):
        scenes = load_scenes(output_dir)
    moods = plan_moods(render_items, scenes) if look.get("mood") else [None] * len(render_items)
    mood_on = any(moods)
    # The panels of the reading (boxes and faces): what each clip must keep in frame.
    panels = load_panels(output_dir)

    audio_dir = os.path.join(output_dir, "audio")
    final_video = os.path.join(output_dir, "video_final.mp4")
    timeline_file = os.path.join(output_dir, "video_timeline.json")

    # A forced re-render replaces the video; clips already rendered for the same content
    # (kept in video_parts by an interrupted run) are reused.
    if force_rerender:
        if os.path.exists(final_video):
            try:
                os.unlink(final_video)
            except OSError:
                pass
    elif os.path.exists(final_video) and os.path.getsize(final_video) > 1000 and (fmt == "reels") == os.path.isdir(reels_dir):
        return final_video

    report = _Progress(progress_cb)

    # ── Step 1: Resolve every clip (image, audio slice, timing) and prepare its stills ──
    clips = []
    # End time (seconds) of every rendered clip - where reels may be cut.
    boundaries = []
    # The narration actually on screen, timed on the rendered timeline (captions follow it).
    caption_segments = []
    timeline = 0.0
    total = len(render_items)
    mx, my = panel_margins(w, h, fill, inset, margin)
    box_aspect = (w - 2 * mx) / (h - 2 * my)
    turn = 0
    prev_motion = None
    # The duo clip before this one (its panel stays in the other half), and the last punch.
    duo_prev = None
    last_impact = -IMPACT_GAP
    last_impact_t = -1e9

    def image_of(seg):
        """The picture a clip shows: a beat carries its exact panel crop; otherwise its page."""
        page_num = seg["page"]
        page_image = seg.get("panel_image") if use_beats else None
        if page_image and not os.path.exists(page_image):
            page_image = None
        if not page_image:
            for pf in page_files:
                basename = os.path.basename(pf)
                # Match page number (0001.jpg, page_0001.jpg, etc.)
                if f"{page_num:04d}" in basename or f"{page_num + 1:04d}" in basename:
                    page_image = pf
                    break
        if not page_image and isinstance(page_num, int) and page_num < len(page_files):
            page_image = page_files[page_num]
        return page_image

    # Panels the video shows larger than their scan are redrawn once by the upscaler (kept per
    # chapter); the stills and the layered panels are then made from that copy.
    try:
        sharp = upscale_panels([s for s in (image_of(seg) for seg in render_items) if s], w, h, output_dir, config, video_dir)
    except Exception as e:
        print(f"[video] Upscaler skipped: {e}", file=sys.stderr)
        sharp = {}

    def prepare(idx, seg):
        """The clip's source, focus crop and stills (independent of every other clip, so they
        are made side by side), or None when it cannot be shown."""
        page_num = seg["page"]
        audio_file = seg.get("audio_file", f"page_{page_num:04d}.wav")
        audio_path = os.path.join(audio_dir, audio_file)

        if not os.path.exists(audio_path):
            return None

        page_image = image_of(seg)
        if not page_image:
            return None
        source_image = page_image

        # What must stay whole on screen (see Framing): a studio panel crop is its panel, grown
        # to its faces; a page framed on the AI's focus box is cropped to that box snapped out to
        # its panel (aspect matched to the panel box, or to a half of the frame when a tall
        # focus goes side by side; only ever widened, never cut into).
        crop = PANEL_FILE.search(str(page_image)) is not None
        region = crop_region(beat_panel(seg, panels)) if crop else (PANEL_TRIM, PANEL_TRIM, 1 - PANEL_TRIM, 1 - PANEL_TRIM)
        # The same picture, upscaled (every box below is in fractions of it).
        hi = sharp.get(page_image)
        page_image = hi or page_image
        focus_point = seg.get("focus_point")
        if focus_point and len(focus_point) == 4 and not crop:
            try:
                want = focus_region(focus_point, panels.get(page_num))
                aspect = box_aspect
                if duo_on:
                    from PIL import Image
                    with Image.open(page_image) as im:
                        fw_, fh_ = im.size
                    if (want[2] - want[0]) * fw_ < DUO_MAX_ASPECT * (want[3] - want[1]) * fh_:
                        aspect = 0.75
                page_image, region = crop_to_region(page_image, want, aspect, os.path.join(video_dir, f"crop_{idx:04d}.jpg"))
            except Exception as e:
                print(f"[video] Failed to crop page to focus point: {e}", file=sys.stderr)
        else:
            focus_point = None

        override = seg.get("motion") if seg.get("motion") in EDIT_MOTIONS else None
        try:
            tall = False
            # A beat may ask to be shown whole ("layout": "single"), like the hook's closing cover,
            # and an editor motion other than a shake moves the panel on its own.
            if duo_on and seg.get("layout") != "single" and override in (None, "shake"):
                from PIL import Image
                with Image.open(page_image) as im:
                    tall = im.width < DUO_MAX_ASPECT * im.height
            grade = moods[idx]["grade"] if moods[idx] else None
            if tall:
                # Placed in its half below (the stills are the same for either half).
                duo, images = _prepare_duo_images(page_image, idx, w, h, 0, video_dir, look["glow"], inset, drift, grade), None
            else:
                duo, images = None, _prepare_clip_images(page_image, idx, w, h, fill, video_dir, look["glow"], inset, drift, grade, margin)
        except Exception as e:
            print(f"[video] Skipping unreadable image for page {page_num}: {e}", file=sys.stderr)
            return None
        return {"audio_path": audio_path, "source": source_image, "focus": focus_point, "duo": duo, "images": images, "region": region, "hi": hi}

    prepared = [None] * total
    with ThreadPoolExecutor(max_workers=max(2, min(16, cpus()))) as pool:
        futures = {pool.submit(prepare, idx, seg): idx for idx, seg in enumerate(render_items)}
        for n, done in enumerate(as_completed(futures)):
            prepared[futures[done]] = done.result()
            report("prepare", (n + 1) / total)

    for idx, seg in enumerate(render_items):
        p = prepared[idx]
        if p is None:
            continue
        page_num = seg["page"]
        audio_path, source_image, focus_point = p["audio_path"], p["source"], p["focus"]

        # For beats, play just this panel's slice of the page audio; for pages,
        # use the whole file.
        audio_offset = 0.0
        if use_beats and seg.get("duration_ms"):
            audio_offset = max(0.0, float(seg.get("audio_offset_ms", 0)) / 1000.0)
            duration = float(seg["duration_ms"]) / 1000.0
        else:
            duration = get_audio_duration_sec(audio_path)
        # The editor's speed: the slice plays retimed (see _retime_clips), the clip as long as that.
        speed = _num(seg.get("speed"), 1.0, 0.5, 2.0)
        src_duration = round(duration, 3)
        duration = duration / speed
        if duration < 0.1:
            continue
        duration = round(duration, 3)

        override = seg.get("motion") if seg.get("motion") in EDIT_MOTIONS else None
        if p["duo"]:
            same = duo_prev is not None and duo_prev["source"] == source_image and duo_prev["focus"] == focus_point
            if same:
                duo = dict(duo_prev["duo"], hold=True, bg_prev=None)
            else:
                slot = 0 if duo_prev is None else 1 - duo_prev["duo"]["slot"]
                duo = dict(p["duo"], geo=duo_slot(p["duo"]["src_size"], w, h, slot, inset))
                duo["slot"] = slot
                # The background drifts the way the panel slides in (and back with the next).
                duo["bg_dir"] = 1 if slot == 0 else -1
                duo["bg_prev_dir"] = duo_prev["duo"].get("bg_dir", 1) if duo_prev else 1
                duo["bg_prev"] = duo_prev["duo"]["bg"] if duo_prev else None
                duo["partner"] = {"fg": duo_prev["duo"]["fg"], "geo": duo_prev["duo"]["geo"]} if duo_prev else None
            images = None
        else:
            duo = None
            images = p["images"]

        motion = "still"
        move = None
        if duo:
            motion = "duo"
            duo_prev = {"duo": duo, "source": source_image, "focus": focus_point}
        else:
            duo_prev = None
            if override == "still":
                motion = "still"
            elif override and override != "shake":
                motion = "edit"
                move = {"kind": override, "amount": seg.get("amount"), "from": seg.get("focus_from"), "to": seg.get("focus_to")}
            elif effect != "none":
                motion = pick_motion(images["aspect"], images["box_aspect"], turn, prev_motion)
                turn += 1
                prev_motion = motion
            if override == "shake" and motion == "still":
                motion = "push"
        mood = moods[idx]
        if override:
            # The editor's move wins: its shake punches, any other move does not.
            impact = override == "shake"
        elif mood_on:
            # The story decides: an intense page with a loud line or a hit (the words alone
            # without a reading), spaced in time as well as in clips (punch_ok).
            want = mood["punch"] if mood else is_impact(seg.get("text", ""))
            impact = look["impact"] and want and punch_ok(timeline, len(clips), last_impact_t, last_impact)
        else:
            impact = look["impact"] and len(clips) - last_impact >= IMPACT_GAP and is_impact(seg.get("text", ""))
        if impact:
            last_impact = len(clips)
            last_impact_t = timeline

        clips.append({
            "idx": idx, "page": page_num, "images": images, "motion": motion, "move": move,
            "duo": duo, "impact": impact, "push": look["push"], "ramp": look.get("ramp", False),
            "punch": seg["amount"] if override == "shake" and seg.get("amount") is not None else 0.12,
            "chroma": look.get("chroma", False), "chroma_k": look["chroma_k"], "motion_k": look["motion_k"],
            "source": source_image, "focus": focus_point, "hi": p.get("hi"),
            "audio_path": audio_path, "offset": audio_offset, "speed": speed, "src_duration": src_duration,
            "start": timeline, "duration": duration, "cut": bool(seg.get("cut")), "trans": seg.get("transition"),
            "mood": mood, "grade": mood["grade"] if mood else None, "region": p["region"],
        })
        caption_segments.append({
            "text": seg.get("text", ""),
            "start_ms": int(round(timeline * 1000)),
            "end_ms": int(round((timeline + duration) * 1000)),
            "audio_path": audio_path,
            "audio_offset_ms": int(round(audio_offset * 1000)),
            # When each word is said (sync_map / align.py), in the page's audio: the captions
            # follow it. `word_offset_ms` and `speed` place those times on the clip.
            **({"words": seg["words"], "word_offset_ms": int(round(audio_offset * 1000)), "speed": speed} if seg.get("words") else {}),
            # Series mode (beta): a character's first line brings up their name card.
            **({"name_card": seg["name_card"]} if seg.get("name_card") else {}),
        })
        timeline += duration
        boundaries.append(timeline)

    if not clips:
        raise RuntimeError("No video segments were generated")

    report = _Progress(progress_cb)
    report("prepare", 1.0)

    # ── Step 2: Render each clip's video (with a tail that the next crossfade eats) ──
    durations = [c["duration"] for c in clips]
    # A beat showing the same panel as the one before (a caption-only page holds the last panel)
    # is a hold: planned as the same page (a quick dissolve, no page-turn effect), its panel
    # staying put.
    holds, pages = [], []
    for k, c in enumerate(clips):
        holds.append(k > 0 and c["source"] == clips[k - 1]["source"])
        pages.append(pages[-1] if holds[-1] else c["page"])
    # A duo clip that follows another slides in by itself (a cut), and a punch flashes in.
    joins = [None]
    for k in range(1, len(clips)):
        if clips[k]["cut"] or (clips[k]["duo"] and clips[k - 1]["duo"]):
            joins.append("cut")
        elif clips[k]["impact"]:
            joins.append("impact")
        else:
            joins.append(None)
    # The editor's own transition into a clip (a hard cut wins: it is a join above).
    forced = [None] + [tuple(c["trans"]) if c.get("trans") and not c["cut"] else None for c in clips[1:]]
    starts, trans = plan_transitions(
        durations, pages, trans_kind, trans_dur, look["transitions"], joins, forced,
        [c["mood"] for c in clips] if mood_on else None,
    )
    frame_counts = [clip_frames(starts, trans, k, durations[-1]) for k in range(len(clips))]
    # Page-turn effects the clips draw themselves, and the light frames they share (made once).
    clip_fx = plan_clip_fx(
        starts, trans, pages, joins, look.get("leaks", False), [c["impact"] for c in clips],
        forced=forced, quiet=[bool(c["mood"]) and c["mood"]["mood"] in ("horror", "sad") for c in clips],
    )
    if effect != "none":
        for f, moves in zip(clip_fx, plan_panel_moves(starts, trans, pages, joins, [bool(c["duo"]) for c in clips], forced, holds)):
            f.update(moves)
    assets = {}
    for kind, key in (("leak", "leak"), ("burn", "burn_in")):
        if any(key in f for f in clip_fx):
            try:
                assets[kind] = make_flare_frames(kind, w, h, video_dir)
            except Exception as e:
                print(f"[video] Could not make the {kind} frames: {e}", file=sys.stderr)
    # The mood's atmosphere per clip (with the particles switch on, dust where the mood has none).
    atmos = {}
    for c in clips:
        kind = c["mood"]["particles"] if c["mood"] else None
        if mood_on and look.get("particles") and not kind:
            kind = "dust"
        c["atmos_kind"] = kind if mood_on else None
        if c["atmos_kind"] and c["atmos_kind"] not in atmos:
            try:
                atmos[c["atmos_kind"]] = make_atmosphere_loop(c["atmos_kind"], w, h, video_dir)
            except Exception as e:
                print(f"[video] Could not make the {c['atmos_kind']} loop: {e}", file=sys.stderr)
                atmos[c["atmos_kind"]] = None
    for k, c in enumerate(clips):
        loop = atmos.get(c["atmos_kind"]) if c["atmos_kind"] else None
        c["atmos"] = (loop[0], loop[1], round(starts[k] * FPS)) if loop else None
    all_frames = max(1, sum(frame_counts))
    rendered = 0
    clip_paths = []
    todo = []
    framing = []
    # Layered motion: which clips get animated panels (the pack changes nothing about timing).
    layered = select_layered_clips(look, config, clips, render_items, panels)
    reads = layered_plan.load_read(output_dir) if layered else {}
    layer_jobs = {}
    last_pixel = -PIXEL_GAP
    for k, c in enumerate(clips):
        frames = frame_counts[k]
        c["fx"], c["assets"] = clip_fx[k], assets
        # Framing QA: the clip's must-see region on screen the whole time, else re-planned.
        rec = frame_clip(c, frames)
        framing.append({"clip": k, "idx": c["idx"], "page": c["page"], **rec})
        drawn = lambda n: n.split("_")[0] in assets or "zoom" in n or n.startswith("panel_") or n.split("_")[0] in CLIP_DRAWN and not n.startswith("burn")
        spec = [c["motion"], c["impact"], c["push"], look["glow"], c["ramp"], c["chroma"] and c["impact"],
                inset, drift, sorted((n, v) for n, v in clip_fx[k].items() if drawn(n)),
                c["move"], c["punch"], c["chroma_k"], c["motion_k"], c["grade"],
                (c["atmos_kind"], c["atmos"][2] % c["atmos"][1]) if c["atmos"] else None,
                None if c["duo"] else c["region"]]
        if c["duo"]:
            # The pan ends where the next clip cuts in, so the panel left behind does not jump.
            nxt = round(starts[k + 1] * FPS) if k + 1 < len(starts) else round(starts[k] * FPS) + frames
            c["duo"]["move_frames"] = nxt - round(starts[k] * FPS)
            d = c["duo"]
            spec += [d["slot"], d.get("hold", False), d["move_frames"], d.get("bg_prev"), (d.get("partner") or {}).get("fg"),
                     d.get("bg_dir"), d.get("bg_prev_dir")]
        if margin != FG_MARGIN:
            spec = spec + [("margin", margin)]
        c["spec"] = spec
        if k in layered:
            # A key clip opening a new page may have its characters assemble from pixels. Its
            # layers, when already prepared, say whether it has characters to cut out: without,
            # the whole panel comes in from pixels instead (xfade's pixelize, same length).
            pixel = (k == 0 or pages[k] != pages[k - 1]) and starts[k] - last_pixel >= PIXEL_GAP and not c["duo"]
            job = layered_job(c, frames, render_items[c["idx"]], panels, reads, look, output_dir, video_dir, PIXEL_ENTRANCE if pixel else None)
            if job and pixel:
                last_pixel = starts[k]
                # The entrance stays in the job whatever the prep finds (the pack skips it with no
                # character), so the clip's key is the same on every run; prepared layers with no
                # character have the whole panel come in from pixels instead.
                man = layered_plan.read_manifest(job["cache"])
                if man is not None and not man.get("layers") and k and trans[k] and not forced[k] and trans[k][0] not in CLIP_DRAWN and trans[k][1] >= 0.2:
                    trans[k] = ("pixelize", trans[k][1])
            if job:
                layer_jobs[k] = job
                spec = spec + [("layered", job["anim_key"])]
        key = clip_key(c["source"], c["focus"], spec, frames, w, h, fill)
        path = os.path.join(video_dir, f"clip_{c['idx']:04d}_{key}.mp4")
        clip_paths.append(path)
        # Reuse a clip an interrupted run already rendered (when it is whole: see clip_usable).
        if clip_usable(path, frames):
            rendered += frames
            layer_jobs.pop(k, None)
        else:
            if os.path.exists(path):
                print(f"[video] Clip {c['idx']} from an earlier run is damaged; rendering it again", file=sys.stderr)
                try:
                    os.unlink(path)
                except OSError:
                    pass
            todo.append((c, frames, path))
    report("clips", rendered / all_frames)
    at = lambda ks: ", ".join(f"{starts[k]:.1f}s" for k in ks) or "none"
    print(f"[video] Edit: particle bursts at {at([k for k, t in enumerate(trans) if t and t[0] == 'particles'])}; "
          f"pixel entrances at {at([k for k, j in layer_jobs.items() if j.get('entrance')])}; "
          f"pixel reveals at {at([k for k, t in enumerate(trans) if t and t[0] == 'pixelize'])}; "
          f"{sum('panel_in' in f for f in clip_fx)} panel slide(s)", file=sys.stderr)
    fixed = [f for f in framing if f["fix"]]
    print(f"[video] Framing: {len(framing)} clips, worst {min(f.get('worst_after', f['worst']) for f in framing):.3f} of the "
          f"must-see region on screen, {len(fixed)} re-planned", file=sys.stderr)
    try:
        with open(os.path.join(output_dir, "video_framing.json"), "w", encoding="utf-8") as f:
            json.dump({"clips": framing}, f, indent=1)
    except OSError as e:
        print(f"[video] Could not save the framing report: {e}", file=sys.stderr)

    def render_one(c, frames, path):
        try:
            _render_clip(c, frames, w, h, path)
        except Exception as e:
            print(f"[video] {e} (page {c['page']}); retrying as a still", file=sys.stderr)
            if c.get("images"):
                c["images"].pop("fg_anim", None)
            if c["duo"]:
                c["images"] = _prepare_clip_images(c.get("hi") or c["source"], c["idx"], w, h, fill, video_dir, look["glow"], inset, drift, c["grade"], margin)
                c["duo"] = None
            c["motion"] = "still"
            c["move"] = None
            c["impact"] = False
            c["fx"] = {}
            _render_clip(c, frames, w, h, path)
        return frames

    # Narration, sample-accurate against the same timeline (sped-up clips retimed first), and the
    # captions timed to it: made beside the clips (they read only the timing, never a clip).
    narration = os.path.join(video_dir, "narration.wav")

    def make_sound_and_captions():
        _retime_clips(clips, caption_segments, video_dir)
        _build_narration(clips, video_dir, narration)
        try:
            import captions
            ass = captions.write_styled_captions(caption_segments, output_dir, w, h, config=config)
        except Exception as e:
            print(f"[video] Failed to generate styled captions: {e}", file=sys.stderr)
            ass = None
        # Series mode (beta): the cast's name cards ride on the caption track (or one of their own).
        try:
            import name_cards
            return name_cards.add_cards(ass, caption_segments, w, h, output_dir)
        except Exception as e:
            print(f"[video] Name cards skipped: {e}", file=sys.stderr)
            return ass

    def make_edge():
        # The edge glow's loops for the finish (kept in the fx cache), drawn while clips render.
        if look.get("edge_glow"):
            try:
                for rgb in dict.fromkeys(tuple(c["mood"]["edge"] if c["mood"] else EDGE_COLORS["neutral"]) for c in clips):
                    make_edge_glow(w, h - 2 * inset, rgb, video_dir)
            except Exception as e:
                print(f"[video] {e}; the finish will try the edge glow again", file=sys.stderr)

    # Each clip is its own ffmpeg run whose filter graph mostly uses one core, so several run
    # side by side; the clips are independent files, joined in order below. The longest go first,
    # so the last to finish is never a long clip started late, running on its own. While the
    # layered pack runs, the plain clips render beside it in half the slots (the pack's encoder
    # and its torch work need the rest; starved, it runs several times slower); its clips then go
    # ahead of the plain ones still waiting.
    t_clips = time.monotonic()
    jobs = render_jobs()
    lock = threading.Lock()
    slots = threading.Semaphore(jobs)
    # The GPU engine draws every clip itself in the finish: only the layered panels are made here.
    gpu = render_engine(config) == "gpu"
    queue = [] if gpu else sorted((t for t in todo if clips.index(t[0]) not in layer_jobs), key=lambda t: -t[1])

    def worker():
        nonlocal rendered
        while True:
            with slots:
                with lock:
                    if not queue:
                        return
                    t = queue.pop(0)
                frames = render_one(*t)
            with lock:
                rendered += frames
                report("clips", rendered / all_frames)

    layered_todo = [t for t in todo if clips.index(t[0]) in layer_jobs]
    reserve = jobs // 2 if layer_jobs else 0
    for _ in range(reserve):
        slots.acquire()
    with ThreadPoolExecutor(max_workers=2) as side, ThreadPoolExecutor(max_workers=2 * jobs) as pool:
        sound = side.submit(make_sound_and_captions)
        side.submit(make_edge)
        futures = [pool.submit(worker) for _ in range(jobs)]
        # The layered clips' panels, made in one run of the pack (models one after another on the
        # GPU) while the plain clips render; a clip whose panel could not be made renders as
        # usual, under its plain key.
        if layer_jobs:
            t_layered = time.monotonic()
            # Layers an older version of the pack cut are never read again.
            layered_plan.prune_stale(output_dir)
            try:
                # The GPU engine animates the layered panels itself, straight into the video: the
                # pack only cuts them into layers (cached).
                pack_jobs = {k: dict(j, prep_only=True) for k, j in layer_jobs.items()} if gpu else layer_jobs
                made = run_layered(pack_jobs, config, video_dir, threads=max(2, cpus() // 2))
            finally:
                for _ in range(reserve):
                    slots.release()
            print(f"[video] Timing: layered pack {time.monotonic() - t_layered:.1f} s", file=sys.stderr)
            for n, (c, frames, path) in enumerate(layered_todo):
                k = clips.index(c)
                job = layer_jobs[k]
                if gpu and made.get(job["id"], {}).get("ok"):
                    c["images"] = dict(c["images"], fg_layered=job)
                elif made.get(job["id"], {}).get("ok") and os.path.isfile(job["out"]):
                    c["images"] = dict(c["images"], fg_anim=job["out"])
                else:
                    plain = clip_key(c["source"], c["focus"], c["spec"], frames, w, h, fill)
                    clip_paths[k] = os.path.join(video_dir, f"clip_{c['idx']:04d}_{plain}.mp4")
                    layered_todo[n] = (c, frames, clip_paths[k])
            if not gpu:
                with lock:
                    queue[:0] = sorted(layered_todo, key=lambda t: -t[1])
            # Workers that found the queue empty while the pack ran have left: start them again.
            futures += [pool.submit(worker) for _ in range(jobs)]
        for f in futures:
            f.result()
        ass_path = sound.result()
    print(f"[video] Timing: {len(todo)} clips {time.monotonic() - t_clips:.1f} s ({jobs} at once)", file=sys.stderr)

    # The clips go straight into the finish (made in parts side by side, see
    # _finalize_video_with_audio_mix_and_captions): no intermediate join, so no extra encode.
    video_inputs, video_starts, video_trans = clip_paths, starts, trans

    # ── Step 4: Burn captions + mix narration and music + move to final location ──
    bg_music = config.get("bg_music")
    bg_music_volume = float(config.get("bg_music_volume") or 0.15)
    narration_volume = _num(config.get("narration_volume"), 1.0, 0.0, 4.0)
    if edit.get("narration_mute"):
        narration_volume = 0.0
    bg_music_loop = bool(config.get("bg_music_loop", True))
    try:
        music_volume = max(0.0, float(config.get("music_volume", 1.0)))
    except (TypeError, ValueError):
        music_volume = 1.0
    # MangaVox's own score (mv-score): already timed to the narration, ducked and limited.
    score = config.get("score")
    # Sound effects on the story's big moments (sfx.py): a sparse stem under the narration.
    sfx_track = _build_sfx_stem(config, output_dir, render_items, clips, starts, trans, timeline, narration, narration_volume, video_dir)

    video = {
        "inputs": video_inputs, "starts": video_starts, "trans": video_trans, "look": look,
        "overlays": edit.get("overlays") or [], "work_dir": video_dir,
    }
    # The scene bursts, drawn once over their joins by the finish (above the pixel break-up).
    bursts = []
    for f0, rgb in scene_bursts(starts, trans, [c["mood"]["edge"] if c["mood"] else EDGE_COLORS["neutral"] for c in clips]):
        try:
            bursts.append((f0,) + make_flare_frames("burst", w, h, video_dir, rgb))
        except Exception as e:
            print(f"[video] Could not make the particle burst: {e}", file=sys.stderr)
    video["bursts"] = bursts
    if chapter_card_on(config, look):
        try:
            video["chapter_card"] = make_chapter_card(w, h, config.get("chapter"), video_dir)
        except Exception as e:
            print(f"[video] Could not make the chapter card: {e}", file=sys.stderr)
    if look.get("particles") and not mood_on:
        # (With the mood on, each clip carries its own atmosphere instead.)
        try:
            video["particles"] = make_particle_loop(w, h, video_dir)
        except Exception as e:
            print(f"[video] Could not make the particles: {e}", file=sys.stderr)
    if look.get("edge_glow"):
        # The glow takes each clip's mood colour as it lands.
        video["edge_colors"] = [(c["start"], c["mood"]["edge"] if c["mood"] else EDGE_COLORS["neutral"]) for c in clips]
    on_final = lambda f: report("finalize", f)
    t_final = time.monotonic()
    success = False
    if gpu:
        # The whole picture in one pass on the graphics card (gpu_render.py); clips an earlier run
        # already rendered are read as they are. Anything wrong: the CPU path below, as always.
        waiting = {id(t[0]) for t in todo}
        made = [None if id(c) in waiting else clip_paths[k] for k, c in enumerate(clips)]
        # The picture fills the clips' share of the progress bar (the finish's is the join and mux).
        picture = lambda path, on: _gpu_picture(video, clips, frame_counts, made, w, h, timeline, ass_path, config, path,
                                                lambda f: report("clips", f))
        drawn = os.path.join(video_dir, "final_gpu.mp4")
        if os.path.exists(drawn):
            os.remove(drawn)
        success = _finalize_video_with_audio_mix_and_captions(
            video, narration, final_video, ass_path, bg_music,
            narration_volume, bg_music_volume, bg_music_loop, w, h, timeline,
            score=score, music_volume=music_volume, on_fraction=on_final, sfx=sfx_track, picture=picture,
        )
        print(f"[video] Timing: GPU engine {'finished' if success else 'failed'} in {time.monotonic() - t_final:.1f} s", file=sys.stderr)
        if not success and os.path.isfile(drawn):
            # The picture was made (captions in it); only the sound or the join failed: try again
            # without the music, reusing the picture, before drawing it all again on the CPU.
            print("[video] Finalization failed, retrying without music (the GPU picture kept)", file=sys.stderr)
            kept = drawn + ".keep.mp4"
            os.replace(drawn, kept)
            success = _finalize_video_with_audio_mix_and_captions(
                video, narration, final_video, ass_path, None,
                narration_volume, 0.0, False, w, h, timeline,
                score=None, music_volume=0.0, on_fraction=on_final, picture=lambda path, on: shutil.copyfile(kept, path),
            )
        if not success:
            print("[video] The GPU engine could not make the video; rendering it the usual way", file=sys.stderr)
            # Its layered panels were only cut into layers: animate them now (the layers are cached).
            waiting_anim = {k: layer_jobs[k] for k, c in enumerate(clips) if (c.get("images") or {}).get("fg_layered")}
            if waiting_anim:
                done_anim = run_layered(waiting_anim, config, video_dir, threads=max(2, cpus() // 2))
                for n, (c, frames, path) in enumerate(layered_todo):
                    k = clips.index(c)
                    job = layer_jobs[k]
                    c["images"] = {key: v for key, v in c["images"].items() if key != "fg_layered"}
                    if k not in waiting_anim:
                        continue
                    if done_anim.get(job["id"], {}).get("ok") and os.path.isfile(job["out"]):
                        c["images"]["fg_anim"] = job["out"]
                    else:
                        plain = clip_key(c["source"], c["focus"], c["spec"], frames, w, h, fill)
                        clip_paths[k] = os.path.join(video_dir, f"clip_{c['idx']:04d}_{plain}.mp4")
                        layered_todo[n] = (c, frames, clip_paths[k])
            rest = [t for t in todo if clips.index(t[0]) not in layer_jobs] + layered_todo
            with ThreadPoolExecutor(max_workers=jobs) as pool:
                list(pool.map(lambda t: render_one(*t), sorted(rest, key=lambda t: -t[1])))
            t_final = time.monotonic()
    if not success:
        success = _finalize_video_with_audio_mix_and_captions(
            video, narration, final_video, ass_path, bg_music,
            narration_volume, bg_music_volume, bg_music_loop, w, h, timeline,
            score=score, music_volume=music_volume, on_fraction=on_final, sfx=sfx_track,
        )
        print(f"[video] Timing: finish {time.monotonic() - t_final:.1f} s", file=sys.stderr)
    if not success:
        print("[video] Finalization failed, retrying without captions, music and finish", file=sys.stderr)
        success = _finalize_video_with_audio_mix_and_captions(
            dict(video, look=None, overlays=[], chapter_card=None, bursts=[]), narration, final_video, None, None,
            narration_volume, 0.0, False, w, h, timeline,
            score=None, music_volume=0.0, on_fraction=on_final,
        )
    if not success:
        raise RuntimeError("Failed to finalize the video")

    try:
        with open(timeline_file, "w", encoding="utf-8") as f:
            json.dump({"duration": round(timeline, 3), "boundaries": [round(b, 3) for b in boundaries[:-1]]}, f)
    except OSError as e:
        print(f"[video] Could not save the timeline: {e}", file=sys.stderr)

    # Clean up temp segment files
    try:
        shutil.rmtree(video_dir)
    except Exception:
        pass

    if fmt == "reels":
        total_sec = get_audio_duration_sec(final_video) if os.path.exists(final_video) else timeline
        target = float(config.get("reel_seconds") or REEL_TARGET_SEC)
        split_reels(final_video, plan_reel_cuts(boundaries[:-1], total_sec, target=target), reels_dir)
    elif os.path.isdir(reels_dir):
        # Reels from an earlier upright render no longer match this video.
        shutil.rmtree(reels_dir, ignore_errors=True)

    report("finalize", 1.0)
    return final_video


# Music duck when the gain curve (sfx.make_duck_gain) cannot be made: a sidechain compressor
# keyed by the narration. The old gentle one (threshold 0.03, ratio 4) only took the music down
# ~7 dB under a -20 dBFS voice; this one takes it down ~19 dB (see tests/test_audio_mix.py).
SIDECHAIN_DUCK = "threshold=0.01:ratio=20:attack=10:release=300:makeup=1"


def _audio_mix_graph(first, has_music, narration_vol, music_vol, solo=False, duck=None, sfx=None):
    """Narration (input `first`) plus optional music (input `first + 1`) and SFX stem (input
    `sfx`) mixed at unity into [mix]. The music is ducked under the narration by the gain curve
    on input `duck` (sfx.make_duck_gain), else by SIDECHAIN_DUCK. With `solo` (and music) the
    narration alone is also left in [solo]."""
    fmt = f"aresample={AUDIO_RATE},aformat=sample_fmts=fltp:channel_layouts=stereo"
    nar = f"[{first}:a]{fmt},volume={narration_vol}"
    if not has_music and sfx is None:
        return f"{nar}[mix]"
    outs = ["nar"] + (["key"] if has_music and duck is None else []) + (["solo"] if solo and has_music else [])
    parts = [f"{nar}[nar]" if len(outs) == 1 else f"{nar},asplit={len(outs)}" + "".join(f"[{o}]" for o in outs)]
    labels = ["[nar]"]
    if has_music:
        parts.append(f"[{first + 1}:a]{fmt},volume={music_vol}[mus]")
        if duck is not None:
            # The same gain on both sides (an up-mix would take it down 3 dB).
            parts.append(f"[{duck}:a]aresample={AUDIO_RATE},aformat=sample_fmts=fltp,pan=stereo|c0=c0|c1=c0[gain]")
            parts.append("[mus][gain]amultiply[duck]")
        else:
            parts.append(f"[mus][key]sidechaincompress={SIDECHAIN_DUCK}[duck]")
        labels.append("[duck]")
    if sfx is not None:
        parts.append(f"[{sfx}:a]{fmt}[fx]")
        labels.append("[fx]")
    parts.append("".join(labels) + f"amix=inputs={len(labels)}:duration=first:normalize=0[mix]")
    return ";".join(parts)


def _build_sfx_stem(config, output_dir, items, clips, starts, trans, total, narration, narration_vol, work_dir):
    """The SFX stem (sfx.build_track) for the rendered clips, or None: switched off (config
    `sfx`: false / "off"; on by default), nothing worth a sound, or it failed (the video never
    waits on its effects)."""
    try:
        import sfx as sfxlib
        mode = sfxlib.mode_of(config.get("sfx", True))
        if mode == "off":
            return None
        out = os.path.join(work_dir, "sfx.wav")
        events = sfxlib.build_track(
            items, clips, starts, trans, total, narration, out, mode=mode,
            user_dir=sfxlib.user_dir_for(config, output_dir), scenes=sfxlib.load_scenes(output_dir),
            narration_vol=narration_vol,
        )
        if events:
            print("[video] Sound effects: " + ", ".join(f"{e['name']} at {e['at']:.1f}s ({e['rel_db']:+.0f} dB)" for e in events), file=sys.stderr)
            return out
    except Exception as e:
        print(f"[video] Sound effects skipped: {e}", file=sys.stderr)
    return None


def _music_duck(narration, music_args, music_vol, narration_vol, total, work_dir):
    """The music's gain curve under the voice (sfx.make_duck_gain), or None."""
    try:
        import sfx as sfxlib
        return sfxlib.make_duck_gain(narration, music_args, music_vol, narration_vol, total, os.path.join(work_dir, "music_duck.wav"))
    except Exception as e:
        print(f"[video] Music duck unavailable: {e}", file=sys.stderr)
        return None


def _measure_loudness(audio_inputs, mix_graph):
    """Integrated loudness (LUFS) of the mix, from a fast audio-only EBU R128 analysis pass."""
    cmd = [
        "ffmpeg", "-hide_banner", "-nostats", *audio_inputs,
        "-filter_complex", f"{mix_graph};[mix]ebur128=framelog=quiet[out]",
        "-map", "[out]", "-f", "null", "-",
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
        found = re.findall(r"^\s*I:\s*(-?[\d.]+|-inf)\s*LUFS", result.stderr or "", re.M)
        if result.returncode != 0 or not found:
            return None
        value = float(found[-1])
        # Below ~-70 LUFS is effectively silence: no meaningful gain to derive.
        return value if math.isfinite(value) and value > -70 else None
    except Exception as e:
        print(f"[video] Loudness measurement failed: {e}", file=sys.stderr)
        return None


def plan_segments(starts, trans, total_frames, count, min_frames=2 * FPS):
    """Where the finished picture is split so its parts can be made side by side: frame indexes
    [0, c1, ..., total_frames]. Every cut sits inside one clip, after the crossfade into it has
    finished and before the next one starts, so a part never begins mid-transition; cuts land as
    near to total_frames / count apart as that allows, no part shorter than `min_frames`.
    starts / trans as for xfade_chain."""
    base = round(starts[0] * FPS)
    first = [round(s * FPS) - base for s in starts]
    windows = []
    for k, s in enumerate(first):
        lo = s + (math.ceil(trans[k][1] * FPS) + 1 if k and trans[k] else 0)
        hi = first[k + 1] - 1 if k + 1 < len(first) else total_frames - 1
        if lo <= hi:
            windows.append((lo, hi))
    cuts = [0]
    for i in range(1, max(1, count)):
        ideal = total_frames * i / count
        best = None
        for lo, hi in windows:
            c = min(hi, max(lo, round(ideal)))
            if c - cuts[-1] < min_frames or total_frames - c < min_frames:
                continue
            if best is None or abs(c - ideal) < abs(best - ideal):
                best = c
        if best is not None and best > cuts[-1]:
            cuts.append(best)
    return cuts + [total_frames]


def segment_jobs():
    """Parts of the finished picture made at once: each is one ffmpeg (decode, crossfades, look,
    captions, encode) moving whole 1440p frames through memory, so more than about four at
    once only fight over memory bandwidth (measured on a 16-core desktop): one per four cores, 2 to 4
    (MANGAVOX_RENDER_JOBS overrides)."""
    return _env_int("MANGAVOX_RENDER_JOBS") or max(2, min(4, cpus() // 4))


def _concat_list(paths, list_path):
    """An ffmpeg concat-demuxer list of `paths`."""
    with open(list_path, "w", encoding="utf-8") as f:
        for p in paths:
            q = os.path.abspath(p).replace("\\", "/").replace("'", "'\\''")
            f.write(f"file '{q}'\n")


def _finalize_video_with_audio_mix_and_captions(
    video, narration, output_video, ass_path, bg_music,
    narration_vol, bg_vol, bg_loop, w, h, total, score=None, music_volume=1.0, on_fraction=None, sfx=None, picture=None,
):
    """
    Joins the clip videos with their crossfades, gives them the look, burns subtitles, and
    mixes the narration with the score (or looped background music). With music, a second
    "Narration only" audio track is written too.

    The picture is made in parts side by side (plan_segments; each its own ffmpeg, cut where no
    transition runs, on the whole video's clock so captions and layers line up) while the audio
    is mixed beside it; the parts are then joined without re-encoding and the audio muxed in.

    Loudness is two-pass and linear: the mix is measured first, then one fixed gain brings
    it to LOUD_I and a limiter catches peaks, so music never swells in narration pauses.
    The music is ducked under the voice by a gain curve made from both (sfx.make_duck_gain),
    and `sfx` (an SFX stem, levelled against the narration already) is mixed in at unity.
    """
    import bisect
    import threading
    work = video.get("work_dir") or os.path.dirname(os.path.abspath(output_video))
    os.makedirs(work, exist_ok=True)

    # The chapter's own score replaces any looped background track.
    has_score = bool(score) and os.path.exists(score) and music_volume > 0
    has_bg = bool(bg_music) and os.path.exists(bg_music) and not has_score and music_volume > 0 and bg_vol > 0
    audio_inputs = ["-i", narration]
    music_vol = 1.0
    if has_score:
        audio_inputs += ["-i", score]
        music_vol = music_volume
    elif has_bg:
        if bg_loop:
            audio_inputs += ["-stream_loop", "-1"]
        audio_inputs += ["-i", bg_music]
        music_vol = bg_vol * music_volume
    has_music = has_score or has_bg
    has_sfx = bool(sfx) and os.path.exists(sfx)
    if on_fraction:
        on_fraction(0.0)

    # ── Audio (made beside the picture): duck, measure, then one gain, a limiter, AAC ──
    audio_path = os.path.join(work, "final_audio.m4a")

    def make_audio():
        inputs = list(audio_inputs)
        n = 2 if has_music else 1
        duck = fx = None
        if has_music:
            gain = _music_duck(narration, audio_inputs[2:], music_vol, narration_vol, total, work)
            if gain:
                inputs += ["-i", gain]
                duck, n = n, n + 1
        if has_sfx:
            inputs += ["-i", sfx]
            fx = n
        mix_graph = lambda solo=False: _audio_mix_graph(0, has_music, narration_vol, music_vol, solo=solo, duck=duck, sfx=fx)
        measured = _measure_loudness(inputs, mix_graph())
        gain_db = 0.0 if measured is None else max(-20.0, min(20.0, LOUD_I - measured))
        limit = round(10 ** (LOUD_TP / 20), 4)
        graph = [
            mix_graph(solo=has_music),
            f"[mix]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a]",
        ]
        maps = ["-map", "[a]"]
        if has_music:
            graph.append(f"[solo]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a2]")
            maps += ["-map", "[a2]"]
        cmd = [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error", *inputs,
            "-filter_complex", ";".join(graph), *maps,
            "-c:a", "aac", "-b:a", "192k", "-ar", str(AUDIO_RATE), "-t", f"{total:.3f}", audio_path,
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
        if result.returncode != 0 or not os.path.exists(audio_path):
            raise RuntimeError(f"audio mix failed: {result.stderr[:300]}")

    # ── The picture, in parts ──
    starts, trans = video["starts"], video["trans"]
    base = round(starts[0] * FPS)
    first = [round(s * FPS) - base for s in starts]
    total_frames = max(1, math.ceil(total * FPS - 1e-6))
    jobs = segment_jobs()
    # Two parts per job at least, and no more than about four clips per part (each opens its
    # clips at the frames it needs), so a long chapter never opens dozens of decoders at once.
    count = max(jobs * 2, math.ceil(len(starts) / 4))
    cuts = plan_segments(starts, trans, total_frames, max(1, min(count, total_frames // (3 * FPS))))
    parts = [(a, b, os.path.join(work, f"final_{i:03d}.mp4")) for i, (a, b) in enumerate(zip(cuts, cuts[1:]))]
    jobs = min(jobs, len(parts))
    threads = ffmpeg_threads(jobs)
    has_subs = bool(ass_path) and os.path.exists(ass_path)
    particles = video.get("particles") if video.get("look") and video["look"].get("particles") else None
    if particles and not os.path.exists(particles):
        particles = None
    vmask = None
    if video.get("look") and vignette_angle(video["look"]):
        try:
            vmask = make_vignette_mask(w, h, vignette_angle(video["look"]), work)
        except Exception as e:
            print(f"[video] {e}; using the vignette filter", file=sys.stderr)
    # The edge glow's stills in each mood colour the video uses (none drawn if one cannot be made).
    edge_stills = {}
    if video.get("look") and video["look"].get("edge_glow"):
        ih = h - 2 * (letterbox_bar(w, h) if video["look"].get("letterbox") else 0)
        try:
            for rgb in dict.fromkeys(tuple(c) for _, c in video.get("edge_colors") or [(0, EDGE_COLORS["neutral"])]):
                edge_stills[rgb] = make_edge_glow(w, ih, rgb, work)
        except Exception as e:
            print(f"[video] {e}; no edge glow", file=sys.stderr)
            edge_stills = {}

    def part_cmd(a, b, path, codec):
        ka = bisect.bisect_right(first, a) - 1
        kb = bisect.bisect_right(first, b - 1) - 1
        inputs, graph, labels = [], [], []

        def clip_input(k, start):
            """Clip k from its frame `start` (seeking half a frame early, so rounding never
            drops that frame; only frames from the keyframe before it are decoded)."""
            seek = ["-ss", f"{(start - 0.5) / FPS:.6f}"] if start else []
            inputs.extend(["-threads", str(threads), *seek, "-i", video["inputs"][k]])
            n = len(labels)
            labels.append(f"p{n}")
            # Held in the clips' own 4:2:0: xfade works in 4:4:4 only, and left free the whole part
            # (concat takes one format) was turned to 4:4:4 and back, two full-frame conversions on
            # every frame. Now only a transition's few frames are.
            graph.append(f"[{n}:v]settb=AVTB,setpts=PTS-STARTPTS,format=yuv420p[p{n}]")
            return f"p{n}"

        # The part as a run of pieces: each clip alone between transitions, and each transition
        # crossfaded from just the frames it covers. Every input is opened at the frame it is
        # needed from, so nothing is decoded twice or held in memory waiting for its turn.
        ends = []
        for k in range(ka + 1, kb + 1):
            ends.append(min(b, first[k] + math.ceil(trans[k][1] * FPS) + 1))
        overlapping = any(e > (first[k + 1] if k + 1 <= kb else b) for k, e in zip(range(ka + 1, kb + 1), ends))
        if overlapping:
            # Transitions closer together than their own length: one crossfade chain instead.
            labels_ = [clip_input(k, a - first[k] if k == ka else 0) for k in range(ka, kb + 1)]
            graph.append(xfade_chain(labels_, [(a + base) / FPS] + list(starts[ka + 1:kb + 1]), [None] + list(trans[ka + 1:kb + 1]), "vcat"))
        else:
            pieces = []
            pos = a
            for k in range(ka, kb + 1):
                if k > ka:
                    # The transition into clip k: from where it starts until it has finished. It is
                    # crossfaded from one frame earlier and that frame dropped, so the crossfade's
                    # offset (a hair before the frame, see xfade_chain) is the whole video's.
                    end = ends[k - ka - 1]
                    prev = clip_input(k - 1, first[k] - 1 - first[k - 1])
                    cur = clip_input(k, 0)
                    graph.append(xfade_chain([prev, cur], [(first[k] - 1) / FPS, first[k] / FPS], [None, trans[k]], f"x{k}"))
                    graph.append(f"[x{k}]trim=start_frame=1:end_frame={end - first[k] + 1},setpts=PTS-STARTPTS[t{k}]")
                    pieces.append(f"t{k}")
                    pos = end
                stop = min(b, first[k + 1]) if k < kb else b
                if stop > pos:
                    lab = clip_input(k, pos - first[k])
                    graph.append(f"[{lab}]trim=end_frame={stop - pos},setpts=PTS-STARTPTS[s{k}]")
                    pieces.append(f"s{k}")
                    pos = stop
            if len(pieces) == 1:
                graph.append(f"[{pieces[0]}]null[vcat]")
            else:
                graph.append("".join(f"[{q}]" for q in pieces) + f"concat=n={len(pieces)}:v=1:a=0[vcat]")
        # Back on an exact frame clock, starting at 0.
        graph.append(f"[vcat]settb=AVTB,setpts=N/({FPS}*TB)[vrel]")
        # On the whole video's clock from here, so the captions, layers and particles line up.
        t0 = a / FPS
        graph.append(f"[vrel]setpts=PTS+{t0:.6f}/TB[vraw0]")
        extra = []
        # The scene bursts over this part's joins (each from the frame it has reached here).
        cur = "vraw0"
        for i, (f0, pattern, count) in enumerate(video.get("bursts") or []):
            if f0 + count <= a or f0 >= b:
                continue
            skip = max(0, a - f0)
            graph.append(f"[{len(labels) + extra.count('-i')}:v]scale={w}:{h}:flags=bicubic,format=yuva420p,setparams=colorspace=bt470bg:range=tv,"
                         f"settb=AVTB,setpts=(N+{f0 + skip})/({FPS}*TB)[bst{i}]")
            graph.append(f"[{cur}][bst{i}]overlay=0:0:format=yuv420:eof_action=pass[bo{i}]")
            extra += ["-framerate", str(FPS), "-start_number", str(skip), "-i", pattern]
            cur = f"bo{i}"
        graph.append(f"[{cur}]null[vraw]")
        if video.get("look"):
            plabel = vlabel = None
            if particles:
                loop = round(PARTICLE_SEC * FPS)
                graph.append(f"[{len(labels) + extra.count('-i')}:v]trim=start_frame={a % loop},setparams=colorspace=bt470bg:range=tv,settb=AVTB,setpts=(N+{a})/({FPS}*TB)[ptc]")
                extra += ["-stream_loop", "-1", "-i", particles]
                plabel = "ptc"
            if vmask:
                # One still: overlay repeats it on every frame.
                vlabel = f"{len(labels) + extra.count('-i')}:v"
                extra += ["-i", vmask]
            edge = []
            if edge_stills:
                colors = video.get("edge_colors") or [(0.0, EDGE_COLORS["neutral"])]
                for s0, s1, rgb in edge_segments(colors, t0, b / FPS):
                    edge.append((s0, s1, f"{len(labels) + extra.count('-i')}:v"))
                    extra += ["-i", edge_stills[tuple(rgb)]]
            graph.append(look_filters(video["look"], w, h, "vraw", "vx", plabel, vlabel, edge))
        else:
            graph.append("[vraw]null[vx]")
        card = video.get("chapter_card")
        overlay_inputs, overlays = overlay_graph(
            video.get("overlays"), w, h, total, "vx", "vc" if card and a < card[1] else "vo",
            len(labels) + extra.count("-i"), work, t0=t0,
        )
        graph.append(overlays)
        if card and a < card[1]:
            # The chapter badge over the opening seconds (under the captions).
            bar = letterbox_bar(w, h) if (video.get("look") or {}).get("letterbox") else 0
            graph.append(chapter_card_graph(card, w, h, bar, "vc", "vo", f"{len(labels) + extra.count('-i') + overlay_inputs.count('-i')}:v", a))
            overlay_inputs = overlay_inputs + ["-framerate", str(FPS), "-start_number", str(a), "-i", card[0]]
        if has_subs:
            from caption_engine import subtitles_filter
            graph.append(f"[vo]{subtitles_filter(ass_path)},setpts=PTS-STARTPTS,format=yuv420p[v]")
        else:
            graph.append("[vo]setpts=PTS-STARTPTS,format=yuv420p[v]")
        return [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-filter_complex_threads", str(threads),
            *inputs, *extra, *overlay_inputs,
            *graph_args(";".join(graph), os.path.join(work, f"graph_{a:07d}.txt")),
            "-map", "[v]", "-frames:v", str(b - a), "-r", str(FPS),
            *codec, "-an", path,
        ]

    lock = threading.Lock()
    progress = {}

    def report_part(i, f):
        if on_fraction:
            with lock:
                progress[i] = f * (parts[i][1] - parts[i][0])
                on_fraction(min(0.98, 0.98 * sum(progress.values()) / total_frames))

    def make_part(i):
        a, b, path = parts[i]
        tmp = path + ".part.mp4"

        def run(codec):
            code, stderr = run_ffmpeg_progress(part_cmd(a, b, tmp, codec), (b - a) / FPS, lambda f: report_part(i, f), timeout=3600)
            if code != 0 or not os.path.exists(tmp):
                print(f"[video] Part {i + 1} of the picture failed with {codec[1]}: {stderr[:300]}", file=sys.stderr)
                return False, stderr
            return True, stderr

        ok, stderr = run_codecs(final_codecs(threads), run)
        if not ok:
            raise RuntimeError(f"part {i + 1} of the picture failed: {stderr[:300]}")
        os.replace(tmp, path)

    if picture:
        # The GPU engine: `picture(path, on_fraction)` draws the whole picture in one pass, in
        # place of the parts (the audio, the join and the mux are the same).
        parts = [(0, total_frames, os.path.join(work, "final_gpu.mp4"))]
        jobs = 1

        def make_part(i):
            a, b, path = parts[i]
            tmp = path + ".part.mp4"
            picture(tmp, lambda f: report_part(i, f))
            os.replace(tmp, path)

    try:
        # The audio is mixed beside the parts (it needs about one core), not in a part's slot.
        with ThreadPoolExecutor(max_workers=1) as side, ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = [side.submit(make_audio)] + [pool.submit(make_part, i) for i in range(len(parts))]
            for f in as_completed(futures):
                f.result()
    except Exception as e:
        print(f"[video] Finalization failed: {e}", file=sys.stderr)
        return False

    # ── Join the parts (no re-encode) and mux the audio in ──
    list_path = os.path.join(work, "final_parts.txt")
    _concat_list([p for _, _, p in parts], list_path)
    first_track = "Narration + music" if has_music else "Narration"
    # MP4 keeps a track's name as its handler name (players list tracks by it).
    tracks = ["-metadata:s:a:0", f"title={first_track}", "-metadata:s:a:0", f"handler_name={first_track}", "-disposition:a:0", "default"]
    if has_music:
        tracks += ["-metadata:s:a:1", "title=Narration only", "-metadata:s:a:1", "handler_name=Narration only", "-disposition:a:1", "0"]
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-f", "concat", "-safe", "0", "-i", list_path, "-i", audio_path,
        "-map", "0:v:0", "-map", "1:a", "-c", "copy",
        # The concat demuxer rounds each part's length, which drifts the picture's timestamps by
        # a fraction of a millisecond per part: snap them back onto the frame clock.
        "-bsf:v", f"setts=pts=round(PTS*TB*{FPS})/(TB*{FPS}):dts=round(DTS*TB*{FPS})/(TB*{FPS})",
        *tracks,
        "-movflags", "+faststart", output_video,
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
        if result.returncode != 0:
            print(f"[video] Joining the picture failed: {result.stderr[:400]}", file=sys.stderr)
            return False
        if on_fraction:
            on_fraction(1.0)
        return os.path.exists(output_video) and os.path.getsize(output_video) > 1000
    except Exception as e:
        print(f"[video] Joining the picture failed: {e}", file=sys.stderr)
        return False
