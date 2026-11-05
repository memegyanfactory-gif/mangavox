#!/usr/bin/env python3
"""The intro's hand-off from the last cast card to the recap, the way Netflix opens a show.

As the last card ends, a light sweeps across it and its surface (the character and their backdrop
on the tilted card) turns out to be made of thin vertical ribbons of light in the cards' colours
(a smooth spectrum of related hues, a few white-hot); they grow up and down past the frame and part
from the middle while everything else goes dark; the camera dives through them, near ones wide,
soft and fast, far ones thin, dim and slow, the lens bending them as it rushes, until they clear
from the centre outward onto the recap's first frame (REVEAL_SEC), which settles from a slight zoom.

The ribbons are lit strips in 3D seen through a moving pinhole camera. Each is separable (a profile
down its length times a cross-section), so all of them render as one matrix product, with analytic
motion blur (a box swept over the shutter, softened) and a glow per ribbon. Seeded from any frame
and a mask of what turns into light (RibbonDive), so it is the same everywhere it is used.

The sound is designed to the picture (all synthesised: nothing to license): a glassy shimmer that
runs across with the light, a riser into a whoosh that peaks as the camera passes through the
ribbons (PASS_SEC, a low thump under it), and a soft, airy release as the recap lands.
numpy + Pillow.
"""
import colorsys
import math

import numpy as np
from PIL import Image, ImageFilter

RIBBON_SEC = 1.2
REVEAL_SEC = 0.5
PASS_SEC = 0.768  # seconds into the transition when the camera passes through the ribbons
SR = 48000

# Moments of the dive, as fractions of RIBBON_SEC (the picture and the sound share them).
LIGHT_U = (0.0, 0.26)  # the light sweeps across the card, which turns into lines of light
GROW_U = (0.2, 0.46)   # each line grows up and down past the frame from the card
SPLIT_U = (0.26, 0.58)  # ...then the lines part from the middle into the corridor's walls
PASS_U = PASS_SEC / RIBBON_SEC  # the camera passes through the card's ribbons (the whoosh peaks)
OPEN_U = 0.76          # the centre clears onto the recap...
OPEN_END_U = 1.16      # ...and it is all there


def total_seconds():
    """The transition's length, from the last card's end to the recap's first frame."""
    return RIBBON_SEC + REVEAL_SEC


def sstep(a, b, x):
    """Smoothstep from a to b (scalars or arrays; a > b runs it backwards)."""
    t = np.clip((np.asarray(x, np.float32) - a) / (b - a), 0.0, 1.0)
    return t * t * (3 - 2 * t)


def hex_rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


def rgb_hex(c):
    return "#%02x%02x%02x" % tuple(int(max(0, min(255, round(v)))) for v in c)


# ─────────────────────────────────────────────────────────────── colours

def _olive(h):
    """Hues that turn olive or khaki when dim (yellow-green): the ribbons' gradient avoids them."""
    return 58 / 360 < h % 1 < 105 / 360


def ribbon_palette(pal):
    """(stops, hot): the ribbons' gradient (sRGB 0..1 colours in order, a smooth path through
    related hues) and their white-hot colour. The palette's saturated colours, grouped by hue
    and each group fanned out to its neighbours (a deeper hue on one side, a lighter on the
    other); two groups (green and pink, say) meet through a pale, white-hot middle."""
    cols = []
    for k in ("primary", "glow", "secondary", "accent", "light"):
        try:
            cols.append(hex_rgb(pal[k]))
        except (KeyError, ValueError, TypeError, AttributeError, IndexError):
            pass
    if not cols:
        cols = [(215, 31, 38), (255, 90, 70)]  # a warm red
    chroma, light = [], None
    for c in cols:
        h, l, s = colorsys.rgb_to_hls(*(v / 255 for v in c))
        if s > 0.28 and 0.1 < l < 0.86 and not (70 / 360 < h < 105 / 360):
            chroma.append((h, l, s))
        elif l >= 0.5 and light is None:
            light = (h, l, s)
    if not chroma:
        yellowish = any(_olive(colorsys.rgb_to_hls(*(v / 255 for v in c))[0]) for c in cols)
        chroma = [(45 / 360, 0.52, 0.95)] if yellowish else [(0.99, 0.47, 0.8)]
    chroma.sort(key=lambda c: c[0])
    groups = []
    for c in chroma:
        for g in groups:
            if abs(((c[0] - g[0][0]) + 0.5) % 1 - 0.5) < 0.11:
                g.append(c)
                break
        else:
            groups.append([c])
    if len(groups) > 1 and abs(((groups[0][0][0] - groups[-1][-1][0]) + 0.5) % 1 - 0.5) < 0.11:
        groups[0] = groups.pop() + groups[0]
    groups.sort(key=lambda g: -len(g))
    groups = groups[:2]

    def gold(h):
        return 50 / 360 if 52 / 360 < h % 1 < 70 / 360 else h   # lemon reads greenish in light: gold

    def fan(g):
        g = [(gold(h), l, s) for h, l, s in g]
        base = g[0][0]
        rel = sorted(((h - base + 0.5) % 1 - 0.5, l, s) for h, l, s in g)
        lo, hi = base + rel[0][0], base + rel[-1][0]
        ext = max(0.06, 0.11 - (hi - lo) / 2)   # a single hue fans out wider
        deep, bright = lo - ext, hi + ext * 0.8
        out = []
        if not _olive(deep):
            out += [(deep % 1, 0.4, 0.95), ((lo - ext / 2) % 1, 0.45, 0.95)]
        for r, l, s in rel:
            out.append(((base + r) % 1, min(0.58, max(0.46, l)), max(0.88, s)))
        if not _olive(bright) and not _olive(hi + ext * 0.4):
            out += [((hi + ext * 0.4) % 1, 0.52, 0.95), (bright % 1, 0.58, 0.95)]
        else:
            out.append((hi % 1, 0.64, 0.95))
        return [colorsys.hls_to_rgb(*c) for c in out]

    hot_h, hot_s = (light[0], min(0.5, light[2])) if light else (groups[0][0][0], 0.35)
    hot = colorsys.hls_to_rgb(hot_h, 0.94, hot_s)
    if len(groups) == 1:
        stops = fan(groups[0])
    else:
        a, b = (_even(fan(g), 5) for g in groups)
        stops = a + [colorsys.hls_to_rgb(hot_h, 0.8, max(0.5, hot_s))] + b[::-1]
    return stops, hot


def _even(stops, n):
    """The stops resampled to n, evenly along the gradient."""
    return [tuple(float(v) for v in c) for c in gradient(stops, n)]


def gradient(stops, n=256):
    """n colours along the stops (sRGB 0..1), hue-interpolated between saturated neighbours."""
    out = np.zeros((n, 3), np.float32)
    m = len(stops)
    if m == 1:
        out[:] = stops[0]
        return out
    for i in range(n):
        p = i / (n - 1) * (m - 1)
        j = min(int(p), m - 2)
        f = p - j
        a, b = stops[j], stops[j + 1]
        ha, la, sa = colorsys.rgb_to_hls(*a)
        hb, lb, sb = colorsys.rgb_to_hls(*b)
        if sa > 0.3 and sb > 0.3 and la < 0.8 and lb < 0.8:
            dh = ((hb - ha + 0.5) % 1) - 0.5
            out[i] = colorsys.hls_to_rgb((ha + dh * f) % 1, la + (lb - la) * f, sa + (sb - sa) * f)
        else:
            out[i] = [a[q] + (b[q] - a[q]) * f for q in range(3)]
    return out


def deepen(rgb):
    """A deeper, more saturated neighbour of each colour, for the ribbons' glow and their dim
    parts (a dim yellow reads as olive, a dim orange as brown: they drift to amber and red)."""
    out = np.zeros_like(rgb)
    for i, c in enumerate(np.clip(rgb, 0, 1)):
        h, l, s = colorsys.rgb_to_hls(*c)
        if s < 0.25:
            out[i] = c * 0.6
            continue
        h2 = h - 0.07 if 0.1 < h < 0.2 else h - 0.035 if (h < 0.1 or h > 0.85) else h + 0.03
        out[i] = colorsys.hls_to_rgb(h2 % 1, max(0.25, l * 0.75), min(1.0, s * 1.1))
    return out


# ─────────────────────────────────────────────────────────────── the source

def mask_box(mask, w, h):
    """The box (x0, y0, x1, y1, px) of what turns into light; the middle of the frame when empty."""
    ys, xs = np.nonzero(mask > 0.3)
    if len(xs) < 16:
        return int(w * 0.3), int(h * 0.15), int(w * 0.7), int(h * 0.85)
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def behind(img, m):
    """The frame with the masked part taken out: its pixels filled in from around it, blurred
    (normalised blurs at a few scales of a small copy)."""
    h, w = img.shape[:2]
    sw, sh = max(8, w // 8), max(8, h // 8)
    small = np.asarray(Image.fromarray(img).resize((sw, sh), Image.BILINEAR), np.float32)
    keep = 1 - np.asarray(Image.fromarray((m * 255).astype(np.uint8)).resize((sw, sh), Image.BILINEAR), np.float32) / 255
    keep = np.clip(keep * 1.4 - 0.4, 0, 1)
    acc = np.zeros_like(small)
    wsum = np.zeros((sh, sw), np.float32)
    for r in (3, 8, 20, 48):
        den = np.asarray(Image.fromarray((keep * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(r)), np.float32) / 255
        num = np.asarray(Image.fromarray(np.clip(small * keep[..., None], 0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(r)), np.float32)
        wgt = np.clip(den * 4, 0, 1) * (1 - np.clip(wsum, 0, 1))
        acc += num / np.maximum(den, 1e-3)[..., None] * wgt[..., None]
        wsum += wgt
    acc /= np.maximum(wsum, 1e-3)[..., None]
    fill = np.asarray(Image.fromarray(np.clip(acc, 0, 255).astype(np.uint8)).resize((w, h), Image.BILINEAR), np.float32)
    return np.clip(img.astype(np.float32) * (1 - m[..., None]) + fill * m[..., None], 0, 255).astype(np.uint8)


# ─────────────────────────────────────────────────────────────── picture

def _erf(x):
    """erf (Abramowitz & Stegun 7.1.26, |error| < 1.5e-7) of a float32 array."""
    a = np.abs(x)
    t = 1.0 / (1.0 + 0.3275911 * a)
    y = 1.0 - ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * np.exp(-a * a)
    return np.copysign(y, x)


def softbox(xs, a, b, hw, sig):
    """Ribbon cross-sections (one row each): a box of half width `hw` swept from centre a to b
    over the shutter, edges softened by `sig`. Peak 1 when still; it keeps its energy (grows
    dimmer as it smears) when it moves."""
    lo = np.minimum(a, b) - hw
    hi = np.maximum(a, b) + hw
    k = 1.0 / (1.41421356 * sig)
    amp = (hw / (hi - lo)) / _erf(hw * k)
    return amp * (_erf((xs - lo) * k) - _erf((xs - hi) * k))


def glow(xs, a, b, sig):
    """A soft gaussian glow (one row each) swept from a to b, its width grown to match the smear."""
    n = np.abs(b - a)
    se = np.sqrt(sig * sig + n * n / 12.0)
    return (sig / se) * np.exp(-((xs - 0.5 * (a + b)) ** 2) / (2 * se * se))


class RibbonDive:
    """The source frame (`source`, uint8 H x W x 3: the last card) turning into ribbons of light
    where `mask` (float H x W, 0..1: the card and the character) is, the dive through them and the
    recap's first frame (`reveal`, or the palette's dark) settling into place. frame(t) for t in
    (0, RIBBON_SEC + REVEAL_SEC]; deterministic for a given seed."""

    def __init__(self, w, h, source, mask, palette, reveal=None, fps=30.0, seed=5):
        self.w, self.h, self.fps = w, h, float(fps)
        rng = np.random.default_rng(seed)
        self.F = F = 0.95 * w  # the pinhole's focal length (px)
        self.px = px = w / 1920.0  # pixel sizes below are tuned at 1080p
        pal = palette if isinstance(palette, dict) else {}
        src = np.zeros((h, w, 3), np.uint8) if source is None else np.ascontiguousarray(np.asarray(source, np.uint8)[:h, :w, :3])
        if src.shape[:2] != (h, w):
            src = np.asarray(Image.fromarray(src).resize((w, h), Image.BILINEAR))
        if mask is None:
            mask = np.zeros((h, w), np.float32)
        mask = np.clip(np.asarray(mask, np.float32), 0, 1)
        if mask.shape != (h, w):
            mask = np.asarray(Image.fromarray((mask * 255).astype(np.uint8)).resize((w, h), Image.BILINEAR), np.float32) / 255
        if float(mask.max(initial=0)) < 0.3:  # nothing to turn into light: the middle of the frame does
            bx = mask_box(mask, w, h)
            mask = np.zeros((h, w), np.float32)
            mask[bx[1]:bx[3], bx[0]:bx[2]] = 1
        bx = mask_box(mask, w, h)
        self.bx = bx
        lx, ly = (bx[0] + bx[2]) / 2, (bx[1] + bx[3]) / 2
        self.lx, self.ly = lx, ly
        bw = bx[2] - bx[0]
        stops, hot = ribbon_palette(pal)
        base = gradient(stops)
        self.lut = base ** 2                      # linear light
        self.lut_deep = deepen(base) ** 2
        self.hot = np.array(hot, np.float32) ** 2
        try:
            dk = hex_rgb(pal.get("dark") or "#08060a")
        except (ValueError, TypeError, IndexError, AttributeError):
            dk = (8, 6, 10)
        self.dark = (np.array(dk, np.float32) / 255) ** 2 * 0.45
        # The source, the source with the card taken out (dimmed: shown where the card turns to
        # light) and the card's mask.
        self.src_rgb = Image.fromarray(src)
        self.src_mask = Image.fromarray((mask * 255).astype(np.uint8)).filter(ImageFilter.MaxFilter(9)).filter(ImageFilter.GaussianBlur(3))
        bare = behind(src, np.asarray(self.src_mask, np.float32) / 255).astype(np.float32) * math.sqrt(0.6)
        self.src_bare = Image.fromarray((bare + 0.5).astype(np.uint8))
        # The camera's depth along the dive: easing in, through the card at PASS_U, easing out
        # under the reveal.
        us = np.linspace(0, 1.6, 1601)
        v = 0.12 + sstep(0.05, 0.64, us) + 0.25 * sstep(0.5, 0.85, us) - sstep(0.92, 1.3, us)
        v = np.maximum(v, 0.04)
        cz = np.concatenate([[0], np.cumsum((v[1:] + v[:-1]) / 2 * np.diff(us))])
        cz = cz / cz[np.searchsorted(us, PASS_U)]
        self.cam_u, self.cam_z = us, cz
        # ── the card's columns: its surface as thin ribbons of light
        K = int(np.clip(round(bw / 4.6), 40, 330))   # thin columns, about 4.6 px apart at 1080p
        dx = bw / K
        xc = bx[0] + (np.arange(K) + 0.5) * dx
        self.xc = xc
        cs = np.concatenate([np.zeros((h, 1), np.float32), np.cumsum(mask, axis=1)], axis=1)
        lum = src.max(axis=2).astype(np.float32) / 255   # the surface's value: its shading
        csl = np.concatenate([np.zeros((h, 1), np.float32), np.cumsum(lum * mask, axis=1)], axis=1)
        lo = np.clip(np.floor(xc - dx / 2).astype(int), 0, w - 1)
        hi = np.maximum(np.clip(np.floor(xc + dx / 2).astype(int), 1, w), lo + 1)
        mcol = (cs[:, hi] - cs[:, lo]) / (hi - lo)
        shade = (csl[:, hi] - csl[:, lo]) / np.maximum(1e-4, cs[:, hi] - cs[:, lo])
        ref = float(np.percentile(shade[mcol > 0.5], 90)) if (mcol > 0.5).any() else 1.0
        scol = np.clip(shade / max(ref, 1e-3), 0, 1.25) ** 2.2   # the surface's relief
        inside = mcol > 0.3
        has = inside.any(axis=0)
        self.top0 = np.where(has, np.argmax(inside, axis=0), ly).astype(np.float32)
        self.bot0 = np.where(has, h - 1 - np.argmax(inside[::-1], axis=0), ly).astype(np.float32)
        self.mcol = np.vstack([mcol, np.zeros((2, K))]).astype(np.float32)
        self.scol = np.vstack([scol, np.zeros((2, K))]).astype(np.float32)
        self.shine = (0.55 + 0.45 * scol.max(axis=0)).astype(np.float32)
        Zl = 1.0 + rng.uniform(-0.1, 0.3, K)
        Xl = (xc - lx) / F * Zl
        order = (xc - bx[0]) / max(1, bw)
        hero = rng.random(K) < 0.26 * min(1.0, 170.0 / K)   # about the same number of lines survive however wide the card
        thin = np.where(rng.random(K) < 0.4, rng.uniform(0.8, 1.6, K), rng.uniform(0.12, 0.3, K))
        # ── the corridor: ribbons in depth on both sides, in bundles (wide soft bands and thin hot
        # filaments), each placed by where it sits on screen when it is brightest, out of the middle
        X, Z, HW, SG, B, CU, YT, YB = [], [], [], [], [], [], [], []
        nb = 24
        for b in range(nb):
            side = 1 if b % 2 == 0 else -1
            Zb = 1.45 + 4.6 * (b + rng.uniform(0, 1)) / nb
            off = (190 + rng.uniform(0, 1) ** 1.2 * 700) * px
            Xb = side * off * 1.6 / F
            cb = 0.5 + side * (0.2 + 0.3 * (off / px - 190) / 700)
            for _ in range(int(rng.integers(2, 5))):
                band = rng.random() < 0.35
                X.append(Xb + side * abs(rng.normal(0, 0.02)))
                Z.append(Zb + rng.normal(0, 0.05))
                HW.append((rng.uniform(4, 14) if band else rng.uniform(0.5, 1.3)) * px / F)
                SG.append((rng.uniform(1.5, 3.0) if band else rng.uniform(0.5, 0.9)) * max(px, 0.5))
                B.append(rng.uniform(0.45, 0.9) if band else math.exp(rng.normal(0.6, 0.45)))
                CU.append(float(np.clip(cb + rng.normal(0, 0.03), 0, 1)))
                YT.append(-rng.uniform(0.35, 0.9))
                YB.append(rng.uniform(0.35, 0.9))
        for _ in range(10):  # loose, far, faint ones
            side = 1 if rng.random() < 0.5 else -1
            off = (120 + rng.uniform(0, 1) * 500) * px
            X.append(side * off * 1.6 / F)
            Z.append(rng.uniform(2.6, 6.0))
            HW.append(rng.uniform(0.4, 0.8) * px / F)
            SG.append(0.6 * max(px, 0.5))
            B.append(math.exp(rng.normal(-0.4, 0.3)))
            CU.append(float(np.clip(0.5 + side * (0.15 + 0.3 * rng.uniform()), 0, 1)))
            YT.append(-rng.uniform(0.3, 0.7))
            YB.append(rng.uniform(0.3, 0.7))
        self.yt, self.yb = np.array(YT, np.float32), np.array(YB, np.float32)
        self.K, self.nn = K, K + len(X)
        self.Z = np.concatenate([Zl, Z]).astype(np.float32)
        self.X0 = np.concatenate([Xl, X]).astype(np.float32)
        self.hw_card = (dx * 0.36 / F * Zl).astype(np.float32)
        self.hw1 = np.concatenate([self.hw_card * thin, HW]).astype(np.float32)
        self.sg = np.concatenate([np.full(K, 0.7 * max(px, 0.5)), SG]).astype(np.float32)
        self.b0 = np.concatenate([np.exp(rng.normal(0.45, 0.3, K)), B]).astype(np.float32)
        self.cu = np.concatenate([np.clip(0.04 + 0.92 * order + rng.normal(0, 0.02, K), 0, 1), CU]).astype(np.float32)
        self.hotk = np.where(rng.random(self.nn) < 0.08, rng.uniform(0.55, 0.85, self.nn), rng.uniform(0.04, 0.1, self.nn)).astype(np.float32)
        self.hotk[K:][np.array(HW) * F > 3] *= 0.3
        self.hero = hero.astype(np.float32)
        self.phase = rng.uniform(0, 1, self.nn).astype(np.float32)
        self.vfreq = rng.uniform(0.35, 0.9, self.nn).astype(np.float32)
        self.order = order.astype(np.float32)
        self.delay = (np.abs(order - 0.5) * 0.12 + rng.uniform(0, 0.015, K)).astype(np.float32)  # the middle first
        self.spread = rng.uniform(0.3, 1.0, K).astype(np.float32)
        self.jit = rng.normal(0, 0.006, K).astype(np.float32)
        self.xs = np.arange(w, dtype=np.float32)[None, :]
        self.ys = np.arange(h, dtype=np.float32)
        self.yn = (self.ys / h)[:, None]
        self.reveal = None
        if reveal is not None:
            r = reveal if isinstance(reveal, Image.Image) else Image.fromarray(np.ascontiguousarray(np.asarray(reveal, np.uint8)[..., :3]))
            r = r.convert("RGB")
            self.reveal = r if r.size == (w, h) else r.resize((w, h), Image.LANCZOS)
        yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
        vig = np.clip(1.15 - 0.8 * (((xx - w / 2) / (w * 0.6)) ** 2 + ((yy - h / 2) / (h * 0.72)) ** 2), 0.2, 1.15)
        dark_field = (self.dark[:, None, None] * vig[None]).astype(np.float32)
        self.dark_img = Image.fromarray(np.ascontiguousarray((np.sqrt(dark_field) * 255 + 0.5).astype(np.uint8).transpose(1, 2, 0)))
        self.black_img = Image.new("RGB", (w, h), (0, 0, 0))
        self.centre_glow = np.exp(-(((xx - w / 2) / (w * 0.09)) ** 2) - (((yy - h / 2) / (h * 0.42)) ** 2)).astype(np.float32)
        self.sq = ((np.arange(256, dtype=np.float32) / 255) ** 2).astype(np.float32)
        self.dark_lin = np.ascontiguousarray(self.sq[np.asarray(self.dark_img)].transpose(2, 0, 1))
        self.end_u = (RIBBON_SEC + REVEAL_SEC) / RIBBON_SEC

    def cam(self, u):
        return float(np.interp(u, self.cam_u, self.cam_z))

    def gap(self, u):
        """Half width (px) of the clearing in the middle that opens onto the recap."""
        p = float(np.clip((u - OPEN_U) / (OPEN_END_U - OPEN_U), 0, 1))
        return self.w * 0.7 * p ** 1.6

    def geometry(self, u):
        """Every ribbon's screen x, half width, edge softness (px) and brightness at progress u."""
        K = self.K
        cz = self.cam(u)
        cx = self.lx + (self.w / 2 - self.lx) * float(sstep(0.2, 0.7, u))
        split = sstep(SPLIT_U[0], SPLIT_U[1], u - self.delay)
        grow = sstep(GROW_U[0], GROW_U[1], u - self.delay)
        X = self.X0.copy()
        side = np.sign(X[:K]) + (X[:K] == 0)
        # The lines part from the card's centre line outward into the corridor's walls.
        X[:K] = X[:K] * (1 + 0.5 * self.spread * split) + (side * 0.07 + self.jit) * split ** 1.4
        d = self.Z - cz
        dd = np.maximum(d, 0.015)
        x = cx + self.F * X / dd
        hw = self.hw1.copy()
        hw[:K] = self.hw_card + (self.hw1[:K] - self.hw_card) * split
        hw = self.F * hw / dd
        sig = self.sg + 9.0 * self.px * np.clip(0.3 - d, 0, 0.3) / 0.3   # near ones out of focus
        bright = self.b0 * sstep(0.02, 0.15, d)                  # very near ones fade as they pass
        far = sstep(4.2, 2.6, d) * np.exp(-0.28 * np.maximum(d, 0))
        bright[K:] *= far[K:] * float(sstep(0.26, 0.52, u))
        bright *= 1 - float(sstep(OPEN_END_U - 0.02, OPEN_END_U + 0.1, u))   # gone once the recap is all there
        bright[:K] *= self.hero + (1 - self.hero) * (1 - sstep(0.05, 0.45, grow))  # the fine lines thin out to the heroes
        g = self.gap(u)
        if g > 0:
            dxs = x - self.w / 2
            x = self.w / 2 + np.sign(dxs) * np.sqrt(dxs * dxs + g * g)
        return x, hw, sig, bright, d, grow, split, cz

    def lens(self, img, k, strips=24):
        """A pincushion bend: rows far from the middle squeeze toward the centre line in the source,
        so vertical ribbons flare outward toward the top and bottom of the frame."""
        w, h = self.w, self.h
        cx = w / 2
        mesh = []
        for i in range(strips):
            y0, y1 = h * i / strips, h * (i + 1) / strips
            s0 = 1 + k * ((y0 - h / 2) / (h / 2)) ** 2
            s1 = 1 + k * ((y1 - h / 2) / (h / 2)) ** 2
            mesh.append(((0, int(round(y0)), w, int(round(y1))), (cx - cx / s0, y0, cx - cx / s1, y1, cx + cx / s1, y1, cx + cx / s0, y0)))
        return img.transform((w, h), Image.MESH, mesh, Image.BILINEAR)

    def light(self, u):
        """The ribbons' light (linear, channels first) and the card's light-up state."""
        w, h, K = self.w, self.h, self.K
        du = 0.3 / self.fps / RIBBON_SEC  # a 108-degree shutter
        xa = self.geometry(u - du / 2)[0]
        xb = self.geometry(u + du / 2)[0]
        x, hw, sig, bright, d, grow, split, cz = self.geometry(u)
        cy = self.ly + (h / 2 - self.ly) * float(sstep(0.2, 0.7, u))
        ys, yn = self.ys[:, None], self.yn
        # The light sweeps across the card (a hot front, the lit lines behind it).
        front = (u - LIGHT_U[0]) / (LIGHT_U[1] - LIGHT_U[0] - 0.06)
        lit = sstep(0, 1, (front - self.order) / 0.16)
        flare = np.exp(-(((front - self.order) / 0.045) ** 2)) * (front < 1.3)
        # The card as the columns see it (on the card's plane, pushed in with the camera)...
        ysrc = self.ly + (ys - cy) * (d[:K] / self.Z[:K])[None, :]
        ok = (ysrc >= 0) & (ysrc < h - 1)
        ysrc = np.clip(ysrc, 0, h - 1.001)
        i0 = ysrc.astype(np.int32)
        fr = ysrc - i0
        cols = np.arange(K)[None, :]
        mc = self.mcol[i0, cols] * (1 - fr) + self.mcol[i0 + 1, cols] * fr
        scc = self.scol[i0, cols] * (1 - fr) + self.scol[i0 + 1, cols] * fr
        surface = mc * scc * ok
        # ...and each column growing into a line from the card's top and bottom, past the frame.
        g12 = grow ** 1.2
        kk = self.Z[:K] / np.maximum(d[:K], 0.02)
        top = cy + (self.top0 - self.ly - g12 * 2.2 * h) * kk
        bot = cy + (self.bot0 - self.ly + g12 * 2.2 * h) * kk
        soft = (14 + 90 * g12 * (1 - g12)) * self.px
        line = sstep(top - soft, top + soft, ys) * sstep(bot + soft, bot - soft, ys)
        tips = np.exp(-(((ys - top) / (40.0 * self.px)) ** 2)) + np.exp(-(((ys - bot) / (40.0 * self.px)) ** 2))
        line = line * self.shine[None, :] + 0.9 * tips * (g12 * (1 - g12) * 4)[None, :]
        # Light runs along every ribbon; the middle of the frame is a little brighter.
        grade = 0.4 + 0.6 * np.exp(-(((self.ys - cy) / (0.42 * h)) ** 2))[:, None]
        env = grade * (0.3 + 0.7 * (0.5 + 0.5 * np.cos(2 * np.pi * (yn * self.vfreq[None, :] + self.phase[None, :] - 0.9 * u))) ** 1.5)
        # The ribbons in depth have ends: far ones are short, near ones span the frame.
        dd = np.maximum(d[K:], 0.05)
        t_d, b_d = cy + self.F * self.yt / dd, cy + self.F * self.yb / dd
        env[:, K:] *= sstep(t_d - 0.3 * (cy - t_d), t_d + 0.3 * (cy - t_d), ys) * sstep(b_d + 0.3 * (b_d - cy), b_d - 0.3 * (b_d - cy), ys)
        fade_surface = 1 - sstep(0.06, 0.4, grow)
        settle = sstep(0.5, 1.0, grow)
        show = sstep(0.0, 0.18, grow)[None, :]
        env[:, :K] = 0.48 * surface * (lit + flare) * fade_surface + line * show * (1 - settle) * lit + env[:, :K] * settle * line
        env *= bright[None, :]
        # Only the ribbons that can be seen are drawn.
        reach = np.abs(xb - xa) + hw + 3 * (hw + 60 * self.px)
        vis = (env.max(axis=0) > 1e-3) & (x + reach > 0) & (x - reach < w)
        idx = np.nonzero(vis)[0]
        L = np.zeros((3, h, w), np.float32)
        if len(idx):
            e = env[:, idx]
            ci = np.clip(self.cu[idx] * 255, 0, 255).astype(int)
            k_ = np.clip(bright[idx] / 0.9, 0, 1)[None, :]
            deep = self.lut_deep[ci].T
            ct = deep * (1 - k_) + self.lut[np.clip(ci + 18, 0, 255)].T * k_    # graded along the length
            cb = deep * (1 - k_) + self.lut[np.clip(ci - 18, 0, 255)].T * k_
            n = len(idx)
            V = np.empty((3, h, 2 * n), np.float32)
            V[:, :, :n] = e[None] * (ct[:, None, :] * (1 - yn)[None] + cb[:, None, :] * yn[None])
            V[:, :, n:] = e[None] * deep[:, None, :]
            a_, b_, hw_, sg_ = xa[idx, None], xb[idx, None], np.maximum(hw[idx, None], 0.15), sig[idx, None]
            core = softbox(self.xs, a_, b_, hw_, sg_)
            pack = np.ones(n, np.float32)              # packed card columns would glow into a blur
            lg = idx < K
            pack[lg] = 0.03 + 0.97 * split[idx[lg]] ** 1.5
            halo = (0.11 * pack)[:, None] * glow(self.xs, a_, b_, hw_ + 14 * self.px)
            halo += (0.022 * pack)[:, None] * glow(self.xs, a_, b_, np.full((n, 1), 60 * self.px, np.float32))
            P = np.concatenate([core, halo], axis=0)
            L = (V.reshape(3 * h, -1) @ P).reshape(3, h, w)
            hotv = e * self.hotk[idx][None, :]
            hotv[:, lg] += e[:, lg] * (0.8 * flare[idx[lg]] * (1 - settle[idx[lg]]))[None, :]
            L += (hotv @ glow(self.xs, a_, b_, 0.35 * hw_ + 0.4 * sg_))[None] * self.hot[:, None, None]
        c = float(sstep(0.35, 0.72, u) * (1 - sstep(OPEN_U - 0.04, OPEN_U + 0.12, u)))   # light at the end of the corridor
        if c > 0:
            L += (0.09 * c) * self.centre_glow[None] * (0.6 * self.lut[128] + 0.4 * self.hot)[:, None, None]
        return L, lit, grow, cz

    def frame(self, t):
        """The frame (uint8 H x W x 3) at t seconds into the transition."""
        w, h = self.w, self.h
        u = t / RIBBON_SEC
        L, lit, grow, cz = self.light(u)
        # The source: pushed in with the camera, the card's pixels giving way to its light (what
        # was behind shows as the lines part), going dark toward the palette's dark with a vignette.
        # (Composited in 8 bits with Pillow, then taken to linear light once.)
        dk = float(0.35 * sstep(0.0, 0.22, u) + 0.65 * sstep(0.2, 0.38, u))
        layer = self.dark_img
        if dk < 1:
            s = 1.0 / max(0.05, 1.0 - min(cz, 0.55))
            x0, y0 = self.lx - self.lx / s, self.ly - self.ly / s
            ext = (x0, y0, x0 + w / s, y0 + h / s)
            wi = self.src_rgb.transform((w, h), Image.EXTENT, ext, Image.BILINEAR)
            wb = self.src_bare.transform((w, h), Image.EXTENT, ext, Image.BILINEAR)
            m = np.asarray(self.src_mask.transform((w, h), Image.EXTENT, ext, Image.BILINEAR), np.float32)
            litx = np.interp((np.arange(w, dtype=np.float32) - self.lx) / s + self.lx, self.xc, lit, left=0, right=0)
            gone = m * (litx * (1 - 0.3 * (1 - min(1.0, 2.5 * float(grow.mean())))))[None, :]
            layer = Image.composite(wb, wi, Image.fromarray(gone.astype(np.uint8)))
            if dk > 0.002:
                layer = Image.blend(layer, self.dark_img, dk)
        # The recap through the clearing in the middle, settling from a slight zoom.
        g = self.gap(u)
        if g > 0:
            pz = float(np.clip((u - OPEN_U) / (self.end_u - OPEN_U), 0, 1))
            if self.reveal is not None:
                z = 1 + 0.07 * (1 - pz) ** 2
                cw, ch = w / z, h / z
                rec = self.reveal if z < 1.0005 else self.reveal.transform((w, h), Image.EXTENT, ((w - cw) / 2, (h - ch) / 2, (w + cw) / 2, (h + ch) / 2), Image.BILINEAR)
            else:
                rec = Image.blend(self.dark_img, self.black_img, pz)   # no recap: the dark, easing to black
            soft = (70 * self.px + 0.3 * g) * (1 - 0.7 * min(1.0, (u - OPEN_U) / (OPEN_END_U - OPEN_U)))
            gm = (sstep(g, g - soft, np.abs(np.arange(w, dtype=np.float32) - w / 2)) * 255 + 0.5).astype(np.uint8)
            layer = rec if gm.min() == 255 else Image.composite(rec, layer, Image.fromarray(np.ascontiguousarray(np.broadcast_to(gm, (h, w)))))
        bg = self.dark_lin if layer is self.dark_img else np.ascontiguousarray(self.sq[np.asarray(layer)].transpose(2, 0, 1))
        # The light screened over it, saturating softly to white-hot; back to sRGB.
        np.negative(L, out=L)
        np.exp(L, out=L)
        out = 1 - (1 - bg) * L
        np.sqrt(np.clip(out, 0, 1, out=out), out=out)
        out *= 255
        out += 0.5
        img = Image.merge("RGB", [Image.fromarray(out[c].astype(np.uint8)) for c in range(3)])
        bend = float(0.07 * sstep(0.3, 0.6, u) * (1 - sstep(OPEN_U - 0.04, OPEN_U + 0.2, u)))
        if bend > 0.002:
            img = self.lens(img, bend)
        return np.asarray(img)

    def frames(self, n):
        """The transition's n frames (n = its share of the intro's frames), the last one the recap."""
        span = RIBBON_SEC + REVEAL_SEC
        for k in range(n):
            yield self.frame((k + 1) * span / n)


# ─────────────────────────────────────────────────────────────── sound

def fft_filter(x, lo=None, hi=None):
    """x through a gentle high-pass at `lo` and low-pass at `hi` (Hz), along its first axis."""
    n = len(x)
    X = np.fft.rfft(x, axis=0)
    f = np.fft.rfftfreq(n, 1 / SR)
    g = np.ones_like(f)
    if lo:
        g *= 1 / (1 + (lo / np.maximum(f, 1)) ** 4)
    if hi:
        g *= 1 / (1 + (f / hi) ** 4)
    if X.ndim == 2:
        g = g[:, None]
    return np.fft.irfft(X * g, n=n, axis=0)


def reverb(x, secs=2.2, mix=0.3, seed=3):
    """A synthetic hall (stereo noise tail) over x."""
    rng = np.random.default_rng(seed)
    n = int(secs * SR)
    t = np.arange(n) / SR
    env = np.exp(-t * 6.9 / secs)
    ir = np.stack([rng.standard_normal(n) * env, rng.standard_normal(n) * env], 1)
    ir = fft_filter(ir, lo=120, hi=8000)
    ir /= np.sqrt(np.sum(ir ** 2, axis=0, keepdims=True)) + 1e-9
    size = 1 << (len(x) + n - 1).bit_length()
    wet = np.fft.irfft(np.fft.rfft(x, size, axis=0) * np.fft.rfft(ir, size, axis=0), size, axis=0)[: len(x)]
    return x * (1 - mix) + wet * mix * 1.6


def envelope(n, a, d):
    t = np.arange(n) / SR
    return np.minimum(1, t / max(a, 1e-4)) * np.exp(-np.maximum(0, t - a) / d)


def pan(x, p):
    return np.stack([x * math.sqrt((1 - p) / 2), x * math.sqrt((1 + p) / 2)], 1)


def add(mix, x, at):
    """x added into mix at `at` seconds (cut to fit)."""
    i = int(round(at * SR))
    if i < 0:
        x, i = x[-i:], 0
    if i >= len(mix):
        return
    x = x[: len(mix) - i]
    mix[i:i + len(x)] += x


def sound(root=55.0, minor=True, seed=11):
    """The transition's sound (stereo float at SR), from its first moment, with the reverb's tail:
    a glassy shimmer running across with the light, a riser into a whoosh that peaks as the camera
    passes through the ribbons (a low thump under it), and a soft, airy release as the recap lands.
    `root` (Hz) and `minor` tune the shimmer (a run up the root's chord, three octaves up) and the
    thump to the music's key."""
    rng = np.random.default_rng(seed)
    rs = RIBBON_SEC
    n = int((rs + REVEAL_SEC + 2.2) * SR)
    out = np.zeros((n, 2))
    # The shimmer: a quick run up the chord, panned left to right with the sweep.
    sweep = (LIGHT_U[1] - LIGHT_U[0]) * rs
    steps = (0, 3, 7, 12, 15, 19) if minor else (0, 4, 7, 12, 16, 19)
    for i, semis in enumerate(steps):
        f0 = root * 8 * 2 ** (semis / 12)
        m = int(0.9 * SR)
        tt = np.arange(m) / SR
        ping = (np.sin(2 * np.pi * f0 * tt) + 0.3 * np.sin(2 * np.pi * f0 * 2.01 * tt)) * envelope(m, 0.004, 0.32)
        add(out, pan(ping * 0.035, -0.7 + 1.4 * i / 5), LIGHT_U[0] * rs + sweep * i / 6)
    # The riser and the whoosh: noise through a band that climbs as the camera speeds up, loudest
    # (and widest) at the pass, then falling away fast.
    pass_t = PASS_SEC
    m = int((pass_t + 0.6) * SR)
    tt = np.arange(m) / SR
    env = np.where(tt < pass_t, (tt / pass_t) ** 2.4, np.exp(-(tt - pass_t) / 0.16))
    # The band's centre follows the camera: bands of noise crossfaded along it (smooth, no zipper).
    f_c = np.where(tt < pass_t, 500 + 5500 * np.clip(tt / pass_t, 0, 1) ** 2, 6000 * np.exp(-(tt - pass_t) / 0.25) + 400)
    whoosh = np.zeros((m, 2))
    wsum = np.zeros(m)
    for fc in (400.0, 900.0, 2000.0, 4500.0, 9000.0):
        wk = np.exp(-((np.log(f_c) - math.log(fc)) / 0.6) ** 2)
        band = np.stack([fft_filter(rng.standard_normal(m), lo=fc * 0.5, hi=fc * 2.0) for _ in range(2)], 1)
        band /= np.sqrt(np.mean(band ** 2)) + 1e-9
        whoosh += band * wk[:, None]
        wsum += wk
    whoosh *= (env / (wsum + 1e-6) * 0.26)[:, None]
    add(out, whoosh, 0.0)
    thump_m = int(0.8 * SR)
    tt = np.arange(thump_m) / SR
    thump = np.sin(2 * np.pi * np.cumsum(root * 0.75 + root * 1.2 * np.exp(-tt / 0.08)) / SR) * envelope(thump_m, 0.005, 0.35)
    add(out, pan(thump * 0.35, 0), pass_t - 0.01)
    # The release: air opening up as the recap lands.
    land = OPEN_U * rs
    m = int(2.0 * SR)
    tt = np.arange(m) / SR
    shape = np.minimum(1, tt / 0.25) * np.exp(-tt / 0.5)
    add(out, pan(fft_filter(rng.standard_normal(m), lo=2500, hi=12000) * shape * 0.05, -0.2)
        + pan(fft_filter(rng.standard_normal(m), lo=2500, hi=12000) * shape * 0.05, 0.2), land)
    return reverb(out, 2.0, 0.22)
