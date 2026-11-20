#!/usr/bin/env python3
"""
Anime compositing for Creative's "Make an anime": the photography stage (撮影, satsuei) of an
anime studio, done on this PC. The AI director plans every cut as layers; the image and video
models paint and animate them; this script reads the layers back and puts each cut together the
way a compositor does in After Effects:

  * the background plate (one painting per place and angle, reused by every cut set there),
    framed by a virtual camera that pushes, pulls, pans, tilts or shakes over it, as far and as
    slowly as the app says (speeds measured on real episodes: a slow drift, hardly any zoom);
  * the characters, painted and animated on a flat chroma screen (green, or magenta for a
    character with green in their look), keyed (screen-difference key, automatic screen colour,
    despill, choke and a one-pixel feather), stepped on threes like drawn animation, seen
    through the same camera as the plate (what stands on its ground stays on it),
    colour-matched to the plate, light-wrapped by it and grounded with a soft contact shadow;
  * effects elements (fire, aura, sparks, smoke) painted or animated on black and screened or
    added over the frame, placed where the director put them, plus procedural ones (dust motes,
    light rays, impact flash, speed vignette);
  * the finish every cut shares, so the episode reads as one film: bloom on highlights, a soft
    diffusion glow, a colour grade from the colour script, a touch of chromatic aberration,
    vignette and one film grain over everything.

A cut rendered "full" (an action cut the video model animated whole) skips the layers and only
gets the finish, so it sits with the layered cuts.

The animatic uses the same camera on the manga panel itself ("base" with "fit": "contain" shows
any panel whole over a blurred copy of itself; "scan" travels a tall panel from end to end, as a
reader scrolls it), a text card for a cut that has no panel, and a slate in the corner.

Compositing runs on PyTorch (the VFX pack's venv: on the graphics card with CUDA, else the CPU);
`panels` and `edit` need only numpy, Pillow and FFmpeg on PATH.

Usage:
  python anime_comp.py panels <job.json>   cut a chapter's pages into panels
  python anime_comp.py comp <job.json>     render one cut to MP4
  python anime_comp.py still <job.json>    the cut's first frame as PNG (key frames, previews)
  python anime_comp.py edit <job.json>     join the cuts with their transitions and sound

Every command prints one JSON line last: {"ok": true, ...} or {"ok": false, "message": "..."}.
Progress lines before it: {"type": "progress", "done": n, "total": m}.

panels job: {"pages": [path, ...], "out_dir": dir, "rtl": true, "min_area": 0.035}
  -> {"ok": true, "panels": [{"file", "page" (0-based), "box": [x0, y0, x1, y1], "w", "h"}]}

comp / still job:
  {"out": path, "width": 1920, "height": 1080, "fps": 24, "seconds": 3.5,
   "plate": {"path": img, "move": "push_in"} | null,
   "layout": {"path": img} | null  (the cut's layout frame the first char layer was lifted from:
           its shadows are carried under the layer; if the plate drifted from it, it is the plate)
   "base": {"path": video_or_image, "step": 1, "fit": "cover" | "contain" | "scan"} | null
           (a "full" cut: the animated clip itself; or, for the animatic, the manga panel)
   "card": {"title": "...", "text": "..."} | null   (animatic: a cut with no picture yet)
   "slate": "C012  3.5s  medium / low  push in" | null, "quality": "final" | "rough",
   "chars": [{"path": video_or_image, "key": "green" | "alpha" | "none", "step": 2,
              "shadow": true, "match": 0.3, "wrap": 0.5, "parallax": 1.0}],
   "book": [{"path": png_or_video, "key": "alpha" | "green", "parallax": 1.35}]  (foreground
           objects in front of the characters: pillars, rubble, leaves; slightly out of focus)
   "fx": [{"path": video_or_image, "blend": "screen" | "add" | "over", "opacity": 1.0,
           "x": 0.5, "y": 0.5, "scale": 1.0, "front": true}],
   "camera": {"move": "fix|push_in|pull_out|pan_left|pan_right|tilt_up|tilt_down|shake|
              track_in|whip", "travel": 0.12 (frame widths the view slides over the cut),
              "zoom": 0.04 (how much it grows or shrinks), "shake": 0.0},
   "grade": "neutral" | "cold_dungeon" | ..., "finish": {"bloom": 0.35, "diffusion": 0.15,
   "grain": 0.035, "ca": 1.0, "vignette": 0.25, "dust": 0, "rays": null | [x, y],
   "flash": 0.0, "speed": 0.0}, "seed": 1}

edit job: {"out": path, "width", "height", "fps", "quality": "final" | "rough",
           "cuts": [{"video": path, "seconds": 3.5, "transition": "cut|dissolve|white|black|smash",
                     "voice": wav | null, "voice_at": 0.5}],
           "music": path | null, "music_db": -17}
  -> {"ok": true, "out", "seconds", "starts": [where each cut starts]}. Cuts are whole frames
     long; hard cuts are joined without re-encoding, soft ones cross-faded.
"""
import json
import math
import os
import random
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image

FLAGS = 0x08000000 if os.name == "nt" else 0  # no console window on Windows

VIDEO_EXT = (".mp4", ".mov", ".webm", ".mkv", ".gif", ".avi")


def emit(obj):
    print(json.dumps(obj), flush=True)


def progress(done, total):
    emit({"type": "progress", "done": done, "total": total})


# ----------------------------------------------------------------------------- panels


def _uniform_rows(gray, axis, bg_tol=10.0, std_tol=6.0):
    """Rows (axis=1) or columns (axis=0) that are one flat colour: a gutter candidate. Returns a
    bool array and the colour of each line."""
    mean = gray.mean(axis=axis)
    std = gray.std(axis=axis)
    rng = gray.max(axis=axis) - gray.min(axis=axis)
    return (std < std_tol) & (rng < bg_tol * 4), mean


def _bands(flat, min_len):
    """[start, end) runs of True at least `min_len` long."""
    out, start = [], None
    for i, f in enumerate(list(flat) + [False]):
        if f and start is None:
            start = i
        elif not f and start is not None:
            if i - start >= min_len:
                out.append((start, i))
            start = None
    return out


def _split(gray, axis, min_gap):
    """Splits along gutters: returns [start, end) spans of content between flat bands (the bands
    at the edges are trimmed away)."""
    length = gray.shape[0] if axis == 1 else gray.shape[1]
    flat, _ = _uniform_rows(gray, axis)
    bands = _bands(flat, min_gap)
    spans, prev = [], 0
    for a, b in bands:
        if a > prev:
            spans.append((prev, a))
        prev = b
    if prev < length:
        spans.append((prev, length))
    return [s for s in spans if s[1] - s[0] > 2]


def _trim(gray):
    """Bounding box of the non-flat content of a crop: (y0, y1, x0, x1)."""
    h, w = gray.shape
    rows, _ = _uniform_rows(gray, 1)
    cols, _ = _uniform_rows(gray, 0)
    ys = np.where(~rows)[0]
    xs = np.where(~cols)[0]
    if len(ys) == 0 or len(xs) == 0:
        return None
    return int(ys[0]), int(ys[-1]) + 1, int(xs[0]), int(xs[-1]) + 1


def find_panels(gray, rtl=True, min_area=0.035, depth=0):
    """Recursive X-Y cut on flat gutters. `gray` is a float array. Returns boxes (x0, y0, x1, y1)
    in reading order (top to bottom; within a row right to left when `rtl`)."""
    h, w = gray.shape
    gap = max(4, int(min(h, w) * 0.012))
    boxes = []
    rows = _split(gray, 1, gap)
    for y0, y1 in rows:
        band = gray[y0:y1]
        cols = _split(band, 0, gap) if depth < 4 else [(0, w)]
        if rtl:
            cols = cols[::-1]
        for x0, x1 in cols:
            cell = band[:, x0:x1]
            t = _trim(cell)
            if t is None:
                continue
            ty0, ty1, tx0, tx1 = t
            box = (x0 + tx0, y0 + ty0, x0 + tx1, y0 + ty1)
            sub = gray[box[1]:box[3], box[0]:box[2]]
            # A cell that still holds gutters inside it (a row of panels inside a column) is cut again.
            if depth < 4 and (len(cols) > 1 or len(rows) > 1):
                inner = find_panels(sub, rtl, 0.0, depth + 1)
                if len(inner) > 1:
                    boxes.extend((box[0] + a, box[1] + b, box[0] + c, box[1] + d) for a, b, c, d in inner)
                    continue
            boxes.append(box)
    if depth == 0:
        area = float(h * w)
        boxes = [b for b in boxes if (b[2] - b[0]) * (b[3] - b[1]) >= area * min_area and (b[2] - b[0]) > 40 and (b[3] - b[1]) > 40]
    return boxes


def webtoon_panels(gray, min_frac=0.12):
    """A tall strip: panels are the stretches between flat horizontal bands. Very tall stretches
    are kept whole (the reader scrolls through them; the director reframes them)."""
    h, w = gray.shape
    gap = max(12, int(w * 0.03))
    spans = _split(gray, 1, gap)
    out = []
    for y0, y1 in spans:
        if y1 - y0 < w * min_frac:
            continue
        t = _trim(gray[y0:y1])
        if t is None:
            continue
        ty0, ty1, tx0, tx1 = t
        out.append((tx0, y0 + ty0, tx1, y0 + ty1))
    return out


def cmd_panels(job):
    out_dir = job["out_dir"]
    os.makedirs(out_dir, exist_ok=True)
    rtl = bool(job.get("rtl", True))
    min_area = float(job.get("min_area", 0.035))
    pages = job["pages"]
    found = []
    for pi, path in enumerate(pages):
        try:
            img = Image.open(path).convert("RGB")
        except Exception as e:  # a damaged page is skipped, not the chapter
            emit({"type": "warning", "message": f"could not read {path}: {e}"})
            continue
        gray = np.asarray(img.convert("L"), dtype=np.float32)
        h, w = gray.shape
        boxes = webtoon_panels(gray) if h / max(w, 1) > 2.2 else find_panels(gray, rtl, min_area)
        if not boxes:
            boxes = [(0, 0, w, h)]
        for k, (x0, y0, x1, y1) in enumerate(boxes):
            pad = 2
            x0, y0, x1, y1 = max(0, x0 - pad), max(0, y0 - pad), min(w, x1 + pad), min(h, y1 + pad)
            crop = img.crop((x0, y0, x1, y1))
            # Very tall webtoon stretches are kept but capped so a vision model can read them.
            if crop.height > crop.width * 3:
                crop = crop.crop((0, 0, crop.width, crop.width * 3))
                y1 = y0 + crop.height
            name = os.path.join(out_dir, f"p{pi + 1:03d}_{k + 1:02d}.jpg")
            crop.save(name, quality=92)
            found.append({"file": name, "page": pi, "box": [x0, y0, x1, y1], "w": crop.width, "h": crop.height})
        progress(pi + 1, len(pages))
    emit({"ok": True, "panels": found})


# ----------------------------------------------------------------------------- media


def ffprobe_info(path):
    """(width, height, fps, seconds) of a video."""
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate,nb_frames:format=duration", "-of", "json", path], capture_output=True, text=True, creationflags=FLAGS)
    if r.returncode != 0:
        raise RuntimeError(f"could not read {path}: {r.stderr.strip()[-200:]}")
    j = json.loads(r.stdout or "{}")
    s = (j.get("streams") or [{}])[0]
    num, _, den = str(s.get("r_frame_rate", "24/1")).partition("/")
    fps = float(num) / float(den or 1) if float(den or 1) else 24.0
    dur = float((j.get("format") or {}).get("duration") or 0) or 0.0
    return int(s.get("width", 0)), int(s.get("height", 0)), fps or 24.0, dur


class Source:
    """A still or a video, read as RGB(A) float frames at any time t (videos are decoded once,
    fully, at their own rate; a cut is only a few seconds)."""

    def __init__(self, path, size=None):
        self.path = path
        self.is_video = path.lower().endswith(VIDEO_EXT)
        if self.is_video:
            w, h, fps, dur = ffprobe_info(path)
            if size:
                w, h = size
            self.fps = fps
            cmd = ["ffmpeg", "-v", "error", "-i", path, "-vf", f"scale={w}:{h}:flags=lanczos", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
            raw = subprocess.run(cmd, capture_output=True, creationflags=FLAGS).stdout
            n = len(raw) // (w * h * 3)
            if n == 0:
                raise RuntimeError(f"{path} has no frames")
            self.frames = np.frombuffer(raw[: n * w * h * 3], dtype=np.uint8).reshape(n, h, w, 3)
            self.alpha = None
        else:
            img = Image.open(path)
            has_alpha = img.mode in ("RGBA", "LA") or "transparency" in img.info
            img = img.convert("RGBA")
            if size:
                img = img.resize(size, Image.LANCZOS)
            a = np.asarray(img)
            self.frames = a[None, :, :, :3]
            self.alpha = a[:, :, 3] if has_alpha else None
            self.fps = 24.0
        self.n = len(self.frames)
        self.h, self.w = self.frames.shape[1:3]

    def index(self, t, step=1, out_fps=24.0):
        """The frame shown at t seconds. `step` 2 holds each drawing for two frames of the cut
        (animation on twos); past its end the last drawing is held, as a held cel is."""
        if self.n == 1:
            return 0
        f = int(t * out_fps)
        f -= f % max(1, step)
        i = int(f / out_fps * self.fps)
        return max(0, min(self.n - 1, i))

    def rgb(self, i):
        return self.frames[i].astype(np.float32) / 255.0


# ----------------------------------------------------------------------------- the GPU side
#
# Every frame is worked on as a float tensor (3, H, W) in 0..1, on the graphics card when there
# is one (the VFX pack's PyTorch with CUDA), else on the CPU through PyTorch's own threads.
# Large blurs are done on a smaller copy and scaled back, as compositors do for glows.

_torch = None


def torch_mod():
    global _torch
    if _torch is None:
        try:
            import torch  # noqa: F401
        except ImportError as e:  # the app runs this in the VFX pack's venv, which has it
            raise RuntimeError("compositing needs the VFX pack (Studio › Setup › Layered motion): PyTorch was not found") from e
        _torch = torch
    return _torch


def device():
    t = torch_mod()
    if os.environ.get("MANGAVOX_ANIME_GPU", "").strip().lower() in ("0", "off", "false", "no"):
        return t.device("cpu")
    return t.device("cuda") if t.cuda.is_available() else t.device("cpu")


def tens(arr, dev):
    """HxWx3 uint8 / float numpy -> (3, H, W) float tensor; HxW -> (1, H, W)."""
    t = torch_mod()
    x = t.from_numpy(np.array(arr, copy=not arr.flags.writeable or not arr.flags.c_contiguous))
    if x.dtype == t.uint8:
        x = x.to(dev, non_blocking=True).float() / 255.0
    else:
        x = x.to(dev).float()
    return x.permute(2, 0, 1).contiguous() if x.ndim == 3 else x[None]


def gblur(x, sigma):
    """Gaussian blur of a (C, H, W) tensor. Big radii are blurred at a fraction of the size."""
    t = torch_mod()
    F = t.nn.functional
    if sigma <= 0.3:
        return x
    c, h, w = x.shape
    scale = 1
    while sigma / scale > 3.0 and min(h, w) / (scale * 2) > 16:
        scale *= 2
    y = x[None]
    if scale > 1:
        y = F.interpolate(y, size=(max(1, h // scale), max(1, w // scale)), mode="area")
    s = sigma / scale
    r = max(1, int(math.ceil(s * 2.5)))
    k = t.arange(-r, r + 1, device=x.device, dtype=x.dtype)
    k = t.exp(-(k * k) / (2 * s * s))
    k = (k / k.sum()).view(1, 1, 1, -1).repeat(c, 1, 1, 1)
    y = F.conv2d(F.pad(y, (r, r, 0, 0), mode="replicate"), k, groups=c)
    y = F.conv2d(F.pad(y, (0, 0, r, r), mode="replicate"), k.transpose(2, 3), groups=c)
    if scale > 1:
        y = F.interpolate(y, size=(h, w), mode="bilinear", align_corners=False)
    return y[0]


def lum_of(x):
    return x[0] * 0.299 + x[1] * 0.587 + x[2] * 0.114


# ----------------------------------------------------------------------------- keying


def screen_colour(rgb):
    """The chroma screen's colour (3 floats): the median of the border of a character plate,
    which is almost all screen. `rgb` is a (3, H, W) tensor."""
    t = torch_mod()
    _, h, w = rgb.shape
    b = max(2, min(h, w) // 40)
    border = t.cat([rgb[:, :b].reshape(3, -1), rgb[:, -b:].reshape(3, -1), rgb[:, :, :b].reshape(3, -1), rgb[:, :, -b:].reshape(3, -1)], dim=1)
    return border.median(dim=1).values


def chroma_key(rgb, screen=None, choke=1, softness=1.0):
    """Screen-difference key (the Keylight idea): alpha from how far the screen colour stands
    above what is left of the pixel, measured against the screen itself, then despill and a
    choke + feather on the matte. Works on green, blue and magenta screens (magenta for
    characters with green hair or eyes). Returns (rgb despilled, alpha (1, H, W))."""
    t = torch_mod()
    if screen is None:
        screen = screen_colour(rgb)
    r, g, b = rgb[0], rgb[1], rgb[2]
    sr, sg, sb = float(screen[0]), float(screen[1]), float(screen[2])
    if sr > sg and sb > sg:  # magenta: red and blue over green
        diff = t.minimum(r, b) - g
        sdiff = min(sr, sb) - sg
        cap_ch = None
    else:
        ch = 1 if sg >= sb else 2
        o = [c for c in range(3) if c != ch]
        diff = rgb[ch] - t.maximum(rgb[o[0]], rgb[o[1]])
        sdiff = float(screen[ch]) - max(float(screen[o[0]]), float(screen[o[1]]))
        cap_ch = (ch, o)
    sdiff = max(sdiff, 0.08)
    alpha = 1.0 - ((diff - 0.12 * sdiff) / (0.62 * sdiff * softness)).clamp(0, 1)
    out = rgb.clone()
    if cap_ch is not None:
        ch, o = cap_ch
        cap = t.maximum(rgb[o[0]], rgb[o[1]])
        spill = (rgb[ch] - cap).clamp(min=0)
        out[ch] = rgb[ch] - spill * 0.65
        out[o[0]] += spill * 0.35
        out[o[1]] += spill * 0.35
    else:
        spill = (t.minimum(r, b) - g).clamp(min=0)
        out[0] = r - spill * 0.65
        out[2] = b - spill * 0.65
        out[1] = g + spill * 0.35
    return out.clamp(0, 1), refine_matte(alpha[None], choke)


def refine_matte(alpha, choke=1):
    """Clears specks (screen noise left in the background), chokes the edge `choke` pixels and
    feathers it by one; near-0 and near-1 are crushed so the plate shows clean and the body
    stays solid."""
    t = torch_mod()
    F = t.nn.functional
    a = alpha[None]
    # 3x3 median: unfold the neighbourhood and take its middle value.
    patches = F.unfold(F.pad(a, (1, 1, 1, 1), mode="replicate"), 3)
    a = patches.median(dim=1).values.view_as(a)
    for _ in range(max(0, int(choke))):
        a = -F.max_pool2d(-F.pad(a, (1, 1, 1, 1), mode="replicate"), 3, stride=1)
    a = gblur(a[0], 0.8)[None]
    return ((a[0] - 0.04) / 0.92).clamp(0, 1)


# ----------------------------------------------------------------------------- camera


def ease(u):
    """Slow in, slow out: how a rostrum camera starts and stops."""
    return u * u * (3 - 2 * u)


def camera_extent(cam):
    """(travel, zoom) of a camera job: the fraction of the frame the picture slides over the cut
    and the fraction it grows. The app gives them outright ("travel", "zoom"), from the speeds
    measured on real episodes; a job without them gets the usual slow drift, scaled by "amount"."""
    move = cam.get("move", "fix")
    a = float(cam.get("amount", 1.0))
    travel = cam.get("travel")
    zoom = cam.get("zoom")
    if travel is None:
        travel = {"pan_left": 0.12, "pan_right": 0.12, "tilt_up": 0.12, "tilt_down": 0.12, "whip": 0.5}.get(move, 0.0) * a
    if zoom is None:
        zoom = {"push_in": 0.04, "pull_out": 0.04, "track_in": 0.14}.get(move, 0.0) * a
    return max(0.0, float(travel)), max(0.0, float(zoom))


def camera_at(move, u, travel, zoom):
    """The camera at progress u (0..1) of the cut: (z, ox, oy). z multiplies the base zoom; ox and
    oy are how far the view has moved right and down, in frame widths and heights. A pan right
    moves the view right (the picture slides left); a tilt up moves the view up."""
    e = ease(u)
    if move == "push_in":
        return 1.0 + zoom * e, 0.0, 0.0
    if move == "pull_out":
        return 1.0 + zoom * (1 - e), 0.0, 0.0
    if move == "track_in":
        # A fast move in that lands: most of it early.
        return 1.0 + zoom * (1 - (1 - u) ** 3), 0.0, 0.0
    if move == "pan_right":
        return 1.0, travel * (e - 0.5), 0.0
    if move == "pan_left":
        return 1.0, travel * (0.5 - e), 0.0
    if move == "tilt_down":
        return 1.0, 0.0, travel * (e - 0.5)
    if move == "tilt_up":
        return 1.0, 0.0, travel * (0.5 - e)
    if move == "whip":
        w = 1 - (1 - min(1.0, u * 3)) ** 3
        return 1.0, travel * (w - 0.5), 0.0
    # "fix" (and "shake", which only shakes): a held frame is truly held.
    return 1.0, 0.0, 0.0


def base_zoom(move, travel, lw, lh, W, H, shake=0.0):
    """How much a layer of lw x lh must be enlarged beyond covering the frame so the view can
    travel without showing its edge: nothing when the layer is already that much wider (a wide
    plate under a pan) or taller (a tall one under a tilt)."""
    s0 = max(W / lw, H / lh)
    cover_x, cover_y = lw * s0 / W, lh * s0 / H
    need_x = 1 + (travel if move in ("pan_left", "pan_right", "whip") else 0.0) + 0.03 * shake
    need_y = 1 + (travel if move in ("tilt_up", "tilt_down") else 0.0) + 0.03 * shake
    return max(1.0, need_x / cover_x, need_y / cover_y)


def shake_at(t, strength, seed):
    """A hand-held / impact shake that decays over the first second (frame fractions)."""
    if strength <= 0:
        return 0.0, 0.0
    r = random.Random(int(t * 24) + seed * 7919)
    decay = max(0.15, 1.0 - t)
    return (r.random() - 0.5) * 0.02 * strength * decay, (r.random() - 0.5) * 0.02 * strength * decay


def frame_through(x, z, ox, oy, W, H, pad="border"):
    """What the camera sees of a layer (C, h, w), as a (C, H, W) frame. The layer covers the frame
    and is enlarged by z about its centre; the view is moved (ox, oy) frame widths / heights right
    and down. pad "border" keeps the view inside the layer (a plate); "zeros" leaves what lies
    outside the layer empty (a character layer registered to the middle of the plate)."""
    t = torch_mod()
    F = t.nn.functional
    c, h, w = x.shape
    s = max(W / w, H / h) * z  # screen pixels per layer pixel
    cw, chh = W / s, H / s  # the part of the layer in view, in layer pixels
    cx = w / 2 + ox * W / s
    cy = h / 2 + oy * H / s
    if pad == "border":
        cx = min(max(cw / 2, cx), w - cw / 2) if w > cw else w / 2
        cy = min(max(chh / 2, cy), h - chh / 2) if h > chh else h / 2
    theta = t.tensor([[cw / w, 0, (2 * cx / w) - 1], [0, chh / h, (2 * cy / h) - 1]], dtype=x.dtype, device=x.device)[None]
    grid = F.affine_grid(theta, (1, c, H, W), align_corners=False)
    src = x[None]
    # A big shrink is pre-filtered so fine linework does not alias.
    f = int(cw / W)
    if f > 1:
        src = F.avg_pool2d(src, f)
    return F.grid_sample(src, grid, mode="bilinear", padding_mode=pad, align_corners=False)[0]


# ----------------------------------------------------------------------------- integration


def colour_match(fg, alpha, bg, strength):
    """Moves the character's tint and a little of its brightness toward the plate's, so a
    character painted on a screen sits in the plate's light (a cold dungeon tints them blue)."""
    if strength <= 0:
        return fg
    m = alpha[0] > 0.5
    if int(m.sum()) < 50:
        return fg
    fg_mean = fg[:, m].mean(dim=1) + 1e-3
    bg_mean = bg.reshape(3, -1).mean(dim=1) + 1e-3
    tint = (bg_mean / bg_mean.mean()) / (fg_mean / fg_mean.mean())
    gain = (bg_mean.mean() / fg_mean.mean()) ** 0.35
    k = (1 + (tint * gain - 1) * strength).view(3, 1, 1)
    return (fg * k).clamp(0, 1)


def light_wrap(fg, alpha, bg, amount, radius):
    """The plate's light spilling round the character's edges: the blurred plate over the inner
    rim of the matte (lighten), the way it is done in a compositor."""
    if amount <= 0:
        return fg
    inner = alpha * (1 - gblur(alpha, radius))
    wrap = gblur(bg, radius * 1.5)
    k = (inner * amount * 2.0).clamp(0, 1)
    return fg * (1 - k) + fg.maximum(wrap) * k


def layout_ground(bg, layout, cel_alpha):
    """The background a lifted layer goes over, and the shadows to lay under it. The layout frame
    (characters drawn into the cut's background, lit, with cast and contact shadows) is divided by
    the background: where it is darker near the characters and not under them, that is shadow.
    When the image model moved the background while drawing into it (they differ away from the
    characters), the layout itself is the background: it already holds them, lit and grounded."""
    t = torch_mod()
    F = t.nn.functional
    h, w = bg.shape[1:]
    lay = F.interpolate(layout[None], size=(h, w), mode="bilinear", align_corners=False)[0]
    a = F.interpolate(cel_alpha[None], size=(h, w), mode="bilinear", align_corners=False)[0].clamp(0, 1)
    near = (gblur(a, h / 14) > 0.01).float()
    away = 1 - near
    lb, ll = lum_of(bg), lum_of(lay)
    drift = float(((ll - lb).abs() * away).sum() / away.sum().clamp(min=1))
    if drift > 0.06:
        return lay, None
    # Not on or right around them (their edges differ between the two pictures) and only on the
    # ground, below the middle of where they stand: shadows fall on the floor.
    under = (gblur(a, h / 70) > 0.05).float()
    rows = (a[0].max(dim=1).values > 0.5).nonzero()
    ground = t.zeros_like(a)
    if rows.numel():
        ground[:, (int(rows[0]) + int(rows[-1])) // 2:, :] = 1.0
    shade = (1 - ll / (lb + 0.03)).clamp(0, 0.85) * near * (1 - under) * ground
    shade = t.where(shade > 0.06, shade, t.zeros_like(shade))
    return bg, gblur(shade, h / 300)


def contact_shadow(alpha, strength=0.45):
    """A soft shadow under a full figure: an ellipse at their feet, blurred, so they stand on
    the plate instead of floating over it. None for a close-up (no floor in frame)."""
    t = torch_mod()
    a = alpha[0]
    H, W = a.shape
    rows = (a.max(dim=1).values > 0.5).nonzero()
    if rows.numel() == 0:
        return None
    bottom = int(rows[-1])
    if bottom < H * 0.6:
        return None
    cols = (a[max(0, bottom - 6):bottom + 1].max(dim=0).values > 0.5).nonzero()
    if cols.numel() == 0:
        return None
    x0, x1 = int(cols[0]), int(cols[-1])
    cx, cy = (x0 + x1) / 2, min(H - 1, bottom + 2)
    rx, ry = max(8.0, (x1 - x0) * 0.75), max(4.0, (x1 - x0) * 0.09)
    yy = t.arange(H, device=a.device, dtype=a.dtype).view(-1, 1)
    xx = t.arange(W, device=a.device, dtype=a.dtype).view(1, -1)
    d = ((xx - cx) / rx) ** 2 + ((yy - cy) / ry) ** 2
    return gblur((1 - d).clamp(0, 1)[None], max(3.0, ry)) * strength


def blend(base, layer, mode, opacity, alpha=None):
    if mode == "screen":
        out = 1 - (1 - base) * (1 - layer)
    elif mode == "add":
        out = base + layer
    elif mode == "multiply":
        out = base * layer
    else:  # "over"
        a = alpha if alpha is not None else 1.0
        out = base * (1 - a) + layer * a
    return (base + (out - base) * opacity).clamp(0, 1)


def place_fx(fx, W, H, x, y, scale):
    """An effects element (C, h, w) scaled to `scale` of the frame and centred at (x, y), on
    black (Screen / Add treat black as no light)."""
    t = torch_mod()
    F = t.nn.functional
    c, h, w = fx.shape
    s = min(W / w, H / h) * scale
    nw, nh = max(2, int(w * s)), max(2, int(h * s))
    small = F.interpolate(fx[None], size=(nh, nw), mode="bilinear", align_corners=False)[0]
    canvas = t.zeros((c, H, W), dtype=fx.dtype, device=fx.device)
    x0, y0 = int(x * W - nw / 2), int(y * H - nh / 2)
    sx0, sy0 = max(0, -x0), max(0, -y0)
    dx0, dy0 = max(0, x0), max(0, y0)
    cw, chh = min(nw - sx0, W - dx0), min(nh - sy0, H - dy0)
    if cw > 0 and chh > 0:
        canvas[:, dy0:dy0 + chh, dx0:dx0 + cw] = small[:, sy0:sy0 + chh, sx0:sx0 + cw]
    return canvas


# ----------------------------------------------------------------------------- finish (satsuei)

# Colour script grades: (lift rgb, gamma rgb, gain rgb, saturation). Lift raises the shadows,
# gamma bends the mids, gain scales the highlights. The director picks one per sequence.
GRADES = {
    "neutral": ((0.0, 0.0, 0.0), (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), 1.04),
    "warm_home": ((0.02, 0.01, 0.0), (0.97, 1.0, 1.06), (1.04, 1.0, 0.92), 1.06),
    "cold_dungeon": ((0.0, 0.015, 0.035), (1.06, 1.0, 0.94), (0.9, 0.98, 1.08), 0.86),
    "system_blue": ((0.0, 0.02, 0.05), (1.05, 0.98, 0.9), (0.85, 1.0, 1.15), 0.95),
    "blood_red": ((0.03, 0.0, 0.0), (0.94, 1.06, 1.06), (1.1, 0.9, 0.88), 0.92),
    "night": ((0.0, 0.01, 0.04), (1.1, 1.05, 0.95), (0.8, 0.88, 1.0), 0.8),
    "hospital": ((0.01, 0.02, 0.02), (1.0, 0.97, 0.98), (0.98, 1.02, 1.0), 0.85),
    "sunset": ((0.03, 0.01, 0.0), (0.92, 1.0, 1.1), (1.1, 0.96, 0.82), 1.1),
    "flashback": ((0.04, 0.035, 0.02), (0.95, 0.97, 1.02), (1.0, 0.97, 0.9), 0.55),
    "horror": ((0.0, 0.01, 0.0), (1.08, 1.0, 1.05), (0.88, 0.95, 0.9), 0.7),
    "daylight": ((0.0, 0.0, 0.01), (0.99, 1.0, 1.0), (1.02, 1.02, 1.0), 1.08),
}


def grade(x, name):
    t = torch_mod()
    lift, gamma, gain, sat = GRADES.get(name or "neutral", GRADES["neutral"])
    v = lambda a: t.tensor(a, dtype=x.dtype, device=x.device).view(3, 1, 1)  # noqa: E731
    out = (x * v(gain) + v(lift) * (1 - x)).clamp(0, 1) ** v(gamma)
    lum = lum_of(out)[None]
    return (lum + (out - lum) * sat).clamp(0, 1)


class Finisher:
    """The look every cut shares: bloom, diffusion, light rays, the grade, dust, speed lines,
    chromatic aberration, vignette, an impact flash and one film grain."""

    def __init__(self, f, grade_name, W, H, seed, dev):
        t = torch_mod()
        self.f, self.grade, self.W, self.H, self.seed, self.dev = f, grade_name, W, H, seed, dev
        yy = t.linspace(-1, 1, H, device=dev).view(-1, 1)
        xx = t.linspace(-1, 1, W, device=dev).view(1, -1)
        r = t.sqrt(xx * xx + yy * yy) / 1.42
        self.vignette = (1 - 0.55 * ((r - 0.45) / 0.55).clamp(0, 1) ** 1.6).clamp(0, 1)[None]
        self.edge = ((r - 0.35) / 0.65).clamp(0, 1)[None]
        self.gen = t.Generator(device=dev)

    def __call__(self, x, ti):
        t = torch_mod()
        f, W, H = self.f, self.W, self.H
        out = x
        lum = lum_of(out)
        bloom = float(f.get("bloom", 0.35))
        if bloom > 0:
            hi = ((lum - 0.72) / 0.28).clamp(0, 1)[None] * out
            glow = gblur(hi, H / 45) + gblur(hi, H / 140) * 0.5
            out = 1 - (1 - out) * (1 - (glow * bloom).clamp(0, 1))
        diff = float(f.get("diffusion", 0.15))
        if diff > 0:
            soft = gblur(out, H / 220)
            out = out + (out.maximum(soft) - out) * diff
        rays = f.get("rays")
        if rays:
            out = self.light_rays(out, lum, rays)
        out = grade(out, self.grade)
        dust = int(f.get("dust", 0))
        if dust:
            out = self.dust(out, dust, ti)
        speed = float(f.get("speed", 0.0))
        if speed > 0:
            out = self.speed_lines(out, speed, ti)
        ca = float(f.get("ca", 1.0))
        if ca > 0:
            px = max(1, int(round(ca * W / 1920)))
            shifted = out.clone()
            shifted[0, :, px:] = out[0, :, :-px]
            shifted[2, :, :-px] = out[2, :, px:]
            out = out * (1 - self.edge) + shifted * self.edge
        vig = float(f.get("vignette", 0.25))
        if vig > 0:
            out = out * self.vignette ** vig
        flash = float(f.get("flash", 0.0))
        if flash > 0 and ti < 0.25:
            out = out + (1 - out) * flash * (1 - ti / 0.25)
        g = float(f.get("grain", 0.035))
        if g > 0:
            self.gen.manual_seed(self.seed * 100003 + int(ti * 24))
            n = t.randn((1, 1, H // 2, W // 2), generator=self.gen, device=self.dev)
            n = t.nn.functional.interpolate(n, size=(H, W), mode="bilinear", align_corners=False)[0] * 0.25
            l2 = lum_of(out)[None]
            out = out + n * g * 2 * (4 * l2 * (1 - l2))  # grain in the mids, as on film
        return out.clamp(0, 1)

    def light_rays(self, x, lum, origin):
        """God rays from a point: the highlights streaked toward (x, y) of the frame, screened."""
        t = torch_mod()
        F = t.nn.functional
        H, W = self.H, self.W
        small = ((lum - 0.6) / 0.4).clamp(0, 1)[None, None]
        h4, w4 = H // 4, W // 4
        small = F.interpolate(small, size=(h4, w4), mode="area")
        ox, oy = float(origin[0]) * 2 - 1, float(origin[1]) * 2 - 1
        acc = t.zeros_like(small)
        for i, s in enumerate((1.0, 1.04, 1.09, 1.15, 1.22, 1.3, 1.4)):
            # Sampling a copy shrunk toward the origin streaks the light along lines from it.
            theta = t.tensor([[1 / s, 0, ox - ox / s], [0, 1 / s, oy - oy / s]], device=x.device, dtype=x.dtype)[None]
            grid = F.affine_grid(theta, small.shape, align_corners=False)
            acc += F.grid_sample(small, grid, align_corners=False, padding_mode="zeros") * (1 - i / 8)
        rays = F.interpolate(acc / 4.0, size=(H, W), mode="bilinear", align_corners=False)[0]
        rays = gblur(rays, H / 120)
        tint = t.tensor([1.0, 0.97, 0.9], device=x.device).view(3, 1, 1)
        return 1 - (1 - x) * (1 - (rays * tint * 0.55).clamp(0, 1))

    def dust(self, x, count, ti):
        """Floating dust caught in the light: soft dots drifting up slowly (drawn small)."""
        t = torch_mod()
        F = t.nn.functional
        H, W = self.H, self.W
        h4, w4 = H // 2, W // 2
        r = random.Random(self.seed)
        layer = np.zeros((h4, w4), dtype=np.float32)
        for _ in range(count):
            px = (r.random() * w4 + ti * (r.random() - 0.5) * 20) % w4
            py = (r.random() * h4 - ti * (5 + r.random() * 15)) % h4
            rad = 0.6 + r.random() * 1.4
            bright = 0.35 + r.random() * 0.4
            x0, x1, y0, y1 = int(max(0, px - 4)), int(min(w4, px + 5)), int(max(0, py - 4)), int(min(h4, py + 5))
            if x1 <= x0 or y1 <= y0:
                continue
            yy, xx = np.ogrid[y0:y1, x0:x1]
            d = np.exp(-(((xx - px) ** 2 + (yy - py) ** 2) / (rad * rad))) * bright
            layer[y0:y1, x0:x1] = np.maximum(layer[y0:y1, x0:x1], d)
        lt = F.interpolate(t.from_numpy(layer).to(x.device)[None, None], size=(H, W), mode="bilinear", align_corners=False)[0]
        tint = t.tensor([1.0, 0.96, 0.85], device=x.device).view(3, 1, 1)
        return 1 - (1 - x) * (1 - lt * tint)

    def speed_lines(self, x, strength, ti):
        """Speed lines from the frame's edge toward its centre, changing every other frame."""
        from PIL import ImageDraw

        t = torch_mod()
        F = t.nn.functional
        H, W = self.H, self.W
        h2, w2 = H // 2, W // 2
        r = random.Random(self.seed + int(ti * 12))
        img = Image.new("L", (w2, h2), 0)
        d = ImageDraw.Draw(img)
        cx, cy = w2 / 2, h2 / 2
        diag = math.hypot(w2, h2)
        for _ in range(int(90 * strength)):
            a = r.random() * math.tau
            r0 = (0.42 + r.random() * 0.25) * diag / 2
            d.line([(cx + math.cos(a) * r0, cy + math.sin(a) * r0), (cx + math.cos(a) * diag, cy + math.sin(a) * diag)], fill=255, width=1 + int(r.random() * 2))
        lt = t.from_numpy(np.asarray(img, dtype=np.float32) / 255.0).to(x.device)[None, None]
        lt = F.interpolate(lt, size=(H, W), mode="bilinear", align_corners=False)[0]
        return 1 - (1 - x) * (1 - lt * 0.6 * min(1.0, strength))


# ----------------------------------------------------------------------------- cards and slates


def _font(size):
    from PIL import ImageFont

    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # an older Pillow: its small bitmap font
        return ImageFont.load_default()


def _wrap(draw, text, font, width):
    lines, line = [], ""
    for word in str(text).split():
        trial = (line + " " + word).strip()
        if draw.textlength(trial, font=font) <= width or not line:
            line = trial
        else:
            lines.append(line)
            line = word
    if line:
        lines.append(line)
    return lines


def render_card(card, W, H):
    """A cut that has no picture yet (one the director added), for the animatic: its number and
    what it will show, as text on a dark card."""
    from PIL import ImageDraw

    img = Image.new("RGB", (W, H), (20, 22, 28))
    d = ImageDraw.Draw(img)
    title_font, text_font = _font(max(18, H // 18)), _font(max(14, H // 26))
    x, y = W // 10, H // 4
    d.text((x, y), str(card.get("title", "")), font=title_font, fill=(150, 170, 255))
    y += int(title_font.size * 1.8) if hasattr(title_font, "size") else 40
    for line in _wrap(d, card.get("text", ""), text_font, W - 2 * x)[:9]:
        d.text((x, y), line, font=text_font, fill=(232, 232, 236))
        y += int(getattr(text_font, "size", 16) * 1.45)
    return np.asarray(img)


def render_slate(text, W, H):
    """The animatic's slate (cut number, length, shot, camera): small text in the lower left, as
    (rgb HxWx3 uint8, alpha HxW uint8)."""
    from PIL import ImageDraw

    font = _font(max(12, H // 40))
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    pad = max(4, H // 120)
    tw = int(d.textlength(text, font=font))
    th = int(getattr(font, "size", 12) * 1.3)
    x, y = pad * 2, H - th - pad * 3
    d.rectangle([x - pad, y - pad, x + tw + pad, y + th + pad], fill=(0, 0, 0, 150))
    d.text((x, y), text, font=font, fill=(255, 255, 255, 235))
    a = np.asarray(img)
    return a[:, :, :3], a[:, :, 3]


def contain_canvas(img, W, H):
    """A picture of any shape shown whole in the frame, over a blurred, darkened copy of itself
    that fills the rest (a tall or square manga panel in a 16:9 animatic)."""
    t = torch_mod()
    F = t.nn.functional
    back = gblur(frame_through(img, 1.0, 0.0, 0.0, W, H), H / 28) * 0.4
    _, h, w = img.shape
    s = min(W / w, H / h)
    nw, nh = max(2, int(round(w * s))), max(2, int(round(h * s)))
    small = F.interpolate(img[None], size=(nh, nw), mode="bilinear", align_corners=False, antialias=True)[0]
    x0, y0 = (W - nw) // 2, (H - nh) // 2
    back[:, y0:y0 + nh, x0:x0 + nw] = small
    return back


# ----------------------------------------------------------------------------- the cut


class Cut:
    """One cut of the storyboard as layers, rendered frame by frame."""

    def __init__(self, job):
        self.dev = device()
        self.W = int(job.get("width", 1920))
        self.H = int(job.get("height", 1080))
        self.fps = float(job.get("fps", 24))
        self.seconds = max(0.3, float(job.get("seconds", 3)))
        self.seed = int(job.get("seed", 1))
        cam = job.get("camera") or {}
        self.move = cam.get("move", "fix")
        self.travel, self.zoom = camera_extent(cam)
        self.shake = float(cam.get("shake", 0.0))
        self.finisher = Finisher(job.get("finish") or {}, job.get("grade", "neutral"), self.W, self.H, self.seed, self.dev)
        W, H = self.W, self.H
        self.base, self.base_fit, self.canvas = None, "cover", None
        b = job.get("base")
        if b and b.get("path"):
            self.base = (Source(b["path"]), int(b.get("step", 1)))
            self.base_fit = b.get("fit", "cover")
        card = job.get("card")
        self.card = tens(render_card(card, W, H), self.dev) if card else None
        self.slate = None
        if job.get("slate"):
            rgb, a = render_slate(str(job["slate"]), W, H)
            self.slate = (tens(rgb, self.dev), tens(a, self.dev))
        p = job.get("plate")
        self.plate = Source(p["path"]) if p and p.get("path") else None
        self.plate_t = tens(self.plate.frames[0], self.dev) if self.plate else None
        # Characters and BOOK foregrounds are layers over the plate, in order (back to front).
        self.layers = [(Source(c["path"]), c) for c in (job.get("chars") or []) if c.get("path")]
        self.layers += [(Source(c["path"]), dict(c, book=True)) for c in (job.get("book") or []) if c.get("path")]
        self.fx = [(Source(x["path"]), x) for x in (job.get("fx") or []) if x.get("path")]
        self.cache = {}
        self.shadow_t = None
        lay = job.get("layout")
        if lay and lay.get("path") and self.plate_t is not None and self.layers:
            self.plate_t, self.shadow_t = layout_ground(self.plate_t, tens(Source(lay["path"]).frames[0], self.dev), self.drawing(self.layers[0][0], 0, self.layers[0][1].get("key", "green"))[1])
        # The base zoom every layer shares, from what the camera travels over.
        lw, lh = (self.plate.w, self.plate.h) if self.plate else (W, H)
        if self.base is not None and self.base_fit == "scan":
            # A tall panel travelled from end to end, as a reader scrolls it; a wide one likewise.
            src = self.base[0]
            s0 = max(W / src.w, H / src.h)
            tall = src.h * s0 / H - 1 > src.w * s0 / W - 1
            self.travel = max(0.0, (src.h * s0 / H if tall else src.w * s0 / W) - 1)
            if tall and self.move not in ("tilt_up", "tilt_down"):
                self.move = "tilt_down"
            if not tall and self.move not in ("pan_left", "pan_right"):
                self.move = "pan_right"
            self.z0 = 1.0
        elif self.base is not None and self.base_fit == "contain":
            self.canvas = contain_canvas(tens(self.base[0].frames[0], self.dev), W, H)
            self.z0 = base_zoom(self.move, self.travel, W, H, W, H, self.shake)
        else:
            self.z0 = base_zoom(self.move, self.travel, lw, lh, W, H, self.shake)

    def frames(self):
        return max(1, int(round(self.seconds * self.fps)))

    def drawing(self, src, i, mode):
        """A layer's drawing with its matte, uploaded and keyed once (a drawing held on threes is
        keyed once for its three frames)."""
        k = (id(src), i, mode)
        if k not in self.cache:
            rgb = tens(src.frames[i], self.dev)
            if mode == "alpha" and src.alpha is not None:
                a = tens(src.alpha, self.dev)
            elif mode == "none":
                a = rgb.new_ones((1,) + rgb.shape[1:])
            else:
                rgb, a = chroma_key(rgb)
            if len(self.cache) > 12:
                self.cache.clear()
            self.cache[k] = (rgb, a)
        return self.cache[k]

    def fx_frame(self, src, opt, t):
        i = int(t * src.fps) % src.n if opt.get("loop", True) else src.index(t, int(opt.get("step", 1)), self.fps)
        k = (id(src), i, "fx")
        if k not in self.cache:
            self.cache[k] = (tens(src.frames[i], self.dev), None)
        return self.cache[k][0]

    def fx_layer(self, src, opt, t):
        return place_fx(self.fx_frame(src, opt, t), self.W, self.H, float(opt.get("x", 0.5)), float(opt.get("y", 0.5)), float(opt.get("scale", 1.0)))

    def render(self, fi):
        W, H, t = self.W, self.H, fi / self.fps
        u = 0.0 if self.frames() <= 1 else fi / (self.frames() - 1)
        z, ox, oy = camera_at(self.move, u, self.travel, self.zoom)
        sx, sy = shake_at(t, self.shake, self.seed)
        z, ox, oy = self.z0 * z, ox + sx, oy + sy
        layered = False
        if self.card is not None:
            frame = self.card
        elif self.base is not None and self.base_fit == "contain":
            frame = frame_through(self.canvas, z, ox, oy, W, H)
        elif self.base is not None and self.base_fit == "scan":
            src, step = self.base
            frame = frame_through(tens(src.frames[src.index(t, step, self.fps)], self.dev), z, ox, oy, W, H)
        elif self.base is not None:
            # A "full" cut: the video model moved the camera already; shake, effects and finish.
            src, step = self.base
            rgb = tens(src.frames[src.index(t, step, self.fps)], self.dev)
            frame = frame_through(rgb, 1.0 + 0.03 * self.shake, sx, sy, W, H)
        else:
            layered = True
            # Plate, effects and characters are seen through one camera: what stands on the
            # plate's ground stays on it. Only BOOK foregrounds, nearer the lens, slide further.
            frame = frame_through(self.plate_t, z, ox, oy, W, H) if self.plate_t is not None else torch_mod().zeros((3, H, W), device=self.dev)
            if self.shadow_t is not None:
                # The layout's own cast and contact shadows, carried onto the clean background.
                frame = frame * (1 - frame_through(self.shadow_t, z, ox, oy, W, H, "zeros"))
            for src, opt in self.fx:
                if not opt.get("front", True):
                    frame = blend(frame, frame_through(self.fx_layer(src, opt, t), z, ox, oy, W, H, "zeros"), opt.get("blend", "screen"), float(opt.get("opacity", 1.0)))
            for src, opt in self.layers:
                book = bool(opt.get("book"))
                i = src.index(t, int(opt.get("step", 1 if book else 3)), self.fps)
                rgb, a = self.drawing(src, i, opt.get("key", "green"))
                par = float(opt.get("parallax", 1.35 if book else 1.0))
                fg = frame_through(rgb, z, ox * par, oy * par, W, H, "zeros")
                fa = frame_through(a, z, ox * par, oy * par, W, H, "zeros")
                if not book and opt.get("shadow", True):
                    sh = contact_shadow(fa)
                    if sh is not None:
                        frame = frame * (1 - sh)
                fg = colour_match(fg, fa, frame, float(opt.get("match", 0.15 if book else 0.3)))
                if not book:
                    fg = light_wrap(fg, fa, frame, float(opt.get("wrap", 0.5)), max(2.0, H / 180))
                else:
                    # Foreground objects sit out of focus, like a real lens close to them.
                    fg = gblur(fg, float(opt.get("defocus", H / 400)))
                    fa = gblur(fa, float(opt.get("defocus", H / 400)))
                frame = frame * (1 - fa) + fg * fa
        for src, opt in self.fx:
            if layered and opt.get("front", True):
                frame = blend(frame, frame_through(self.fx_layer(src, opt, t), z, ox, oy, W, H, "zeros"), opt.get("blend", "screen"), float(opt.get("opacity", 1.0)))
            elif not layered:
                frame = blend(frame, self.fx_layer(src, opt, t), opt.get("blend", "screen"), float(opt.get("opacity", 1.0)))
        frame = self.finisher(frame, t)
        if self.slate is not None:
            rgb, a = self.slate
            frame = frame * (1 - a) + rgb * a
        return frame


def to_u8(x):
    """(3, H, W) tensor -> HxWx3 uint8 numpy."""
    return (x.clamp(0, 1) * 255 + 0.5).byte().permute(1, 2, 0).contiguous().cpu().numpy()


def cmd_still(job):
    cut = Cut(job)
    out = job["out"]
    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    with torch_mod().no_grad():
        Image.fromarray(to_u8(cut.render(0))).save(out)
    emit({"ok": True, "out": out})


def encoder_args(quality="final"):
    """A finished cut is near-lossless (it is encoded once more in the edit); an animatic's is quick."""
    if quality == "rough":
        return ["-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-pix_fmt", "yuv420p"]
    return ["-c:v", "libx264", "-preset", "medium", "-crf", "16", "-pix_fmt", "yuv420p"]


def cmd_comp(job):
    cut = Cut(job)
    out = job["out"]
    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    n = cut.frames()
    tmp = out + ".part.mp4"
    cmd = ["ffmpeg", "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{cut.W}x{cut.H}", "-r", str(cut.fps), "-i", "-", *encoder_args(job.get("quality", "final")), "-movflags", "+faststart", tmp]
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=FLAGS)
    try:
        with torch_mod().no_grad():
            for fi in range(n):
                proc.stdin.write(to_u8(cut.render(fi)).tobytes())
                if fi % 6 == 0 or fi == n - 1:
                    progress(fi + 1, n)
        proc.stdin.close()
        err = proc.stderr.read().decode("utf-8", "replace")
        proc.stderr.close()
        if proc.wait() != 0:
            raise RuntimeError(f"FFmpeg could not encode the cut: {err.strip()[-300:]}")
    except BaseException:
        proc.kill()
        raise
    os.replace(tmp, out)
    emit({"ok": True, "out": out, "frames": n, "device": str(cut.dev)})


# ----------------------------------------------------------------------------- edit

# Soft transitions into a cut: (ffmpeg xfade name, seconds). Anything else is a hard cut.
SOFT = {"dissolve": ("fade", 0.5), "white": ("fadewhite", 0.3), "black": ("fadeblack", 0.6), "smash": ("fadewhite", 0.08)}


def edit_plan(cuts, fps):
    """Where every cut starts in the episode, and the reels it is joined from. `cuts` is
    [(seconds, transition)]. Cuts are whole frames long. Runs of hard cuts form a reel (joined
    without re-encoding, so a hard cut is exactly hard); a soft transition into a cut starts a
    new reel that overlaps the one before by the transition's length.
    Returns (starts, reels, total): reels are {"first", "count", "start", "xf": (name, seconds) | None}."""
    starts, reels, t, prev = [], [], 0.0, 0.0
    for i, (secs, tr) in enumerate(cuts):
        soft = SOFT.get(tr) if reels else None
        if soft:
            d = max(1, int(round(min(soft[1], secs / 2, prev / 2) * fps))) / fps
            t -= d
            reels.append({"first": i, "count": 1, "start": t, "xf": (soft[0], d)})
        elif not reels:
            reels.append({"first": 0, "count": 1, "start": 0.0, "xf": None})
        else:
            reels[-1]["count"] += 1
        starts.append(t)
        t += secs
        prev = secs
    return starts, reels, t


def _run(args, what):
    r = subprocess.run(args, capture_output=True, text=True, creationflags=FLAGS)
    if r.returncode != 0:
        raise RuntimeError(f"{what}: {r.stderr.strip()[-300:]}")


def cmd_edit(job):
    """The episode: every cut fitted to its length, joined with its transition, the voices laid
    where each cut starts, the music under them (ducked while anyone speaks)."""
    out = job["out"]
    W, H, fps = int(job.get("width", 1920)), int(job.get("height", 1080)), float(job.get("fps", 24))
    quality = job.get("quality", "final")
    cuts = [c for c in job.get("cuts") or [] if c.get("video") and os.path.isfile(c["video"])]
    if not cuts:
        raise RuntimeError("there are no finished cuts to join")
    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    work = tempfile.mkdtemp(prefix="anime-edit-")
    try:
        # 1. Picture: each cut normalised to a whole number of frames (its last frame held if the
        #    clip is short), all encoded alike so hard cuts can be joined without re-encoding.
        parts, lengths = [], []
        for i, c in enumerate(cuts):
            p = os.path.join(work, f"c{i:04d}.mp4")
            frames = max(2, int(round(float(c.get("seconds", 3)) * fps)))
            secs = frames / fps
            vf = f"scale={W}:{H}:force_original_aspect_ratio=increase,crop={W}:{H},fps={fps},tpad=stop_mode=clone:stop_duration={secs + 1:.3f},trim=end_frame={frames},setpts=PTS-STARTPTS,setsar=1,format=yuv420p"
            _run(["ffmpeg", "-y", "-v", "error", "-i", c["video"], "-vf", vf, "-an", "-r", str(fps), *encoder_args(quality), "-g", str(int(fps)), "-video_track_timescale", "24000", p], f"cut {i + 1}")
            parts.append(p)
            lengths.append((secs, c.get("transition", "cut")))
            progress(i + 1, len(cuts) + 2)
        starts, reels, total = edit_plan(lengths, fps)
        # 2. Reels of hard cuts (stream copy), then the soft transitions between reels.
        reel_files = []
        for k, r in enumerate(reels):
            mine = parts[r["first"]:r["first"] + r["count"]]
            if len(mine) == 1:
                reel_files.append(mine[0])
                continue
            lst = os.path.join(work, f"reel{k:03d}.txt")
            with open(lst, "w", encoding="utf-8") as f:
                f.write("".join("file '{}'\n".format(m.replace("\\", "/").replace("'", "'\\''")) for m in mine))
            rf = os.path.join(work, f"reel{k:03d}.mp4")
            _run(["ffmpeg", "-y", "-v", "error", "-f", "concat", "-safe", "0", "-i", lst, "-c", "copy", rf], "joining the cuts")
            reel_files.append(rf)
        picture = os.path.join(work, "picture.mp4")
        if len(reel_files) == 1:
            picture = reel_files[0]
        else:
            inputs, filt, last = [], [], "[0:v]"
            for rf in reel_files:
                inputs += ["-i", rf]
            for k in range(1, len(reels)):
                name, d = reels[k]["xf"]
                filt.append(f"{last}[{k}:v]xfade=transition={name}:duration={d:.4f}:offset={reels[k]['start']:.4f}[v{k}]")
                last = f"[v{k}]"
            _run(["ffmpeg", "-y", "-v", "error", *inputs, "-filter_complex", ";".join(filt), "-map", last, *encoder_args(quality), picture], "joining the scenes")
        progress(len(cuts) + 1, len(cuts) + 2)
        # 3. Sound: each cut's voice a moment after the cut, music under (ducked by the voices).
        voices = [(c["voice"], starts[i] + float(c.get("voice_at", 0.5))) for i, c in enumerate(cuts) if c.get("voice") and os.path.isfile(c["voice"])]
        music = job.get("music") if job.get("music") and os.path.isfile(job.get("music")) else None
        if not voices and not music:
            shutil.copyfile(picture, out)
        else:
            ins = ["-i", picture]
            graph = []
            labels = []
            for k, (v, at) in enumerate(voices):
                ins += ["-i", v]
                ms = int(at * 1000)
                graph.append(f"[{k + 1}:a]aresample=48000,aformat=channel_layouts=stereo,adelay={ms}|{ms}[vo{k}]")
                labels.append(f"[vo{k}]")
            if voices:
                graph.append(f"{''.join(labels)}amix=inputs={len(labels)}:normalize=0,apad=whole_dur={total:.3f}[voice]")
            if music:
                mi = len(voices) + 1
                ins += ["-stream_loop", "-1", "-i", music]
                db = float(job.get("music_db", -17))
                graph.append(f"[{mi}:a]aresample=48000,aformat=channel_layouts=stereo,volume={db}dB,atrim=duration={total:.3f},afade=t=in:d=1.5,afade=t=out:st={max(0, total - 2.5):.3f}:d=2.5[mus]")
                if voices:
                    graph.append("[voice]asplit=2[voice1][key]")
                    graph.append("[mus][key]sidechaincompress=threshold=0.03:ratio=6:attack=20:release=400[duck]")
                    graph.append("[voice1][duck]amix=inputs=2:normalize=0[aout]")
                else:
                    graph.append("[mus]anull[aout]")
            else:
                graph.append("[voice]anull[aout]")
            _run(["ffmpeg", "-y", "-v", "error", *ins, "-filter_complex", ";".join(graph), "-map", "0:v", "-map", "[aout]", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-t", f"{total:.3f}", "-movflags", "+faststart", out], "mixing the sound")
        progress(len(cuts) + 2, len(cuts) + 2)
        emit({"ok": True, "out": out, "seconds": round(total, 3), "starts": [round(s, 3) for s in starts]})
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main():
    if len(sys.argv) < 3:
        emit({"ok": False, "message": "usage: anime_comp.py panels|comp|still|edit job.json"})
        return 2
    run = {"panels": cmd_panels, "comp": cmd_comp, "still": cmd_still, "edit": cmd_edit}.get(sys.argv[1])
    if run is None:
        emit({"ok": False, "message": f"unknown command {sys.argv[1]}"})
        return 2
    try:
        with open(sys.argv[2], "r", encoding="utf-8") as f:
            job = json.load(f)
        run(job)
        return 0
    except Exception as e:  # the app shows the message
        emit({"ok": False, "message": str(e) or e.__class__.__name__})
        return 1


if __name__ == "__main__":
    sys.exit(main())
