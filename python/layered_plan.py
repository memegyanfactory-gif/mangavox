"""
Layered motion (2.5D panels): the pure, model-free half. Stdlib + numpy only, so the renderer's
own Python and the tests can use it without the optional pack.

The pack (python/layered_motion.py, run in `<data>/models/vfx/venv`) cuts a panel into layers
(SAM2 mattes prompted from read.json, LaMa clean plates, Depth Anything V2 depth) and animates
them. Everything that decides *what* happens lives here:

  * prompts:   SAM2 prompts for a panel's characters from read.json (char_prompts)
  * lettering: an automatic bubble / lettering detector on the panel (detect_lettering), so
               text is kept rigid on top and never warps
  * gates:     whether a SAM2 matte is good enough to move (mask_gate), else the layer is dropped
  * analysis:  speed-line direction (speed_lines), impact centre (impact_centre), long thin
               objects (thin_object_ok)
  * motion:    which effects a panel gets from its scene (motion_plan)
  * key clips: which clips of a chapter get the treatment in "key" mode (select_layered)
  * cache:     `<chapter>/studio/layers/<panel-hash>/manifest.json` (panel_key, read/write_manifest)
  * pack:      where the pack's Python is, and whether it is installed (pack_python)
"""
import hashlib
import json
import math
import os
import re

# Bumped whenever the prep (masks, plates, depth, analysis) changes, so cached layers are redone.
PREP_VERSION = 3
# Bumped whenever the animation of prepared layers changes (part of the clip cache key).
ANIM_VERSION = 2
MODES = ("off", "key", "all")
DEFAULT_MODE = "off"

# Key moments: a clip qualifies at this smoothed intensity, or as a reveal / a hit; at most one
# clip in KEY_SHARE gets it, and two picked clips are at least KEY_GAP clips apart.
KEY_INTENSITY = 7.0
KEY_SHARE = 4
KEY_GAP = 2

# Mask gate (a layer is dropped rather than risk tearing).
MIN_AREA = 0.015          # share of the panel
MAX_AREA = 0.70
MIN_FACE_COVER = 0.55     # share of the character's face box inside its matte
MIN_LARGEST = 0.80        # share of the matte in its largest piece
MAX_PIECES = 6            # pieces bigger than 0.5 % of the matte
MAX_SIDES = 2             # panel sides the matte may touch (a close-up touches two)
MAX_BORDER = 0.45         # share of the panel's border the matte may run along
MIN_SCORE = 0.55          # SAM2's own predicted IoU

# A long thin object (whip, chain, rope): its matte's length over width and length over the
# panel diagonal, and its area bounds.
THIN_ELONGATION = 5.0
THIN_MIN_LENGTH = 0.35
THIN_AREA = (0.003, 0.12)

HIT_TAGS = re.compile(
    r"\b(explo\w*|impact|punch\w*|clash\w*|strik\w*|slash\w*|attack\w*|ambush|blast\w*|smash\w*|"
    r"crush\w*|crash\w*|shatter\w*|slam\w*|first blow|surprise attack|kick\w*|blow|hit|blood\w*|gore|debris|rubble)\b",
    re.I,
)
GROUND_TAGS = re.compile(r"\b(explo\w*|impact|smash\w*|crash\w*|shatter\w*|slam\w*|crush\w*|ground|floor|crater|quake|shockwave|blast\w*)\b", re.I)
DRIFT_TAGS = re.compile(r"\b(blood\w*|spray|splatter\w*|debris|rubble|shatter\w*|explo\w*|dust|gore|impact|smash\w*)\b", re.I)
THIN_TAGS = re.compile(r"\b(chain\w*|whip\w*|rope\w*|tentacle\w*|tail|scarf|cape|cable\w*|vine\w*|lash\w*|sword\w*|spear\w*|blade\w*|ribbon\w*|thread\w*)\b", re.I)
REVEAL_KINDS = ("reveal",)
ACTION_KINDS = ("fight", "action")


# --------------------------------------------------------------------------------- boxes

def _box(v):
    try:
        b = tuple(float(x) for x in v)
    except (TypeError, ValueError):
        return None
    return b if len(b) == 4 and b[2] > b[0] and b[3] > b[1] else None


def crop_box(panel):
    """The page area (fractions) of a studio panel crop p<page>_<n>.jpg: the panel's bbox with
    the crop's 1 % padding (mv-recap read.rs crop_panel; video_generator.crop_region)."""
    b = _box((panel or {}).get("bbox"))
    if not b:
        return (0.0, 0.0, 1.0, 1.0)
    px, py = (b[2] - b[0]) * 0.01, (b[3] - b[1]) * 0.01
    return tuple(min(1.0, max(0.0, v)) for v in (b[0] - px, b[1] - py, b[2] + px, b[3] + py))


def to_crop(box, crop):
    """A page box (fractions) in fractions of the crop, clamped to it."""
    cw, ch = crop[2] - crop[0], crop[3] - crop[1]
    out = ((box[0] - crop[0]) / cw, (box[1] - crop[1]) / ch, (box[2] - crop[0]) / cw, (box[3] - crop[1]) / ch)
    out = tuple(min(1.0, max(0.0, v)) for v in out)
    return out if out[2] > out[0] and out[3] > out[1] else None


def panel_faces(panel):
    """[(name, face box in crop fractions)] of a read.json panel's characters with a face."""
    crop = crop_box(panel)
    out = []
    for c in (panel or {}).get("characters") or []:
        if not isinstance(c, dict):
            continue
        f = _box(c.get("face"))
        f = to_crop(f, crop) if f else None
        if f:
            out.append((str(c.get("name") or "character"), f))
    return out


def char_prompts(panel, w, h):
    """SAM2 prompts (pixels of a w x h crop) for each character of a read.json panel that has a
    face box: a positive point on the face, and a loose body box (the face widened 1.4x each
    side and run down 4.5 faces, inside the panel). A character without a face gets no layer.
    Two faces that overlap a lot are one character."""
    out = []
    for name, f in panel_faces(panel):
        fx0, fy0, fx1, fy1 = f[0] * w, f[1] * h, f[2] * w, f[3] * h
        fw, fh = fx1 - fx0, fy1 - fy0
        if fw < 3 or fh < 3:
            continue
        if any(_iou((fx0, fy0, fx1, fy1), p["face"]) > 0.5 for p in out):
            continue
        box = (max(0.0, fx0 - 1.4 * fw), max(0.0, fy0 - 0.35 * fh), min(float(w), fx1 + 1.4 * fw), min(float(h), fy1 + 4.5 * fh))
        out.append({"name": name, "face": (fx0, fy0, fx1, fy1), "point": ((fx0 + fx1) / 2, (fy0 + fy1) / 2), "box": box})
    return out


def _iou(a, b):
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    inter = ix * iy
    u = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / u if u > 0 else 0.0


# --------------------------------------------------------------------------------- masks

def label(mask):
    """4-connected components of a boolean 2-D numpy array: (labels int32 array, count).
    Run-length union-find, fast enough for a panel at analysis size."""
    import numpy as np
    m = np.asarray(mask, bool)
    h, w = m.shape
    runs = []          # (row, start, end) end exclusive
    row_runs = []
    for y in range(h):
        r = m[y]
        if not r.any():
            row_runs.append(())
            continue
        d = np.diff(np.concatenate(([0], r.view(np.int8), [0])))
        starts, ends = np.nonzero(d == 1)[0], np.nonzero(d == -1)[0]
        base = len(runs)
        runs.extend((y, int(s), int(e)) for s, e in zip(starts, ends))
        row_runs.append(range(base, len(runs)))
    parent = list(range(len(runs)))

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    for y in range(1, h):
        prev, cur = row_runs[y - 1], row_runs[y]
        if not prev or not cur:
            continue
        i, j = prev[0], cur[0]
        pe, ce = prev[-1] + 1, cur[-1] + 1
        while i < pe and j < ce:
            _, s0, e0 = runs[i]
            _, s1, e1 = runs[j]
            if s0 < e1 and s1 < e0:
                a, b = find(i), find(j)
                if a != b:
                    parent[max(a, b)] = min(a, b)
            if e0 < e1:
                i += 1
            else:
                j += 1
    lab = np.zeros((h, w), np.int32)
    ids = {}
    for k, (y, s, e) in enumerate(runs):
        r = find(k)
        if r not in ids:
            ids[r] = len(ids) + 1
        lab[y, s:e] = ids[r]
    return lab, len(ids)


def fill_holes(mask):
    """`mask` with every enclosed hole filled (background not reachable from the border)."""
    import numpy as np
    m = np.asarray(mask, bool)
    inv = np.pad(~m, 1, constant_values=True)
    lab, _ = label(inv)
    outside = lab == lab[0, 0]
    return m | ~outside[1:-1, 1:-1]


def mask_stats(mask, face=None, score=1.0):
    """What mask_gate needs of a boolean matte: area share, the share of `face` (pixel box)
    inside it, the share in its largest piece, the number of real pieces, the panel sides it
    touches and the share of the border it runs along, and SAM2's score."""
    import numpy as np
    m = np.asarray(mask, bool)
    h, w = m.shape
    area = int(m.sum())
    st = {"area": area / float(h * w), "score": float(score), "face_cover": 1.0}
    if area == 0:
        st.update(largest=0.0, pieces=0, sides=0, border=0.0, face_cover=0.0)
        return st
    lab, n = label(m)
    sizes = np.bincount(lab.ravel())[1:]
    st["largest"] = float(sizes.max()) / area
    st["pieces"] = int((sizes > max(4, 0.005 * area)).sum())
    edges = [m[0, :], m[-1, :], m[:, 0], m[:, -1]]
    st["sides"] = int(sum(1 for e in edges if e.mean() > 0.03))
    st["border"] = float(sum(e.sum() for e in edges)) / float(2 * (w + h))
    if face is not None:
        x0, y0, x1, y1 = (int(round(v)) for v in face)
        x0, y0, x1, y1 = max(0, x0), max(0, y0), min(w, max(x0 + 1, x1)), min(h, max(y0 + 1, y1))
        sub = m[y0:y1, x0:x1]
        st["face_cover"] = float(sub.mean()) if sub.size else 0.0
    return st


# The second SAM2 pass (layered_motion._sam_masks) may grow a character's matte to the limbs its
# first, face-sized box cut off: it must keep this share of the first matte and stay under this
# many times its area (more is the chair, the floor or the next character).
GROW_KEEP = 0.9
GROW_MAX = 1.4


def grown_ok(first, grown):
    """A grown matte is the same character, whole: nearly all of the first matte, not much more."""
    import numpy as np
    a, g = np.asarray(first, bool), np.asarray(grown, bool)
    area = int(a.sum())
    return area > 0 and int((a & g).sum()) >= GROW_KEEP * area and int(g.sum()) <= GROW_MAX * area


def drop_specks(mask, share=0.005):
    """`mask` without its pieces smaller than `share` of it (stray dots a matte leaves on the
    art around it, which would move apart from everything)."""
    import numpy as np
    m = np.asarray(mask, bool)
    area = int(m.sum())
    if area == 0:
        return m
    lab, n = label(m)
    if n <= 1:
        return m
    sizes = np.bincount(lab.ravel(), minlength=n + 1)
    keep = sizes >= max(4, share * area)
    keep[0] = False
    return keep[lab]


def mask_gate(st):
    """(ok, reason) for a character matte's mask_stats: a tiny, huge, fragmented, faceless or
    border-hugging matte (most likely background) is dropped rather than moved."""
    if st.get("score", 1.0) < MIN_SCORE:
        return False, "low score"
    if st["area"] < MIN_AREA:
        return False, "tiny"
    if st["area"] > MAX_AREA:
        return False, "too big"
    if st.get("face_cover", 1.0) < MIN_FACE_COVER:
        return False, "misses the face"
    if st["largest"] < MIN_LARGEST or st["pieces"] > MAX_PIECES:
        return False, "fragmented"
    if st["sides"] > MAX_SIDES or st["border"] > MAX_BORDER:
        return False, "hugs the panel border"
    return True, "ok"


def thin_stats(mask):
    """Length (share of the diagonal), elongation, area share and the two ends (pixels) of a
    boolean matte, from the principal axis of its pixels."""
    import numpy as np
    m = np.asarray(mask, bool)
    h, w = m.shape
    ys, xs = np.nonzero(m)
    if len(xs) < 20:
        return {"length": 0.0, "elongation": 0.0, "area": 0.0, "ends": None}
    pts = np.stack([xs, ys], 1).astype(np.float64)
    c = pts.mean(0)
    cov = np.cov((pts - c).T)
    vals, vecs = np.linalg.eigh(cov)
    major = vecs[:, 1]
    proj = (pts - c) @ major
    lo, hi = np.percentile(proj, 1), np.percentile(proj, 99)
    length = hi - lo
    width = 4 * math.sqrt(max(vals[0], 1e-9))
    return {
        "length": float(length / math.hypot(w, h)), "elongation": float(length / max(width, 1.0)),
        "area": float(len(xs)) / (w * h), "ends": [list(map(float, c + major * lo)), list(map(float, c + major * hi))],
    }


def thin_object_ok(st):
    """A matte is a long thin object (whip, chain, rope) worth swaying."""
    return (st.get("ends") is not None and st["elongation"] >= THIN_ELONGATION and st["length"] >= THIN_MIN_LENGTH
            and THIN_AREA[0] <= st["area"] <= THIN_AREA[1])


# --------------------------------------------------------------------------------- lettering

def _components(mask):
    """[(label id, area, x0, y0, x1, y1)] of a boolean mask's components, and the labels."""
    import numpy as np
    lab, n = label(mask)
    if n == 0:
        return [], lab
    flat = lab.ravel()
    area = np.bincount(flat, minlength=n + 1)
    ys, xs = np.indices(lab.shape)
    big = np.iinfo(np.int32).max
    x0 = np.full(n + 1, big); y0 = np.full(n + 1, big)
    x1 = np.full(n + 1, -1); y1 = np.full(n + 1, -1)
    np.minimum.at(x0, flat, xs.ravel()); np.minimum.at(y0, flat, ys.ravel())
    np.maximum.at(x1, flat, xs.ravel()); np.maximum.at(y1, flat, ys.ravel())
    out = [(i, int(area[i]), int(x0[i]), int(y0[i]), int(x1[i]) + 1, int(y1[i]) + 1) for i in range(1, n + 1)]
    return out, lab


def _letter_like(c, h):
    """A component the size and shape of a letter (stroke or counter) in a panel of size `h` (the square root of its area): not
    a hatching line (too thin) nor a solid block."""
    _, a, x0, y0, x1, y1 = c
    bw, bh = x1 - x0, y1 - y0
    return 0.012 * h <= bh <= 0.12 * h and 0.22 * bh <= bw <= 1.8 * bh and a >= 6 and 0.12 * bw * bh <= a <= 0.85 * bw * bh


def _rows(cands, h, min_count=3, elongation=1.5):
    """Groups letter-like boxes (label, area, x0, y0, x1, y1) into text runs: at least 3 boxes
    of similar height side by side on one baseline (or stacked, for vertical text) with gaps
    under about one letter height, their heights consistent and the run longer than it is
    tall. Returns [(box, member indices)]."""
    n = len(cands)
    if n < 3:
        return []
    parent = list(range(n))

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    def join(i, j):
        a, b = find(i), find(j)
        if a != b:
            parent[max(a, b)] = min(a, b)

    hs = [c[5] - c[3] for c in cands]
    ws = [c[4] - c[2] for c in cands]
    reach = 0.15 * h
    # Rows: neighbours along x (sorted), on the same line with similar heights.
    order = sorted(range(n), key=lambda k: cands[k][2])
    for p, i in enumerate(order):
        a = cands[i]
        for j in order[p + 1:]:
            b = cands[j]
            if b[2] - a[4] > reach:
                break
            if not 0.6 <= hs[j] / max(1, hs[i]) <= 1.65:
                continue
            gap_x = b[2] - a[4]
            same_line = abs(b[5] - a[5]) < 0.35 * max(hs[i], hs[j]) and min(a[5], b[5]) - max(a[3], b[3]) > 0.4 * min(hs[i], hs[j])
            if same_line and gap_x < 1.0 * max(hs[i], hs[j]):
                join(i, j)
    # Columns (vertical lettering): neighbours along y, centred on one another.
    order = sorted(range(n), key=lambda k: cands[k][3])
    for p, i in enumerate(order):
        a = cands[i]
        for j in order[p + 1:]:
            b = cands[j]
            if b[3] - a[5] > reach:
                break
            if not 0.6 <= ws[j] / max(1, ws[i]) <= 1.65:
                continue
            gap_y = b[3] - a[5]
            centred = abs((a[2] + a[4]) - (b[2] + b[4])) / 2 < 0.35 * max(ws[i], ws[j])
            if centred and gap_y < 0.8 * max(hs[i], hs[j]):
                join(i, j)
    groups = {}
    for k in range(n):
        groups.setdefault(find(k), []).append(k)
    runs = []
    for members in groups.values():
        if len(members) < min_count:
            continue
        boxes = [cands[k] for k in members]
        box = (min(b[2] for b in boxes), min(b[3] for b in boxes), max(b[4] for b in boxes), max(b[5] for b in boxes))
        heights = sorted(hs[k] for k in members)
        med = heights[len(heights) // 2]
        spread = (heights[int(len(heights) * 0.8)] - heights[int(len(heights) * 0.2)]) / max(1, med)
        long_side, short_side = max(box[2] - box[0], box[3] - box[1]), min(box[2] - box[0], box[3] - box[1])
        if long_side >= elongation * max(med, short_side * 0.5) and spread <= 0.45:
            runs.append((box, members))
    return runs


def detect_lettering(gray, faces=()):
    """Speech bubbles and lettering of a panel (gray: 2-D float array 0..1, any size, best
    300-600 px), as a boolean mask of the same size. No model: a bubble is a light region that
    is not the background (it does not run along two or more panel sides), fairly compact, with
    letter-sized dark holes inside it; free lettering is three or more letter-sized pieces in a
    row (dark letters on a light ground, or light letters outlined in dark). Runs that sit mostly
    on a face (pixel boxes) are not lettering. Over-detection only keeps a region still, so the
    rules lean towards finding text."""
    import numpy as np
    g = np.asarray(gray, np.float32)
    h, w = g.shape
    ref = math.sqrt(h * w)
    light = g > 0.72
    out = np.zeros((h, w), bool)
    comps, lab = _components(light)
    panel = float(h * w)
    letters = []
    for c in comps:
        i, a, x0, y0, x1, y1 = c
        bw, bh = x1 - x0, y1 - y0
        if _letter_like(c, ref):
            letters.append(c)
        if a < 0.002 * panel or bw < 0.04 * w or bh < 0.03 * h:
            continue
        sides = int(x0 == 0) + int(y0 == 0) + int(x1 == w) + int(y1 == h)
        if sides >= 2 or a > 0.4 * panel:
            continue
        reg = lab[y0:y1, x0:x1] == i
        filled = fill_holes(reg)
        fill = filled.sum() / float(bw * bh)
        holes = filled & ~reg
        if fill < 0.42 or not holes.any():
            continue
        hcomps, _ = _components(holes)
        glyphs = [hc for hc in hcomps if _letter_like(hc, ref)]
        ink = holes.sum() / float(filled.sum())
        if len(glyphs) >= 3 and 0.02 <= ink <= 0.55 and _rows(glyphs, ref):
            out[y0:y1, x0:x1] |= filled
    # Free lettering (sound effects, captions without a bubble): longer, consistent runs of
    # letter pieces, light letters outlined in dark or dark letters on light ground. Only the
    # letters themselves (grown a little) are kept still, never a box round them, so a stray
    # match on the art is a speck rather than a frozen patch.
    dark_comps, dark_lab = _components(g < 0.35)
    letters = [("L",) + c[1:] + (c[0],) for c in letters] + [("D",) + c[1:] + (c[0],) for c in dark_comps if _letter_like(c, ref)]
    run_mask = np.zeros((h, w), bool)
    for box, members in _rows(letters, ref, min_count=4, elongation=2.2):
        area = (box[2] - box[0]) * (box[3] - box[1])
        if area > 0.06 * panel:
            continue
        if any(_overlap(box, f) > 0.3 * area for f in faces):
            continue
        for k in members:
            src, lid = letters[k][0], letters[k][6]
            x0, y0, x1, y1 = letters[k][2:6]
            sub = (lab if src == "L" else dark_lab)[y0:y1, x0:x1] == lid
            run_mask[y0:y1, x0:x1] |= sub
    if run_mask.any():
        r = max(1, int(0.006 * ref))
        from PIL import Image, ImageFilter
        grown = Image.fromarray(run_mask.astype(np.uint8) * 255).filter(ImageFilter.MaxFilter(2 * r + 1))
        run_mask = np.asarray(grown) > 127
        if run_mask.mean() <= 0.12:
            out |= run_mask
    return out


def _overlap(a, b):
    ix = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    return ix * iy


# --------------------------------------------------------------------------------- analysis

def speed_lines(gray, bg=None):
    """The direction (unit x, y) of parallel speed lines across a panel's background, or None:
    a structure tensor over the background (bg: boolean, True where background) whose strong,
    coherent part covers enough of it and points one way. Radial focus lines point every way
    and give None."""
    import numpy as np
    g = np.asarray(gray, np.float32)
    gy, gx = np.gradient(g)

    def box(a, r=4):
        c = np.cumsum(np.cumsum(np.pad(a, ((r + 1, r), (r + 1, r)), mode="edge"), 0), 1)
        return (c[2 * r + 1:, 2 * r + 1:] - c[:-2 * r - 1, 2 * r + 1:] - c[2 * r + 1:, :-2 * r - 1] + c[:-2 * r - 1, :-2 * r - 1]) / float((2 * r + 1) ** 2)

    jxx, jyy, jxy = box(gx * gx), box(gy * gy), box(gx * gy)
    tr = jxx + jyy
    coh = np.sqrt((jxx - jyy) ** 2 + 4 * jxy ** 2) / (tr + 1e-6)
    sel = (coh > 0.6) & (tr > np.percentile(tr, 60))
    if bg is not None:
        sel &= np.asarray(bg, bool)
        base = max(1, int(np.asarray(bg, bool).sum()))
    else:
        base = g.size
    if sel.sum() < 0.18 * base:
        return None
    ang = np.arctan2(2 * jxy[sel], (jxx - jyy)[sel])          # 2 x gradient angle
    c, s = np.cos(ang).mean(), np.sin(ang).mean()
    if math.hypot(c, s) < 0.55:
        return None
    theta = 0.5 * math.atan2(s, c) + math.pi / 2               # along the lines
    return (round(math.cos(theta), 4), round(math.sin(theta), 4))


def impact_centre(gray, bg=None):
    """Where a hit lands in a panel (x, y fractions): the densest patch of small dark specks
    (debris, blood, rubble) in its lower 85 %, or None when nothing stands out."""
    import numpy as np
    g = np.asarray(gray, np.float32)
    h, w = g.shape
    comps, _ = _components(g < 0.3)
    small = [c for c in comps if 3 <= c[1] <= 0.0015 * h * w]
    if len(small) < 12:
        return None
    gh, gw = 8, 8
    dens = np.zeros((gh, gw))
    for _, a, x0, y0, x1, y1 in small:
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        if bg is not None and not np.asarray(bg, bool)[int(min(h - 1, cy)), int(min(w - 1, cx))]:
            continue
        dens[min(gh - 1, int(cy / h * gh)), min(gw - 1, int(cx / w * gw))] += 1
    dens[0, :] *= 0.4
    if dens.max() < 2.0 * max(1e-6, dens.mean()) or dens.max() < 4:
        return None
    iy, ix = np.unravel_index(int(dens.argmax()), dens.shape)
    return (round((ix + 0.5) / gw, 3), round((iy + 0.5) / gh, 3))


# --------------------------------------------------------------------------------- motion

def _words(scene):
    """What a scene is about: its tags and summary (not its mood words: an "explosive" mood is
    not an explosion)."""
    scene = scene if isinstance(scene, dict) else {}
    tags = " ".join(str(t) for t in scene.get("tags") or [] if isinstance(t, (str, int, float)))
    return f"{tags} {scene.get('summary') or ''}"


def motion_plan(scene, analysis, intensity=None):
    """The effects of one layered clip, from its scene (read.json kind, mood, intensity, tags)
    and the panel's prep analysis {"layers": [{"name", "depth", "area", "face"}], "speed": (x, y)
    or None, "impact": (x, y) or None, "thin": {...} or None}. Returns:
      parallax: amplitude (share of the panel width the nearest layer moves), zoom parallax
      breathe:  {"layer", "amp", "period"} on the main character, or None
      sway:     the thin object's {"root", "tip", "amp"}, or None
      drift:    {"centre", "k"} blood / debris drift outward on a hit, or None
      shock:    {"centre"} shockwave ring, rubble drift and floor dust on a big hit, or None
      streaks:  {"vec", "strength"} along detected speed lines, or None
    Faces are never warped: the animator keeps local fields off them."""
    scene = scene if isinstance(scene, dict) else {}
    kind = str(scene.get("kind") or "").lower()
    try:
        inten = float(intensity if intensity is not None else scene.get("intensity") or 5)
    except (TypeError, ValueError):
        inten = 5.0
    inten = min(10.0, max(0.0, inten))
    words = _words(scene)
    layers = [l for l in (analysis or {}).get("layers") or [] if isinstance(l, dict)]
    hit = bool(HIT_TAGS.search(words))
    plan = {
        "parallax": round(0.006 + 0.0005 * inten, 4) if layers else round(0.004 + 0.0003 * inten, 4),
        "zoom": 0.012 if layers else 0.0,
        "breathe": None, "sway": None, "drift": None, "shock": None, "streaks": None,
    }
    faced = [l for l in layers if l.get("face")]
    if faced:
        main = max(faced, key=lambda l: l.get("area", 0))
        plan["breathe"] = {"layer": main["name"], "amp": 0.006 if kind in ACTION_KINDS else 0.004,
                           "period": 2.8 if kind in ACTION_KINDS or inten >= 7 else 3.8}
    thin = (analysis or {}).get("thin")
    if thin and thin.get("root") and thin.get("tip"):
        plan["sway"] = {"root": thin["root"], "tip": thin["tip"], "amp": round(0.0025 + 0.0003 * inten, 4)}
    centre = (analysis or {}).get("impact")
    if hit and DRIFT_TAGS.search(words):
        plan["drift"] = {"centre": centre or (0.5, 0.55), "k": round(0.012 + 0.0015 * inten, 4)}
    if inten >= 8 and hit and GROUND_TAGS.search(words):
        plan["shock"] = {"centre": centre or (0.5, 0.7)}
    vec = (analysis or {}).get("speed")
    if vec:
        plan["streaks"] = {"vec": tuple(vec), "strength": 0.16 if inten < 7 else 0.2}
    return plan


def plan_effects(plan):
    """The names of a motion plan's effects that are on, for logs and the cache key."""
    out = ["parallax"] if plan.get("parallax") else []
    return out + [k for k in ("breathe", "sway", "drift", "shock", "streaks") if plan.get(k)]


# --------------------------------------------------------------------------------- key clips

def key_candidate(mood):
    """A clip's key-moment score from its mood plan entry (video_generator.plan_moods), or None
    when it does not qualify: a smoothed intensity of KEY_INTENSITY, or a reveal or a hit."""
    if not mood:
        return None
    inten = float(mood.get("intensity") or 0)
    kind = str(mood.get("kind") or "")
    hit = bool(HIT_TAGS.search(" ".join(mood.get("tags") or [])))
    if inten >= KEY_INTENSITY or kind in REVEAL_KINDS or (hit and inten >= 6):
        return inten + (1.5 if hit else 0.0) + (1.0 if kind in REVEAL_KINDS else 0.0)
    return None


def select_layered(mode, eligible, scores):
    """Indices of the clips that get layered motion. mode "all": every eligible clip; "key":
    the best scoring (scores[i], None = not a key moment) eligible clips, at most one in
    KEY_SHARE (at least one when any qualifies) and KEY_GAP clips apart; "off": none."""
    n = len(eligible)
    if mode == "all":
        return [i for i in range(n) if eligible[i]]
    if mode != "key":
        return []
    cands = sorted((i for i in range(n) if eligible[i] and scores[i] is not None), key=lambda i: (-scores[i], i))
    cap = max(1, n // KEY_SHARE) if cands else 0
    picked = []
    for i in cands:
        if len(picked) >= cap:
            break
        if all(abs(i - j) >= KEY_GAP for j in picked):
            picked.append(i)
    return sorted(picked)


# --------------------------------------------------------------------------------- cache

def panel_key(src, panel, size=None):
    """The cache folder name of a panel's layers: its pixels (file content), the read.json data
    the prompts come from, and PREP_VERSION."""
    h = hashlib.sha1()
    try:
        with open(src, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    except OSError:
        h.update(str(src).encode("utf-8"))
    faces = [[round(v, 4) for v in f] for _, f in panel_faces(panel)]
    h.update(json.dumps([PREP_VERSION, faces, size], sort_keys=True).encode("utf-8"))
    return h.hexdigest()[:16]


def layers_root(output_dir):
    """`<chapter>/studio/layers`: one folder of prepared layers per panel (panel_key)."""
    return os.path.join(output_dir, "studio", "layers")


def prune_stale(output_dir):
    """Removes the layer folders an older version of the pack cut (they are never read again);
    returns how many went. A folder being written (no manifest yet) is left alone."""
    import shutil
    root, gone = layers_root(output_dir), 0
    try:
        names = os.listdir(root)
    except OSError:
        return 0
    for name in names:
        path = os.path.join(root, name, "manifest.json")
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
        except (OSError, ValueError):
            continue
        if isinstance(data, dict) and data.get("version") != PREP_VERSION:
            shutil.rmtree(os.path.join(root, name), ignore_errors=True)
            gone += 1
    return gone


def load_read(output_dir):
    """{page index: page} of `<chapter>/studio/read.json` (scene, summary, panels), or {}."""
    try:
        with open(os.path.join(output_dir, "studio", "read.json"), "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    out = {}
    for i, p in enumerate(data.get("pages") or [] if isinstance(data, dict) else []):
        if isinstance(p, dict) and isinstance(p.get("index", i), int):
            out[p.get("index", i)] = p
    return out


def write_manifest(folder, data):
    """Writes `<folder>/manifest.json` (atomically) with the prep version stamped in."""
    os.makedirs(folder, exist_ok=True)
    data = dict(data, version=PREP_VERSION)
    tmp = os.path.join(folder, "manifest.json.part")
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=1, sort_keys=True)
    os.replace(tmp, os.path.join(folder, "manifest.json"))


def read_manifest(folder):
    """The manifest of a prepared panel, or None when it is missing, unreadable, from another
    prep version, or names files that are gone."""
    try:
        with open(os.path.join(folder, "manifest.json"), "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or data.get("version") != PREP_VERSION:
        return None
    files = [data.get("plate"), data.get("text"), data.get("clean"), data.get("depth")] + [l.get("file") for l in data.get("layers") or [] if isinstance(l, dict)]
    if any(not f or not os.path.isfile(os.path.join(folder, f)) for f in files):
        return None
    return data


def anim_key(prep_key, plan_input):
    """Identity of a clip's animation (part of the rendered clip's cache key)."""
    raw = json.dumps([ANIM_VERSION, prep_key, plan_input], sort_keys=True, default=str)
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:12]


# --------------------------------------------------------------------------------- the pack

def data_dir():
    return os.environ.get("MANGAVOX_DATA") or os.environ.get("MANGAVOX_HOME") or os.path.join(os.path.expanduser("~"), ".mangavox")


def pack_dir(root=None):
    """`<data>/models/vfx`: the pack's venv, weights and install marker."""
    return os.path.join(root or data_dir(), "models", "vfx")


def venv_python(folder):
    venv = os.path.join(folder, "venv")
    return os.path.join(venv, "Scripts", "python.exe") if os.name == "nt" else os.path.join(venv, "bin", "python3")


def pack_marker(folder):
    try:
        with open(os.path.join(folder, "mangavox-vfx.json"), "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def pack_python(config=None, root=None):
    """(python path, None) of an installed pack, or (None, why not). The app passes the pack's
    interpreter as config["vfx_python"]; MANGAVOX_VFX_PYTHON overrides; else the default place
    under the data folder, when its install marker says it works."""
    config = config if isinstance(config, dict) else {}
    folder = pack_dir(root)
    explicit = config.get("vfx_python") or os.environ.get("MANGAVOX_VFX_PYTHON")
    if explicit:
        return (explicit, None) if os.path.isfile(explicit) else (None, f"the layered motion pack's Python is missing ({explicit})")
    py = venv_python(folder)
    if not os.path.isfile(py):
        return None, "the layered motion pack is not installed"
    if not pack_marker(folder).get("ok"):
        return None, "the layered motion pack did not finish installing"
    return py, None
