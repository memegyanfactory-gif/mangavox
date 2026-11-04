"""The GPU engine's plan of a video (MANGAVOX_RENDER_ENGINE=gpu): every picture the clips and the
finish would draw, worked out frame by frame ahead of time, for gpu_render.py to draw on the
graphics card in one pass straight into the encoder (no clip files, no second encode).

The plan is made from the CPU path's own helpers and its very ffmpeg expressions (evaluated
for every frame by ffexpr), so both paths move every picture the same way; this module only
mirrors how video_generator's _render_clip, _render_duo_clip, _finish_clip and the finish
(_finalize_video_with_audio_mix_and_captions' part_cmd) put them together. Anything it does
not know raises Unsupported, and the video is made on the CPU path instead.

A clip is a list of layers drawn bottom up over black, each a picture (a still, or a video
read frame by frame) cut out of its source (`crop`: whole pixels; `warp`: a sub-pixel window
stretched over `size`), laid at `pos` with an optional `alpha`, on frames [on[0], on[1]) only;
then a `flash` and the clip's own effects (`fx`). Numbers that change per frame are lists, one
value per frame; numbers that never change are one number."""
import math
import os
import sys

import numpy as np

try:
    import ffexpr
except ImportError:  # imported as python.gpu_timeline (the tests)
    from . import ffexpr

PLAN_VERSION = 1

# The transitions gpu_render draws (as ffmpeg's xfade does: same formulas).
TRANSITIONS = ("fade", "dissolve", "fadeblack", "fadewhite", "smoothleft", "smoothright", "smoothup", "smoothdown",
               "slideleft", "slideright", "slideup", "slidedown", "wipeleft", "wiperight", "wipeup", "wipedown",
               "circleopen", "circleclose", "pixelize")


class Unsupported(Exception):
    """Something only the CPU path draws."""


def _vg():
    m = sys.modules.get("video_generator") or sys.modules.get("python.video_generator")
    if m is None:
        try:
            import video_generator as m
        except ImportError:
            from . import video_generator as m
    return m


def even(v):
    """Where ffmpeg's overlay puts a 4:2:0 picture: truncated to whole pixels, then to even."""
    a = np.trunc(np.asarray(v, dtype=np.float64)).astype(np.int64)
    return a & ~1


def pack(values, digits=3):
    """A per-frame series for the plan: one number when it never changes, else a list."""
    a = np.asarray(values)
    if a.ndim == 0:
        a = a.reshape(1)
    if a.size == 0:
        return 0
    integer = np.issubdtype(a.dtype, np.integer)
    if np.all(a == a.flat[0]):
        return int(a.flat[0]) if integer else round(float(a.flat[0]), digits)
    return [int(x) for x in a] if integer else [round(float(x), digits) for x in a]


def series(expr, count, **extra):
    return ffexpr.frames(str(expr), count, _vg().FPS, **extra)


def camera_series(expr, count, **extra):
    """An expression the CPU path hands to perspective, which counts `on` / `in` from 1."""
    return ffexpr.frames(str(expr), count, _vg().FPS, on_base=1, **extra)


def crop_even(v, hi):
    """Where ffmpeg's crop puts a 4:2:0 window: rounded (lrint), inside 0..hi, then even."""
    return np.clip(np.rint(np.asarray(v, dtype=np.float64)).astype(np.int64), 0, hi) & ~1


def fade_in(count, st, d, t0=0.0):
    """ffmpeg fade=in (alpha) over `count` frames: 0 before st, rising over d seconds."""
    t = t0 + np.arange(count) / _vg().FPS
    return np.clip((t - st) / d, 0.0, 1.0) if d > 0 else (t >= st).astype(np.float64)


def fade_out(count, st, d, t0=0.0):
    t = t0 + np.arange(count) / _vg().FPS
    return np.clip(1.0 - (t - st) / d, 0.0, 1.0) if d > 0 else (t < st).astype(np.float64)


# ── Clips ──

def plan_clip(c, frames, w, h, prerendered=None):
    """The layers and effects of clip `c` (as generate_video prepared it), `frames` frames long;
    with `prerendered` (a clip file already rendered by the CPU path) that file as it is."""
    if prerendered:
        return {"kind": "video", "frames": frames, "path": prerendered}
    plan = _plan_duo(c, frames, w, h) if c.get("duo") else _plan_single(c, frames, w, h)
    plan["fx"] = plan_fx(c, frames, w, h)
    return plan


def _plan_single(c, frames, w, h):
    """Mirrors _render_clip."""
    vg = _vg()
    FPS = vg.FPS
    imgs = c["images"]
    motion = c["motion"]
    move = c.get("move")
    impact = c.get("impact", False)
    ramp = c.get("ramp", False)
    fg_zoom, window, ease = vg.clip_camera(c, frames)
    k = min(1.0, frames / (3.0 * FPS))
    if ramp and frames < 2 * FPS:
        k = max(k, 0.7)
    punch = c.get("punch", 0.12)
    fx, fy = imgs["fg_pos"]
    jolt = round(h * 0.1 * punch) if impact else 0
    drift = imgs.get("drift", 0)
    bg = {"src": imgs["bg"], "size": [w, h], "pos": [0, 0]}
    if drift:
        dx, dy = vg.move_dirs(move) if move else vg.PARALLAX_DIRS.get(motion, (0.6, 0))
        e = f"(2*{vg.ramp_ease('n', frames, ramp)}-1)"
        bx = series(f"{drift}+{round(dx * drift * k, 2)}*{e}", frames)
        by = series(f"{drift}+{round(dy * drift * k, 2)}*{e}", frames)
        bg["crop"] = [pack(crop_even(bx, 2 * drift)), pack(crop_even(by, 2 * drift))]
    else:
        bg["crop"] = [0, 0]
    # The panel: the eased window 1/zoom of its size, stretched over it (perspective, cubic).
    anim = imgs.get("fg_anim")
    fw, fh = imgs["fg_size"]
    e = ease or vg._ease("on", frames)
    z = camera_series("(" + fg_zoom.format(e=e) + ")", frames, W=fw, H=fh)
    px, py = (camera_series(v.format(e=e), frames, W=fw, H=fh) for v in window)
    cw, ch = fw / z, fh / z
    ox = f"{fx}+{vg._shake(jolt, 'x')}+{vg._panel_move(c.get('fx') or {}, w, 'x')}"
    oy = f"{fy}+{vg._shake(jolt, 'y')}"
    fg = {
        "src": anim or imgs["fg"], "video": bool(anim), "full_range": bool(anim), "size": [fw, fh],
        "warp": [pack((fw - cw) * px), pack((fh - ch) * py), pack(cw), pack(ch)], "interp": "bicubic",
        "pos": [pack(even(series(ox, frames))), pack(even(series(oy, frames)))],
    }
    if imgs.get("fg_layered"):
        # A layered panel the engine animates itself (layered_motion.Animator, on the GPU) from the
        # pack's cached layers: the pack's job as it is.
        fg["animate"] = imgs["fg_layered"]
        fg["video"] = False
    return {"kind": "layers", "frames": frames, "layers": [bg, fg], "flash": _flash(impact, frames)}


def _plan_duo(c, frames, w, h):
    """Mirrors _render_duo_clip."""
    vg = _vg()
    d = c["duo"]
    impact = c.get("impact", False)
    hold = d.get("hold", False)
    move = max(2, d.get("move_frames", frames))
    drift = d.get("drift", 0)
    layers = []
    bg = {"src": d["bg"], "size": [w, h], "pos": [0, 0], "crop": [0, 0]}
    if drift:
        sign = d.get("bg_dir", 1)
        if hold:
            bx = np.full(frames, drift - sign * drift, dtype=np.float64)
        else:
            e = f"(0.5-0.5*cos(PI*min(n,{move - 1})/{move - 1}))"
            bx = series(f"{drift}-{sign * drift}*(2*{e}-1)", frames)
        bg["crop"] = [pack(crop_even(bx, 2 * drift)), 0]
    layers.append(bg)
    if d.get("bg_prev") and not hold:
        # The old background fades out over the new one, only while it shows.
        n = min(frames, max(1, math.ceil(vg.DUO_BG_FADE * vg.FPS - 1e-9)))
        x = (drift - d.get("bg_prev_dir", 1) * drift) if drift else 0
        layers.append({"src": d["bg_prev"], "size": [w, h], "pos": [0, 0], "crop": [int(x) & ~1, 0],
                       "alpha": pack(fade_out(n, 0.0, vg.DUO_BG_FADE)), "on": [0, n]})
    jolt = round(h * 0.012) if impact else 0
    sx, sy = series(vg._shake(jolt, "x"), frames), series(vg._shake(jolt, "y"), frames)
    partner = d.get("partner")
    if partner:
        g = partner["geo"]
        dw, _ = g["size"]
        px, py = g["pos"]
        layers.append({"src": partner["fg"], "size": [dw, g["vis"]], "crop": [0, int(g["slack"]) & ~1],
                       "pos": [pack(even(px + sx)), pack(even(py + sy))]})
    g = d["geo"]
    dw, _ = g["size"]
    x0, y0 = g["pos"]
    fg = {"src": d["fg"], "size": [dw, g["vis"]]}
    if hold or g["slack"] <= 0:
        fg["crop"] = [0, (int(g["slack"]) & ~1) if hold else 0]
    else:
        y = camera_series(f"{g['slack']}*(0.5-0.5*cos(PI*min(on,{move - 1})/{move - 1}))", frames)
        fg["warp"] = [0, pack(y), dw, g["vis"]]
        fg["interp"] = "bicubic"
    if hold:
        fg["pos"] = [int(x0) & ~1, int(y0) & ~1]
    else:
        slide = vg.DUO_SLIDE * (0.55 if impact else 1.0)
        dist = round(w * vg.DUO_SLIDE_DIST * (1.6 if impact else 1.0)) * (-1 if d["slot"] == 0 else 1)
        fg["alpha"] = pack(fade_in(frames, 0.0, round(slide * 0.7, 3)))
        ox = series(f"{x0}+({dist})*pow(1-min(1,t/{slide}),3)", frames) + sx
        fg["pos"] = [pack(even(ox)), pack(even(y0 + sy))]
    layers.append(fg)
    return {"kind": "layers", "frames": frames, "layers": layers, "flash": _flash(impact, frames)}


def _flash(impact, frames):
    """_flash's white flash: eq's brightness for each frame (None without one)."""
    if not impact:
        return None
    return pack(series(f"0.35*pow(max(0,1-t/{_vg().IMPACT_SEC}),2)", frames), 4)


def plan_fx(c, frames, w, h):
    """The clip's own effects (mirrors _finish_clip): each on frames [a, b) of the clip, with its
    numbers per frame of that window (its clock starting at 0, as _window's chain has it)."""
    vg = _vg()
    FPS = vg.FPS
    fx = c.get("fx") or {}
    assets = c.get("assets") or {}
    out = []

    def window(a, b):
        a, b = max(0, min(a, frames)), max(0, min(b, frames))
        return (a, b) if b > a else None

    def zoom_blur(a, b, dur, leaving):
        win = window(a, b)
        if not win:
            return
        n = win[1] - win[0]
        span = max(1, round(dur * FPS))
        j = np.arange(n)
        # The zoom goes through perspective (its `on` counts from 1); the blur's t from 0.
        p = np.minimum(1.0, (j + 1) / span)
        z = 1 + vg.ZOOM_BLUR * (p ** 2 if leaving else (1 - p) ** 3)
        s = w / 1280
        steps = ((0.3, 0.55, 2), (0.55, 0.8, 5), (0.8, 99, 10)) if leaving else ((0, 0.25, 10), (0.25, 0.5, 5), (0.5, 0.75, 2))
        t = j / FPS
        sigma = np.zeros(n)
        for lo, hi, sg in steps:
            sigma = np.where((t >= lo * dur) & (t < hi * dur), round(sg * s, 1), sigma)
        out.append({"op": "zoomblur", "on": list(win), "zoom": pack(z, 5), "sigma": pack(sigma)})

    def chroma(a, b, span, strength, grow):
        win = window(a, b)
        if not win:
            return
        n = win[1] - win[0]
        s = max(2, round(w * 0.006 * strength))
        steps = ((0, 0.5, 0.3), (0.5, 0.8, 0.6), (0.8, 99, 1.0)) if grow else ((0, 0.12, 1.0), (0.12, 0.24, 0.6), (0.24, 99, 0.3))
        t = np.arange(n) / FPS
        amt = np.zeros(n, dtype=np.int64)
        for lo, hi, kk in steps:
            amt = np.where((t >= lo * span) & (t < hi * span), max(1, round(s * kk)), amt)
        out.append({"op": "rgbshift", "on": list(win), "rh": pack(-amt), "bh": pack(amt),
                    "rv": pack(-np.maximum(0, amt // 3)), "bv": pack(np.maximum(0, amt // 3))})

    if "zoomblur_out" in fx:
        t0, dur = fx["zoomblur_out"]
        zoom_blur(round(t0 * FPS), frames, dur, True)
    if "zoomblur_in" in fx:
        dur = fx["zoomblur_in"]
        zoom_blur(0, max(2, round(dur * 1.6 * FPS)), dur * 1.6, False)
    ck = c.get("chroma_k", 1.0)
    if "glitch_out" in fx:
        t0, dur = fx["glitch_out"]
        a = round(t0 * FPS)
        chroma(a, frames, dur, max(0.5, ck) * 2.2, True)
        if window(a, frames):
            out.append({"op": "noise", "on": list(window(a, frames)), "amp": 18, "uniform": True})
    if "glitch_in" in fx:
        dur = fx["glitch_in"] * 1.5
        b = max(2, round(dur * FPS))
        chroma(0, b, dur, max(0.5, ck) * 2.2, False)
        if window(0, b):
            out.append({"op": "noise", "on": list(window(0, b)), "amp": 18, "uniform": True})
    for whip in ("whipleft", "whipright", "whipup", "whipdown"):
        sideways = whip in ("whipleft", "whipright")
        radius = [max(2, round(w * 0.012)), 1] if sideways else [1, max(2, round(h * 0.02))]
        if f"{whip}_out" in fx:
            t0, dur = fx[f"{whip}_out"]
            a = round(t0 * FPS)
            win = window(a, a + max(1, round(dur * FPS)))
            if win:
                out.append({"op": "boxblur", "on": list(win), "radius": radius})
        if f"{whip}_in" in fx:
            win = window(0, max(1, round(fx[f"{whip}_in"] * FPS)))
            if win:
                out.append({"op": "boxblur", "on": list(win), "radius": radius})
    if c.get("impact") and c.get("chroma") and ck > 0:
        chroma(0, round(vg.IMPACT_SEC * FPS), vg.IMPACT_SEC, ck, False)
    atmos = c.get("atmos")
    if atmos:
        path, count, at = atmos
        out.append({"op": "loop", "on": [0, frames], "src": path, "count": int(count), "at": int(at % count)})
    flares = []
    if "burn_out" in fx and assets.get("burn"):
        t0, dur = fx["burn_out"]
        flares.append((assets["burn"], t0 + dur / 2 - vg.BURN_PEAK, fx.get("burn_flip", False)))
    if "burn_in" in fx and assets.get("burn"):
        flares.append((assets["burn"], fx["burn_in"] / 2 - vg.BURN_PEAK, fx.get("burn_flip_in", False)))
    if fx.get("leak") and assets.get("leak"):
        flares.append((assets["leak"], 0.0, False))
    for (pattern, count), start, flip in flares:
        first = max(0, math.ceil(-start * FPS - 1e-6))
        offset = round(max(0.0, start) * FPS)
        if first >= count or offset >= frames:
            continue
        # Frame `offset` of the clip shows the sequence's frame `first`, and so on to its end.
        out.append({"op": "sequence", "on": [offset, min(frames, offset + count - first)], "src": pattern,
                    "count": int(count), "first": int(first), "flip": bool(flip)})
    known = {"zoomblur_out", "zoomblur_in", "glitch_out", "glitch_in", "burn_out", "burn_in", "burn_flip", "burn_flip_in", "leak",
             "panel_in", "panel_out"} | {f"{wh}_{s}" for wh in ("whipleft", "whipright", "whipup", "whipdown") for s in ("in", "out")}
    left = set(fx) - known
    if left:
        raise Unsupported(f"clip effects {sorted(left)}")
    return out


# ── The whole video ──

def plan_video(video, clips, w, h, total, ass_filter=None, codec=None, out=None):
    """The plan of the finished picture (mirrors the finish): `video` as generate_video hands it
    to the finish, `clips` the clips' plans (plan_clip) in order, `total` seconds; `ass_filter`
    the captions' ffmpeg filter (drawn by the encoder, as the CPU path does), `codec` the
    encoder's arguments, `out` the picture file to write."""
    vg = _vg()
    FPS = vg.FPS
    starts, trans = video["starts"], video["trans"]
    base = round(starts[0] * FPS)
    first = [round(s * FPS) - base for s in starts]
    total_frames = max(1, math.ceil(total * FPS - 1e-6))
    joins = [None]
    for k in range(1, len(starts)):
        if not trans[k] or trans[k][1] <= 0:
            joins.append(None)
            continue
        name, dur = trans[k]
        name = vg.CLIP_DRAWN.get(name, name)
        if name not in TRANSITIONS:
            raise Unsupported(f"the {name} transition")
        joins.append({"name": name, "dur": float(dur), "offset": max(0.0, first[k] / FPS - 0.001)})
    plan = {"v": PLAN_VERSION, "w": w, "h": h, "fps": FPS, "frames": total_frames, "first": first, "joins": joins,
            "clips": clips, "out": out, "codec": list(codec or []), "vf": ass_filter}
    plan["bursts"] = [{"at": int(f0), "src": pattern, "count": int(count)} for f0, pattern, count in video.get("bursts") or []]
    look = video.get("look")
    if look:
        plan["look"] = plan_look(video, look, w, h, total_frames)
    plan["overlays"] = plan_overlays(video.get("overlays"), w, h, total, video.get("work_dir") or ".")
    card = video.get("chapter_card")
    if card:
        bar = vg.letterbox_bar(w, h) if (look or {}).get("letterbox") else 0
        n = round(vg.CARD_OUT * FPS)
        x, y = vg.chapter_card_motion(card, w, h, bar)
        alpha = fade_in(n, vg.CARD_IN, round(vg.CARD_EASE * 0.7, 3)) * fade_out(n, vg.CARD_OUT - vg.CARD_FADE, vg.CARD_FADE)
        plan["card"] = {"src": card[0], "on": [0, n], "pos": [pack(even(series(x, n))), pack(even(series(y, n)))], "alpha": pack(alpha, 4)}
    return plan


def plan_look(video, look, w, h, total_frames):
    """The finish's look (mirrors look_filters): one lookup table, particles, the vignette,
    grain, the edge glow and letterbox bars."""
    vg = _vg()
    FPS = vg.FPS
    out = {}
    lut = vg.color_lut(look.get("grade"), look.get("color"))
    if lut:
        val = np.arange(256, dtype=np.float64)
        out["lut"] = [[int(v) for v in np.clip(np.trunc(np.broadcast_to(ffexpr.evaluate(e, val=val), (256,))), 0, 255)] for e in lut]
    particles = video.get("particles")
    if particles and look.get("particles") and os.path.exists(particles):
        out["particles"] = {"src": particles, "count": round(vg.PARTICLE_SEC * FPS)}
    angle = vg.vignette_angle(look)
    if angle:
        out["vignette"] = angle
    gk = look.get("grain_k", 1.0)
    if look.get("grain") and round(4 * gk) > 0:
        out["grain"] = round(4 * gk)
    bar = vg.letterbox_bar(w, h) if look.get("letterbox") else 0
    out["bars"] = bar
    if look.get("edge_glow"):
        ih = h - 2 * bar
        colors = video.get("edge_colors") or [(0.0, vg.EDGE_COLORS["neutral"])]
        segs = vg.edge_segments(colors, 0.0, total_frames / FPS)
        places, at = vg.edge_moves(w, h, bar)
        work = video.get("work_dir") or "."
        t = np.arange(total_frames) / FPS
        seg = np.zeros(total_frames, dtype=np.int64)
        atlases = []
        for n, (a, b, rgb) in enumerate(segs):
            atlases.append(vg.make_edge_glow(w, ih, tuple(rgb), work))
            if n:
                seg = np.where(t >= a - 0.5 / FPS, n, seg)
        pos = {}
        for name, (x, y) in at.items():
            pos[name] = [pack(even(np.broadcast_to(ffexpr.evaluate(str(v), t=t), (total_frames,)))) for v in (x, y)]
        out["edge"] = {"atlases": atlases, "segment": pack(seg), "places": {k: list(v) for k, v in places.items()}, "pos": pos}
    return out


def plan_overlays(overlays, w, h, total, work_dir):
    """The Video editor's image and text layers (mirrors overlay_graph). Text is drawn into a
    picture here (Pillow, the same font, colour, shadow and box), so the GPU lays it like an image."""
    vg = _vg()
    out = []
    for i, o in enumerate(overlays or []):
        if not isinstance(o, dict):
            continue
        start = vg._num(o.get("start"), 0.0, 0.0, total)
        end = vg._num(o.get("end"), total, 0.0, total)
        if end - start < 0.05:
            continue
        x = vg._num(o.get("x"), 50.0, 0.0, 100.0) / 100.0
        y = vg._num(o.get("y"), 50.0, 0.0, 100.0) / 100.0
        alpha = vg._num(o.get("opacity"), 1.0, 0.0, 1.0)
        # between(t, start, end): on from the first frame at or after start to the last at or before end.
        on = [math.ceil(start * vg.FPS - 1e-9), math.floor(end * vg.FPS + 1e-9) + 1]
        if o.get("kind") == "image":
            path = o.get("path")
            if not isinstance(path, str) or not os.path.isfile(path):
                continue
            ow = max(2, round(w * vg._num(o.get("scale"), 0.2, 0.02, 1.0)) // 2 * 2)
            out.append({"src": path, "width": ow, "anchor": [x, y], "alpha": alpha, "on": on})
        elif o.get("kind") == "text":
            made = _text_picture(o, i, h, alpha, work_dir)
            if made:
                path, (tw, th), (ix, iy) = made
                # drawtext's x = X*w - text_w/2 (the text, not its box or shadow), whole pixels.
                px = int(x * w - tw / 2) - ix
                py = int(y * h - th / 2) - iy
                out.append({"src": path, "pos": [px, py], "alpha": 1.0, "on": on})
    return out


def _text_picture(o, i, h, alpha, work_dir):
    """An editor text layer drawn as drawtext would: (path, (text w, text h), where the text
    starts in the picture)."""
    vg = _vg()
    text = o.get("text")
    if not isinstance(text, str) or not text.strip():
        return None
    if "\n" in text or "\r" in text:
        # drawtext's line pitch is not Pillow's: the CPU path draws multi-line text.
        raise Unsupported("multi-line editor text")
    try:
        from caption_engine import FONTS_DIR
    except ImportError:
        from .caption_engine import FONTS_DIR
    font = os.path.join(FONTS_DIR, vg.OVERLAY_FONT)
    if not os.path.isfile(font):
        return None
    import re
    from PIL import Image, ImageDraw, ImageFont
    fs = max(8, round(h * vg._num(o.get("size"), 0.06, 0.01, 0.3)))
    color = o.get("color")
    if not (isinstance(color, str) and re.fullmatch(r"#[0-9a-fA-F]{6}", color)):
        color = "#ffffff"
    rgb = tuple(int(color[k:k + 2], 16) for k in (1, 3, 5))
    f = ImageFont.truetype(font, fs)
    box = max(4, fs // 4) if o.get("box") else 0
    l, t, r, b = ImageDraw.Draw(Image.new("L", (1, 1))).multiline_textbbox((0, 0), text, font=f)
    tw, th = r - l, b - t
    pad = box + 2
    size = (tw + 2 * pad + 2, th + 2 * pad + 2)
    # Box, shadow and text each on their own layer, composited (drawing them on one layer would
    # replace what is under a see-through text instead of showing it through).
    img = Image.new("RGBA", size, (0, 0, 0, 0))
    steps = []
    if box:
        steps.append(lambda d: d.rectangle([pad - box, pad - box, pad + tw + box, pad + th + box], fill=(0, 0, 0, round(255 * 0.55 * alpha))))
    steps.append(lambda d: d.multiline_text((pad - l + 2, pad - t + 2), text, font=f, fill=(0, 0, 0, round(255 * 0.6 * alpha))))
    steps.append(lambda d: d.multiline_text((pad - l, pad - t), text, font=f, fill=rgb + (round(255 * alpha),)))
    for step in steps:
        layer = Image.new("RGBA", size, (0, 0, 0, 0))
        step(ImageDraw.Draw(layer))
        img = Image.alpha_composite(img, layer)
    os.makedirs(work_dir, exist_ok=True)
    path = os.path.join(work_dir, f"overlay_{i}_gpu.png")
    img.save(path)
    return path, (tw, th), (pad, pad)
