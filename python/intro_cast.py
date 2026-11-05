#!/usr/bin/env python3
"""The intro's cast: each main character drawn clean by Magnific (Nano Banana 2.1) from the manga's
own panels and face crops, standing whole on one flat key colour, keyed out to a transparent PNG
and checked before a card uses it; and behind each, a painted background of the place the manga
shows them in.

The drawings (Drawer.draw):
  1. References: the character's best body shots from the manga (intro_maker.cast_cards ranks
     them: the reader's own name for the character first, a lone standing figure, no dialogue),
     then its face crops from the brain; captioned so the prompt can name them.
  2. The key colour: green, blue or magenta, whichever is farthest from the character's own hues
     (measured in the references and its description); green when in doubt.
  3. The drawing: a clean full-body character-select illustration in the manga's style, turned
     towards the card's centre, on one perfectly flat key colour (`build_prompt`); a black-and-
     white manga's characters are coloured the way an anime adaptation would.
  4. The key (`key_out`): the real background read from the border, a colour-difference key
     against it with a soft ramp, edges unmixed and despilled, specks and loose islands dropped,
     holes the key ate inside the figure (green eyes on a green key) given back, the edge
     feathered about a pixel, cropped to the figure.
  5. The checks (`check`): a flat key-coloured border, one figure, standing, whole (head and feet
     inside), a sensible size, no key colour on the character. A failed drawing is drawn once
     more (the next key colour when the key leaked into the character, else a firmer prompt);
     never more than MAX_DRAWS per character per export.

The backgrounds (Drawer.paint), one 16:9 1K image per character:
  1. References (`backdrop_references`): the full panels of the shots the drawing was made from
     (the place where the manga shows the character, looking the way it is drawn), then panels
     of the same place from those pages and the pages around them, establishing panels without
     people first.
  2. The place in words (`place_notes`): from the reads, the panels' descriptions (places,
     buildings, props), the pages' summaries and scenes (place tags, time of day, weather,
     light, mood).
  3. The painting (`backdrop_prompt`): a wide, cinematic, fully painted background of that place
     in the manga's own art style, completely empty of people and text, with depth and
     atmosphere, calmer on the side where the character will stand, in colours that sit well
     with the character's.
  4. The checks (`check_backdrop`): about 16:9, not blank or flat, not mostly lettering. A failed
     background is painted once more; when that fails too, the card keeps the manga's own
     establishing panel.

Accepted drawings are kept per manga in <intro_dir>/cast/<character id>.png (the cut-out; .json
beside it, and .raw.png, the drawing as Magnific made it, to key again without paying), the
backgrounds as <character id>.bg.png with .bg.json beside it (name, model, prompt version,
references, credits, side, made_at); both are reused on every later export, so nothing is paid
for twice, and a PNG the owner puts there by hand is used as it is. A character whose drawings
(or backgrounds) failed the checks in MAX_FAILED_EXPORTS exports is no longer drawn (or painted):
delete its .json (.bg.json) to try again. With `cast_draw` false only kept files are used (no
request is made). Credits are reported to the caller (the app records them); this module never
writes Studio's usage file. The API key is never printed or logged.

numpy + Pillow + the standard library.
"""
import base64
import io
import json
import math
import os
import re
import threading
import time
import urllib.error
import urllib.request

import numpy as np
from PIL import Image, ImageFilter

API = "https://api.magnific.com/v1/ai/text-to-image/nano-banana-2-1"
MODEL = "nano-banana-2-1"
CREDITS = 50.0            # one 1K image (the thinking level does not change the price)
PROMPT_VERSION = 2
MAX_DRAWS = 2             # drawings per character per export
MAX_FAILED_EXPORTS = 2    # exports whose drawings all failed before a character is no longer drawn
MAX_PARALLEL = 4
MAX_REFS = 6
POLL_SEC = 2.0
WAIT_SEC = 360.0
KEYS = {"green": (0, 255, 0), "blue": (0, 0, 255), "magenta": (255, 0, 255)}
KEY_NAMES = {"green": ("pure green", "#00FF00"), "blue": ("pure blue", "#0000FF"), "magenta": ("pure magenta", "#FF00FF")}
HUES = {"green": 120.0, "blue": 240.0, "magenta": 300.0}
# The key's soft ramp, in units of the background's own key strength: at or under LO a pixel is
# the figure, at or over HI it is the background.
LO, HI = 0.12, 1.0
USER_AGENT = "MangaVox-intro"


# ─────────────────────────────────────────────────────────────── progress

class Progress:
    """The intro's progress for the app: {"p": 0..1, "step": text, "stage": name} written
    atomically to `path` (a .tmp beside it, then os.replace) whenever the step changes and at
    most every `gap` seconds otherwise. `p` never goes back. No path: nothing is written."""

    def __init__(self, path, gap=0.25):
        self.path = path or None
        self.gap = gap
        self.p = 0.0
        self.step = ""
        self.stage = ""
        self.at = -1e9
        self.writes = 0
        self.lock = threading.Lock()

    def set(self, p=None, step=None, stage=None, minor=False):
        """Moves to `p` with `step` / `stage` (None keeps them). A changed step is written at once
        unless `minor` (a counter in the text: throttled like a progress move)."""
        with self.lock:
            if p is not None:
                self.p = max(self.p, min(1.0, max(0.0, float(p))))
            changed = (step is not None and step != self.step) or (stage is not None and stage != self.stage)
            if step is not None:
                self.step = step
            if stage is not None:
                self.stage = stage
            if not self.path:
                return
            now = time.monotonic()
            if (minor or not changed) and now - self.at < self.gap:
                return
            self.at = now
            self._write()

    def part(self, a, b):
        """A setter for the span a..b of the whole: f(frac, step=None, stage=None, minor=False)
        (frac None: only the step)."""
        def set_part(f, step=None, stage=None, minor=False):
            self.set(None if f is None else a + (b - a) * min(1.0, max(0.0, f)), step, stage, minor)
        return set_part

    def flush(self):
        with self.lock:
            if self.path:
                self.at = time.monotonic()
                self._write()

    def _write(self):
        data = json.dumps({"p": round(self.p, 4), "step": self.step, "stage": self.stage})
        tmp = self.path + ".tmp"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                f.write(data)
            for _ in range(10):
                try:
                    os.replace(tmp, self.path)
                    self.writes += 1
                    return
                except PermissionError:  # the app is reading it this instant (Windows)
                    time.sleep(0.02)
        except OSError:
            pass


# ─────────────────────────────────────────────────────────────── Magnific

class MagnificError(Exception):
    """kind: "key" (401), "credits" (402/403), "rate" (429), "filter" (the task FAILED, usually
    the content filter), "timeout", "network" or "error"."""

    def __init__(self, kind, message):
        super().__init__(message)
        self.kind = kind

    @property
    def fatal(self):
        """No more drawings this run."""
        return self.kind in ("key", "credits")

    def short(self):
        """For the owner's notes: "Magnific refused (not enough credits)"."""
        return {
            "key": "Magnific rejected the API key",
            "credits": "Magnific refused (not enough credits, or the plan does not include Nano Banana 2.1)",
            "rate": "Magnific was rate limiting",
            "filter": "Magnific's content filter blocked the drawing",
            "timeout": "Magnific took too long",
            "network": "Magnific could not be reached",
        }.get(self.kind, str(self))


def api_key(studio_dir):
    """Magnific's key: `studio_dir`/magnific.json's "api_key", else MAGNIFIC_API_KEY (None)."""
    key = None
    if studio_dir:
        try:
            with open(os.path.join(studio_dir, "magnific.json"), encoding="utf-8") as f:
                key = json.load(f).get("api_key")
        except (OSError, ValueError, AttributeError):
            key = None
    if not isinstance(key, str) or not key.strip():
        key = os.environ.get("MAGNIFIC_API_KEY") or ""
    return key.strip() or None


def request_body(prompt, refs, aspect="9:16", resolution="1K", thinking="high"):
    """The Nano Banana 2.1 request: `refs` are (base64 JPEG, caption) pairs."""
    body = {"prompt": prompt, "aspect_ratio": aspect, "resolution": resolution, "thinking_level": thinking}
    if refs:
        body["reference_images"] = [{"image": data, "mime_type": "image/jpeg", "text": text} for data, text in refs]
    return body


def jpeg_b64(img, long_side=1536, min_side=512):
    """`img` (a PIL image or a path) as base64 JPEG, at most `long_side` px on its long side (a
    small crop is enlarged to `min_side` so the model sees it clearly)."""
    im = (img if isinstance(img, Image.Image) else Image.open(img)).convert("RGB")
    big = max(im.size)
    s = long_side / big if big > long_side else (min_side / big if big < min_side else 1.0)
    if s != 1.0:
        im = im.resize((max(1, round(im.width * s)), max(1, round(im.height * s))), Image.LANCZOS)
    buf = io.BytesIO()
    im.save(buf, "JPEG", quality=92)
    return base64.b64encode(buf.getvalue()).decode("ascii")


def _error(code, body):
    msg = ""
    try:
        v = json.loads(body.decode("utf-8", "replace"))
        msg = str(v.get("message") or v.get("detail") or "")[:200] if isinstance(v, dict) else ""
    except ValueError:
        pass
    if code == 401:
        return MagnificError("key", f"Magnific rejected the API key ({msg or 'unauthorized'})")
    if code in (402, 403):
        return MagnificError("credits", f"Magnific refused the request ({msg or 'not enough API credits, or the plan does not include this model'})")
    if code == 429:
        return MagnificError("rate", "Magnific is rate limiting")
    return MagnificError("error", f"Magnific error {code}" + (f": {msg}" if msg else ""))


class Client:
    """Magnific's Nano Banana 2.1 over HTTPS (urllib): submit a task, poll it every `poll` seconds
    for up to `wait`, download the image. Tests replace `call` and `fetch`."""

    def __init__(self, key, poll=POLL_SEC, wait=WAIT_SEC, backoff=5.0):
        self._key = key
        self.poll, self.wait, self.backoff = poll, wait, backoff

    def __repr__(self):  # never show the key
        return "Client(<key hidden>)"

    def call(self, method, url, body=None):
        data = json.dumps(body).encode("utf-8") if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("x-magnific-api-key", self._key)
        req.add_header("Accept", "application/json")
        req.add_header("User-Agent", USER_AGENT)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                raw = r.read()
        except urllib.error.HTTPError as e:
            raise _error(e.code, e.read() or b"") from None
        except (urllib.error.URLError, OSError, ValueError):
            raise MagnificError("network", "could not reach Magnific") from None
        try:
            return json.loads(raw.decode("utf-8"))
        except ValueError:
            raise MagnificError("error", "Magnific sent an unreadable answer") from None

    def fetch(self, url):
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        for k in range(3):
            try:
                with urllib.request.urlopen(req, timeout=120) as r:
                    return r.read()
            except (urllib.error.URLError, OSError):
                if k == 2:
                    raise MagnificError("network", "could not download the drawing") from None
                time.sleep(2)

    def generate(self, prompt, refs, on_wait=None, aspect="9:16"):
        """One image (the file's bytes): a drawing (9:16) or a background (16:9). Raises
        MagnificError."""
        body = request_body(prompt, refs, aspect)
        for k in range(4):
            try:
                v = self.call("POST", API, body)
                break
            except MagnificError as e:
                if e.kind != "rate" or k == 3:
                    raise
                time.sleep(self.backoff * 2 ** k)
        d = v.get("data", v) if isinstance(v, dict) else {}
        task = d.get("task_id") if isinstance(d, dict) else None
        if not task:
            raise MagnificError("error", "Magnific did not return a task")
        t0 = time.monotonic()
        while time.monotonic() - t0 < self.wait:
            time.sleep(self.poll)
            if on_wait:
                on_wait(time.monotonic() - t0)
            try:
                v = self.call("GET", f"{API}/{task}")
            except MagnificError as e:
                if e.kind in ("network", "rate"):
                    continue
                raise
            d = v.get("data", v) if isinstance(v, dict) else {}
            status = str(d.get("status") or "").upper()
            if status == "COMPLETED":
                gen = d.get("generated") or []
                src = gen[0] if gen else None
                if isinstance(src, dict):
                    src = src.get("url")
                if not src:
                    raise MagnificError("error", "Magnific finished without an image")
                return self.fetch(src)
            if status == "FAILED":
                raise MagnificError("filter", "Magnific could not make this image (its content filter may have blocked it)")
        raise MagnificError("timeout", "Magnific took too long")


# ─────────────────────────────────────────────────────────────── the key colour

def _hue_sat(a):
    """Hue (degrees), saturation and value of an RGB array (0..1)."""
    mx, mn = a.max(axis=2), a.min(axis=2)
    d = mx - mn
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    safe = np.where(d > 1e-6, d, 1.0)
    h = np.where(mx == r, ((g - b) / safe) % 6, np.where(mx == g, (b - r) / safe + 2, (r - g) / safe + 4)) * 60.0
    return np.where(d > 1e-6, h, 0.0), d / np.maximum(mx, 1e-6), mx


def hue_shares(images):
    """How much of the strongly coloured picture (centre-weighted, where a character stands) lies
    near each key hue, over `images`."""
    tot = {k: 0.0 for k in HUES}
    weight = 0.0
    for im in images:
        im = im.convert("RGB")
        im.thumbnail((160, 160))
        a = np.asarray(im, dtype=np.float32) / 255
        h, w = a.shape[:2]
        hue, sat, val = _hue_sat(a)
        yy, xx = np.mgrid[0:h, 0:w]
        cw = np.exp(-(((xx - w / 2) / (0.38 * w)) ** 2 + ((yy - h / 2) / (0.45 * h)) ** 2))
        strong = (sat > 0.35) & (val > 0.22)
        for k, kh in HUES.items():
            near = np.abs((hue - kh + 180) % 360 - 180) < 40
            tot[k] += float((cw * sat * (strong & near)).sum())
        weight += float(cw.sum())
    return {k: v / max(weight, 1e-6) for k, v in tot.items()}


_WORDS = {
    "green": r"\b(green|emerald|olive|lime|jade|mint)\b",
    "blue": r"\b(blue|navy|azure|cyan|teal|turquoise|indigo|cobalt)\b",
    "magenta": r"\b(purple|violet|magenta|pink|lilac|lavender|fuchsia|plum)\b",
}


def monochrome(images):
    """The references are black and white (a manga printed without colour)."""
    coloured, total = 0, 0
    for im in images:
        im = im.convert("RGB")
        im.thumbnail((128, 128))
        a = np.asarray(im, dtype=np.float32) / 255
        _, sat, val = _hue_sat(a)
        coloured += int(((sat > 0.25) & (val > 0.2)).sum())
        total += sat.size
    return total > 0 and coloured / total < 0.03


def choose_colours(images, description=""):
    """The key colours, best first: the one farthest from the character's own hues (measured in
    `images` and named in `description`), green when in doubt. Returns (order, shares)."""
    shares = hue_shares(images)
    d = (description or "").lower()
    for k, pat in _WORDS.items():
        if re.search(pat, d):
            shares[k] += 0.04
    order = sorted(KEYS, key=lambda k: (shares[k], k != "green"))
    if order[0] != "green" and shares["green"] <= shares[order[0]] + 0.012:
        order.remove("green")
        order.insert(0, "green")
    return order, shares


# ─────────────────────────────────────────────────────────────── the key

def strength(a, colour):
    """How much of the key colour each pixel holds: green g - max(r, b), blue b - max(r, g),
    magenta min(r, b) - g (arrays of RGB in 0..1, last axis the channels)."""
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    if colour == "green":
        return g - np.maximum(r, b)
    if colour == "blue":
        return b - np.maximum(r, g)
    return np.minimum(r, b) - g


def despill(rgb, colour):
    """`rgb` with the key colour's excess taken out (a pixel no longer leans towards the key)."""
    out = rgb.copy()
    ex = np.clip(strength(rgb, colour), 0, None)
    if colour == "green":
        out[..., 1] -= ex
    elif colour == "blue":
        out[..., 2] -= ex
    else:
        out[..., 0] -= ex
        out[..., 2] -= ex
    return out


def _border(a, frac=0.015):
    h, w = a.shape[:2]
    m = max(2, int(round(min(h, w) * frac)))
    return np.concatenate([a[:m].reshape(-1, 3), a[-m:].reshape(-1, 3), a[m:-m, :m].reshape(-1, 3), a[m:-m, -m:].reshape(-1, 3)])


def background(a, colour):
    """The drawing's real background colour (models never paint the exact value): the median of
    its key-coloured border pixels, or None when the border is not mostly the key colour."""
    px = _border(a)
    good = px[strength(px, colour) > 0.25]
    if len(good) < 0.5 * len(px):
        return None
    return np.median(good, axis=0)


def _pool(mask, f):
    """Any-pooling by `f` (a small grid that keeps thin parts and narrow gaps)."""
    h, w = mask.shape
    H, W = -(-h // f), -(-w // f)
    pad = np.zeros((H * f, W * f), bool)
    pad[:h, :w] = mask
    return pad.reshape(H, f, W, f).any(axis=(1, 3))


def _unpool(small, f, shape):
    return np.repeat(np.repeat(small, f, axis=0), f, axis=1)[: shape[0], : shape[1]]


def label(mask):
    """4-connected pieces of a (small) boolean array: (labels, sizes), sizes[0] unused."""
    h, w = mask.shape
    m = mask.ravel().tolist()
    n_px = h * w
    lab = [0] * n_px
    sizes = [0]
    cur = 0
    for start in np.flatnonzero(mask.ravel()).tolist():
        if lab[start]:
            continue
        cur += 1
        lab[start] = cur
        stack = [start]
        n = 0
        while stack:
            p = stack.pop()
            n += 1
            x = p % w
            for q in (p - 1 if x > 0 else -1, p + 1 if x < w - 1 else -1, p - w, p + w):
                if 0 <= q < n_px and m[q] and not lab[q]:
                    lab[q] = cur
                    stack.append(q)
        sizes.append(n)
    return np.array(lab, np.int32).reshape(h, w), sizes


def _filter(mask, flt):
    return np.asarray(Image.fromarray(mask.astype(np.uint8) * 255).filter(flt)) > 127


def key_out(img, colour):
    """`img` (the raw drawing) with its flat `colour` background keyed out: (RGBA cropped to the
    figure with a small margin, info for `check`). The RGBA is None when there is no figure or no
    key-coloured background."""
    a = np.asarray(img.convert("RGB"), dtype=np.float32) / 255
    h, w = a.shape[:2]
    info = {"colour": colour, "size": (w, h)}
    B = background(a, colour)
    info["bg"] = None if B is None else [round(float(v), 4) for v in B]
    if B is None:
        return None, info
    sb = float(strength(B, colour))
    info["bg_strength"] = round(sb, 4)
    if sb < 0.3:
        return None, info
    s = strength(a, colour)
    t = s / sb
    dist = np.sqrt(((a - B) ** 2).sum(axis=2))
    bp = _border(np.dstack([dist, dist, dist]))[:, 0]
    info["border_flat"] = float((bp < 0.12).mean())
    alpha = np.clip((HI - t) / (HI - LO), 0.0, 1.0)
    alpha[dist < 0.07] = 0.0
    # Edges: unmix the background out of partly see-through pixels, then despill.
    part = (alpha > 0.0) & (alpha < 1.0)
    rgb = a.copy()
    if part.any():
        al = np.maximum(alpha[part], 0.25)[:, None]
        rgb[part] = np.clip((a[part] - (1.0 - alpha[part])[:, None] * B) / al, 0.0, 1.0)
    solid = alpha > 0.5
    f = max(1, int(math.ceil(math.sqrt(h * w / 140000.0))))
    small = _pool(solid, f)
    lab, sizes = label(small)
    total = sum(sizes)
    if total == 0:
        return None, info
    main = int(np.argmax(sizes))
    ys, xs = np.nonzero(lab == main)
    my0, my1, mx0, mx1 = ys.min(), ys.max(), xs.min(), xs.max()
    grow = 0.06 * (my1 - my0 + 1)
    keep_ids = [main]
    for i in range(1, len(sizes)):
        if i == main or sizes[i] < max(3, 0.03 * sizes[main]):
            continue
        yi, xi = np.nonzero(lab == i)
        if yi.min() >= my0 - grow and yi.max() <= my1 + grow and xi.min() >= mx0 - grow and xi.max() <= mx1 + grow:
            keep_ids.append(i)
    info["main_share"] = sizes[main] / float(total)
    info["pieces"] = len(sizes) - 1
    keep_small = np.isin(lab, keep_ids)
    # The figure's silhouette (its enclosed holes filled). An enclosed hole with no background
    # colour in it is the figure itself, which the key ate (green eyes on a green key): given back
    # in its own colours. A hole with background colour is a real gap (between an arm and the
    # body). Solid pixels inside, away from the outer edge and from any gap, are the figure too.
    bl, bsizes = label(~keep_small)
    border_ids = np.unique(np.concatenate([bl[0], bl[-1], bl[:, 0], bl[:, -1]]))
    outside = np.isin(bl, border_ids)
    sil = _unpool(~outside, f, (h, w)) & _filter(alpha > 0, ImageFilter.MaxFilter(5))
    bgish = dist < 0.1
    # Darker or lighter shades of the background (seen through a narrow gap between strands of
    # hair or fingers, or mixed with a dark outline) are background too, never the figure.
    bn = B / max(float(np.linalg.norm(B)), 1e-6)
    along = a @ bn
    off = np.linalg.norm(a - along[..., None] * bn, axis=2) / np.maximum(np.linalg.norm(a, axis=2), 1e-6)
    shade = ((off < 0.12) & (along > 0.12)) | ((off < 0.42) & (a.max(axis=2) < 0.45) & (s > 0.08))
    bgish2 = bgish | shade
    figure_px = sizes[main] * f * f
    protect = np.zeros((h, w), bool)
    for i in range(1, len(bsizes)):
        if i in border_ids or bsizes[i] * f * f > 0.006 * figure_px:
            continue
        hole = _unpool(bl == i, f, (h, w)) & (alpha < 0.999)
        n = int(hole.sum())
        if n and int((hole & bgish2).sum()) <= 0.02 * n:
            protect |= hole
    if protect.any():  # with their antialiased rims (a mix of two of the figure's own colours)
        protect = _filter(protect, ImageFilter.MaxFilter(5)) & (alpha < 0.999) & ~shade
    holes = sil & ((alpha < 0.5) | shade) & ~protect
    near_hole = _filter(holes, ImageFilter.MaxFilter(13)) | _filter(bgish, ImageFilter.MaxFilter(7))
    firm = sil & _filter(sil, ImageFilter.MinFilter(9)) & ~near_hole & (alpha > 0.5) & ~shade
    inside = firm | protect
    sil_px = max(1, int(sil.sum()))
    info["eaten"] = float((inside & (alpha < 0.9)).sum()) / sil_px
    keep = _unpool(keep_small, f, (h, w))
    keep = (_filter(keep, ImageFilter.MaxFilter(5)) & (alpha > 0)) | inside
    alpha = np.where(keep, alpha, 0.0)
    alpha[inside] = 1.0
    rgb[inside] = a[inside]
    # Despill everything but the figure's solid inside (its edges, gaps and their rims), which
    # keeps its own colours.
    solid = alpha > 0.5
    rgb[~inside] = despill(rgb[~inside][None], colour)[0]
    # The checks' numbers, on the figure as kept.
    ys, xs = np.nonzero(solid)
    if len(ys) == 0:
        return None, info
    y0, y1, x0, x1 = int(ys.min()), int(ys.max()) + 1, int(xs.min()), int(xs.max()) + 1
    info["bbox"] = [x0, y0, x1, y1]
    info["area"] = int(len(ys))
    near = np.zeros((h, w), bool)
    gy, gx = int(0.04 * h), int(0.04 * w)
    near[max(0, y0 - gy): y1 + gy, max(0, x0 - gx): x1 + gx] = True
    edge = _filter(solid, ImageFilter.MaxFilter(7)) & ~_filter(solid, ImageFilter.MinFilter(7))
    # Key-coloured pixels that are not the background, near the figure but outside what was
    # kept whole: the key ate part of the character (a green sleeve), or the background has a
    # floor, shadow or glow.
    keyish = near & (t > 0.3) & (dist > 0.25) & ~edge & ~inside
    info["leak"] = float(keyish.sum()) / max(1, len(ys))
    # Feather the edge about a pixel; nothing of the background stays in the transparent part.
    alpha = np.asarray(Image.fromarray((alpha * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(0.7))).astype(np.float32) / 255
    alpha = np.where(keep | _filter(keep, ImageFilter.MaxFilter(3)), alpha, 0.0)
    rgb[alpha <= 0.0] = 0.0
    out = np.dstack([np.clip(rgb, 0, 1) * 255, alpha * 255]).round().astype(np.uint8)
    m = max(4, int(0.02 * (y1 - y0)))
    box = (max(0, x0 - m), max(0, y0 - m), min(w, x1 + m), min(h, y1 + m))
    return Image.fromarray(out).crop(box), info


# Why a drawing failed: (reason for the owner, what the next drawing must do).
FIXES = {
    "background": "Important: the background must be one flat, solid {cname} ({hex}) from edge to edge: no gradient, floor, shadow or glow.",
    "figures": "Important: draw only {name}, alone: no other people, creatures, effects or objects around.",
    "cut": "Important: the whole body must fit inside the picture with empty space all around: leave a gap above the head, below the feet and at both sides; keep the arms, any weapon and any effect inside the picture (a compact pose); draw the character a little smaller.",
    "shape": "Important: {name} stands upright, full body from head to feet, taller than wide.",
    "small": "Important: {name} should fill about 80 percent of the picture's height.",
    "leak": "Important: no {cname} anywhere on the character (clothes, hair, eyes, effects) and no {cname} light or glow on them.",
    "safe": "Important: keep it calm and family-friendly: a relaxed, confident standing pose, no weapon drawn, no blood, no injuries, fully clothed.",
}


def leaks(info):
    """The key colour shows on the character (or the key ate into it), whatever else is wrong."""
    return info.get("leak", 0) > 0.006 or info.get("eaten", 0) > 0.015


def check(info):
    """Why a keyed drawing can't be used: (reason, fix) with fix a FIXES key ("leak" asks for
    the next key colour), or None when it is good."""
    name = KEY_NAMES[info.get("colour", "green")][0]
    if info.get("bg") is None or info.get("bg_strength", 0) < 0.3:
        return f"the background is not {name}", "background"
    if "bbox" not in info:
        return "no figure was found", "figures"
    w, h = info["size"]
    x0, y0, x1, y1 = info["bbox"]
    if info.get("main_share", 0) < 0.92:
        return "more than one figure is in it", "figures"
    # Models draw the figure big (feet a few pixels above the bottom is normal); one that reaches
    # an edge is cut off by it.
    mx, my = max(3, 0.003 * w), max(3, 0.002 * h)
    cut = [s for s, bad in (("top", y0 < my), ("feet", y1 > h - my), ("left", x0 < mx), ("right", x1 > w - mx)) if bad]
    if cut:
        return "the figure is cut off at the " + " and ".join(cut), "cut"
    if info.get("border_flat", 0) < 0.95:
        return f"the background is not flat {name}", "background"
    bw, bh = x1 - x0, y1 - y0
    if bh < 1.2 * bw:
        return "it is not a standing full-body figure", "shape"
    if bh < 0.55 * h:
        return "the figure is too small", "small"
    fill = info.get("area", 0) / float(max(1, bw * bh))
    if not 0.15 <= fill <= 0.88:
        return "the figure has an odd shape", "shape"
    if leaks(info):
        return f"{name} shows on the character", "leak"
    return None


# ─────────────────────────────────────────────────────────────── the prompt

def clean_description(text):
    """The brain's appearance notes without blood and gore (safe for work)."""
    t = re.sub(r"\b(blood|gore)[- ](spattered|splattered|stained|soaked|drenched|caked|smeared)\s*", "", text or "", flags=re.I)
    t = re.sub(r"\b(bloodied|bloody|gory)\s+", "", t, flags=re.I)
    parts = [p for p in re.split(r"(?<=[,;])\s*", t) if not re.search(r"\b(blood|gore|corpse|wound)", p, re.I)]
    t = " ".join(parts).strip().rstrip(",;").strip()
    return re.sub(r"\s{2,}", " ", t)


def _images(nums):
    if len(nums) == 1:
        return f"Image {nums[0]}"
    if len(nums) == 2:
        return f"Images {nums[0]} and {nums[1]}"
    return "Images " + ", ".join(str(n) for n in nums[:-1]) + f" and {nums[-1]}"


def build_prompt(name, description, kinds, colour, facing, fixes=(), mono=False):
    """The drawing's prompt. `kinds`: "body" / "face" per reference image (Image 1 first);
    `facing`: "left" or "right" (the viewer's); `fixes`: FIXES keys from a failed drawing; `mono`:
    the manga is black and white (coloured the way an anime adaptation would)."""
    cname, hexv = KEY_NAMES[colour]
    body = [i + 1 for i, k in enumerate(kinds) if k == "body"]
    face = [i + 1 for i, k in enumerate(kinds) if k == "face"]
    out = [FIXES[f].format(name=name, cname=cname, hex=hexv) for f in dict.fromkeys(fixes) if f in FIXES]
    if body:
        out.append(f"{_images(body)} {'shows' if len(body) == 1 else 'show'} {name} in the manga.")
    if face:
        out.append(f"{_images(face)} {'is a close-up' if len(face) == 1 else 'are close-ups'} of {name}'s face.")
    if body and face:
        out.append(f"They are all the same person. If the pictures show {name} in different outfits or looks, draw the outfit and look that the "
                   "character notes describe.")
    out += [
        f"Draw {name} as one clean, finished full-body character illustration, like the character art on a video game's character-select screen.",
        (f"Keep {name} exactly as in the reference images: the same face, hairstyle, age, height and build, and the same outfit with all its details, "
         "drawn in the manga's own art style: clean, confident line art and cel shading. The manga is black and white: colour the character the way an "
         "official anime adaptation would, with natural colours that follow the character notes.") if mono else
        (f"Keep {name} exactly as in the reference images: the same face, hairstyle and hair colour, eye colour, age, height and build, and the same outfit "
         "with all its colours and details, drawn in the manga's own art style: clean, confident line art and cel shading with the same colouring."),
        f"{name} stands alone in a confident pose, in a three-quarter view, the body and the face turned towards the {facing} side of the picture "
        f"(the viewer's {facing}).",
        "The whole body is in the picture, from the top of the head down to the soles of both feet, with clear empty space all around it: nothing "
        "touches or crosses the edges of the image. The character is centred and fills about 80 percent of the image height.",
        f"Background: the entire background is one perfectly flat, uniform, solid {cname} ({hexv}) filling the whole image from edge to edge, like a "
        "chroma-key screen. No gradient, no floor or ground, no cast shadow, no glow, no light rays, no vignette, no border or frame, no text or letters, "
        "no speech bubbles, no emblems or brand marks, no signature, no other people or creatures, no scenery and no objects.",
        f"Never use {cname} or any colour close to it on the character, the clothes, the hair or the eyes: no {cname} rim light, glow, reflections or "
        "outline; the outline is dark line art.",
    ]
    notes = clean_description(description)
    if notes:
        out.append(f"Character notes (where they disagree with the reference images, the images win): {notes}")
    out.append("Safe for work: fully clothed, no blood, no gore.")
    return "\n".join(out)


# ─────────────────────────────────────────────────────────────── references

def references(card, max_refs=MAX_REFS):
    """The drawing's reference images, best first: crops of the card's body shots (the reader's
    own name for the character first; at most 3), then its face crops. [(image, caption, kind)]."""
    name = card["name"]
    bodies, faces = [], []
    seen = set()
    shots = card.get("shots") or []
    # A shot crowded with other people or talk (a low score) misleads more than it helps; the best
    # one still serves when it is all there is.
    good = [j for j in shots if j.get("score", 0) >= -3.0] or [j for j in shots[:1] if j.get("score", 0) >= -9]
    for j in good:
        if len(bodies) >= 3:
            break
        try:
            page = Image.open(j["image"])
            page.load()
        except (OSError, KeyError):
            continue
        x0, y0, x1, y1 = [float(v) for v in j["box"]]
        if (x1 - x0) < 48 or (y1 - y0) < 120:
            continue
        key = (j["image"], round(x0), round(y0))
        if key in seen:
            continue
        seen.add(key)
        pad = 0.06 * (y1 - y0)
        box = (max(0, int(x0 - pad)), max(0, int(y0 - pad)), min(page.width, int(x1 + pad)), min(page.height, int(y1 + pad)))
        bodies.append(page.convert("RGB").crop(box))
    for f in card.get("faces") or []:
        try:
            im = Image.open(f).convert("RGB")
        except OSError:
            continue
        if min(im.size) < 64:
            continue
        faces.append(im)
    # The brain's first faces are the character's own first sightings (later ones can come from
    # a wrong merge), so at most four, in its order.
    n_face = min(len(faces), max(2, max_refs - len(bodies)), 4)
    bodies = bodies[: max_refs - n_face]
    out = []
    for im in bodies:
        out.append((im, f"Image {len(out) + 1}: {name}, full body in the manga", "body"))
    for im in faces[:n_face]:
        out.append((im, f"Image {len(out) + 1}: {name}'s face in the manga", "face"))
    return out


# ─────────────────────────────────────────────────────────────── backgrounds

BG_PROMPT_VERSION = 1
BG_ASPECT = "16:9"
MAX_BG_DRAWS = 2          # a background and at most one redraw per character per export
MAX_BG_REFS = 5
MAX_BG_WITH = 2           # panels with the character in them (the rest show the place alone)
BG_NEAR_PAGES = 8         # how far around the character's panels (in pages) the place's other panels are looked for
# What a place is made of, in the reader's panel descriptions: places, buildings and their parts,
# landscape, and the props that furnish a room.
PLACE_NOUNS = set("""courtyard temple shrine pagoda hall halls monastery gym gymnasium room bedroom office classroom corridor hallway
lobby street streets alley alleyway city town village rooftop roof rooftops skyline mountain mountains cliff cliffs forest woods jungle
field fields meadow river lake sea ocean beach shore harbor harbour port bridge castle palace throne dungeon cave cavern tunnel ruins
desert arena stadium station platform subway school academy campus hospital ward church cathedral library shop store market
restaurant cafe bar kitchen table tables garden park plaza square gate gates wall walls tower towers skyscraper skyscrapers building buildings
apartment house home mansion estate warehouse factory lab laboratory prison cell battlefield camp tent ship deck sky clouds stairs
staircase steps balcony terrace window windows curtains floor ceiling door doors pillars columns pavilion veranda yard grounds
estate fortress inn tavern hut cabin valley canyon hill hills waterfall pond swamp island highway road path trail plain plains
studio apartment dojo arena stage hallway basement attic rooftop shelves bookshelf desk bed lanterns lantern statue statues altar
bamboo trees tree rocks boulders crater portal gateway void interior exterior landscape cityscape""".split())
TIME_WORDS = ("dawn", "sunrise", "morning", "noon", "midday", "afternoon", "sunset", "dusk", "twilight", "evening", "night", "midnight",
              "nighttime", "daytime", "daylight")
WEATHER_WORDS = ("rain", "rainy", "raining", "storm", "stormy", "thunder", "lightning", "snow", "snowy", "snowing", "fog", "foggy", "mist",
                 "misty", "haze", "hazy", "cloudy", "overcast", "clear sky", "windy", "wind", "dust", "smoke", "smoky", "blizzard")
LIGHT_WORDS = ("sunlight", "moonlight", "moonlit", "sunlit", "lantern", "lanterns", "torch", "torches", "candle", "candles", "candlelight",
               "neon", "lamp", "lamps", "streetlight", "streetlights", "firelight", "fire", "glow", "glowing", "spotlight", "shadows",
               "shadowy", "dim", "dimly", "dark", "bright", "golden", "warm light", "cold light", "backlit", "fluorescent")
WIDE = re.compile(r"\b(establishing|wide shot|wide view|wide angle|panoram\w*|exterior|interior|view of|overview|landscape|skyline|aerial|"
                  r"bird's-eye|from above|vista|cityscape)\b", re.I)
BG_BAD = re.compile(r"\b(close-up|closeup|close up|close view|close shot|extreme close|black panel|blank|white panel|white space|empty panel|speech balloons?|"
                    r"speech bubbles?|text box|narration box|caption box|title card|title page|credits|chibi|floating heads?|gore|gory|blood|"
                    r"bloody|corpses?|naked|nude|sound effects?|onomatopoeia|emblem|wordmark|screen shows|phone screen|live-stream|livestream|chat overlay|"
                    r"thumbnail|diagram|map of|top of a new panel|new panel|partial view|partially|cut off|thought bubbles?|"
                    r"thought bursts?|burst balloon|balloons?|bubbles?|mostly blank|fading into|tail end|caption|screen|monitor|portrait|system window|"
                    r"status window|quest window|notification|hologram|interface|energy|speed lines|streaks|aura|tendrils|the top of a|"
                    r"edge of a figure|(?:blank|white|plain|empty|black) background)\b", re.I)
PEOPLE_WORDS = re.compile(r"\b(people|person|persons|crowd|man|men|woman|women|boy|boys|girl|girls|figures?|monks?|children|child|kids?|"
                          r"soldiers?|guards?|he|she|his|her|hers|him|himself|herself|they|them|their|someone|characters?|silhouettes?|"
                          r"faces?|couple|group|students?|novices?|trainees?|warriors?|villagers?|townsfolk|fighters?|knights?|recruits?|"
                          r"disciples?|elders?|masters?|priests?|nuns?|servants?|merchants?|citizens?|passers-?by|pedestrians?|customers?|"
                          r"spectators?|onlookers?|audience|teens?|teenagers?|adults?|ladies|gentlemen|thugs?|gang|members?|everyone|"
                          r"friends?|family|soldiers|hunters?|players?|monsters?|creatures?)\b", re.I)
# Scene moods that say something about a place's atmosphere (the rest are about people).
ATMOSPHERE = ("serene", "peaceful", "calm", "quiet", "tranquil", "tense", "ominous", "gloomy", "dark", "eerie", "mysterious", "somber",
              "sombre", "grim", "warm", "cozy", "cosy", "bright", "epic", "majestic", "dramatic", "lonely", "nostalgic", "melancholic",
              "melancholy", "hopeful", "foreboding", "menacing", "chaotic", "festive", "lively", "bustling", "solemn", "sacred", "dreamy",
              "desolate", "bleak", "cold", "eerie", "creepy", "dreadful", "dread", "sinister", "romantic", "wistful", "still", "heavy")
_TOKEN = re.compile(r"[A-Za-z][A-Za-z'-]*|[,.;:!?()]")
_STOP = set("""a an the his her their its this that these those of in on at to into onto from by with without for as while and or but under
over above below behind beside near through across along among amid inside outside within toward towards around between before after
during is are was were be being been stands stand standing sits sit sitting lies lie lying walks walking looks looking faces facing
holds holding seen shows showing shown where which who whom whose what when then there here it he she they them him we you i one
also just only very""".split())
_POSSESSIVE = {"his", "her", "their", "its", "my", "our", "your"}


# Place words every place has: a match on them alone does not make two panels the same place.
GENERIC_PLACE = {"room", "floor", "wall", "walls", "window", "windows", "door", "doors", "sky", "clouds", "table", "tables", "desk", "building",
                 "buildings", "tree", "trees", "steps", "stairs", "interior", "exterior", "landscape", "curtains", "bed", "home", "house"}


def _is_place(w):
    return w in PLACE_NOUNS or (len(w) > 3 and w.endswith("s") and w[:-1] in PLACE_NOUNS)


def place_phrases(text):
    """The places a description speaks of, as short phrases ("temple courtyard", "wooden floor of the
    room", "red wall"): runs of place words with the words just before them that describe them (not
    people), and an "of ..." after them."""
    toks = _TOKEN.findall(text or "")
    low = [t.lower() for t in toks]
    out = []
    i = 0
    while i < len(low):
        verb = low[i] in ("towers", "tower", "walls", "floors", "roofs") and i + 1 < len(low) and low[i + 1] in (
            "over", "above", "behind", "beside", "around", "up", "off", "in")
        if not _is_place(low[i]) or verb:
            i += 1
            continue
        k = i + 1
        while k < len(low) and _is_place(low[k]):
            k += 1
        j = i
        while j > 0 and i - j < 3 and low[j - 1].isalpha() and low[j - 1] not in _STOP and not PEOPLE_WORDS.fullmatch(low[j - 1]):
            j -= 1
        words = toks[j:k]
        if k < len(low) - 1 and low[k] == "of":
            m = k + 1
            lead = "the"
            if low[m] in ("a", "an", "the") or low[m] in _POSSESSIVE:
                lead = "a" if low[m] in ("a", "an") else "the"
                m += 1
            e = m
            while e < len(low) and e - m < 3 and low[e].isalpha() and low[e] not in _STOP and not PEOPLE_WORDS.fullmatch(low[e]):
                e += 1
            if e > m and any(_is_place(x) for x in low[m:e]):
                words = words + ["of", lead] + toks[m:e]
                k = e
        phrase = " ".join(words).strip()
        if phrase and not PEOPLE_WORDS.search(phrase):
            out.append(phrase)
        i = k
    return out


def _words_in(text, words):
    t = (text or "").lower()
    return [w for w in words if re.search(rf"\b{re.escape(w)}\b", t)]


def _sentences(text):
    return [s.strip() for s in re.split(r"(?<=[.!?])\s+", text or "") if s.strip()]


def _nouns(text):
    """The place words of a text (singular)."""
    out = set()
    for w in re.findall(r"[a-z]+", (text or "").lower()):
        if w in PLACE_NOUNS:
            out.add(w)
        elif len(w) > 3 and w.endswith("s") and w[:-1] in PLACE_NOUNS:
            out.add(w[:-1])
    return out


def _place_tags(scene):
    tags = scene.get("tags") if isinstance(scene, dict) else None
    out = []
    for t in tags or []:
        if isinstance(t, str) and (_nouns(t) or _words_in(t, TIME_WORDS + WEATHER_WORDS)) and not PEOPLE_WORDS.search(t):
            out.append(t.strip())
    return out


def _page_near(key, other, reach):
    """Pages `key` and `other` ("chapter:page") are in the same chapter, at most `reach` apart."""
    try:
        c1, p1 = (int(v) for v in key.split(":"))
        c2, p2 = (int(v) for v in other.split(":"))
    except (ValueError, AttributeError):
        return None
    return abs(p1 - p2) if c1 == c2 and abs(p1 - p2) <= reach else None


def _panel_crop(page, box):
    """The panel `box` (0..1 of the page) cut from `page` (PIL), with a little room around it."""
    W, H = page.size
    x0, y0, x1, y1 = box
    pad = 0.01
    b = (max(0, int((x0 - pad) * W)), max(0, int((y0 - pad * W / H) * H)), min(W, int((x1 + pad) * W)), min(H, int((y1 + pad * W / H) * H)))
    return page.crop(b) if b[2] - b[0] > 8 and b[3] - b[1] > 8 else page


def _usable(img):
    """A panel worth showing: big enough, not a sliver, not blank or black."""
    if img.width < 220 or img.height < 200 or not 0.45 <= img.width / img.height <= 3.2:
        return False
    g = np.asarray(img.convert("L").resize((96, 64)), np.float32)
    return 25 < g.mean() < 238 and g.std() > 24


def _context_nouns(j, pages):
    """The place words of a shot: its panel's description, its page's summary and place tags."""
    pg = pages.get(j["page"]) or {}
    out = _nouns(j.get("description")) | _nouns(pg.get("summary"))
    for t in _place_tags(pg.get("scene")):
        out |= _nouns(t)
    return out


def _panel_score(pn, a_nouns, dist):
    """How well panel `pn` shows the place whose words are `a_nouns`, `dist` pages away (None: not a
    picture of a place at all)."""
    d = pn.get("description") or ""
    if BG_BAD.search(d) or GORE_WORDS.search(d):
        return None
    nouns = _nouns(d)
    if not nouns and not WIDE.search(d):
        return None
    # A wide panel of the place shows it even with people in it; one without anyone is best.
    people = int(pn.get("people") or 0)
    s = (3.0 if not PEOPLE_WORDS.search(d) else 1.0) if people == 0 else -0.6 * min(4, people)
    s -= 0.4 * min(4, int(pn.get("talk") or 0))
    specific = (nouns & a_nouns) - GENERIC_PLACE
    generic = (nouns & a_nouns) & GENERIC_PLACE
    s += min(4, len(nouns)) + (2.0 if WIDE.search(d) else 0.0) + 1.5 * min(2, len(specific)) + 0.5 * min(2, len(generic))
    if (a_nouns - GENERIC_PLACE) and (nouns - GENERIC_PLACE) and not specific:
        s -= 2.0  # it names another place
    return s - 0.25 * dist


def _around(page, pages, a_nouns, skip=(), reach=BG_NEAR_PAGES):
    """[(score, key, panel)] of the panels of the pages around `page` ("chapter:page") that show
    the place (best first)."""
    try:
        ch, p0 = (int(v) for v in page.split(":"))
    except (ValueError, AttributeError):
        return []
    out = []
    for dp in range(-reach, reach + 1):
        key = f"{ch}:{p0 + dp}"
        for pn in (pages.get(key) or {}).get("panels") or []:
            if (key, tuple(round(v, 4) for v in pn.get("bbox") or [])) in skip:
                continue
            sc = _panel_score(pn, a_nouns, abs(dp))
            if sc is not None and sc > 1.5:
                out.append((sc, key, pn))
    return sorted(out, key=lambda c: -c[0])


def backdrop_references(card, pages, max_refs=MAX_BG_REFS):
    """The references for a character's background, best first: the full panels of the shots its
    drawing was made from (the place where the manga shows the character, looking the way it is
    drawn: of those shots, and then the character's other shots, the one whose panel and pages
    show its place best, then one more of the same place), then other panels of that place from
    the same pages and the pages around them (establishing panels without people first; failing
    those, of the same place anywhere in the chapter). `pages` is intro_maker's page index
    ({"chapter:page": {image, summary, scene, panels}}). [(image, caption, kind, source)] with kind
    "with" or "place" and source {image, box, page, description}."""
    name = card.get("name") or "the character"
    shots = [j for j in card.get("shots") or [] if j.get("panel") and j.get("page") in pages]
    drawn = [j for j in shots if j.get("score", 0) >= -3.0][:3] or [j for j in shots[:1] if j.get("score", 0) >= -9]
    if not drawn:
        return []
    pool = drawn + [j for j in shots if not any(j is x for x in drawn)][:3]

    def key_of(j):
        return (j["page"], tuple(round(v, 4) for v in j["panel"]))

    best = None
    for idx, j in enumerate(pool):
        d = j.get("description") or ""
        nouns = _context_nouns(j, pages)
        near = _around(j["page"], pages, nouns, {key_of(j)})
        sc = (2.0 * len(_nouns(d)) + (2.0 if WIDE.search(d) else 0.0) + 0.5 * len(nouns) + sum(c[0] for c in near[:2]) / 2
              + (4.0 if any(j is x for x in drawn) else 0.0) + (1.0 if idx == 0 else 0.0) - (3.0 if BG_BAD.search(d) or GORE_WORDS.search(d) else 0.0))
        if best is None or sc > best[0]:
            best = (sc, j, nouns)
    _, anchor, a_nouns = best
    ap = pages[anchor["page"]]
    chosen = [anchor]
    for j in pool:
        if len(chosen) >= MAX_BG_WITH:
            break
        if any(key_of(j) == key_of(c) for c in chosen):
            continue
        near = _page_near(anchor["page"], j["page"], 2) is not None
        if near or ((_context_nouns(j, pages) & a_nouns) - GENERIC_PLACE):
            chosen.append(j)
    taken = {key_of(c) for c in chosen}
    found = {}
    for c in chosen:
        for sc, key, pn in _around(c["page"], pages, a_nouns, taken):
            k = (key, tuple(round(v, 4) for v in pn.get("bbox") or []))
            if k not in found or found[k][0] < sc:
                found[k] = (sc, key, pn)
    cands = sorted(found.values(), key=lambda c: -c[0])
    if len([c for c in cands if c[0] > 3.0]) < 2 and a_nouns:
        # Too little around the character's panels: the same place without people anywhere in the
        # chapter (two of its place words, or one of its pages' place tags, in the description).
        chapter = anchor["page"].split(":")[0]
        a_tags = [t.lower() for t in _place_tags(ap.get("scene")) if len(t.split()) > 1]
        for key, pg in pages.items():
            if key.split(":")[0] != chapter or _page_near(anchor["page"], key, BG_NEAR_PAGES) is not None:
                continue
            for pn in pg.get("panels") or []:
                d = pn.get("description") or ""
                if int(pn.get("people") or 0) or PEOPLE_WORDS.search(d):
                    continue
                if len(_nouns(d) & a_nouns) < 2 and not any(t in d.lower() for t in a_tags):
                    continue
                sc = _panel_score(pn, a_nouns, BG_NEAR_PAGES + 2)
                if sc is not None and sc > 3.0:
                    cands.append((sc, key, pn))
        cands.sort(key=lambda c: -c[0])
    out, opened = [], {}

    def page_img(path):
        if path not in opened:
            try:
                im = Image.open(path)
                im.load()
                opened[path] = im.convert("RGB")
            except OSError:
                opened[path] = None
        return opened[path]

    for j in chosen:
        pg = page_img(j["image"])
        if pg is None:
            continue
        crop = _panel_crop(pg, j["panel"])
        if crop.width < 160 or crop.height < 120:
            continue
        out.append((crop, f"Image {len(out) + 1}: a panel of the manga showing {name} in this place", "with",
                    {"image": j["image"], "box": [round(v, 4) for v in j["panel"]], "page": j["page"], "description": j.get("description") or ""}))
    if not out:
        return []
    for sc, key, pn in cands:
        if len(out) >= max_refs:
            break
        pg = page_img(pages[key]["image"])
        if pg is None:
            continue
        crop = _panel_crop(pg, pn["bbox"])
        if not _usable(crop):
            continue
        out.append((crop, f"Image {len(out) + 1}: the same place in another panel of the manga", "place",
                    {"image": pages[key]["image"], "box": [round(v, 4) for v in pn["bbox"]], "page": key, "description": pn.get("description") or ""}))
    return out


GORE_WORDS = re.compile(r"\b(gor[ey]|blood|bloody|bloodied|corpses?|severed|entrails|guts|naked|nude)\b", re.I)


def manga_monochrome(pages, refs=(), n=8):
    """The manga is black and white: judged over `n` of its pages spread through it, and the
    references."""
    keys = sorted(pages, key=lambda k: tuple(int(v) for v in k.split(":")) if k.replace(":", "").isdigit() else (0, 0))
    pick = [keys[int(i * (len(keys) - 1) / max(1, n - 1))] for i in range(min(n, len(keys)))] if keys else []
    ims = [r[0] for r in refs]
    for k in pick:
        try:
            im = Image.open(pages[k]["image"])
            im.draft("RGB", (160, 160))
            im = im.convert("RGB")
            im.thumbnail((160, 160))
            ims.append(im)
        except (OSError, KeyError, ValueError):
            continue
    if not ims:
        return False
    votes = [monochrome([im]) for im in ims]
    return sum(votes) >= 0.7 * len(votes)


def place_notes(refs, pages):
    """What the reads say about the place in `refs` (backdrop_references): {place: [phrases],
    seen: [sentences without people], time, weather, light, mood, tags}."""
    texts = []  # (text, weight)
    keys = []
    for k, (_, _, kind, src) in enumerate(refs):
        texts.append((src.get("description") or "", 2.0 if k == 0 else (1.5 if kind == "with" else 1.2)))
        if src.get("page") not in keys:
            keys.append(src.get("page"))
    phrase_texts = list(texts)  # the places are named from the panels' descriptions (summaries tell the story)
    for key in keys:
        pg = pages.get(key) or {}
        texts.append((pg.get("summary") or "", 0.8))
    names = set()
    for key in keys:
        for pn in (pages.get(key) or {}).get("panels") or []:
            for nm in pn.get("names") or []:
                names |= {w for w in re.findall(r"[a-z]+", str(nm).lower()) if len(w) > 2}
    counts, first = {}, {}
    for text, wt in phrase_texts:
        for ph in place_phrases(text):
            if names & set(re.findall(r"[a-z]+", ph.lower())):
                continue
            head = ph.lower().split(" of ")[0].split()[-1]
            k = (head, ph.lower())
            counts[k] = counts.get(k, 0.0) + wt * (1 + 0.15 * len(ph.split()))
            first.setdefault(k, ph)
    best = {}
    for (head, low), c in sorted(counts.items(), key=lambda kv: -kv[1]):
        if head not in best and not any(low in o.lower() for o in best.values()):
            best[head] = first[(head, low)]
    place = list(best.values())[:5]
    seen = []
    for text, wt in texts:
        for s in _sentences(text):
            if len(seen) >= 2:
                break
            if _nouns(s) and not PEOPLE_WORDS.search(s) and not BG_BAD.search(s) and not GORE_WORDS.search(s) and len(s) < 220 and s not in seen:
                seen.append(s)

    def top(words):
        c = {}
        for text, wt in texts:
            for w in _words_in(text, words):
                c[w] = c.get(w, 0.0) + wt
        return [w for w, _ in sorted(c.items(), key=lambda kv: -kv[1])]

    scene = (pages.get(keys[0]) or {}).get("scene") if keys else {}
    scene = scene if isinstance(scene, dict) else {}
    tags = []
    for key in keys:
        for t in _place_tags((pages.get(key) or {}).get("scene") or {}):
            if t.lower() not in [x.lower() for x in tags]:
                tags.append(t)
    mood = ", ".join(_words_in(scene.get("mood") if isinstance(scene.get("mood"), str) else "", ATMOSPHERE)[:2])
    return {"place": place, "seen": seen, "time": (top(TIME_WORDS) or [None])[0], "weather": top(WEATHER_WORDS)[:2],
            "light": top(LIGHT_WORDS)[:3], "mood": mood.strip(), "tags": tags[:4]}


COLOUR_NAMES = ((15, "red"), (40, "orange"), (68, "yellow"), (160, "green"), (195, "teal"), (255, "blue"), (290, "purple"), (335, "pink"), (360, "red"))


def colour_words(img, n=3):
    """The main colours of a character's drawing (RGBA, its opaque part) in words, the biggest first."""
    if img is None:
        return []
    try:
        im = (img if isinstance(img, Image.Image) else Image.open(img)).convert("RGBA")
    except OSError:
        return []
    im.thumbnail((128, 128))
    a = np.asarray(im, np.float32) / 255
    rgb = a[..., :3][a[..., 3] > 0.6]
    if not len(rgb):
        return []
    h, s, v = _hue_sat(rgb[None])
    h, s, v = h[0], s[0], v[0]
    names = np.empty(len(rgb), object)
    for i in range(len(rgb)):
        if v[i] < 0.2:
            names[i] = "black"
        elif s[i] < 0.18:
            names[i] = "white" if v[i] > 0.78 else "grey"
        else:
            name = next(nm for lim, nm in COLOUR_NAMES if h[i] < lim)
            if name in ("orange", "red") and v[i] < 0.55:
                name = "brown" if name == "orange" else "dark red"
            names[i] = name
    uniq, cnt = np.unique(names.astype(str), return_counts=True)
    order = [u for u, c in sorted(zip(uniq, cnt), key=lambda x: -x[1]) if c >= 0.06 * len(rgb)]
    return order[:n]


def _join(words):
    words = [w for w in words if w]
    return words[0] if len(words) == 1 else ", ".join(words[:-1]) + " and " + words[-1] if words else ""


BG_FIXES = {
    "wide": "Important: a wide 16:9 landscape picture, painted edge to edge.",
    "detail": "Important: a fully painted, detailed place filling the whole picture: no blank, white, black or flat areas.",
    "text": "Important: no text, letters, speech bubbles, captions, panels or sound effects anywhere: only the painted place.",
    "safe": "Important: keep it calm and peaceful: an empty place, no people, no blood, no weapons, no damage.",
}


def backdrop_prompt(name, kinds, notes, side, colours=(), fixes=(), mono=False):
    """The background's prompt. `kinds`: "with" / "place" per reference image (Image 1 first);
    `notes`: place_notes(); `side`: where the character will stand ("left" or "right": that side is
    kept calmer); `colours`: the character's colours in words; `fixes`: BG_FIXES keys from a failed
    background; `mono`: the manga is black and white."""
    other = "left" if side == "right" else "right"
    with_ = [i + 1 for i, k in enumerate(kinds) if k == "with"]
    place = [i + 1 for i, k in enumerate(kinds) if k == "place"]
    out = [BG_FIXES[f] for f in dict.fromkeys(fixes) if f in BG_FIXES]
    if with_:
        out.append(f"{_images(with_)} {'is a panel' if len(with_) == 1 else 'are panels'} of the manga showing {name} in a place.")
    if place:
        out.append(f"{_images(place)} {'shows' if len(place) == 1 else 'show'} the same place in other panels around them.")
    out.append(f"Paint the background for {name}'s card on a video game's character-select screen: a wide, cinematic, fully painted "
               "background of that place, drawn in the manga's own art style (its line work, colouring and rendering), like a finished "
               "background painting from an official anime adaptation of the manga.")
    if notes.get("place"):
        out.append("The place: " + "; ".join(notes["place"]) + ".")
    if notes.get("seen"):
        out.append("From the manga: " + " ".join(notes["seen"]))
    if notes.get("tags"):
        out.append("Setting: " + ", ".join(notes["tags"]) + ".")
    look = []
    if notes.get("time"):
        look.append(f"time of day: {notes['time']}")
    if notes.get("weather"):
        look.append("weather: " + _join(notes["weather"]))
    if notes.get("light"):
        look.append("light: " + _join(notes["light"]))
    if notes.get("mood"):
        look.append(f"mood: {notes['mood']}")
    if look:
        out.append("As in the panels, " + "; ".join(look) + ".")
    out += [
        f"The picture is completely empty of people: no characters, no people, no faces, no silhouettes, no figures in the distance, no hands, "
        f"no creatures; leave {name} and everyone else out: only the place itself.",
        "No text anywhere: no letters, words, signs with writing, speech bubbles, captions, sound effects, emblems or brand marks, watermarks, signatures, "
        "user interface, panel borders or frames.",
        f"Composition: a wide establishing shot with depth: a clear foreground, middle ground and far distance, with atmospheric perspective "
        f"(haze and light in the air). The {side} third of the picture is calmer and simpler, with fewer details and softer contrast (a "
        f"character will stand in front of it there); the place's most interesting features sit in the centre and on the {other} side. The "
        "ground or floor runs along the bottom of the picture.",
    ]
    if colours:
        c = _join(list(colours))
        out.append(f"{name} wears {c}: keep the place's own colours, a little calmer and darker than the character, so a character in front "
                   f"of it stands out, in colours that sit well with {c} without copying them.")
    if mono:
        out.append("The manga is black and white: paint the place in full colour, the way an official anime adaptation would, with natural "
                   "colours that fit the time of day and the light.")
    out.append("Safe for work: no blood, no gore.")
    return "\n".join(out)


def check_backdrop(img):
    """Why a painted background can't be used: (reason, fix) with fix a BG_FIXES key, or None when
    it is good: wide (about 16:9), not blank or flat, not mostly lettering or blank paper."""
    w, h = img.size
    if h <= 0 or not 1.6 <= w / h <= 1.95:
        return "it is not a wide 16:9 picture", "wide"
    g = np.asarray(img.convert("L").resize((256, 144), Image.BILINEAR), np.float32)
    med = float(np.median(g))
    if g.std() < 14 or float((np.abs(g - med) < 6).mean()) > 0.6:
        return "it is blank or flat", "detail"
    if float(np.abs(np.diff(g, axis=1)).mean() + np.abs(np.diff(g, axis=0)).mean()) < 2.0:
        return "it is blank or flat", "detail"
    # Lettering and blank paper: tiles that are nearly all paper white and ink black (sampled, not
    # averaged, so thin strokes stay ink), with ink in them.
    t = np.asarray(img.convert("L").resize((688, 384), Image.NEAREST), np.float32)
    tiles = t.reshape(24, 16, 43, 16).transpose(0, 2, 1, 3).reshape(24 * 43, -1)
    white, black = (tiles > 215).mean(axis=1), (tiles < 70).mean(axis=1)
    paper = ((white + black) > 0.85) & (white > 0.45)
    lettering = paper & (black > 0.03)
    if float(paper.mean()) > 0.55 or float(lettering.mean()) > 0.2:
        return "it is mostly lettering or blank paper", "text"
    return None


# ─────────────────────────────────────────────────────────────── keeping

def kept_path(folder, cid):
    return os.path.join(folder, f"{cid}.png") if folder and cid else None


def read_json(path):
    try:
        with open(path, encoding="utf-8") as f:
            v = json.load(f)
        return v if isinstance(v, dict) else {}
    except (OSError, ValueError):
        return {}


def _write_atomic(path, data):
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, path)


def save_png_atomic(img, path):
    buf = io.BytesIO()
    img.save(buf, "PNG")
    _write_atomic(path, buf.getvalue())


def use_kept(png, work, cid):
    """The cut-out to show for a kept drawing: the PNG itself when it has transparency; an opaque
    one standing on a flat key colour keyed into `work`; otherwise the PNG as it is."""
    try:
        im = Image.open(png)
        im.load()
    except OSError:
        return None
    if im.mode in ("RGBA", "LA", "PA") or (im.mode == "P" and "transparency" in im.info):
        a = im.convert("RGBA").getchannel("A")
        if a.getextrema()[0] < 250:
            return png
    rgb = im.convert("RGB")
    arr = np.asarray(rgb, dtype=np.float32) / 255
    for colour in KEYS:
        B = background(arr, colour)
        if B is not None and strength(B, colour) >= 0.3:
            cut, info = key_out(rgb, colour)
            if cut is not None:
                os.makedirs(work, exist_ok=True)
                out = os.path.join(work, f"{cid}_keyed.png")
                cut.save(out)
                return out
    return png


def slug(name):
    s = re.sub(r"[^a-z0-9]+", "-", (name or "").lower()).strip("-")
    return s or "character"


# ─────────────────────────────────────────────────────────────── the cast

class Drawer:
    """Draws the cast and paints their backgrounds for one intro: knows whether making images is
    possible (a key, Studio's folder and the manga's intro folder `intro_dir`, and `cast_draw` not
    false) and where drawings and backgrounds are kept (<intro_dir>/cast)."""

    def __init__(self, a, client=None):
        self.a = a
        cache = a.get("intro_dir")
        self.folder = os.path.join(cache, "cast") if cache else None
        self.work = os.path.join(a.get("work_dir") or (self.folder or "."), "cast")
        self.studio = a.get("studio_dir")
        self.allowed = a.get("cast_draw", True) is not False
        self.key = api_key(self.studio) if (self.studio and self.folder and self.allowed) else None
        self.client = (client or Client(self.key)) if self.key else None
        self.budget = a.get("draw_budget")
        self.credits = []
        self.notes = []
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.reserved = 0.0
        self.ready = 0
        self.why_not = None
        if not self.folder:
            self.why_not = "no intro folder"
        elif not self.allowed:
            self.why_not = "drawing is off"
        elif not self.studio:
            self.why_not = "no Studio folder"
        elif not self.key:
            self.why_not = "no Magnific API key"

    @property
    def can(self):
        return self.client is not None and self.folder is not None and self.allowed

    def cid(self, c):
        return c.get("id") or slug(c.get("name"))

    def kept(self, c):
        p = kept_path(self.folder, self.cid(c))
        return p if p and os.path.isfile(p) else None

    def given_up(self, c):
        """Earlier exports could not draw this character (its drawings kept failing)."""
        if not self.folder:
            return False
        v = read_json(os.path.join(self.folder, f"{self.cid(c)}.json"))
        return int(v.get("failed_exports") or 0) >= MAX_FAILED_EXPORTS

    def drawable(self, c):
        """A card can show a drawing of `c` (a kept one, or one this run may make)."""
        return bool(self.kept(c)) or (self.can and not self.given_up(c))

    def kept_backdrop(self, c):
        """The character's kept background (<intro_dir>/cast/<id>.bg.png), or None."""
        p = os.path.join(self.folder, f"{self.cid(c)}.bg.png") if self.folder else None
        return p if p and os.path.isfile(p) else None

    def backdrop_given_up(self, c):
        """Earlier exports could not paint this character's background (it kept failing)."""
        if not self.folder:
            return False
        v = read_json(os.path.join(self.folder, f"{self.cid(c)}.bg.json"))
        return int(v.get("failed_exports") or 0) >= MAX_FAILED_EXPORTS

    def _afford(self):
        with self.lock:
            if self.budget is None:
                return True
            spent = sum(x["credits"] for x in self.credits)
            if spent + self.reserved + CREDITS > float(self.budget) + 1e-6:
                return False
            self.reserved += CREDITS
            return True

    def _paid(self, ok):
        with self.lock:
            if self.budget is not None:
                self.reserved = max(0.0, self.reserved - CREDITS)
            if ok:
                self.credits.append({"model": MODEL, "credits": CREDITS})

    def draw(self, cards, progress=None, span=(0.0, 1.0)):
        """Gives each card a clean drawing of its character: the kept one, else (when it can) a
        new one from Magnific, keyed and checked. Sets card["cutout"], card["drawn"],
        card["facing"]; a card whose drawing failed gets card["draw_fail"] (a short reason)."""
        set_p = progress.part(*span) if progress else (lambda *x, **k: None)
        todo = []
        for i, c in enumerate(cards):
            c.setdefault("facing_want", "left" if i % 2 == 0 else "right")
            png = self.kept(c)
            if png:
                cut = use_kept(png, self.work, self.cid(c))
                if cut:
                    meta = read_json(os.path.join(self.folder, f"{self.cid(c)}.json"))
                    c["cutout"], c["drawn"] = cut, True
                    if meta.get("facing") in ("left", "right") and meta.get("model"):
                        c["facing"] = meta["facing"]
                    set_p(0.0, f"Using the kept drawing of {c['name']}", "draw")
                    continue
            if c.get("reserve"):
                continue
            if not self.can:
                continue
            if self.given_up(c):
                c["draw_fail"] = "its drawings failed the checks in earlier exports"
                continue
            todo.append(c)
        self._parallel(todo, self._draw_one, set_p, "draw_fail", "drawing failed")

    def _parallel(self, todo, one, set_p, fail_key, fail_text):
        """Runs one(c, k, n, state, set_p) for every card in `todo` (at most MAX_PARALLEL at once),
        the bar moving with their shares of the work (state[id(c)], 0..1)."""
        if not todo:
            set_p(1.0)
            return
        os.makedirs(self.folder, exist_ok=True)
        os.makedirs(self.work, exist_ok=True)
        state = {id(c): 0.0 for c in todo}
        done = threading.Event()

        def tick():
            while not done.wait(0.3):
                set_p(sum(state.values()) / len(todo))

        ticker = threading.Thread(target=tick, daemon=True)
        ticker.start()
        threads = []
        sem = threading.Semaphore(MAX_PARALLEL)

        def run(c, k):
            with sem:
                try:
                    one(c, k, len(todo), state, set_p)
                except Exception as e:  # noqa: BLE001 - one character's failure is not the intro's
                    c[fail_key] = f"{fail_text} ({type(e).__name__})"
                finally:
                    state[id(c)] = 1.0

        for k, c in enumerate(todo):
            th = threading.Thread(target=run, args=(c, k + 1), daemon=True)
            th.start()
            threads.append(th)
        for th in threads:
            th.join()
        done.set()
        ticker.join()
        set_p(1.0)

    def _draw_one(self, c, k, n, state, set_p):
        name, cid = c["name"], self.cid(c)
        refs = references(c)
        if not refs:
            c["draw_fail"] = "there was no picture of the character to draw from"
            return
        order, _ = choose_colours([r[0] for r in refs], c.get("description"))
        # A key colour that showed on this character in an earlier export (a green shirt on a
        # green key) is tried last, so the next drawing does not fail the same way.
        avoid = self.leaked_colours(cid)
        order = [o for o in order if o not in avoid] + [o for o in order if o in avoid]
        colour = order[0]
        mono = monochrome([r[0] for r in refs])
        facing = c.get("facing_want") or "left"
        kinds = [r[2] for r in refs]
        payload = [(jpeg_b64(im), cap) for im, cap, _ in refs]
        # What earlier exports' drawings of this character got wrong is asked for from the start.
        known = self.earlier_fixes(cid)
        fixes = list(known)
        seen_fixes = list(known)
        reasons = []
        spent = 0.0
        used = [colour]
        leaked = []  # key colours that showed on the character in this run's drawings
        faults = 0  # drawings that failed because of the drawing itself (checks, content filter)
        for attempt in range(MAX_DRAWS):
            if self.stop.is_set():
                reasons.append(self.stop_reason)
                break
            if not self._afford():
                reasons.append("the drawing budget ran out")
                break
            set_p(None, f"Drawing {name} with Magnific ({k} of {n}{', second try' if attempt else ''})", "draw")
            prompt = build_prompt(name, c.get("description"), kinds, colour, facing, fixes, mono)
            base = state[id(c)]
            span = (1.0 - base) * (0.85 if attempt == 0 else 0.9)

            def waiting(sec, base=base, span=span):
                state[id(c)] = max(state[id(c)], base + span * (1 - math.exp(-sec / 45.0)))

            try:
                data = self.client.generate(prompt, payload, on_wait=waiting)
            except MagnificError as e:
                self._paid(False)
                if e.fatal:
                    self.stop_reason = e.short()
                    self.stop.set()
                reasons.append(e.short())
                if e.kind == "filter":
                    faults += 1
                    fixes = list(dict.fromkeys(known + ["safe"]))
                    continue
                break
            self._paid(True)
            spent += CREDITS
            try:
                raw = Image.open(io.BytesIO(data))
                raw.load()
                raw = raw.convert("RGB")
            except (OSError, ValueError):
                reasons.append("Magnific sent an unreadable image")
                continue
            try:
                raw.save(os.path.join(self.work, f"{cid}_raw_{attempt + 1}.png"))
            except OSError:
                pass
            set_p(None, f"Keying out {name}'s background", "key")
            cut, info = key_out(raw, colour)
            why = check(info) if cut is not None else ("the background could not be keyed out", "background")
            if why is None:
                self._keep(c, cid, raw, cut, colour, facing, spent, attempt + 1, prompt)
                state[id(c)] = 1.0
                with self.lock:
                    self.ready += 1
                    ready = self.ready
                set_p(None, f"{name} is drawn and keyed ({ready} of {n} ready)", "key")
                return
            reason, fix = why
            reasons.append(reason)
            faults += 1
            fixes = list(dict.fromkeys(known + [fix]))
            # The checks stop at the first problem, and the key colour showing on the character is
            # checked last: look for it on its own too, so the next drawing fixes both at once (a
            # figure cut off at the sides and a green shirt on the green key, say).
            if fix != "leak" and fix in ("cut", "figures", "shape", "small") and leaks(info):
                reasons.append(f"{KEY_NAMES[colour][0]} shows on the character")
                fixes.append("leak")
            seen_fixes += [f for f in fixes if f not in seen_fixes]
            if "leak" in fixes:
                leaked.append(colour)
                nxt = [o for o in order if o not in used]
                if nxt:
                    colour = nxt[0]
                    used.append(colour)
            if attempt + 1 < MAX_DRAWS:
                set_p(None, f"Drawing {name} again: {reason}", "draw")
        fail = "; ".join(dict.fromkeys(r for r in reasons if r)) or "the drawing failed"
        c["draw_fail"] = ("the drawings did not pass the checks (" + fail + ")") if faults and all(
            not r.startswith("Magnific") for r in reasons) else fail
        if faults >= MAX_DRAWS:
            self._remember_failure(cid, name, fail, leaked, seen_fixes)
        elif leaked or seen_fixes:
            self._remember_failure(cid, name, fail, leaked, seen_fixes, count=False)

    stop_reason = "drawing stopped"

    def _keep(self, c, cid, raw, cut, colour, facing, spent, draws, prompt):
        png = kept_path(self.folder, cid)
        try:
            save_png_atomic(raw, os.path.join(self.folder, f"{cid}.raw.png"))
        except OSError:
            pass
        save_png_atomic(cut, png)
        meta = {"name": c["name"], "id": cid, "model": MODEL, "key_colour": colour, "prompt_version": PROMPT_VERSION, "credits": spent,
                "draws": draws, "facing": facing, "made_at": int(time.time()), "size": list(cut.size)}
        _write_atomic(os.path.join(self.folder, f"{cid}.json"), json.dumps(meta, indent=1).encode("utf-8"))
        c["cutout"], c["drawn"], c["facing"] = png, True, facing

    def leaked_colours(self, cid):
        """Key colours that showed on this character in earlier drawings: kept in its failure
        record, and named in the last failure of records written before that was kept."""
        v = read_json(os.path.join(self.folder, f"{cid}.json")) if self.folder else {}
        out = [k for k in v.get("leaked_colours") or [] if k in KEYS]
        said = str(v.get("last_failure") or "")
        out += [k for k, (cname, _) in KEY_NAMES.items() if f"{cname} shows on the character" in said and k not in out]
        return out

    def earlier_fixes(self, cid):
        """The prompt fixes (FIXES keys but "leak", which a key colour change handles) that earlier
        exports' drawings of this character needed: kept in its failure record, and read from the
        last failure of records written before that was kept."""
        v = read_json(os.path.join(self.folder, f"{cid}.json")) if self.folder else {}
        out = [f for f in v.get("fixes") or [] if f in FIXES and f != "leak"]
        if "cut off" in str(v.get("last_failure") or "") and "cut" not in out:
            out.append("cut")
        return out

    def _remember_failure(self, cid, name, reason, leaked=(), fixes=(), count=True):
        """Notes in the character's record that this export's drawings failed (`count`), which
        key colours showed on the character (`leaked`) and which fixes they needed (`fixes`), so
        later exports avoid the colours and ask for the fixes from the first drawing."""
        p = os.path.join(self.folder, f"{cid}.json")
        if os.path.isfile(kept_path(self.folder, cid) or ""):
            return
        v = read_json(p)
        v.update({"name": name, "id": cid, "updated": int(time.time())})
        if count:
            v.update({"failed_exports": int(v.get("failed_exports") or 0) + 1, "last_failure": reason})
        if leaked:
            v["leaked_colours"] = sorted(set(v.get("leaked_colours") or []) | set(leaked))
        keep = [f for f in fixes if f in FIXES and f != "leak"]
        if keep:
            v["fixes"] = sorted(set(v.get("fixes") or []) | set(keep))
        try:
            _write_atomic(p, json.dumps(v, indent=1).encode("utf-8"))
        except OSError:
            pass

    # ── backgrounds

    def paint(self, cards, pages, progress=None, span=(0.0, 1.0)):
        """Gives each card a painted background of the place the manga shows its character in: the
        kept one (<id>.bg.png, mirrored when it was painted for the other side), else (when it
        can) a new one from Magnific, checked and kept. Sets card["backdrop"] (and
        card["backdrop_flip"]); a card whose background failed gets card["backdrop_fail"] and
        keeps the manga's establishing panel. `pages` is intro_maker's page index; each card's
        "side" says where its character stands."""
        set_p = progress.part(*span) if progress else (lambda *x, **k: None)
        self.pages = pages or {}
        todo = []
        for c in cards:
            png = self.kept_backdrop(c)
            if png:
                meta = read_json(os.path.join(self.folder, f"{self.cid(c)}.bg.json"))
                c["backdrop"] = png
                c["backdrop_flip"] = meta.get("side") in ("left", "right") and c.get("side") in ("left", "right") and meta["side"] != c["side"]
                set_p(0.0, f"Using the kept background of {c['name']}", "backdrop")
                continue
            if not self.can:
                continue
            if self.backdrop_given_up(c):
                c["backdrop_fail"] = "its backgrounds failed the checks in earlier exports"
                continue
            todo.append(c)
        self._parallel(todo, self._paint_one, set_p, "backdrop_fail", "painting the background failed")

    def _paint_one(self, c, k, n, state, set_p):
        name, cid = c["name"], self.cid(c)
        refs = backdrop_references(c, self.pages)
        if not refs:
            c["backdrop_fail"] = "no panel of the manga shows the place they are in"
            return
        notes = place_notes(refs, self.pages)
        mono = manga_monochrome(self.pages, refs)
        colours = colour_words(c.get("cutout"))
        side = c.get("side") if c.get("side") in ("left", "right") else "right"
        kinds = [r[2] for r in refs]
        payload = [(jpeg_b64(im), cap) for im, cap, _, _ in refs]
        fixes, reasons = [], []
        spent = 0.0
        faults = 0
        for attempt in range(MAX_BG_DRAWS):
            if self.stop.is_set():
                reasons.append(self.stop_reason)
                break
            if not self._afford():
                reasons.append("the image budget ran out")
                break
            again = ", second try" if attempt else ""
            set_p(None, f"Painting {name}'s background with Magnific ({k} of {n}{again})", "backdrop")
            prompt = backdrop_prompt(name, kinds, notes, side, colours, fixes, mono)
            base = state[id(c)]
            share = (1.0 - base) * (0.85 if attempt == 0 else 0.9)

            def waiting(sec, base=base, share=share):
                state[id(c)] = max(state[id(c)], base + share * (1 - math.exp(-sec / 45.0)))

            try:
                data = self.client.generate(prompt, payload, on_wait=waiting, aspect=BG_ASPECT)
            except MagnificError as e:
                self._paid(False)
                if e.fatal:
                    self.stop_reason = e.short()
                    self.stop.set()
                reasons.append(e.short())
                if e.kind == "filter":
                    faults += 1
                    fixes = ["safe"]
                    continue
                break
            self._paid(True)
            spent += CREDITS
            try:
                img = Image.open(io.BytesIO(data))
                img.load()
                img = img.convert("RGB")
            except (OSError, ValueError):
                reasons.append("Magnific sent an unreadable image")
                continue
            try:
                img.save(os.path.join(self.work, f"{cid}_bg_{attempt + 1}.png"))
            except OSError:
                pass
            why = check_backdrop(img)
            if why is None:
                self._keep_backdrop(c, cid, img, side, spent, attempt + 1, refs)
                state[id(c)] = 1.0
                with self.lock:
                    self.bg_ready += 1
                    ready = self.bg_ready
                set_p(None, f"{name}'s background is painted ({ready} of {n} ready)", "backdrop")
                return
            reason, fix = why
            reasons.append(reason)
            faults += 1
            fixes = [fix]
            if attempt + 1 < MAX_BG_DRAWS:
                set_p(None, f"Painting {name}'s background again: {reason}", "backdrop")
        fail = "; ".join(dict.fromkeys(r for r in reasons if r)) or "the background failed"
        checks = faults and all(not r.startswith("Magnific") for r in reasons)
        c["backdrop_fail"] = ("its backgrounds did not pass the checks (" + fail + ")") if checks else fail
        if faults >= MAX_BG_DRAWS:
            self._remember_backdrop_failure(cid, name, fail)

    bg_ready = 0
    pages = {}

    def _keep_backdrop(self, c, cid, img, side, spent, draws, refs):
        png = os.path.join(self.folder, f"{cid}.bg.png")
        save_png_atomic(img, png)
        meta = {"name": c["name"], "id": cid, "model": MODEL, "prompt_version": BG_PROMPT_VERSION, "credits": spent, "draws": draws, "side": side,
                "references": [{"image": src["image"], "box": src["box"], "kind": kind, "caption": cap} for _, cap, kind, src in refs],
                "made_at": int(time.time()), "size": list(img.size)}
        _write_atomic(os.path.join(self.folder, f"{cid}.bg.json"), json.dumps(meta, indent=1).encode("utf-8"))
        c["backdrop"], c["backdrop_flip"] = png, False

    def _remember_backdrop_failure(self, cid, name, reason):
        p = os.path.join(self.folder, f"{cid}.bg.json")
        if os.path.isfile(os.path.join(self.folder, f"{cid}.bg.png")):
            return
        v = read_json(p)
        v.update({"name": name, "id": cid, "failed_exports": int(v.get("failed_exports") or 0) + 1, "last_failure": reason, "updated": int(time.time())})
        try:
            _write_atomic(p, json.dumps(v, indent=1).encode("utf-8"))
        except OSError:
            pass
