#!/usr/bin/env python3
"""
Motion compose: renders a custom motion-graphic video (intro, trailer, character spotlight, text
animation) from a JSON spec written by the assistant. Separate from the recap pipeline.

The app checks the spec strictly and resolves every image source to a file first
(native/crates/ui-studio/src/assist/motion.rs); this script checks it again lightly, so it can
also be run by hand, then draws every frame with Pillow + NumPy and pipes them to FFmpeg
(NVENC when it works, else libx264, the same encoder choice as video_generator.py).

Usage:
  python motion_compose.py render job.json     JSON lines: {"type": "progress", ...}, then
                                               {"type": "done", ...} or {"type": "error", ...}
  python motion_compose.py validate spec.json  one JSON object {"ok": bool, "errors": [...]}

job.json: {"mode": "full" | "frame" | "clip", "spec": {...}, "out": "video.mp4" (full / clip),
           "frame": "preview.png", "frame_at": seconds, "start": seconds (clip), "seconds": 3,
           "scale": 1.0 (0.5 = half size), "work_dir": "...", "config": {"vfx_python": ...}}

Spec (normalised by the app; the same keys work by hand, missing values take defaults):
  {"width", "height", "fps", "background": "#000", "scenes": [{"duration", "start"?,
   "background": {"color", "image": {"path", "crop"?}, "blur", "dim", "fit"},
   "transition": {"type", "duration"} (into this scene), "layers": [...], "effects": [...]}],
   "audio": {"narration": [{"text", "at", "voice", "engine", "speed"}], "music": {"src", "volume",
   "start", "fade_in", "fade_out", "loop"}, "sfx": [{"name" | "src", "at", "volume"}], "volume"}}
"""
from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
import zlib

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont, ImageOps

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

FONT_DIR = os.path.join(HERE, "fonts")
FONTS = {
    "bold": "Inter-Bold.ttf", "black": "Inter-Black.ttf", "regular": "Inter-Bold.ttf", "impact": "Anton-Regular.ttf",
    "serif": "DMSerifDisplay-Regular.ttf", "mono": "SpaceMono-Bold.ttf", "condensed": "ArchivoBlack-Regular.ttf",
}
TRANSITIONS = ("none", "cut", "fade", "fadeblack", "fadewhite", "slide_left", "slide_right", "slide_up", "slide_down",
               "zoom_in", "zoom_out", "wipe_left", "wipe_right", "flash", "whip")
LAYER_TYPES = ("image", "text", "shape", "particles")
ANIMATED = ("x", "y", "scale", "rotation", "opacity")
DEFAULTS = {"x": 0.5, "y": 0.5, "scale": 1.0, "rotation": 0.0, "opacity": 1.0}
PARTICLE_COLORS = {"embers": "#ff8a3d", "snow": "#ffffff", "dust": "#e8dcc8", "sparks": "#ffd36b", "stars": "#ffffff", "rain": "#9fc4ff"}
AUDIO_RATE = 48000
MAX_SECONDS = 180.0


def emit(obj):
    print(json.dumps(obj), flush=True)


def clamp(v, lo, hi):
    return lo if v < lo else hi if v > hi else v


def rgb(hex_color, default=(255, 255, 255)):
    h = str(hex_color or "").strip().lstrip("#")
    if len(h) == 3:
        h = "".join(c * 2 for c in h)
    try:
        return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))
    except (ValueError, IndexError):
        return default


# ── Easing and keyframes ─────────────────────────────────────────────────────────

EASE_ALIASES = {"ease_in": "in", "ease_out": "out", "ease_in_out": "in_out", "back_out": "back", "bounce_out": "bounce", "elastic_out": "elastic"}


def ease(name, p):
    """Eased progress for p in 0..1 (CSS / easings.net curves)."""
    p = clamp(float(p), 0.0, 1.0)
    n = EASE_ALIASES.get(name, name or "in_out")
    if n == "linear":
        return p
    if n == "in":
        return p ** 3
    if n == "out":
        return 1 - (1 - p) ** 3
    if n == "back":
        c1 = 1.70158
        return 1 + (c1 + 1) * (p - 1) ** 3 + c1 * (p - 1) ** 2
    if n == "bounce":
        n1, d1 = 7.5625, 2.75
        if p < 1 / d1:
            return n1 * p * p
        if p < 2 / d1:
            p -= 1.5 / d1
            return n1 * p * p + 0.75
        if p < 2.5 / d1:
            p -= 2.25 / d1
            return n1 * p * p + 0.9375
        p -= 2.625 / d1
        return n1 * p * p + 0.984375
    if n == "elastic":
        if p in (0.0, 1.0):
            return p
        return 2 ** (-10 * p) * math.sin((p * 10 - 0.75) * (2 * math.pi / 3)) + 1
    if n == "step":
        return 1.0 if p >= 1.0 else 0.0
    # in_out
    return 4 * p ** 3 if p < 0.5 else 1 - (-2 * p + 2) ** 3 / 2


def prop_at(layer, name, t):
    """A layer's animated property at scene time t: its keyframes (each with an ease into it),
    held before the first and after the last; the layer's own value without keyframes."""
    base = float(layer.get(name, DEFAULTS[name]))
    kfs = [k for k in layer.get("keyframes") or [] if name in k]
    if not kfs:
        return base
    if t <= kfs[0]["t"]:
        return float(kfs[0][name])
    for a, b in zip(kfs, kfs[1:]):
        if t <= b["t"]:
            span = b["t"] - a["t"]
            p = 1.0 if span <= 0 else (t - a["t"]) / span
            return float(a[name]) + (float(b[name]) - float(a[name])) * ease(b.get("ease", "in_out"), p)
    return float(kfs[-1][name])


def noise(seed, i):
    """Deterministic value in -1..1 for (seed, i)."""
    return (zlib.crc32(f"{seed}:{i}".encode()) / 0xFFFFFFFF) * 2 - 1


# ── Checking (light: the app does the strict check) ──────────────────────────────

def check(spec):
    """Problems that would stop a render, each with its path."""
    errs = []
    if not isinstance(spec, dict):
        return ["the spec must be a JSON object"]
    scenes = spec.get("scenes")
    if not isinstance(scenes, list) or not scenes:
        return ["spec: needs \"scenes\": a list of at least one scene"]
    for i, s in enumerate(scenes):
        p = f"scenes[{i}]"
        if not isinstance(s, dict):
            errs.append(f"{p}: must be an object")
            continue
        d = s.get("duration")
        if not isinstance(d, (int, float)) or not 0.05 <= d <= 600:
            errs.append(f"{p}.duration: needs a number of seconds")
        tr = s.get("transition")
        if isinstance(tr, dict) and tr.get("type") not in TRANSITIONS:
            errs.append(f"{p}.transition.type: \"{tr.get('type')}\" is not one of {', '.join(TRANSITIONS)}")
        for j, l in enumerate(s.get("layers") or []):
            lp = f"{p}.layers[{j}]"
            if not isinstance(l, dict) or l.get("type") not in LAYER_TYPES:
                errs.append(f"{lp}.type: must be one of {', '.join(LAYER_TYPES)}")
                continue
            if l["type"] == "image":
                path = src_path(l.get("src"))
                if not path or not os.path.isfile(path):
                    errs.append(f"{lp}.src: file not found ({path or l.get('src')})")
            if l["type"] == "text" and not str(l.get("text") or "").strip():
                errs.append(f"{lp}: a text layer needs \"text\"")
    return errs


def src_path(src):
    if isinstance(src, str):
        return src
    if isinstance(src, dict):
        return src.get("path")
    return None


def normalise(spec):
    """Fills what a hand-written spec leaves out: size, fps, scene starts (transitions overlap
    the previous scene) and the total duration."""
    spec = dict(spec)
    spec.setdefault("width", 1920)
    spec.setdefault("height", 1080)
    spec.setdefault("fps", 30)
    spec.setdefault("background", "#000000")
    start, prev = 0.0, 0.0
    out = []
    for i, s in enumerate(spec["scenes"]):
        s = dict(s)
        tr = s.get("transition")
        if isinstance(tr, str):
            tr = {"type": tr}
        if i == 0 or not isinstance(tr, dict) or tr.get("type") in (None, "none", "cut"):
            tr = None
        else:
            tr = {"type": tr["type"], "duration": float(min(tr.get("duration", 0.5), 0.9 * min(prev, s["duration"])))}
        s["transition"] = tr
        if i > 0:
            start += prev - (tr["duration"] if tr else 0.0)
        if "start" not in s:
            s["start"] = start
        start = s["start"]
        bg = s.get("background")
        if isinstance(bg, str) or bg is None:
            bg = {"color": bg or spec["background"]}
        s["background"] = bg
        s.setdefault("layers", [])
        s.setdefault("effects", [])
        out.append(s)
        prev = s["duration"]
    spec["scenes"] = out
    spec["duration"] = out[-1]["start"] + out[-1]["duration"]
    spec.setdefault("audio", {})
    return spec


# ── Images, text and shapes as sprites ───────────────────────────────────────────

class Assets:
    """Loaded images and drawn sprites, cached for the whole render."""

    def __init__(self, scale_w):
        self.scale_w = scale_w
        self.images = {}
        self.sprites = {}
        self.fonts = {}

    def image(self, src):
        path = src_path(src)
        crop = src.get("crop") if isinstance(src, dict) else None
        key = (path, tuple(crop) if crop else None)
        if key not in self.images:
            with Image.open(path) as im:
                img = ImageOps.exif_transpose(im).convert("RGBA")
            if crop and len(crop) == 4:
                w, h = img.size
                img = img.crop((int(crop[0] * w), int(crop[1] * h), max(int(crop[0] * w) + 1, int(crop[2] * w)), max(int(crop[1] * h) + 1, int(crop[3] * h))))
            if max(img.size) > 4096:
                img.thumbnail((4096, 4096), Image.LANCZOS)
            self.images[key] = img
        return self.images[key]

    def font(self, name, px):
        key = (name, px)
        if key not in self.fonts:
            path = os.path.join(FONT_DIR, FONTS.get(name, FONTS["bold"]))
            try:
                self.fonts[key] = ImageFont.truetype(path, px)
            except OSError:
                self.fonts[key] = ImageFont.load_default()
        return self.fonts[key]


def wrap(draw, text, font, max_w):
    lines = []
    for para in str(text).split("\n"):
        words, cur = para.split(" "), ""
        for w in words:
            cand = f"{cur} {w}".strip()
            if cur and draw.textlength(cand, font=font) > max_w:
                lines.append(cur)
                cur = w
            else:
                cur = cand
        lines.append(cur)
    return lines


def text_sprite(assets, layer, W, H, chars=None):
    """The text drawn once (stroke, shadow), sized from the frame height."""
    text = str(layer.get("text") or "")
    if chars is not None:
        text = text[:chars]
    px = max(6, int(float(layer.get("size", 0.07)) * H))
    font = assets.font(layer.get("font", "bold"), px)
    probe = ImageDraw.Draw(Image.new("L", (1, 1)))
    lines = wrap(probe, text, font, float(layer.get("max_width", 0.9)) * W) or [""]
    stroke = layer.get("stroke") or {}
    sw = int(float(stroke.get("width", 0)) * px) if stroke else 0
    lh = int(px * float(layer.get("line_height", 1.15)))
    widths = [probe.textlength(l, font=font) for l in lines]
    tw = int(max(widths + [1])) + 2 * sw + 4
    th = lh * len(lines) + 2 * sw + int(px * 0.3)
    img = Image.new("RGBA", (max(1, tw), max(1, th)), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    color = rgb(layer.get("color"), (255, 255, 255)) + (255,)
    align = layer.get("align", "center")
    for i, line in enumerate(lines):
        x = sw + 2 + {"left": 0, "right": tw - 4 - 2 * sw - widths[i]}.get(align, (tw - 4 - 2 * sw - widths[i]) / 2)
        d.text((x, sw + i * lh), line, font=font, fill=color, stroke_width=sw, stroke_fill=rgb(stroke.get("color"), (0, 0, 0)) + (255,) if sw else None)
    if layer.get("shadow"):
        img = with_shadow(img, {"opacity": 0.65, "offset": 0.004, "blur": 0.004, "color": "#000000"}, W)
    return img


def shape_sprite(layer, W, H):
    w = max(1, int(float(layer.get("w", 0.3)) * W))
    h = max(1, int(float(layer.get("h", 0.1)) * H))
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    color = rgb(layer.get("color"), (255, 255, 255)) + (255,)
    outline = layer.get("outline")
    ow = max(1, int(float(outline.get("width", 0.004)) * W)) if outline else 0
    oc = rgb(outline.get("color"), (255, 255, 255)) + (255,) if outline else None
    kind = layer.get("shape", "rect")
    if kind == "circle":
        d.ellipse((0, 0, w - 1, h - 1), fill=None if outline and layer.get("color") is None else color, outline=oc, width=ow)
    elif kind == "line":
        d.line((0, h // 2, w, h // 2), fill=color, width=max(1, h))
    else:
        r = int(float(layer.get("radius", 0)) * min(w, h) * 2)
        d.rounded_rectangle((0, 0, w - 1, h - 1), radius=r, fill=color, outline=oc, width=ow)
    return img


def pad(img, n):
    if n <= 0:
        return img
    out = Image.new("RGBA", (img.width + 2 * n, img.height + 2 * n), (0, 0, 0, 0))
    out.alpha_composite(img, (n, n))
    return out


def tinted(alpha, color):
    out = Image.new("RGBA", alpha.size, color + (0,))
    out.putalpha(alpha)
    return out


def with_shadow(img, fx, W):
    off = int(float(fx.get("offset", 0.01)) * W)
    blur = max(0.0, float(fx.get("blur", 0.012)) * W)
    n = off + int(blur * 2) + 2
    base = pad(img, n)
    a = base.getchannel("A").point(lambda v: int(v * float(fx.get("opacity", 0.6))))
    shadow = tinted(a.filter(ImageFilter.GaussianBlur(blur)) if blur else a, rgb(fx.get("color"), (0, 0, 0)))
    out = Image.new("RGBA", base.size, (0, 0, 0, 0))
    out.alpha_composite(shadow, (off, off) if off < n else (0, 0))
    out.alpha_composite(base)
    return out


def with_effects(img, effects, W):
    """The layer's static effects baked around its sprite (padded so glows are not cut)."""
    for fx in effects or []:
        t = fx.get("type")
        if t == "glow":
            n = max(4, int(0.02 * W))
            base = pad(img, n)
            glow = base.copy().filter(ImageFilter.GaussianBlur(max(2, n // 2)))
            col = rgb(fx.get("color"), (255, 255, 255))
            a = glow.getchannel("A").point(lambda v, s=float(fx.get("strength", 0.6)): int(clamp(v * s * 1.6, 0, 255)))
            out = Image.new("RGBA", base.size, (0, 0, 0, 0))
            out.alpha_composite(tinted(a, col))
            out.alpha_composite(base)
            img = out
        elif t == "edge_glow":
            wpx = max(2, int(float(fx.get("width", 0.012)) * W))
            base = pad(img, wpx * 3)
            a = base.getchannel("A")
            ring = a.filter(ImageFilter.MaxFilter(3)).filter(ImageFilter.GaussianBlur(wpx))
            ring = ring.point(lambda v, s=float(fx.get("strength", 1.0)): int(clamp(v * s * 1.4, 0, 255)))
            out = Image.new("RGBA", base.size, (0, 0, 0, 0))
            out.alpha_composite(tinted(ring, rgb(fx.get("color"), (255, 51, 85))))
            out.alpha_composite(base)
            img = out
        elif t == "shadow":
            img = with_shadow(img, fx, W)
        elif t == "outline":
            wpx = max(1, int(float(fx.get("width", 0.004)) * W))
            base = pad(img, wpx + 1)
            ring = base.getchannel("A").filter(ImageFilter.MaxFilter(2 * wpx + 1))
            out = Image.new("RGBA", base.size, (0, 0, 0, 0))
            out.alpha_composite(tinted(ring, rgb(fx.get("color"), (255, 255, 255))))
            out.alpha_composite(base)
            img = out
        elif t == "blur":
            r = float(fx.get("radius", 0.01)) * W
            img = pad(img, int(r * 2)).filter(ImageFilter.GaussianBlur(r))
    return img


# ── Particles ────────────────────────────────────────────────────────────────────

def glow_dot(size, color):
    size = max(2, int(size))
    g = Image.radial_gradient("L").resize((size, size), Image.BILINEAR)
    a = ImageChops.invert(g).point(lambda v: int(v * v / 255))
    return tinted(a, color)


DOTS = {}


def draw_particles(canvas, layer, t, seed, W, H, opacity=1.0):
    kind = layer.get("kind", "embers")
    n = int(layer.get("count", 60))
    speed = float(layer.get("speed", 1.0))
    size = float(layer.get("size", 1.0)) * H / 1080
    color = rgb(layer.get("color") or PARTICLE_COLORS.get(kind, "#ffffff"))
    rng = np.random.default_rng(seed)
    x0, y0 = rng.random(n), rng.random(n)
    sp, ph, sz = rng.uniform(0.5, 1.5, n), rng.uniform(0, 2 * math.pi, n), rng.uniform(0.5, 1.5, n)
    d = ImageDraw.Draw(canvas)
    dot_cache = DOTS.setdefault(color, {})
    for i in range(n):
        if kind == "rain":
            x = (x0[i] + 0.08 * t * speed) % 1.0 * W
            y = (y0[i] + 1.6 * t * speed * sp[i]) % 1.0 * H
            ln = 30 * size * sp[i]
            d.line((x, y, x + ln * 0.2, y + ln), fill=color + (int(110 * clamp(opacity, 0, 1)),), width=max(1, int(size)))
            continue
        if kind == "embers":
            x = (x0[i] + 0.03 * math.sin(t * 1.5 + ph[i])) % 1.0
            y = (y0[i] - 0.12 * t * speed * sp[i]) % 1.0
            alpha = 0.5 + 0.5 * math.sin(t * 6 + ph[i] * 3)
            r = 10 * size * sz[i]
        elif kind == "snow":
            x = (x0[i] + 0.02 * math.sin(t + ph[i])) % 1.0
            y = (y0[i] + 0.07 * t * speed * sp[i]) % 1.0
            alpha, r = 0.85, 7 * size * sz[i]
        elif kind == "sparks":
            x = (x0[i] + 0.25 * t * speed * sp[i] * math.cos(ph[i])) % 1.0
            y = (y0[i] - 0.25 * t * speed * sp[i] * abs(math.sin(ph[i]))) % 1.0
            alpha, r = 0.6 + 0.4 * math.sin(t * 12 + ph[i]), 6 * size * sz[i]
        elif kind == "stars":
            x, y = x0[i], y0[i]
            alpha, r = 0.35 + 0.65 * (0.5 + 0.5 * math.sin(t * 2.5 * sp[i] + ph[i])), 6 * size * sz[i]
        else:  # dust
            x = (x0[i] + 0.015 * t * speed * math.cos(ph[i])) % 1.0
            y = (y0[i] + 0.01 * t * speed * math.sin(ph[i])) % 1.0
            alpha, r = 0.35, 5 * size * sz[i]
        k = max(2, int(r * 2))
        level = int(round(clamp(alpha * opacity, 0, 1) * 8))
        if level == 0:
            continue
        if (k, level) not in dot_cache:
            dot = glow_dot(k, color)
            if level < 8:
                dot.putalpha(dot.getchannel("A").point(lambda v, a=level / 8: int(v * a)))
            dot_cache[(k, level)] = dot
        paste(canvas, dot_cache[(k, level)], int(x * W - k / 2), int(y * H - k / 2))


# ── Compositing ──────────────────────────────────────────────────────────────────

def paste(canvas, sprite, x, y, blend="normal"):
    """Composites `sprite` at (x, y) of `canvas` (RGBA), clipped to it. add / screen / multiply
    blend the sprite's colour weighted by its alpha (Pillow channel ops, no per-pixel Python)."""
    W, H = canvas.size
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(W, x + sprite.width), min(H, y + sprite.height)
    if x1 <= x0 or y1 <= y0:
        return
    part = sprite.crop((x0 - x, y0 - y, x1 - x, y1 - y)) if (x0 - x, y0 - y, x1 - x, y1 - y) != (0, 0) + sprite.size else sprite
    if blend == "normal":
        canvas.alpha_composite(part, (x0, y0))
        return
    box = (x0, y0, x1, y1)
    dst = canvas.crop(box)
    d_rgb, d_a = dst.convert("RGB"), dst.getchannel("A")
    a = part.getchannel("A")
    a3 = Image.merge("RGB", (a, a, a))
    premult = ImageChops.multiply(part.convert("RGB"), a3)
    if blend == "add":
        mixed = ImageChops.add(d_rgb, premult)
    elif blend == "screen":
        mixed = ImageChops.screen(d_rgb, premult)
    else:  # multiply: d * ((1 - a) + s * a)
        mixed = ImageChops.multiply(d_rgb, ImageChops.add(ImageChops.invert(a3), premult))
    out = mixed.convert("RGBA")
    out.putalpha(ImageChops.lighter(d_a, a))
    canvas.paste(out, box)


class Renderer:
    def __init__(self, spec, scale=1.0, config=None, work_dir=None):
        self.spec = normalise(spec)
        self.scale = scale
        self.W = max(16, int(round(self.spec["width"] * scale / 2)) * 2)
        self.H = max(16, int(round(self.spec["height"] * scale / 2)) * 2)
        self.fps = float(self.spec["fps"])
        self.total = float(self.spec["duration"])
        self.assets = Assets(self.W / 1920)
        self.backgrounds = {}
        self.layer_sprites = {}
        self.vignettes = {}
        self.config = config or {}
        self.work_dir = work_dir or tempfile.mkdtemp(prefix="mv-motion-")
        self.warnings = []
        self.layered = {}

    # Sprites -----------------------------------------------------------------------
    def fit_scale(self, layer, img):
        W, H = self.W, self.H
        iw, ih = img.size
        if layer.get("width") is not None:
            return float(layer["width"]) * W / iw
        fit = layer.get("fit", "contain")
        if fit == "cover":
            return max(W / iw, H / ih)
        if fit == "width":
            return W / iw
        if fit == "height":
            return H / ih
        if fit == "none":
            return W / 1920
        return min(W / iw, H / ih)

    def sprite(self, key, layer, t_local):
        """(sprite with its static effects, content size at scale 1, the scale the sprite is
        drawn at) for a layer at a time. Images are drawn at their largest keyframed scale, so
        a zoom only ever shrinks them (sharp)."""
        ty = layer["type"]
        if ty == "text" and layer.get("typewriter"):
            span = max(0.05, (layer.get("end", 1e9) - layer.get("start", 0)) * 0.6)
            n = len(str(layer.get("text", "")))
            chars = int(clamp((t_local - layer.get("start", 0)) / span, 0, 1) * n)
            k = (key, chars)
            if k not in self.layer_sprites:
                img = text_sprite(self.assets, layer, self.W, self.H, chars)
                self.layer_sprites[k] = (with_effects(img, layer.get("effects"), self.W), img.size, 1.0)
            return self.layer_sprites[k]
        if key not in self.layer_sprites:
            if ty == "image":
                src = self.assets.image(layer["src"])
                s = self.fit_scale(layer, src)
                peak = max([float(layer.get("scale", 1))] + [float(k["scale"]) for k in layer.get("keyframes") or [] if "scale" in k] + [1.0])
                peak = min(peak, 3.0)
                size = (max(1, int(src.width * s * peak)), max(1, int(src.height * s * peak)))
                img = src.resize(size, Image.LANCZOS)
                content = (src.width * s, src.height * s)
                self.layer_sprites[key] = (with_effects(img, layer.get("effects"), self.W), content, peak)
                return self.layer_sprites[key]
            img = text_sprite(self.assets, layer, self.W, self.H) if ty == "text" else shape_sprite(layer, self.W, self.H)
            self.layer_sprites[key] = (with_effects(img, layer.get("effects"), self.W), img.size, 1.0)
        return self.layer_sprites[key]

    def background(self, i, scene):
        if i not in self.backgrounds:
            bg = scene.get("background") or {}
            img = Image.new("RGBA", (self.W, self.H), rgb(bg.get("color"), rgb(self.spec.get("background"), (0, 0, 0))) + (255,))
            src = bg.get("image")
            if src and src_path(src) and os.path.isfile(src_path(src)):
                pic = self.assets.image(src)
                fit = bg.get("fit", "cover")
                s = max(self.W / pic.width, self.H / pic.height) if fit != "contain" else min(self.W / pic.width, self.H / pic.height)
                pic = pic.resize((max(1, int(pic.width * s)), max(1, int(pic.height * s))), Image.LANCZOS)
                blur = float(bg.get("blur", 0)) * self.H / 1080
                if blur > 0:
                    pic = pic.filter(ImageFilter.GaussianBlur(blur))
                paste(img, pic, (self.W - pic.width) // 2, (self.H - pic.height) // 2)
                dim = float(bg.get("dim", 0))
                if dim > 0:
                    img.alpha_composite(Image.new("RGBA", img.size, (0, 0, 0, int(255 * clamp(dim, 0, 1)))))
            self.backgrounds[i] = img
        return self.backgrounds[i].copy()

    # One scene at its own time ------------------------------------------------------
    def scene_frame(self, i, t, frame_no):
        scene = self.spec["scenes"][i]
        canvas = self.background(i, scene)
        W, H = self.W, self.H
        for j, layer in enumerate(scene.get("layers") or []):
            start, end = float(layer.get("start", 0)), float(layer.get("end", scene["duration"]))
            if t < start or t > end + 1e-6:
                continue
            if layer["type"] == "particles":
                draw_particles(canvas, layer, t, i * 1000 + j, W, H, prop_at(layer, "opacity", t))
                continue
            self.draw_layer(canvas, (i, j), layer, t, frame_no)
        for fx in scene.get("effects") or []:
            if fx.get("type") == "particles":
                draw_particles(canvas, fx, t, i * 1000 + 999, W, H)
        return self.scene_effects(canvas.convert("RGB"), scene, t, frame_no)

    def draw_layer(self, canvas, key, layer, t, frame_no):
        W, H = self.W, self.H
        x, y = prop_at(layer, "x", t), prop_at(layer, "y", t)
        scale, rot, op = prop_at(layer, "scale", t), prop_at(layer, "rotation", t), prop_at(layer, "opacity", t)
        for fx in layer.get("effects") or []:
            k = fx.get("type")
            if k == "pulse":
                scale *= 1 + float(fx.get("amount", 0.05)) * math.sin(2 * math.pi * t / float(fx.get("period", 2)))
            elif k == "float":
                y += float(fx.get("amount", 0.02)) * math.sin(2 * math.pi * t / float(fx.get("period", 2)))
            elif k == "shake":
                at, dur = float(fx.get("at", 0)), float(fx.get("duration", 1e9))
                if at <= t <= at + dur:
                    s = float(fx.get("strength", 0.4)) * 0.02
                    x += s * noise(f"{key}x", frame_no)
                    y += s * noise(f"{key}y", frame_no) * W / H
        if op <= 0.003 or scale <= 0.001:
            return
        animated = self.layered_frame(key, layer, t)
        if animated is not None:
            sprite, content, peak = animated, animated.size, 1.0
        else:
            sprite, content, peak = self.sprite(key, layer, t)
        f = scale / peak
        if abs(f - 1) > 1e-3:
            sprite = sprite.resize((max(1, int(sprite.width * f)), max(1, int(sprite.height * f))), Image.BILINEAR)
        if abs(rot) > 0.05:
            sprite = sprite.rotate(-rot, resample=Image.BICUBIC, expand=True)
        if op < 0.999:
            sprite = sprite.copy()
            sprite.putalpha(sprite.getchannel("A").point(lambda v, o=op: int(v * o)))
        ax, ay = (layer.get("anchor") or [0.5, 0.5])[:2]
        cw, ch = content[0] * scale, content[1] * scale
        dx, dy = (0.5 - ax) * cw, (0.5 - ay) * ch
        r = math.radians(rot)
        cx = x * W + dx * math.cos(r) - dy * math.sin(r)
        cy = y * H + dx * math.sin(r) + dy * math.cos(r)
        paste(canvas, sprite, int(round(cx - sprite.width / 2)), int(round(cy - sprite.height / 2)), layer.get("blend", "normal"))

    # The Layered motion pack (2.5D panels), when installed -------------------------
    def layered_frame(self, key, layer, t):
        if layer.get("type") != "image" or not layer.get("layered"):
            return None
        frames = self.layered.get(key)
        if frames is None:
            frames = self.layered[key] = self.run_layered(key, layer) or []
        if not frames:
            return None
        span = max(1e-3, float(layer.get("end", 1)) - float(layer.get("start", 0)))
        k = int(clamp((t - float(layer.get("start", 0))) / span, 0, 1) * (len(frames) - 1))
        return frames[k]

    def run_layered(self, key, layer):
        """Animates a panel with the pack (video_generator.run_layered); [] when the pack is not
        installed or the source has no read.json panel, so the layer renders as a still."""
        src = layer.get("src") if isinstance(layer.get("src"), dict) else {}
        panel, out_dir = src.get("panel"), src.get("output_dir")
        if not panel or not out_dir:
            self.warnings.append("layered: needs a panel source ({\"manga\", \"chapter\", \"page\", \"panel\"}); rendered as a still")
            return []
        try:
            import layered_plan
            import video_generator as vg
        except Exception as e:  # pragma: no cover - depends on the install
            self.warnings.append(f"layered: not available ({e}); rendered as a still")
            return []
        py, why = layered_plan.pack_python(self.config)
        if not py:
            self.warnings.append(f"layered: the Layered motion pack is not installed ({why}); rendered as a still")
            return []
        img = self.assets.image(src)
        s = self.fit_scale(layer, img)
        size = [max(2, int(img.width * s) // 2 * 2), max(2, int(img.height * s) // 2 * 2)]
        frames = max(2, int((float(layer.get("end", 1)) - float(layer.get("start", 0))) * self.fps))
        path = src_path(src)
        out = os.path.join(self.work_dir, f"layered_{key[0]}_{key[1]}.mp4")
        job = {"id": "motion", "src": path, "panel": panel, "scene": {}, "intensity": 5,
               "cache": os.path.join(layered_plan.layers_root(out_dir), layered_plan.panel_key(path, panel)), "size": size,
               "frames": frames, "ease": [i / (frames - 1) for i in range(frames)], "drift": [0.6, 0.0], "grade": None, "glow": False,
               "faces": [list(f) for _, f in layered_plan.panel_faces(panel)], "out": out}
        res = vg.run_layered({"motion": job}, self.config, self.work_dir).get("motion") or {}
        if not res.get("ok") or not os.path.isfile(out):
            self.warnings.append(f"layered: the pack could not animate the panel ({res.get('error', 'no result')}); rendered as a still")
            return []
        raw = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", out, "-f", "rawvideo", "-pix_fmt", "rgba", "-"], capture_output=True, timeout=600).stdout
        n = size[0] * size[1] * 4
        return [Image.frombytes("RGBA", tuple(size), raw[i:i + n]) for i in range(0, len(raw) - n + 1, n)]

    # Scene effects ------------------------------------------------------------------
    def vignette(self, strength):
        """The vignette as an RGB multiply mask (made once per strength)."""
        k = round(strength, 2)
        if k not in self.vignettes:
            yy, xx = np.mgrid[0:self.H, 0:self.W].astype(np.float32)
            d = np.sqrt(((xx - self.W / 2) / (self.W / 2)) ** 2 + ((yy - self.H / 2) / (self.H / 2)) ** 2) / math.sqrt(2)
            m = ((1 - clamp(k, 0, 1) * np.clip(d, 0, 1) ** 2 * 1.2).clip(0, 1) * 255).astype(np.uint8)
            g = Image.fromarray(m)
            self.vignettes[k] = Image.merge("RGB", (g, g, g))
        return self.vignettes[k]

    def grain(self, amount, frame_no):
        """One of a few film-grain plates (grey around 128), cycled per frame."""
        key = ("grain", round(amount, 2), frame_no % 6)
        if key not in self.vignettes:
            rng = np.random.default_rng(key[2] + 7)
            n = np.clip(128 + rng.normal(0, 24 * amount, (self.H, self.W)), 0, 255).astype(np.uint8)
            g = Image.fromarray(n)
            self.vignettes[key] = Image.merge("RGB", (g, g, g))
        return self.vignettes[key]

    def scene_effects(self, img, scene, t, frame_no):
        fxs = scene.get("effects") or []
        if not fxs:
            return img
        W, H = self.W, self.H
        for fx in fxs:
            k = fx.get("type")
            if k == "zoom":
                p = ease(fx.get("ease", "in_out"), t / max(1e-3, scene["duration"]))
                s = float(fx.get("from", 1)) + (float(fx.get("to", 1.1)) - float(fx.get("from", 1))) * p
                img = zoomed(img, s)
            elif k == "shake":
                at, dur = float(fx.get("at", 0)), float(fx.get("duration", 0.4))
                if at <= t <= at + dur:
                    s = float(fx.get("strength", 0.4)) * 0.03 * (1 - (t - at) / max(dur, 1e-3))
                    dx, dy = int(s * W * noise("sx", frame_no)), int(s * H * noise("sy", frame_no))
                    img = ImageChops.offset(img, dx, dy)
        for fx in fxs:
            k = fx.get("type")
            if k == "grade":
                img = grade(img, fx.get("preset", "warm"))
            elif k == "vignette":
                img = ImageChops.multiply(img, self.vignette(float(fx.get("strength", 0.4))))
            elif k == "grain":
                img = ImageChops.add(img, self.grain(float(fx.get("amount", 0.25)), frame_no), 1.0, -128)
            elif k == "chromatic":
                n = max(1, int(float(fx.get("amount", 0.3)) * 0.006 * W))
                r, g, b = img.split()
                img = Image.merge("RGB", (ImageChops.offset(r, -n, 0), g, ImageChops.offset(b, n, 0)))
            elif k == "letterbox":
                b = int(float(fx.get("size", 0.1)) * H / 2)
                if b > 0:
                    d = ImageDraw.Draw(img)
                    d.rectangle((0, 0, W, b - 1), fill=(0, 0, 0))
                    d.rectangle((0, H - b, W, H), fill=(0, 0, 0))
            elif k == "flash":
                at, dur = float(fx.get("at", 0)), max(0.02, float(fx.get("duration", 0.25)))
                if at <= t <= at + dur:
                    p = (t - at) / dur
                    s = 1 - abs(2 * p - 1) if p < 0.5 else 1 - p
                    img = Image.blend(img, Image.new("RGB", img.size, rgb(fx.get("color"), (255, 255, 255))), clamp(s, 0, 1))
        return img

    # The whole video at a time ------------------------------------------------------
    def frame(self, t, frame_no=0):
        scenes = self.spec["scenes"]
        cur = 0
        for i, s in enumerate(scenes):
            if t >= s["start"] - 1e-9:
                cur = i
        s = scenes[cur]
        tr = s.get("transition")
        img = self.scene_frame(cur, clamp(t - s["start"], 0, s["duration"]), frame_no)
        if tr and cur > 0 and t < s["start"] + tr["duration"]:
            prev = scenes[cur - 1]
            out = self.scene_frame(cur - 1, clamp(t - prev["start"], 0, prev["duration"]), frame_no)
            p = clamp((t - s["start"]) / max(1e-3, tr["duration"]), 0, 1)
            img = transition(out, img, tr["type"], p)
        return img


def _lut(fn):
    return [int(clamp(fn(v), 0, 255)) for v in range(256)]


GRADE_LUTS = {
    "warm": (_lut(lambda v: v * 1.08), _lut(lambda v: v), _lut(lambda v: v * 0.9)),
    "cool": (_lut(lambda v: v * 0.9), _lut(lambda v: v), _lut(lambda v: v * 1.1)),
    # Shadows towards teal, highlights towards orange (by each channel's own level).
    "teal_orange": tuple(_lut(lambda v, t=t, o=o: v * (t * (1 - v / 255) + o * v / 255)) for t, o in ((0.85, 1.12), (1.02, 1.0), (1.12, 0.85))),
}
NOIR_CURVE = _lut(lambda v: (v - 128) * 1.25 + 128)


def grade(img, preset):
    """A colour grade on an RGB frame (lookup tables, so it costs about nothing)."""
    if preset in GRADE_LUTS:
        r, g, b = GRADE_LUTS[preset]
        return img.point(r + g + b)
    if preset == "noir":
        l = img.convert("L").point(NOIR_CURVE)
        return Image.merge("RGB", (l, l, l))
    if preset == "vivid":
        from PIL import ImageEnhance
        return ImageEnhance.Color(img).enhance(1.35)
    if preset == "sepia":
        return img.convert("RGB", (0.393, 0.769, 0.189, 0, 0.349, 0.686, 0.168, 0, 0.272, 0.534, 0.131, 0))
    return img


def transition(a, b, kind, p):
    """Blends the outgoing frame `a` into the incoming `b` at progress p (0..1)."""
    W, H = a.size
    e = ease("in_out", p)
    if kind == "fade":
        return Image.blend(a, b, e)
    if kind in ("fadeblack", "fadewhite"):
        mid = Image.new("RGB", a.size, (0, 0, 0) if kind == "fadeblack" else (255, 255, 255))
        return Image.blend(a, mid, min(1, p * 2)) if p < 0.5 else Image.blend(mid, b, (p - 0.5) * 2)
    if kind in ("slide_left", "slide_right", "slide_up", "slide_down", "whip"):
        horiz = kind in ("slide_left", "slide_right", "whip")
        sign = -1 if kind in ("slide_left", "slide_up", "whip") else 1
        out = Image.new("RGB", a.size)
        off = int((W if horiz else H) * e)
        if horiz:
            out.paste(a, (sign * off, 0))
            out.paste(b, (sign * off - sign * W, 0))
        else:
            out.paste(a, (0, sign * off))
            out.paste(b, (0, sign * off - sign * H))
        if kind == "whip":
            k = 1 - abs(2 * p - 1)
            if k > 0.05:
                n = max(2, int(W / (1 + 40 * k)))
                out = out.resize((n, H), Image.BILINEAR).resize((W, H), Image.BILINEAR)
        return out
    if kind in ("zoom_in", "zoom_out"):
        grow = kind == "zoom_in"
        s_a = 1 + (0.25 if grow else -0.2) * e
        s_b = (1.2 if grow else 0.85) + ((1 - (1.2 if grow else 0.85)) * e)
        za = zoomed(a, s_a)
        zb = zoomed(b, s_b)
        return Image.blend(za, zb, e)
    if kind in ("wipe_left", "wipe_right"):
        mask = Image.new("L", a.size, 0)
        edge = int(W * e)
        ImageDraw.Draw(mask).rectangle((W - edge, 0, W, H) if kind == "wipe_left" else (0, 0, edge, H), fill=255)
        return Image.composite(b, a, mask)
    if kind == "flash":
        mixed = Image.blend(a, b, e)
        s = 1 - abs(2 * p - 1)
        return Image.blend(mixed, Image.new("RGB", a.size, (255, 255, 255)), s * 0.9)
    return b


def zoomed(img, s):
    W, H = img.size
    if abs(s - 1) < 1e-3:
        return img
    big = img.resize((max(1, int(W * s)), max(1, int(H * s))), Image.BILINEAR)
    out = Image.new("RGB", (W, H))
    out.paste(big, ((W - big.width) // 2, (H - big.height) // 2))
    return out


# ── Encoding ─────────────────────────────────────────────────────────────────────

def encoder_options():
    """Encoder args to try in order: NVENC first when it works (video_generator's choice)."""
    threads = max(2, (os.cpu_count() or 4) // 2)
    try:
        import video_generator as vg
        return vg.final_codecs(threads)
    except Exception:
        return [["-c:v", "libx264", "-preset", "fast", "-crf", "20", "-pix_fmt", "yuv420p", "-threads", str(threads)]]


# Frames are independent, so a long render draws them in worker processes (each with its own
# Renderer), at most WINDOW per worker in flight so memory stays bounded.
_WORKER = None
WINDOW = 3
PARALLEL_MIN_FRAMES = 48


def _worker_init(spec, scale, config):
    global _WORKER
    os.environ.setdefault("MANGAVOX_VIDEO_GPU", "0")
    _WORKER = Renderer(spec, scale, config)


def _worker_frame(args):
    t, i = args
    return _WORKER.frame(t, i).tobytes()


def workers_for(renderer, n_frames):
    """How many processes draw the frames (1: here). MANGAVOX_MOTION_WORKERS overrides."""
    try:
        forced = int(os.environ.get("MANGAVOX_MOTION_WORKERS", ""))
        return max(1, forced)
    except ValueError:
        pass
    if n_frames < PARALLEL_MIN_FRAMES or any(l.get("layered") for s in renderer.spec["scenes"] for l in s.get("layers") or []):
        return 1
    return max(1, min(6, (os.cpu_count() or 2) - 1))


def frames(renderer, times, workers):
    """The frames' RGB bytes in order."""
    if workers <= 1:
        for i, t in enumerate(times):
            yield renderer.frame(t, i).tobytes()
        return
    from collections import deque
    from concurrent.futures import ProcessPoolExecutor
    with ProcessPoolExecutor(max_workers=workers, initializer=_worker_init, initargs=(renderer.spec, renderer.scale, renderer.config)) as ex:
        pending = deque()
        it = iter(enumerate(times))
        for i, t in it:
            pending.append(ex.submit(_worker_frame, (t, i)))
            if len(pending) >= workers * WINDOW:
                break
        while pending:
            yield pending.popleft().result()
            nxt = next(it, None)
            if nxt is not None:
                pending.append(ex.submit(_worker_frame, (nxt[1], nxt[0])))


def encode(renderer, times, out_path, progress=None):
    """Draws the frames at `times` and encodes them; returns the encoder used."""
    W, H, fps = renderer.W, renderer.H, renderer.fps
    workers = workers_for(renderer, len(times))
    last_err = ""
    for codec in encoder_options():
        cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}",
               "-r", f"{fps:g}", "-i", "-", *codec, "-movflags", "+faststart", out_path]
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=flags)
        ok = True
        t0 = time.time()
        try:
            for i, data in enumerate(frames(renderer, times, workers)):
                proc.stdin.write(data)
                if progress and (i % max(1, int(fps)) == 0 or i == len(times) - 1):
                    progress(i + 1, len(times), t0)
            proc.stdin.close()
        except (BrokenPipeError, OSError):
            ok = False
        err = proc.stderr.read().decode("utf-8", "replace")
        proc.stderr.close()
        proc.wait()
        if ok and proc.returncode == 0 and os.path.isfile(out_path):
            return "h264_nvenc" if "h264_nvenc" in codec else "libx264"
        last_err = err.strip().splitlines()[-1] if err.strip() else f"ffmpeg exited {proc.returncode}"
        try:
            import video_generator as vg
            vg.encoder_failed(codec)
        except Exception:
            pass
    raise RuntimeError(f"FFmpeg could not encode the video: {last_err}")


# ── Audio ────────────────────────────────────────────────────────────────────────

def narration_clips(items, work_dir, warnings):
    """[(path, at)] for the narration items, voiced with the app's TTS (tts_bridge.run_tts)."""
    if not items:
        return []
    out = []
    groups = {}
    for i, n in enumerate(items):
        groups.setdefault((n.get("voice") or "af_heart", n.get("engine") or "kokoro", float(n.get("speed", 1.0))), []).append((i, n))
    try:
        import tts_bridge
    except Exception as e:
        warnings.append(f"narration skipped: the voices are not available ({e})")
        return []
    for (voice, engine, speed), group in groups.items():
        d = os.path.join(work_dir, f"tts_{len(out)}_{abs(hash(voice)) % 10000}")
        os.makedirs(d, exist_ok=True)
        try:
            res = tts_bridge.run_tts([{"page": i, "text": n["text"]} for i, n in group], d, {"engine": engine, "voice": voice, "speed": speed}) or []
        except Exception as e:
            warnings.append(f"narration in {voice} failed ({e})")
            continue
        by_page = {int(r.get("page", -1)): r for r in res if isinstance(r, dict)}
        for i, n in group:
            r = by_page.get(i) or {}
            if r.get("path") and os.path.isfile(r["path"]):
                out.append((r["path"], float(n.get("at", 0))))
            else:
                warnings.append(f"narration {i + 1} was not voiced ({r.get('error') or 'no audio'})")
    return out


def sfx_clips(items, work_dir, warnings):
    out = []
    for i, s in enumerate(items or []):
        at, vol = float(s.get("at", 0)), float(s.get("volume", 1.0))
        if s.get("name"):
            try:
                import sfx
                path = os.path.join(work_dir, f"sfx_{i}_{s['name']}.wav")
                sfx.write_wav(path, sfx.load_sound(s["name"]))
                out.append((path, at, vol))
            except Exception as e:
                warnings.append(f"sound {s['name']} skipped ({e})")
        elif src_path(s.get("src")) and os.path.isfile(src_path(s.get("src"))):
            out.append((src_path(s["src"]), at, vol))
    return out


def mix_audio(spec, total, video_path, out_path, work_dir, warnings, t0=0.0):
    """Muxes narration, music and sound effects under the video. False when there is no audio."""
    audio = spec.get("audio") or {}
    nar = narration_clips(audio.get("narration") or [], work_dir, warnings)
    fx = sfx_clips(audio.get("sfx") or [], work_dir, warnings)
    music = audio.get("music") if isinstance(audio.get("music"), dict) else None
    mpath = src_path(music.get("src")) if music else None
    if music and not (mpath and os.path.isfile(mpath)):
        warnings.append("music skipped: file not found")
        mpath = None
    if not nar and not fx and not mpath:
        return False
    inputs, chains, labels = ["-i", video_path], [], []
    k = 1
    master = float(audio.get("volume", 1.0))
    for path, at in nar:
        if at + 0.01 < t0:
            continue
        inputs += ["-i", path]
        ms = int(max(0.0, at - t0) * 1000)
        chains.append(f"[{k}:a]aresample={AUDIO_RATE},adelay={ms}|{ms},volume={master:.3f}[a{k}]")
        labels.append(f"[a{k}]")
        k += 1
    for path, at, vol in fx:
        if at + 0.01 < t0:
            continue
        inputs += ["-i", path]
        ms = int(max(0.0, at - t0) * 1000)
        chains.append(f"[{k}:a]aresample={AUDIO_RATE},adelay={ms}|{ms},volume={vol * master:.3f}[a{k}]")
        labels.append(f"[a{k}]")
        k += 1
    if mpath:
        if music.get("loop"):
            inputs += ["-stream_loop", "-1"]
        inputs += ["-ss", f"{float(music.get('start', 0)) + t0:.3f}", "-i", mpath]
        fi, fo = float(music.get("fade_in", 0.5)), float(music.get("fade_out", 2.0))
        f = f"[{k}:a]aresample={AUDIO_RATE},atrim=0:{total:.3f},volume={float(music.get('volume', 0.35)) * master:.3f}"
        if fi > 0 and t0 == 0:
            f += f",afade=t=in:st=0:d={fi:.2f}"
        if fo > 0:
            f += f",afade=t=out:st={max(0.0, total - fo):.2f}:d={fo:.2f}"
        chains.append(f + f"[a{k}]")
        labels.append(f"[a{k}]")
        k += 1
    if not labels:
        return False
    graph = ";".join(chains) + f";{''.join(labels)}amix=inputs={len(labels)}:normalize=0:dropout_transition=0,alimiter=limit=0.95,apad,atrim=0:{total:.3f}[aout]"
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *inputs, "-filter_complex", graph, "-map", "0:v", "-map", "[aout]",
           "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-t", f"{total:.3f}", "-movflags", "+faststart", out_path]
    r = subprocess.run(cmd, capture_output=True, timeout=1800, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    if r.returncode != 0:
        tail = r.stderr.decode("utf-8", "replace").strip().splitlines()
        warnings.append(f"audio could not be mixed ({tail[-1] if tail else r.returncode}); the video has no sound")
        return False
    return True


# ── Jobs ─────────────────────────────────────────────────────────────────────────

def frame_times(start, seconds, fps):
    n = max(1, int(round(seconds * fps)))
    return [start + i / fps for i in range(n)]


# Work folders the app hands us live directly inside one of these (motion/.work/<job>,
# motion/.preview/work-<stamp>); no other folder is ever deleted.
WORK_PARENTS = (".work", ".preview")


def removable_work_dir(work):
    """True only for a folder strictly inside a `.work` / `.preview` parent, so a job whose work
    dir resolves to the motion folder itself (a render named "..") never deletes it."""
    if not work:
        return False
    path = os.path.realpath(os.path.abspath(str(work)))
    parent = os.path.dirname(path)
    return parent != path and os.path.basename(parent) in WORK_PARENTS


def render(job):
    spec = job.get("spec") or {}
    errs = check(spec)
    if errs:
        emit({"type": "error", "message": "The spec has problems", "errors": errs})
        return 1
    work = job.get("work_dir") or tempfile.mkdtemp(prefix="mv-motion-")
    os.makedirs(work, exist_ok=True)
    r = Renderer(spec, float(job.get("scale", 1.0)), job.get("config"), work)
    if r.total > MAX_SECONDS + 0.01:
        emit({"type": "error", "message": f"The video would be {r.total:.1f} s long; at most {MAX_SECONDS:.0f} s"})
        return 1
    mode = job.get("mode", "full")
    result = {"type": "done", "width": r.W, "height": r.H, "fps": r.fps, "duration": round(r.total, 3), "warnings": r.warnings}

    def progress(i, n, t0):
        el = time.time() - t0
        eta = el / i * (n - i) if i else 0
        emit({"type": "progress", "fraction": round(i / n, 3), "message": f"frame {i}/{n}" + (f", about {int(eta)} s left" if eta > 3 else "")})

    try:
        frame_path = job.get("frame")
        at = float(job.get("frame_at", min(r.total, r.spec["scenes"][0]["duration"] / 2)))
        if frame_path:
            os.makedirs(os.path.dirname(os.path.abspath(frame_path)), exist_ok=True)
            r.frame(clamp(at, 0, max(0.0, r.total - 1e-3)), int(at * r.fps)).save(frame_path)
            result["frame"] = frame_path
        if mode == "frame":
            emit(result)
            return 0
        out = job["out"]
        os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
        start = float(job.get("start", 0)) if mode == "clip" else 0.0
        seconds = min(float(job.get("seconds", 3)), r.total - start) if mode == "clip" else r.total
        silent = os.path.join(work, "video_silent.mp4")
        result["encoder"] = encode(r, frame_times(start, max(1 / r.fps, seconds), r.fps), silent, progress)
        has_audio = False
        if mode == "full":
            emit({"type": "progress", "fraction": 1.0, "message": "mixing the audio"})
            has_audio = mix_audio(r.spec, r.total, silent, out + ".tmp.mp4", work, r.warnings)
        if has_audio:
            os.replace(out + ".tmp.mp4", out)
        else:
            if os.path.exists(out):
                os.remove(out)
            shutil.move(silent, out)
        result["path"] = out
        result["audio"] = has_audio
        emit(result)
        return 0
    except Exception as e:
        emit({"type": "error", "message": f"{type(e).__name__}: {e}"})
        return 1
    finally:
        if removable_work_dir(job.get("work_dir")):
            shutil.rmtree(work, ignore_errors=True)


def main(argv):
    if len(argv) < 3 or argv[1] not in ("render", "validate"):
        emit({"type": "error", "message": "Usage: motion_compose.py render job.json | validate spec.json"})
        return 2
    with open(argv[2], "r", encoding="utf-8") as f:
        data = json.load(f)
    if argv[1] == "validate":
        errs = check(data)
        print(json.dumps({"ok": not errs, "errors": errs}), flush=True)
        return 0 if not errs else 1
    return render(data)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
