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

FPS = 30
AUDIO_RATE = 48000
# Clips per intermediate xfade pass (keeps the number of open decoders bounded).
CHUNK_CLIPS = 14
# Panel inset from the frame edge (fraction of the frame) when the blurred fill is used.
FG_MARGIN = 0.05
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
CLIP_VERSION = 5

# Looks (Studio → Video → Look): how panels are laid out and move, which transitions turn the
# page, and what the finished picture gets. "duo" puts tall panels side by side like the big
# manhwa recap channels: each new one slides into the older one's place while the other stays.
# "punch" snaps action beats in with a zoom, a shake and a white flash. "ramp" times the moves
# to the beat (a quick snap on a short beat, a slow drift on a long one). The effect switches
# are listed in LOOK_EFFECTS; "color" is one of COLOR_PRESETS.
_FX_OFF = {"leaks": False, "chroma": False, "parallax": False, "particles": False, "letterbox": False}
LOOKS = {
    "clean": {
        "duo": False, "punch": False, "push": FG_PUSH, "transitions": PAGE_TRANSITIONS, "ramp": False,
        "grade": False, "vignette": False, "grain": False, "glow": False, "impact": False,
        **_FX_OFF, "color": "none",
    },
    "cinematic": {
        "duo": False, "punch": False, "push": 0.06, "transitions": ["fade", "fadeblack", "smoothleft", "fade"], "ramp": False,
        "grade": True, "vignette": True, "grain": True, "glow": True, "impact": False,
        **_FX_OFF, "leaks": True, "letterbox": True, "color": "warm",
    },
    "recap": {
        "duo": True, "punch": False, "push": FG_PUSH, "transitions": ["smoothleft", "smoothup", "fade", "smoothright"], "ramp": False,
        "grade": True, "vignette": True, "grain": False, "glow": False, "impact": False,
        **_FX_OFF, "color": "none",
    },
    "dynamic": {
        "duo": True, "punch": True, "push": 0.08, "ramp": True,
        "transitions": ["smoothleft", "burn", "smoothup", "zoomblur", "circleopen", "smoothright", "burn", "slidedown", "zoomblur"],
        "grade": True, "vignette": True, "grain": False, "glow": True, "impact": True,
        **_FX_OFF, "leaks": True, "chroma": True, "parallax": True, "color": "none",
    },
}
DEFAULT_LOOK = "dynamic"
# Effects the user can switch on or off over a look.
LOOK_EFFECTS = ("grade", "vignette", "grain", "glow", "impact", "leaks", "chroma", "parallax", "particles", "letterbox")
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
CLIP_TRANSITIONS = ("burn", "zoomblur")
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
ZOOM_BLUR = 0.14
# Particle loop length (seconds): one seamless loop, made once per render.
PARTICLE_SEC = 4.0
# Duo layout: panels narrower than this (w/h) pair up; frame edge and middle gap (of the
# width); the narrowest a panel is drawn (of its half), taller ones pan down instead.
DUO_MAX_ASPECT = 0.9
DUO_OUTER = 0.04
DUO_GAP = 0.025
DUO_MIN_FILL = 0.72
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


def duo_geometry(iw, ih, w, h, slot):
    """Where a panel of iw x ih sits in a w x h frame's left (slot 0) or right (slot 1) half:
    drawn at `size`, with `vis` rows visible from `pos`; `slack` rows (a tall panel) pan by."""
    outer = round(w * DUO_OUTER)
    slot_w = (w - 2 * outer - round(w * DUO_GAP)) // 2
    dw = min(slot_w, max(round(slot_w * DUO_MIN_FILL), round(iw * h / ih))) // 2 * 2
    dh = max(2, round(ih * dw / iw) // 2 * 2)
    vis = min(dh, h)
    cx = outer + slot_w / 2 if slot == 0 else w - outer - slot_w / 2
    return {"size": (dw, dh), "vis": vis, "pos": (int(cx - dw / 2) // 2 * 2, (h - vis) // 4 * 2), "slack": dh - vis}


def look_filters(look, w, h, src, out, particles=None):
    """Finishing filters for `look` on [src] (a w x h picture), labelled [out]: a gentle grade
    and colour preset (one lookup table), floating particles (the looped [particles] stream,
    when given), a vignette, fine film grain and letterbox bars, each only when the look has it
    on. (The glow is baked into each still instead: it would cost more than the rest together
    per frame.)"""
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
    if look.get("vignette"):
        tail.append("vignette=angle=PI/6")
    if look.get("grain"):
        tail.append("noise=alls=4:allf=t")
    bar = letterbox_bar(w, h) if look.get("letterbox") else 0
    if bar:
        tail.append(f"drawbox=x=0:y=0:w={w}:h={bar}:color=black:t=fill")
        tail.append(f"drawbox=x=0:y={h - bar}:w={w}:h={bar}:color=black:t=fill")
    parts.append(f"[{cur}]{','.join(tail) if tail else 'null'}[{out}]")
    return ";".join(parts)


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

def plan_transitions(durations, pages, kind="crossfade", base=0.5, names=None, joins=None):
    """Plan crossfades between clips of `durations` seconds without moving the narration.

    Returns (starts, trans): starts[k] is when clip k's narration begins (running sum)
    and trans[k] = (xfade transition, seconds) for the change into clip k (trans[0] is
    None). Each clip's video runs on for trans[k + 1] seconds past its narration, so the
    fade into clip k begins exactly at starts[k] and the total stays sum(durations).
    Same-page beats dissolve quickly, page changes rotate through `names` (default
    PAGE_TRANSITIONS), short clips get shorter fades, and kind "none" gives one-frame (hard)
    cuts. joins[k] "cut" cuts into clip k (it animates its own entrance: a duo slide) and
    "impact" makes it a quick dissolve (the clip flashes in itself).
    """
    names = names or PAGE_TRANSITIONS
    joins = joins or [None] * len(durations)
    starts = []
    t = 0.0
    for d in durations:
        starts.append(t)
        t += d
    frame = 1.0 / FPS
    trans = [None]
    turn = 0
    for k in range(1, len(durations)):
        if kind == "none" or joins[k] == "cut":
            name, dur = "fade", frame
        elif joins[k] == "impact":
            name, dur = "fade", 0.12
        elif pages[k] == pages[k - 1]:
            name, dur = SAME_PAGE_TRANSITION[0], min(SAME_PAGE_TRANSITION[1], base)
        else:
            name, dur = names[turn % len(names)], base
            turn += 1
        dur = max(frame, min(dur, 0.45 * durations[k - 1], 0.45 * durations[k]))
        trans.append((name, round(dur, 3)))
    return starts, trans


def plan_clip_fx(starts, trans, pages, joins=None, leaks=False, impacts=None, gap=None):
    """The page-turn effects each clip draws itself, in its own seconds. A burn or zoom blur
    into clip k is drawn by both clips (the same frames at the same moment, under a plain
    dissolve, so the join cannot show): clip k-1 gets "<name>_out": (when the change starts,
    seconds) and clip k "<name>_in": seconds. With `leaks`, a clip that turns to a new page
    some other way (and the opening clip) gets "leak": a flare blooming as it lands, at most
    one every `gap` seconds (a burn counts) so it stays an accent; an action beat (`impacts`)
    flashes instead."""
    joins = joins or [None] * len(starts)
    impacts = impacts or [False] * len(starts)
    gap = LEAK_GAP if gap is None else gap
    fx = [{} for _ in starts]
    last = -gap
    if leaks and fx and not impacts[0]:
        fx[0]["leak"] = True
        last = 0.0
    for k in range(1, len(starts)):
        if not trans[k] or joins[k] == "impact" or impacts[k] or pages[k] == pages[k - 1]:
            continue
        name, dur = trans[k]
        if name in CLIP_TRANSITIONS and dur > 1.5 / FPS:
            last = starts[k]
            fx[k - 1][f"{name}_out"] = (round(starts[k] - starts[k - 1], 3), dur)
            fx[k][f"{name}_in"] = dur
            if name == "burn":
                # Burns alternate sides; both clips draw the same one.
                fx[k - 1]["burn_flip"] = fx[k]["burn_flip_in"] = k % 2 == 0
        elif leaks and starts[k] - last >= gap:
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


def plan_chunks(n, size=CHUNK_CLIPS):
    """Split n clips into consecutive [first, end) runs of at most `size` clips."""
    return [(i, min(n, i + size)) for i in range(0, n, size)]


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
        # A burn or zoom blur is drawn by the clips on both sides; the join itself dissolves.
        name = "fade" if name in CLIP_TRANSITIONS else name
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


def _warp(window, zoom, frames, interp, ease=None):
    """One still held for `frames` frames with an eased move: each frame shows the
    sub-rectangle 1/zoom the size of the image, placed by the `window` fractions, stretched
    over the whole frame. perspective resamples with subpixel precision, so slow moves glide
    (zoompan rounds its crop to whole, chroma-aligned pixels and visibly steps)."""
    e = ease or _ease("on", frames)
    z = "(" + zoom.format(e=e) + ")"
    px, py = (v.format(e=e) for v in window)
    cw, ch = f"W/{z}", f"H/{z}"
    x, y = f"(W-{cw})*{px}", f"(H-{ch})*{py}"
    return (
        f"{_hold(frames)},"
        f"perspective=x0='{x}':y0='{y}':x1='{x}+{cw}':y1='{y}':"
        f"x2='{x}':y2='{y}+{ch}':x3='{x}+{cw}':y3='{y}+{ch}':"
        f"interpolation={interp}:eval=frame"
    )


# ── Per-clip images ──

def _crop_to_focus(page_image, focus_point, aspect, out_path):
    """Crop `page_image` around a 0..1000 focus box (15% margin, widened to `aspect`)."""
    from PIL import Image
    img = Image.open(page_image)
    img_w, img_h = img.size

    ymin, xmin, ymax, xmax = focus_point
    y_min = int(ymin * img_h / 1000)
    x_min = int(xmin * img_w / 1000)
    y_max = int(ymax * img_h / 1000)
    x_max = int(xmax * img_w / 1000)

    pad_w = int((x_max - x_min) * 0.15)
    pad_h = int((y_max - y_min) * 0.15)
    x_min = max(0, x_min - pad_w)
    y_min = max(0, y_min - pad_h)
    x_max = min(img_w, x_max + pad_w)
    y_max = min(img_h, y_max + pad_h)

    box_w = x_max - x_min
    box_h = y_max - y_min
    if box_w <= 0 or box_h <= 0:
        return page_image
    if box_w / box_h > aspect:
        new_h = int(box_w / aspect)
        center_y = (y_min + y_max) // 2
        y_min = max(0, center_y - new_h // 2)
        y_max = min(img_h, y_min + new_h)
        if y_max - y_min < new_h:
            y_min = max(0, y_max - new_h)
    else:
        new_w = int(box_h * aspect)
        center_x = (x_min + x_max) // 2
        x_min = max(0, center_x - new_w // 2)
        x_max = min(img_w, x_min + new_w)
        if x_max - x_min < new_w:
            x_min = max(0, x_max - new_w)
    img.convert("RGB").crop((x_min, y_min, x_max, y_max)).save(out_path, "JPEG", quality=95)
    return out_path


def _blurred_fill(img, w, h):
    """The frame-sized background: `img` zoomed in past "cover", blurred, desaturated, darkened."""
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
    small = ImageEnhance.Brightness(small).enhance(0.55)
    return small.resize((w, h), Image.BICUBIC).filter(ImageFilter.GaussianBlur(radius=max(2, w // 400)))


def _bloom(img):
    """A soft glow on a still: a blurred quarter-size copy screened over it, the highlights most."""
    from PIL import Image, ImageChops, ImageEnhance, ImageFilter
    small = img.resize((max(1, img.width // 4), max(1, img.height // 4)), Image.BOX)
    small = small.filter(ImageFilter.GaussianBlur(radius=max(2.0, small.width / 60)))
    glow = ImageEnhance.Brightness(small.resize(img.size, Image.BICUBIC)).enhance(0.14)
    return ImageChops.screen(img, glow)


# ── Light leaks, film burns and particles (made once per render, small, then scaled) ──

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


def make_flare_frames(kind, w, h, work_dir):
    """PNG frames (a quarter of the frame size, RGBA) of a light leak ("leak": a warm flare with
    a lens streak sweeping across as a page lands) or a film burn ("burn": a hot orange-to-white
    bloom that swells over a cut and dies away). The clips scale them up: they are soft, so the
    low resolution never shows, and they cost a few milliseconds on the frames they touch only.
    Returns (ffmpeg input pattern, frame count)."""
    from PIL import Image
    import random
    sw, sh = max(32, w // 4), max(32, h // 4)
    seconds = LEAK_SEC if kind == "leak" else BURN_SEC
    count = max(2, round(seconds * FPS))
    folder = os.path.join(work_dir, f"fx_{kind}_{sw}x{sh}")
    pattern = os.path.join(folder, "f_%03d.png")
    if os.path.exists(os.path.join(folder, f"f_{count - 1:03d}.png")):
        return pattern, count
    os.makedirs(folder, exist_ok=True)
    rng = random.Random(7)
    for i in range(count):
        u = i / (count - 1)
        img = Image.new("RGBA", (sw, sh), (0, 0, 0, 0))
        if kind == "leak":
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
            env *= 0.88 + 0.12 * rng.random()
            grow = 0.55 + 1.9 * min(1.0, u / peak) ** 0.8
            cx, cy = sw * 0.12, sh * 0.62
            _paint(img, _glow_sprite((sh * 2.6 * grow, sh * 2.2 * grow), (232, 64, 18), 0.95 * env), cx, cy)
            _paint(img, _glow_sprite((sh * 1.9 * grow, sh * 1.6 * grow), (255, 150, 48), 0.95 * env), cx + 0.06 * sw, cy)
            _paint(img, _glow_sprite((sh * 1.2 * grow, sh * 1.0 * grow), (255, 244, 220), 0.98 * env), cx + 0.1 * sw, cy - 0.04 * sh)
            # A second, smaller burn from the far edge.
            _paint(img, _glow_sprite((sh * 1.4 * grow, sh * 1.2 * grow), (255, 120, 30), 0.7 * env), sw * 0.95, sh * 0.2)
        img.save(os.path.join(folder, f"f_{i:03d}.png"))
    return pattern, count


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


def _prepare_duo_images(src, idx, w, h, slot, work_dir, glow=False, inset=0, drift=0):
    """Stills for a duo clip: the panel at its drawn size for `slot` (between letterbox bars
    `inset` pixels high), and the blurred fill (`drift` pixels wider each side, for parallax)."""
    from PIL import Image
    img = Image.open(src)
    img.load()
    img = img.convert("RGB")
    geo = duo_geometry(img.width, img.height, w, h - 2 * inset, slot)
    geo["pos"] = (geo["pos"][0], geo["pos"][1] + inset)
    fg_path = os.path.join(work_dir, f"dfg_{idx:04d}.jpg")
    fg = img.resize(geo["size"], Image.LANCZOS)
    (_bloom(fg) if glow else fg).save(fg_path, "JPEG", quality=95)
    bg_path = os.path.join(work_dir, f"dbg_{idx:04d}{'_d' if drift else ''}.jpg")
    _blurred_fill(img, w + 2 * drift, h).save(bg_path, "JPEG", quality=92)
    return {"fg": fg_path, "bg": bg_path, "geo": geo, "drift": drift}


def panel_margins(w, h, fill, inset=0):
    """Space (x, y pixels) left around a single panel: a margin on the blurred fill, and room
    for letterbox bars `inset` pixels high so the panel sits whole between them."""
    mx, my = (round(w * FG_MARGIN), round(h * FG_MARGIN)) if fill == "blur" else (0, 0)
    if inset:
        my = max(my, inset + (round(h * 0.025) if fill == "blur" else 0))
    return mx, my


def _prepare_clip_images(src, idx, w, h, fill, work_dir, glow=False, inset=0, drift=0):
    """Write this clip's stills: the upscaled panel and the frame-sized background it sits on.
    For the blurred fill the background is the panel zoomed in past "cover", blurred, darkened
    and desaturated, with the panel's soft drop shadow baked in; otherwise black. The
    background is composed once here; with `drift` (pixels) it is that much larger on every
    side, for a parallax drift. Returns where everything goes."""
    from PIL import Image, ImageDraw, ImageFilter

    img = Image.open(src)
    img.load()
    img = img.convert("RGB")
    iw, ih = img.size
    blur = fill == "blur"
    drift = drift if blur else 0
    mx, my = panel_margins(w, h, fill, inset)
    bw, bh = w - 2 * mx, h - 2 * my
    scale = min(bw / iw, bh / ih)
    fw = max(2, int(iw * scale) // 2 * 2)
    fh = max(2, int(ih * scale) // 2 * 2)
    fx = (w - fw) // 4 * 2
    fy = (h - fh) // 4 * 2

    fg_path = os.path.join(work_dir, f"fg_{idx:04d}.jpg")
    fg = img.resize((fw, fh), Image.LANCZOS)
    (_bloom(fg) if glow else fg).save(fg_path, "JPEG", quality=95)

    bg_path = os.path.join(work_dir, f"bg_{idx:04d}{'_d' if drift else ''}.jpg")
    if blur:
        bg = _blurred_fill(img, w + 2 * drift, h + 2 * drift)
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
        "fg": fg_path, "fg_pos": (fx, fy), "bg": bg_path, "drift": drift,
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
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        *inputs,
        "-filter_complex", ";".join(graph),
        "-map", "[v]", "-frames:v", str(frames), "-r", str(FPS),
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
        "-an", tmp,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    if result.returncode != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"clip render failed: {result.stderr[:300]}")
    os.replace(tmp, out_path)


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


def _chroma(w):
    """RGB split for an action beat: red and blue torn apart at the hit, settling back as the
    punch dies out (only its frames go through RGB)."""
    s = max(2, round(w * 0.006))
    chain = ["format=gbrp"]
    for a, b, k in ((0, 0.12, 1.0), (0.12, 0.24, 0.6), (0.24, 99, 0.3)):
        amt = max(1, round(s * k))
        chain.append(
            f"rgbashift=rh=-{amt}:bh={amt}:rv={-max(0, amt // 3)}:bv={max(0, amt // 3)}:"
            f"enable='gte(t,{a * IMPACT_SEC:.3f})*lt(t,{b * IMPACT_SEC:.3f})'"
        )
    chain.append("format=yuv420p")
    return ",".join(chain)


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
    if clip.get("impact") and clip.get("chroma"):
        cur = _window(graph, cur, 0, round(IMPACT_SEC * FPS), frames, _chroma(w), "ca")
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


def _render_clip(clip, frames, w, h, out_path):
    """Render one clip's video (no audio): the still background with the Ken Burns panel on it.
    An action beat ("punch") snaps in from a zoom, jolts and flashes white. With parallax the
    background drifts the panel's way, slower; with "ramp" the move is timed to the beat."""
    if clip.get("duo"):
        return _render_duo_clip(clip, frames, w, h, out_path)
    imgs = clip["images"]
    motion = clip["motion"]
    impact = clip.get("impact", False)
    ramp = clip.get("ramp", False)
    # Short clips move proportionally less, so a half-second beat never lurches; with the speed
    # ramp a short beat snaps through more of the move instead, and quickly.
    k = min(1.0, frames / (3.0 * FPS))
    if ramp and frames < 2 * FPS:
        k = max(k, 0.7)
    push = round(clip.get("push", FG_PUSH) * k, 4)
    fg_zoom = _motion_zoom(motion, push, round(1 + (FG_PAN_ZOOM - 1) * k, 4))
    window = FG_WINDOW[motion]
    ease = ramp_ease("on", frames, ramp)
    if impact and motion != "still":
        n = max(2, round(IMPACT_SEC * FPS * 0.6))
        fg_zoom = f"1+0.12*pow(1-min(1,on/{n}),3)+{push}*{{e}}"
        window = FG_WINDOW["push"]
    fx, fy = imgs["fg_pos"]
    jolt = round(h * 0.012) if impact else 0
    bg = f"[0:v]{TV}format=yuv420p,setsar=1,{_hold(frames)}"
    drift = imgs.get("drift", 0)
    if drift:
        dx, dy = PARALLAX_DIRS.get(motion, (0.6, 0))
        e = f"(2*{ramp_ease('n', frames, ramp)}-1)"
        bg += f",crop={w}:{h}:x='{drift}+{round(dx * drift * k, 2)}*{e}':y='{drift}+{round(dy * drift * k, 2)}*{e}'"
    inputs = ["-i", imgs["bg"], "-i", imgs["fg"]]
    graph = [
        f"{bg}[bg]",
        f"[1:v]{TV}format=yuv420p,{_warp(window, fg_zoom, frames, 'cubic', ease)},setsar=1[fg]",
        f"[bg][fg]overlay=x='{fx}+{_shake(jolt, 'x')}':y='{fy}+{_shake(jolt, 'y')}':eval=frame{_flash(impact)},format=yuv420p[base]",
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


def _render_chunk(paths, starts, trans, out_path, seconds=0.0, on_fraction=None):
    """Crossfade a run of clips into one intermediate video."""
    inputs = []
    graph = []
    labels = []
    for i, p in enumerate(paths):
        inputs += ["-i", p]
        graph.append(f"[{i}:v]settb=AVTB,setpts=PTS-STARTPTS[c{i}]")
        labels.append(f"c{i}")
    graph.append(xfade_chain(labels, starts, trans, "v"))
    tmp = out_path + ".part.mp4"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        *inputs,
        "-filter_complex", ";".join(graph),
        "-map", "[v]", "-r", str(FPS),
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
        "-an", tmp,
    ]
    code, stderr = run_ffmpeg_progress(cmd, seconds, on_fraction, timeout=3600)
    if code != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"Failed to join clips: {stderr[:300]}")
    os.replace(tmp, out_path)


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


# Share of the video step's progress bar each stage takes (they add up to 1).
PROGRESS_SPLIT = {"prepare": 0.05, "clips": 0.55, "chunks": 0.15, "finalize": 0.25}


class _Progress:
    """Maps stage-local 0..1 fractions onto one 0..1000 bar for progress_cb("video", ...)."""

    def __init__(self, cb, has_chunks):
        self.cb = cb
        split = dict(PROGRESS_SPLIT)
        if not has_chunks:
            split["clips"] += split.pop("chunks")
        self.split = split
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


# Clip motions the Video editor can pick per clip (anything else: the look decides).
EDIT_MOTIONS = ("still", "zoom", "shake")
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
    slice, with an optional caption `text`, a `motion` from EDIT_MOTIONS and `cut` (a hard cut
    into it). Unknown sources and clips trimmed below 0.1 s are dropped."""
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
        if left < 100:
            continue
        seg["audio_offset_ms"] = int(seg.get("audio_offset_ms") or 0) + tin
        seg["duration_ms"] = left
        if isinstance(c.get("text"), str):
            seg["text"] = c["text"]
        if c.get("motion") in EDIT_MOTIONS:
            seg["motion"] = c["motion"]
        if c.get("cut"):
            seg["cut"] = True
        out.append(seg)
    return out


def overlay_graph(overlays, w, h, total, src, out, first_input, work_dir):
    """Image and text layers from the Video editor drawn over [src] (a w x h picture) into
    [out]. Each overlay: kind "image" (path, scale = width share of the frame) or "text" (text,
    size = height share of the frame, color, box), centred at x/y percent, with opacity, shown
    from start to end seconds. Images become ffmpeg inputs from `first_input` on; text is read
    from files in `work_dir`. Returns (extra input args, filter string)."""
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
            inputs += ["-loop", "1", "-t", f"{total:.3f}", "-i", path]
            parts.append(
                f"[{idx}:v]scale={ow}:-2,format=rgba,colorchannelmixer=aa={alpha:.2f}[{nxt}_i];"
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
    look = resolve_look(config.get("look"))
    # Side-by-side panels need the wide frame and the blurred fill.
    duo_on = look["duo"] and fmt == "landscape" and fill == "blur" and effect != "none"
    # Letterbox bars (landscape only): panels are laid out between them. Parallax needs the
    # blurred fill to drift.
    inset = letterbox_bar(w, h) if look.get("letterbox") else 0
    drift = round(w * PARALLAX / 2) // 2 * 2 if look.get("parallax") and fill == "blur" and effect != "none" else 0
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
    edit = config.get("edit") if isinstance(config.get("edit"), dict) else {}
    if isinstance(edit.get("clips"), list):
        render_items = apply_edit_clips(render_items, edit["clips"])
        use_beats = True
    if not render_items:
        raise RuntimeError("No segments in sync map — nothing to render")

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

    report = _Progress(progress_cb, False)

    # ── Step 1: Resolve every clip (image, audio slice, timing) and prepare its stills ──
    clips = []
    # End time (seconds) of every rendered clip - where reels may be cut.
    boundaries = []
    # The narration actually on screen, timed on the rendered timeline (captions follow it).
    caption_segments = []
    timeline = 0.0
    total = len(render_items)
    mx, my = panel_margins(w, h, fill, inset)
    box_aspect = (w - 2 * mx) / (h - 2 * my)
    turn = 0
    prev_motion = None
    # The duo clip before this one (its panel stays in the other half), and the last punch.
    duo_prev = None
    last_impact = -IMPACT_GAP

    for idx, seg in enumerate(render_items):
        report("prepare", idx / total)

        page_num = seg["page"]
        audio_file = seg.get("audio_file", f"page_{page_num:04d}.wav")
        audio_path = os.path.join(audio_dir, audio_file)

        if not os.path.exists(audio_path):
            continue

        # Image: a beat carries its exact panel crop; otherwise resolve the page.
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
        if not page_image:
            continue
        source_image = page_image

        # Crop to focus point if present (aspect matched to the panel box, or to a half of
        # the frame when a tall focus goes side by side)
        focus_point = seg.get("focus_point")
        if focus_point and len(focus_point) == 4:
            try:
                aspect = box_aspect
                if duo_on:
                    from PIL import Image
                    with Image.open(page_image) as im:
                        fw_, fh_ = im.size
                    ymin, xmin, ymax, xmax = focus_point
                    if (xmax - xmin) * fw_ < DUO_MAX_ASPECT * (ymax - ymin) * fh_:
                        aspect = 0.75
                page_image = _crop_to_focus(
                    page_image, focus_point, aspect,
                    os.path.join(video_dir, f"crop_{idx:04d}.jpg"),
                )
            except Exception as e:
                print(f"[video] Failed to crop page to focus point: {e}", file=sys.stderr)
        else:
            focus_point = None

        # For beats, play just this panel's slice of the page audio; for pages,
        # use the whole file.
        audio_offset = 0.0
        if use_beats and seg.get("duration_ms"):
            audio_offset = max(0.0, float(seg.get("audio_offset_ms", 0)) / 1000.0)
            duration = float(seg["duration_ms"]) / 1000.0
        else:
            duration = get_audio_duration_sec(audio_path)
        if duration < 0.1:
            continue
        duration = round(duration, 3)

        override = seg.get("motion") if seg.get("motion") in EDIT_MOTIONS else None
        try:
            tall = False
            # A beat may ask to be shown whole ("layout": "single"), like the hook's closing cover,
            # and an editor motion other than a shake moves the panel on its own.
            if duo_on and seg.get("layout") != "single" and override not in ("still", "zoom"):
                from PIL import Image
                with Image.open(page_image) as im:
                    tall = im.width < DUO_MAX_ASPECT * im.height
            if tall:
                same = duo_prev is not None and duo_prev["source"] == source_image and duo_prev["focus"] == focus_point
                if same:
                    duo = dict(duo_prev["duo"], hold=True, bg_prev=None)
                else:
                    slot = 0 if duo_prev is None else 1 - duo_prev["duo"]["slot"]
                    duo = _prepare_duo_images(page_image, idx, w, h, slot, video_dir, look["glow"], inset, drift)
                    duo["slot"] = slot
                    # The background drifts the way the panel slides in (and back with the next).
                    duo["bg_dir"] = 1 if slot == 0 else -1
                    duo["bg_prev_dir"] = duo_prev["duo"].get("bg_dir", 1) if duo_prev else 1
                    duo["bg_prev"] = duo_prev["duo"]["bg"] if duo_prev else None
                    duo["partner"] = {"fg": duo_prev["duo"]["fg"], "geo": duo_prev["duo"]["geo"]} if duo_prev else None
                images = None
            else:
                duo = None
                images = _prepare_clip_images(page_image, idx, w, h, fill, video_dir, look["glow"], inset, drift)
        except Exception as e:
            print(f"[video] Skipping unreadable image for page {page_num}: {e}", file=sys.stderr)
            continue

        motion = "still"
        if duo:
            motion = "duo"
            duo_prev = {"duo": duo, "source": source_image, "focus": focus_point}
        else:
            duo_prev = None
            if override == "still":
                motion = "still"
            elif override == "zoom":
                motion = "push"
            elif effect != "none":
                motion = pick_motion(images["aspect"], images["box_aspect"], turn, prev_motion)
                turn += 1
                prev_motion = motion
            if override == "shake" and motion == "still":
                motion = "push"
        if override:
            impact = override == "shake"
        else:
            impact = look["impact"] and len(clips) - last_impact >= IMPACT_GAP and is_impact(seg.get("text", ""))
        if impact:
            last_impact = len(clips)

        clips.append({
            "idx": idx, "page": page_num, "images": images, "motion": motion,
            "duo": duo, "impact": impact, "push": look["push"], "ramp": look.get("ramp", False),
            "chroma": look.get("chroma", False),
            "source": source_image, "focus": focus_point,
            "audio_path": audio_path, "offset": audio_offset,
            "start": timeline, "duration": duration, "cut": bool(seg.get("cut")),
        })
        caption_segments.append({
            "text": seg.get("text", ""),
            "start_ms": int(round(timeline * 1000)),
            "end_ms": int(round((timeline + duration) * 1000)),
            "audio_path": audio_path,
            "audio_offset_ms": int(round(audio_offset * 1000)),
        })
        timeline += duration
        boundaries.append(timeline)

    if not clips:
        raise RuntimeError("No video segments were generated")

    chunks = plan_chunks(len(clips), CHUNK_CLIPS)
    report = _Progress(progress_cb, len(chunks) > 1)
    report("prepare", 1.0)

    # ── Step 2: Render each clip's video (with a tail that the next crossfade eats) ──
    durations = [c["duration"] for c in clips]
    # A duo clip that follows another slides in by itself (a cut), and a punch flashes in.
    joins = [None]
    for k in range(1, len(clips)):
        if clips[k]["cut"] or (clips[k]["duo"] and clips[k - 1]["duo"]):
            joins.append("cut")
        elif clips[k]["impact"]:
            joins.append("impact")
        else:
            joins.append(None)
    starts, trans = plan_transitions(
        durations, [c["page"] for c in clips], trans_kind, trans_dur, look["transitions"], joins,
    )
    frame_counts = [clip_frames(starts, trans, k, durations[-1]) for k in range(len(clips))]
    # Page-turn effects the clips draw themselves, and the light frames they share (made once).
    clip_fx = plan_clip_fx(
        starts, trans, [c["page"] for c in clips], joins, look.get("leaks", False), [c["impact"] for c in clips],
    )
    assets = {}
    for kind, key in (("leak", "leak"), ("burn", "burn_in")):
        if any(key in f for f in clip_fx):
            try:
                assets[kind] = make_flare_frames(kind, w, h, video_dir)
            except Exception as e:
                print(f"[video] Could not make the {kind} frames: {e}", file=sys.stderr)
    all_frames = max(1, sum(frame_counts))
    rendered = 0
    clip_paths = []
    for k, c in enumerate(clips):
        frames = frame_counts[k]
        c["fx"], c["assets"] = clip_fx[k], assets
        spec = [c["motion"], c["impact"], c["push"], look["glow"], c["ramp"], c["chroma"] and c["impact"],
                inset, drift, sorted((n, v) for n, v in clip_fx[k].items() if n.split("_")[0] in assets or "zoom" in n)]
        if c["duo"]:
            # The pan ends where the next clip cuts in, so the panel left behind does not jump.
            nxt = round(starts[k + 1] * FPS) if k + 1 < len(starts) else round(starts[k] * FPS) + frames
            c["duo"]["move_frames"] = nxt - round(starts[k] * FPS)
            d = c["duo"]
            spec += [d["slot"], d.get("hold", False), d["move_frames"], d.get("bg_prev"), (d.get("partner") or {}).get("fg"),
                     d.get("bg_dir"), d.get("bg_prev_dir")]
        key = clip_key(c["source"], c["focus"], spec, frames, w, h, fill)
        path = os.path.join(video_dir, f"clip_{c['idx']:04d}_{key}.mp4")
        # Reuse a clip an interrupted run already rendered.
        if not (os.path.exists(path) and os.path.getsize(path) > 100):
            try:
                _render_clip(c, frames, w, h, path)
            except Exception as e:
                print(f"[video] {e} (page {c['page']}); retrying as a still", file=sys.stderr)
                if c["duo"]:
                    c["images"] = _prepare_clip_images(c["source"], c["idx"], w, h, fill, video_dir, look["glow"], inset, drift)
                    c["duo"] = None
                c["motion"] = "still"
                c["impact"] = False
                c["fx"] = {}
                _render_clip(c, frames, w, h, path)
        clip_paths.append(path)
        rendered += frames
        report("clips", rendered / all_frames)

    # Narration, sample-accurate against the same timeline.
    narration = os.path.join(video_dir, "narration.wav")
    _build_narration(clips, video_dir, narration)

    # ── Step 3: Crossfade runs of clips into chunks (long chapters only) ──
    if len(chunks) == 1:
        video_inputs = clip_paths
        video_starts, video_trans = starts, trans
    else:
        video_inputs, video_starts, video_trans = [], [], []
        for ci, (a, b) in enumerate(chunks):
            names = [os.path.basename(p) for p in clip_paths[a:b]]
            ckey = clip_key("", None, [names, trans[a + 1:b]], 0, w, h, fill)
            chunk_path = os.path.join(video_dir, f"chunk_{ci:03d}_{ckey}.mp4")
            span = (starts[b] if b < len(starts) else timeline) - starts[a]
            if not (os.path.exists(chunk_path) and os.path.getsize(chunk_path) > 100):
                _render_chunk(
                    clip_paths[a:b], starts[a:b], [None] + trans[a + 1:b], chunk_path, span,
                    lambda f, ci=ci: report("chunks", (ci + f) / len(chunks)),
                )
            video_inputs.append(chunk_path)
            video_starts.append(starts[a])
            video_trans.append(trans[a])
            report("chunks", (ci + 1) / len(chunks))

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

    ass_path = None
    try:
        import captions
        ass_path = captions.write_styled_captions(
            caption_segments, output_dir, w, h, config=config,
        )
    except Exception as e:
        print(f"[video] Failed to generate styled captions: {e}", file=sys.stderr)

    video = {
        "inputs": video_inputs, "starts": video_starts, "trans": video_trans, "look": look,
        "overlays": edit.get("overlays") or [], "work_dir": video_dir,
    }
    if look.get("particles"):
        try:
            video["particles"] = make_particle_loop(w, h, video_dir)
        except Exception as e:
            print(f"[video] Could not make the particles: {e}", file=sys.stderr)
    on_final = lambda f: report("finalize", f)
    success = _finalize_video_with_audio_mix_and_captions(
        video, narration, final_video, ass_path, bg_music,
        narration_volume, bg_music_volume, bg_music_loop, w, h, timeline,
        score=score, music_volume=music_volume, on_fraction=on_final,
    )
    if not success:
        print("[video] Finalization failed, retrying without captions, music and finish", file=sys.stderr)
        success = _finalize_video_with_audio_mix_and_captions(
            dict(video, look=None, overlays=[]), narration, final_video, None, None,
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


def _audio_mix_graph(first, has_music, narration_vol, music_vol, solo=False):
    """Narration (input `first`) plus optional music (input `first + 1`) mixed at unity into
    [mix]; the music is ducked under the narration by a gentle sidechain compressor. With
    `solo` (and music) the narration alone is also left in [solo]."""
    fmt = f"aresample={AUDIO_RATE},aformat=sample_fmts=fltp:channel_layouts=stereo"
    nar = f"[{first}:a]{fmt},volume={narration_vol}"
    if not has_music:
        return f"{nar}[mix]"
    if solo:
        split = f"{nar},asplit=3[nar][key][solo];"
    else:
        split = f"{nar},asplit=2[nar][key];"
    return (
        split +
        f"[{first + 1}:a]{fmt},volume={music_vol}[mus];"
        "[mus][key]sidechaincompress=threshold=0.03:ratio=4:attack=20:release=400[duck];"
        "[nar][duck]amix=inputs=2:duration=first:normalize=0[mix]"
    )


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


def _finalize_video_with_audio_mix_and_captions(
    video, narration, output_video, ass_path, bg_music,
    narration_vol, bg_vol, bg_loop, w, h, total, score=None, music_volume=1.0, on_fraction=None,
):
    """
    Joins the clip/chunk videos with their crossfades, burns subtitles, and mixes the
    narration with the score (or looped background music) in a single FFmpeg command.
    With music, a second "Narration only" audio track is written too.

    Loudness is two-pass and linear: the mix is measured first, then one fixed gain brings
    it to LOUD_I and a limiter catches peaks, so music never swells in narration pauses.
    """
    inputs = []
    for p in video["inputs"]:
        inputs += ["-i", p]
    n_video = len(video["inputs"])

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
    if on_fraction:
        on_fraction(0.0)
    measured = _measure_loudness(audio_inputs, _audio_mix_graph(0, has_music, narration_vol, music_vol))
    gain_db = 0.0 if measured is None else max(-20.0, min(20.0, LOUD_I - measured))
    limit = round(10 ** (LOUD_TP / 20), 4)
    if on_fraction:
        on_fraction(0.05)

    graph = []
    # 1. Video: crossfade the parts, then captions
    labels = []
    for i in range(n_video):
        graph.append(f"[{i}:v]settb=AVTB,setpts=PTS-STARTPTS[p{i}]")
        labels.append(f"p{i}")
    graph.append(xfade_chain(labels, video["starts"], video["trans"], "vraw"))
    # The look's finish (grade, colour, particles, vignette, grain, bars) under the captions.
    first_extra = n_video + audio_inputs.count("-i")
    extra_inputs = []
    if video.get("look"):
        particles = None
        if video["look"].get("particles") and video.get("particles") and os.path.exists(video["particles"]):
            extra_inputs = ["-stream_loop", "-1", "-i", video["particles"]]
            particles = f"{first_extra}:v"
        graph.append(look_filters(video["look"], w, h, "vraw", "vx", particles))
    else:
        graph.append("[vraw]null[vx]")
    # The editor's image and text layers, over the picture and under the captions.
    first_overlay = first_extra + extra_inputs.count("-i")
    overlay_inputs, overlays = overlay_graph(video.get("overlays"), w, h, total, "vx", "vo", first_overlay, video.get("work_dir") or ".")
    graph.append(overlays)
    has_subs = bool(ass_path) and os.path.exists(ass_path)
    if has_subs:
        from caption_engine import subtitles_filter
        graph.append(f"[vo]{subtitles_filter(ass_path)},format=yuv420p[v]")
    else:
        graph.append("[vo]format=yuv420p[v]")

    # 2. Audio: mix, one measured gain, limiter (and the narration alone, when there is music)
    graph.append(_audio_mix_graph(n_video, has_music, narration_vol, music_vol, solo=has_music))
    graph.append(f"[mix]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a]")
    maps = ["-map", "[v]", "-map", "[a]"]
    # MP4 keeps a track's name as its handler name (players list tracks by it).
    first = "Narration + music" if has_music else "Narration"
    tracks = ["-metadata:s:a:0", f"title={first}", "-metadata:s:a:0", f"handler_name={first}", "-disposition:a:0", "default"]
    if has_music:
        graph.append(f"[solo]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a2]")
        maps += ["-map", "[a2]"]
        tracks += ["-metadata:s:a:1", "title=Narration only", "-metadata:s:a:1", "handler_name=Narration only", "-disposition:a:1", "0"]

    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        *inputs, *audio_inputs, *extra_inputs, *overlay_inputs,
        "-filter_complex", ";".join(graph),
        *maps,
        "-c:v", "libx264", "-preset", "fast", "-crf", "21", "-pix_fmt", "yuv420p", "-r", str(FPS),
        "-c:a", "aac", "-b:a", "192k", "-ar", str(AUDIO_RATE),
        *tracks,
        "-t", f"{total:.3f}",
        "-movflags", "+faststart",
        output_video,
    ]

    try:
        code, stderr = run_ffmpeg_progress(
            cmd, total, (lambda f: on_fraction(0.05 + 0.95 * f)) if on_fraction else None,
        )
        if code != 0:
            print(f"[video] Finalization command failed: {stderr[:400]}", file=sys.stderr)
            return False
        return os.path.exists(output_video) and os.path.getsize(output_video) > 1000
    except Exception as e:
        print(f"[video] Finalization command error: {e}", file=sys.stderr)
        return False
