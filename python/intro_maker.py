#!/usr/bin/env python3
"""Render the character intro: a short cinematic opening played before a full recap.

  python intro_maker.py <args.json>

args: {width, height, fps ("30/1"), style ("epic" | "phonk" | "tension" | "sad" | "lofi" | "horror" | "mystery"),
       chapters: [{read, pages}],
       cast: [{name, aliases, id (the brain's character id), description (appearance; its role after ";"),
               traits: [str] (optional), faces: [face crops],
               profile: {role, bio, traits: [3], facts: [0..2]} | null}] (most seen first),
       cast_draw (default true; false: kept drawings and backgrounds only, no image requests),
       studio_dir (Studio's folder: magnific.json holds the API key) | null,
       intro_dir (the manga's intro folder: drawings and backgrounds are kept in intro_dir/cast/; the app
                  keeps the profiles there too, as <id>.info.json, and passes them as cast[i].profile) | null,
       roto_python (the layered-motion pack's python) | null, roto_data (the data folder),
       music: wav | null (its big moment at MAX_CARDS x CARD_SEC + intro_transition.PASS_SEC),
       reveal: the recap's first video | null, out, work_dir, progress_file | null, draw_budget (credits, optional),
       cards: [{name, image, crop: [x0, y0, x1, y1] (0..1 of the page), cutout: png | null, backdrop: png | null,
                env: page | null, env_crop, profile, description, traits}] (without a cast)}
Anything else in the args is ignored.

Each character gets CARD_SEC on a wide, gently tilted card over black, the way a game's loading
screens show their cast: the character standing whole in front of a painted background of the
place the manga puts them in (intro_cast.py paints it once per character with Magnific's Nano
Banana 2.1 from the manga's own panels of that place, and keeps it; without one, an establishing
panel of the manga), the background drifting slowly inside the card, which slides in from one
side (with a whoosh) and out to the other, over music in the manga's own style. On the other side,
low on the card over a soft shade, the character's profile builds up once the card lands: the
name, the role, a one-sentence bio, three traits as chips and a line of facts (`profile` from the
app; without one, the role clause of the description and its first traits), and goes as the card
leaves (Profile). The characters are clean drawings made for the intro (intro_cast.py: drawn
from the manga's own panels and face crops on a flat key colour, keyed out and checked, kept in
intro_dir/cast and reused); a character that can't be drawn is cut out of its best panel
(intro_roto.py) or shown in it.

The last card does not slide out: its profile fades, then the card turns into the ribbon
transition (intro_transition.py), its surface becoming lines of light in the cards' own colours
that part as the camera dives through them onto the recap's first frame (`reveal`). The intro is total_seconds(cards) long:
cards x CARD_SEC + RIBBON_SEC + REVEAL_SEC.

Progress goes to `progress_file` as {"p": 0..1, "step": "...", "stage": "cast|draw|key|cutout|
backdrop|cards|transition|encode"} (written atomically, on every new step and at most ~4 times a
second).

Prints one JSON line: {"ok": true, "path": out, "seconds": ..., "credits": [{"model", "credits"}], "notes": [...]}
or {"ok": false, "message": ..., "credits": [...], "notes": [...]} (credits spent before a failure are reported too).
"""
import colorsys
import functools
import json
import math
import os
import re
import subprocess
import sys
import traceback
import wave
from itertools import combinations

try:
    import numpy as np
    from PIL import Image, ImageDraw, ImageEnhance, ImageFilter, ImageFont, ImageStat
except ImportError as missing:  # a plain Python: the app runs the intro with one that has them
    print(json.dumps({"ok": False, "message": f"the intro needs numpy and Pillow ({missing}): install the effects pack in Studio's Setup"}))
    sys.exit(1)

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import intro_cast  # noqa: E402
import intro_transition as tr  # noqa: E402

CARD_SEC = 2.5
SLIDE_SEC = 0.32
MAX_CARDS = 4
# The music fades in over MUSIC_IN_SEC, rings MUSIC_HOLD_SEC past its big moment, then fades out
# to silence by the intro's end (over at least MUSIC_OUT_MIN_SEC); the whole mix's last
# END_FADE_SEC fades too.
MUSIC_IN_SEC = 1.2
# Each card's slide-in sound: its length, where it peaks, and how loud under the music.
CARD_SWISH_SEC = 0.7
CARD_SWISH_PEAK = 0.3
CARD_SWISH_GAIN = 0.1
MUSIC_HOLD_SEC = 0.15
MUSIC_OUT_MIN_SEC = 1.2
END_FADE_SEC = 0.6
SR = 48000
LOUDNESS = -15.0  # LUFS, integrated (ITU-R BS.1770)
NAME_FONT = os.path.join(HERE, "fonts", "Anton-Regular.ttf")
ROLE_FONT = os.path.join(HERE, "fonts", "Inter-Bold.ttf")
BODY_FONT = os.path.join(HERE, "fonts", "Inter-Medium.ttf")
CHIP_FONT = os.path.join(HERE, "fonts", "Inter-SemiBold.ttf")
# (accent, dark tint) per music style: the names' accent when the cards give no colour.
STYLE_COLORS = {
    "epic": ((242, 179, 61), (24, 16, 6)),
    "phonk": ((255, 46, 136), (20, 4, 18)),
    "tension": ((224, 49, 59), (18, 6, 8)),
    "horror": ((190, 20, 36), (10, 4, 6)),
    "sad": ((86, 156, 236), (6, 12, 24)),
    "lofi": ((255, 159, 104), (24, 14, 10)),
    "mystery": ((150, 102, 247), (12, 8, 24)),
}
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def ease_out(x):
    x = min(1.0, max(0.0, x))
    return 1 - (1 - x) ** 3


def ease_in_out(x):
    x = min(1.0, max(0.0, x))
    return 3 * x * x - 2 * x * x * x


def parse_fps(s):
    try:
        if "/" in str(s):
            a, b = str(s).split("/", 1)
            return float(a) / float(b)
        return float(s)
    except (ValueError, ZeroDivisionError):
        return 30.0


def total_seconds(n_cards):
    """The intro's length with `n_cards` cards: the cards, then the transition onto the recap."""
    return n_cards * CARD_SEC + tr.total_seconds()


def frame_counts(n_cards, fps):
    """(all frames, the cards' frames): exactly round(total_seconds(n_cards) * fps) in all, the
    transition getting the rest."""
    total = int(round(total_seconds(n_cards) * fps))
    cards = min(total - 1, int(round(n_cards * CARD_SEC * fps)))
    return total, cards


def solve_perspective(src, dst):
    """PIL PERSPECTIVE coefficients mapping output points `dst` back to input points `src`."""
    m = []
    for (x, y), (u, v) in zip(dst, src):
        m.append([x, y, 1, 0, 0, 0, -u * x, -u * y])
        m.append([0, 0, 0, x, y, 1, -v * x, -v * y])
    rhs = [c for pt in src for c in pt]
    return np.linalg.solve(np.array(m, dtype=float), np.array(rhs, dtype=float)).tolist()


def fit(img, w, h):
    s = max(w / img.width, h / img.height)
    img = img.resize((max(1, round(img.width * s)), max(1, round(img.height * s))), Image.LANCZOS)
    x, y = (img.width - w) // 2, (img.height - h) // 2
    return img.crop((x, y, x + w, y + h))


# ─────────────────────────────────────────────────────────────── the profile

# The profile's type scale (font sizes) and spacing (the gaps between its parts), as fractions of
# the frame height; a profile too big for its place shrinks all of it together.
TYPE = {"name": 0.08, "name_min": 0.056, "role": 0.0178, "bio": 0.0236, "chip": 0.014, "fact": 0.0166}
GAP = {"bar": 0.014, "role": 0.019, "bio": 0.023, "chips": 0.024, "facts": 0.021, "chip": 0.0085}
NAME_MAX_W, BIO_MAX_W = 0.68, 0.76  # the widest the name and the bio's lines get (of the frame height)
TRACK = {"role": 0.16, "chip": 0.1}   # letter spacing (of the font size)
# When each part arrives (seconds into its card; the chips one after another, the bio's lines a
# moment apart) and how long it takes; it all goes as the card leaves.
INFO_AT = {"name": 0.256, "bar": 0.3, "role": 0.33, "bio": 0.4, "bio_line": 0.045, "chip": 0.5, "chip_step": 0.055, "fact": 0.66}
INFO_IN = 0.3
INFO_SLIDE = {"name": 0.034, "other": 0.016}   # how far each part slides in from the outer side
NAME_COLOUR = (255, 255, 255)
BIO_COLOUR = (228, 231, 238)
FACT_COLOUR = (184, 190, 202)
CHIP_TEXT = (238, 240, 245)
LIMITS = {"role": 6, "bio": 22, "trait": 2, "fact": 7}  # words (what the app writes stays within)
ROLE_HEDGE = re.compile(r"^(?:(?:appears?|seems?) to be|apparently|likely|probably|possibly|perhaps|presumably)\s+", re.I)
ARTICLE = re.compile(r"^(?:a|an|the)\s+", re.I)
NOT_A_ROLE = re.compile(r"^(?:\w+ing|seen|shown|drawn|depicted|only|usually|often|mostly)\b", re.I)
ROLE_CUT = ("who", "that", "which", "while", "with", "by", "from", "for", "in", "at", "on", "after", "before", "and", "or", "of")


def _clean(v):
    return re.sub(r"\s+", " ", v).strip() if isinstance(v, str) else ""


def _strings(v, n):
    return [s for s in (_clean(x) for x in (v if isinstance(v, (list, tuple)) else [])) if s][:n]


def title_case(s):
    """'fitness-obsessed' -> 'Fitness-Obsessed' (capitals already there stay)."""
    return "-".join(" ".join(w[:1].upper() + w[1:] for w in part.split(" ")) for part in s.split("-"))


def role_from(description):
    """The role in a description ("<look>; <role>"), short enough for a card: "a Shaolin hall
    leader" is "Shaolin hall leader", "Level 6 guild master of the Gray Dog guild" is "Level 6
    guild master"; "" when there is none."""
    for clause in (description or "").split(";")[1:]:
        c = ARTICLE.sub("", ROLE_HEDGE.sub("", _clean(clause).strip(" .,:!?")))
        if not c or NOT_A_ROLE.match(c):
            continue
        words = c.split()
        n = LIMITS["role"]
        if len(words) > n:
            # Cut where a phrase starts: the longest cut that fits, before "of" only when nothing else.
            def starts(i):
                return words[i].lower() in ROLE_CUT or words[i - 1].endswith(",") or re.fullmatch(r"[a-z]+(?:ed|ing)", words[i]) is not None
            cuts = [i for i in range(2, min(len(words), n + 1)) if starts(i)]
            cuts = [i for i in cuts if words[i].lower() != "of"] or cuts
            words = words[:max(cuts)] if cuts else words[:n]
            while len(words) > 1 and (words[-1].lower().strip(",") in ROLE_CUT + ("a", "an", "the")
                                      or re.fullmatch(r"[a-z]+(?:ed|ing),?", words[-1])):
                words.pop()
            c = " ".join(words).strip(" ,")
        return c[:1].upper() + c[1:]
    return ""


def profile_of(spec):
    """What a card says about its character: {"role", "bio", "traits" (up to 3), "facts" (up to
    2)} from the app's `profile`; without one (or where it says nothing), the role clause of the
    character's description and its first three traits, with no bio and no facts."""
    role = role_from(spec.get("description"))
    traits = [title_case(t) for t in _strings(spec.get("traits"), 3)]
    p = spec.get("profile")
    if isinstance(p, dict):
        v = {"role": _clean(p.get("role")), "bio": _clean(p.get("bio")), "traits": _strings(p.get("traits"), 3),
             "facts": _strings(p.get("facts"), 2)}
        if any(v.values()):
            v["role"] = v["role"] or role
            v["traits"] = v["traits"] or traits
            return v
    return {"role": role, "bio": "", "traits": traits, "facts": []}


@functools.lru_cache(maxsize=128)
def _font(path, size):
    return ImageFont.truetype(path if os.path.isfile(path) else ROLE_FONT, max(6, int(size)))


def text_width(font, text, tracking=0.0):
    """How wide `text` is in `font` with `tracking` pixels between its letters."""
    if not tracking:
        return font.getlength(text)
    return sum(font.getlength(c) for c in text) + tracking * max(0, len(text) - 1)


def cap_height(font):
    return -font.getbbox("H", anchor="ls")[1]


SHORT_END = set("a an the of in on at to as by for and or but nor his her its their our your my with from into onto is was be "
                "he she it they we you i".split())


def wrap(words, width_of, max_w, max_lines, em=0.0):
    """`words` in as few lines as fit `max_w` (width_of measures a line), at most `max_lines`,
    split as evenly as the words allow; words are never broken. With `em` (the font size) it is
    set as prose: a line rather breaks after a comma than after a little word ("a", "of", "he")
    or a possessive, and the last line is rather shorter than the first. None when they don't fit."""
    if not words or max_lines < 1 or any(width_of(w) > max_w for w in words):
        return None
    n, line = 1, []
    for wd in words:
        if line and width_of(" ".join(line + [wd])) > max_w:
            n, line = n + 1, [wd]
        else:
            line.append(wd)
    if n > max_lines:
        return None
    best = None
    for cut in combinations(range(1, len(words)), n - 1):
        lines = [" ".join(words[a:b]) for a, b in zip((0,) + cut, cut + (len(words),))]
        widths = [width_of(ln) for ln in lines]
        if max(widths) > max_w:
            continue
        cost = max(widths)
        if em and n > 1:
            for c in cut:
                end = words[c - 1]
                little = end.lower() in SHORT_END or end.lower().endswith(("'s", "\u2019s"))  # "Dowan's / name"
                cost += em * (2.0 if little else -1.0 if end[-1] in ",;:" else 0.0)
            cost += 0.4 * max(0.0, widths[-1] - widths[0])
        if best is None or cost < best[0] - 0.01:
            best = (cost, lines)
    return best[1] if best else None


def readable(colour, luma=0.3):
    """`colour` (RGB), lightened towards white until text in it reads on a dark shade (its
    relative luminance at least `luma`)."""
    def lum(c):
        lin = [(v / 255) / 12.92 if v / 255 <= 0.04045 else ((v / 255 + 0.055) / 1.055) ** 2.4 for v in c]
        return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
    for k in range(20):
        mix = tuple(round(v + (255 - v) * k / 20) for v in colour)
        if lum(mix) >= luma:
            return mix
    return (255, 255, 255)


def fit_lines(text, path, size, min_size, max_w, max_lines, tracking=0.0, floor=None):
    """(font, lines): `text` on one line at the biggest size from `size` down to `min_size` that
    fits `max_w`, else on up to `max_lines` lines broken at spaces, from `size` down to `floor`
    (`min_size` unless given); None when nothing fits. `tracking` is a share of the size."""
    s = size
    while s >= min_size - 1e-6:
        f = _font(path, round(s))
        if text_width(f, text, tracking * f.size) <= max_w:
            return f, [text]
        s *= 0.94
    s, floor = size, min_size if floor is None else floor
    while max_lines > 1 and s >= floor - 1e-6:
        f = _font(path, round(s))
        lines = wrap(text.split(), lambda ln: text_width(f, ln, tracking * f.size), max_w, max_lines)
        if lines:
            return f, lines
        s *= 0.94
    return None


def soft_shadow(img, h, strength):
    """`img` (RGBA) over a soft dark shadow of itself, a touch lower."""
    a = img.getchannel("A").filter(ImageFilter.GaussianBlur(max(1.0, h * 0.0026)))
    out = Image.new("RGBA", img.size, (0, 0, 0, 0))
    shadow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    shadow.putalpha(a.point(lambda v: int(v * strength)))
    out.alpha_composite(shadow, (0, max(1, round(h * 0.0012))))
    out.alpha_composite(img)
    return out


def text_line(runs, h, shadow=0.6):
    """One line of text runs [(text, font, colour, tracking px)] side by side on one baseline, over
    a soft shadow: (RGBA image, where the line starts in it, its baseline in it, its width)."""
    fonts = [r[1] for r in runs]
    asc = max(f.getmetrics()[0] for f in fonts)
    desc = max(f.getmetrics()[1] for f in fonts)
    pad = max(3, math.ceil(h * 0.008))
    width = sum(text_width(f, t, tr) for t, f, _, tr in runs) + sum(tr for t, f, _, tr in runs[:-1] if t)
    img = Image.new("RGBA", (math.ceil(width) + 2 * pad, asc + desc + 2 * pad), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    x, base = float(pad), pad + asc
    for text, f, colour, tr in runs:
        for part in (list(text) if tr else [text]):
            d.text((x, base), part, font=f, fill=tuple(colour) + (255,), anchor="ls")
            x += f.getlength(part) + tr
    return soft_shadow(img, h, shadow), pad, base, width


def chip_image(text, font, tracking, accent, h):
    """A trait as a small rounded chip: its uppercase text in a pill with a thin accent outline and
    a soft accent fill, over a faint shadow (RGBA, padded by `pad` all round): (image, pad)."""
    tw = text_width(font, text, tracking)
    ch = round(font.size * 2.2)
    cw = math.ceil(tw + 2 * font.size * 1.0)
    pad = max(3, math.ceil(h * 0.008))
    s = 4
    big = Image.new("RGBA", (cw * s, ch * s), (0, 0, 0, 0))
    lw = max(1, round(h * 0.0014))
    fill = tuple(round(c * 0.3) for c in accent) + (150,)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, cw * s - 1, ch * s - 1), radius=ch * s / 2, fill=fill, outline=tuple(accent) + (235,), width=lw * s)
    pill = Image.new("RGBA", (cw + 2 * pad, ch + 2 * pad), (0, 0, 0, 0))
    pill.alpha_composite(big.resize((cw, ch), Image.LANCZOS), (pad, pad))
    d = ImageDraw.Draw(pill)
    x, base = pad + (cw - tw) / 2, pad + (ch + cap_height(font)) / 2
    for c in text:
        d.text((x, base), c, font=font, fill=CHIP_TEXT + (255,), anchor="ls")
        x += font.getlength(c) + tracking
    return soft_shadow(pill, h, 0.35), pad


class Profile:
    """A character's profile laid out for its card, as a block of parts: the name (Anton, in
    capitals) with the accent bar under it, the role (the accent colour, small tracked capitals),
    the bio (one sentence, up to three lines), the traits as chips and the facts on one muted line
    (" · " between them), aligned on the block's outer edge (`align` "left" or "right") in a
    column `max_w` wide and `max_h` tall. When it is too big for that, all of it gets a little
    smaller (down to 86 %); then the facts, the chips, the bio and the role are left out in turn
    (each time at those sizes again), and only then is it smaller still (down to 72 %). Names and
    lines only ever break at spaces.

    `parts`: [(kind, RGBA image, x, y, arrives)] (each image's top-left in the block); `width` x
    `height`: the block, from the name's cap top to the last line's baseline. draw() puts it on a
    frame at its time in the card."""

    def __init__(self, name, prof, accent, h, max_w, max_h, align="left"):
        self.name, self.prof, self.accent, self.h = _clean(name), prof or {}, tuple(accent), h
        self.max_w, self.max_h, self.align = max(1.0, max_w), max(1.0, max_h), align
        self.scale, self.dropped = 1.0, ()
        drops = ((), ("facts",), ("facts", "chips"), ("facts", "chips", "bio"), ("facts", "chips", "bio", "role"))
        for scales in ((1.0, 0.93, 0.86), (0.79, 0.72)):
            for drop in drops:
                for scale in scales:
                    if self._lay(scale, drop):
                        return
        self._lay(0.72, drops[-1], force=True)

    def _lay(self, scale, drop, force=False):
        h, mw, p = self.h, self.max_w, self.prof
        rows = []  # [y of the row's top, its width, [(kind, image, x in the row, arrives)]]
        said = {}  # the lines of each part, as laid out
        name = self.name.upper() or " "
        nw = min(mw, NAME_MAX_W * h)
        fitted = fit_lines(name, NAME_FONT, TYPE["name"] * h * scale, TYPE["name_min"] * h * scale, nw, 2, floor=h * 0.03)
        if fitted is None:
            # A single word too long for the column at any sensible size: one line that fits.
            f = _font(NAME_FONT, round(TYPE["name"] * h * scale))
            f = _font(NAME_FONT, max(7, int(f.size * nw / max(1.0, text_width(f, name)))))
            fitted = (f, [name])
        f, lines = fitted
        said["name"] = lines
        cap = cap_height(f)
        base = cap
        name_w = 0.0
        for i, ln in enumerate(lines):
            img, pad, b, wd = text_line([(ln, f, NAME_COLOUR, 0.0)], h, 0.7)
            rows.append([base - b, wd, [("name", img, -pad, INFO_AT["name"])]])
            name_w = max(name_w, wd)
            if i < len(lines) - 1:
                base += cap + 0.2 * f.size
        bar_h = max(2, round(h * 0.0052))
        bar_w = max(h * 0.05, min(name_w * 0.42, h * 0.2))
        y = base + GAP["bar"] * h * scale
        bar = Image.new("RGBA", (math.ceil(bar_w), bar_h), self.accent + (255,))
        rows.append([y, bar_w, [("bar", bar, 0, INFO_AT["bar"])]])
        y += bar_h
        last = y
        if p.get("role") and "role" not in drop:
            size = TYPE["role"] * h * scale
            fitted = fit_lines(p["role"].upper(), ROLE_FONT, size, size * 0.88, mw, 2, TRACK["role"], floor=size * 0.82)
            if fitted is None:
                return False
            rf, rlines = fitted
            said["role"] = rlines
            tr = TRACK["role"] * rf.size
            base = y + GAP["role"] * h * scale + cap_height(rf)
            for i, ln in enumerate(rlines):
                img, pad, b, wd = text_line([(ln, rf, readable(self.accent), tr)], h, 0.6)
                rows.append([base - b, wd, [("role", img, -pad, INFO_AT["role"])]])
                last = base
                base += rf.size * 1.55
        if p.get("bio") and "bio" not in drop:
            size, blines = TYPE["bio"] * h * scale, None
            for s in (size, size * 0.94, size * 0.88):
                bf = _font(BODY_FONT, round(s))
                blines = wrap(p["bio"].split(), bf.getlength, min(mw, BIO_MAX_W * h), 3, em=bf.size)
                if blines:
                    break
            if not blines:
                return False
            said["bio"] = blines
            base = last + GAP["bio"] * h * scale + cap_height(bf)
            for i, ln in enumerate(blines):
                img, pad, b, wd = text_line([(ln, bf, BIO_COLOUR, 0.0)], h, 0.6)
                rows.append([base - b, wd, [("bio", img, -pad, INFO_AT["bio"] + i * INFO_AT["bio_line"])]])
                last = base
                base += bf.size * 1.42
        traits = [t for t in (p.get("traits") or []) if t][:3]
        if traits and "chips" not in drop:
            cf = _font(CHIP_FONT, round(TYPE["chip"] * h * scale))
            chips = [chip_image(t.upper(), cf, TRACK["chip"] * cf.size, self.accent, h) for t in traits]
            gap = GAP["chip"] * h * scale
            top = last + GAP["chips"] * h * scale
            row, x = [], 0.0
            for k, (img, pad) in enumerate(chips):
                cw = img.width - 2 * pad
                if row and x + gap + cw > mw:
                    rows.append([top - pad, x, row])
                    top += img.height - 2 * pad + gap
                    row, x = [], 0.0
                if cw > mw:
                    return False
                x += (gap if row else 0.0)
                row.append(("chip", img, x - pad, INFO_AT["chip"] + k * INFO_AT["chip_step"]))
                x += cw
            rows.append([top - pad, x, row])
            last = top + chips[0][0].height - 2 * chips[0][1]
            said["traits"] = [t.upper() for t in traits]
        facts = [t for t in (p.get("facts") or []) if t][:2]
        if facts and "facts" not in drop:
            ff = _font(BODY_FONT, round(TYPE["fact"] * h * scale))
            sep = (" · ", ff, self.accent, 0.0)
            one = []
            for k, ft in enumerate(facts):
                one += ([sep] if k else []) + [(ft, ff, FACT_COLOUR, 0.0)]
            if sum(text_width(r[1], r[0]) for r in one) <= mw:
                flines = [one]
            else:
                flines = []
                for ft in facts:
                    for ln in wrap(ft.split(), ff.getlength, mw, 2, em=ff.size) or [None]:
                        if ln is None:
                            return False
                        flines.append([(ln, ff, FACT_COLOUR, 0.0)])
            said["facts"] = ["".join(r[0] for r in runs) for runs in flines]
            base = last + GAP["facts"] * h * scale + cap_height(ff)
            for runs in flines:
                img, pad, b, wd = text_line(runs, h, 0.6)
                rows.append([base - b, wd, [("fact", img, -pad, INFO_AT["fact"])]])
                last = base
                base += ff.size * 1.5
        width = max(r[1] for r in rows)
        if not force and (width > mw + 0.5 or last > self.max_h):
            return False
        self.width, self.height, self.scale, self.dropped, self.lines = width, last, scale, drop, said
        self.parts = []
        for ry, rw, items in rows:
            rx = 0.0 if self.align == "left" else width - rw
            for kind, img, x, at in items:
                self.parts.append((kind, img, rx + x, ry, at))
        return True

    def shown(self, t):
        """How far the profile has come in at `t` into its card (the name's arrival: 0..1)."""
        return ease_out((t - INFO_AT["name"]) / INFO_IN)

    def draw(self, img, t, x0, y0, gone=0.0):
        """Draws the profile onto the frame `img` at `t` into its card, the block's top-left at
        (x0, y0): each part fades and slides in from the outer side in turn (the bar grows from
        it); `gone` (0..1) fades it all out."""
        out = 1.0 - min(1.0, max(0.0, gone))
        sgn = -1.0 if self.align == "left" else 1.0
        for kind, part, x, y, at in self.parts:
            k = ease_out((t - at) / INFO_IN)
            a = k * out
            if a <= 0.004:
                continue
            dx = sgn * (1 - k) * self.h * INFO_SLIDE["name" if kind == "name" else "other"]
            if kind == "bar":
                # The bar grows from the outer edge instead of sliding.
                wd = max(1, round(part.width * k))
                if self.align != "left":
                    x += part.width - wd
                part, dx = part.crop((0, 0, wd, part.height)), 0.0
            if a < 0.996:
                part = part.copy()
                part.putalpha(part.getchannel("A").point(lambda v, a=a: int(v * a + 0.5)))
            img.alpha_composite(part, (round(x0 + x + dx), round(y0 + y)))


def open_crop(path, rel):
    page = Image.open(path).convert("RGB")
    x0, y0, x1, y1 = rel or [0, 0, 1, 1]
    box = (round(x0 * page.width), round(y0 * page.height), round(x1 * page.width), round(y1 * page.height))
    return page.crop(box) if box[2] - box[0] > 8 and box[3] - box[1] > 8 else page


def grade(img, h):
    """A backdrop behind a character: a little darker, flatter and softer than the character, so
    the character reads in front of it."""
    img = ImageEnhance.Color(img).enhance(0.92)
    img = ImageEnhance.Contrast(img).enhance(0.9)
    img = ImageEnhance.Brightness(img).enhance(0.82)
    return img.filter(ImageFilter.GaussianBlur(max(0.8, h / 640)))


def stands_right(spec, index):
    """The character stands on the card's right (turned towards its middle, so a drawing that faces
    left stands on the right); without a known facing, every other card swaps sides."""
    facing = spec.get("facing")
    return facing == "left" if facing in ("left", "right") else index % 2 == 0


def backdrop_image(spec):
    """The card's background: the character's painted one (mirrored when it was painted for the
    other side), else an establishing panel of the manga, else None."""
    bg = spec.get("backdrop")
    if bg and os.path.isfile(bg):
        try:
            im = Image.open(bg).convert("RGB")
            return im.transpose(Image.Transpose.FLIP_LEFT_RIGHT) if spec.get("backdrop_flip") else im
        except OSError:
            pass
    if spec.get("env") and os.path.isfile(spec["env"]):
        try:
            return open_crop(spec["env"], spec.get("env_crop"))
        except OSError:
            pass
    return None


class Card:
    """One character, the way a game's loading screen shows its cast: a painted background on a
    wide, gently tilted card over black, and the character standing whole in front of it at the
    same height as every other character, the head breaking out of the card's top edge and the
    feet just past its bottom, with a soft shadow on the card behind. The background drifts slowly
    inside the card while the character, nearer the camera, moves a little against it. The card
    slides in from one side and out to the other (the last one stays: the transition grows out of
    it). Without a cut-out the character's own panel fills the card instead.

    The character's profile (Profile) sits low on the card's other side, over a soft shade that
    darkens that corner of the card just enough for it to read: its outer edge a margin in from
    the card's side, its last line a margin above the tilted bottom edge, and never nearer the
    character than a clear gap. It builds up once the card has landed and goes as the card
    leaves, riding out with it (the last card's goes before the transition takes the card)."""

    def __init__(self, spec, w, h, accent, index):
        self.w, self.h, self.accent = w, h, tuple(accent)
        self.right = stands_right(spec, index)
        cut = spec.get("cutout")
        self.cut = Image.open(cut).convert("RGBA") if cut and os.path.isfile(cut) else None
        if self.cut is not None:
            bb = self.cut.getchannel("A").getbbox()
            self.cut = self.cut.crop(bb) if bb else None
        drawn = bool(spec.get("drawn")) and self.cut is not None
        art = backdrop_image(spec) if self.cut is not None else None
        if art is None and self.cut is not None and drawn and spec.get("image") and os.path.isfile(spec["image"]):
            art = open_crop(spec["image"], spec.get("crop")).filter(ImageFilter.GaussianBlur(8))
        if art is None:
            if spec.get("image") and os.path.isfile(spec["image"]):
                art = open_crop(spec["image"], spec.get("crop"))
                self.cut = None
            elif self.cut is not None:
                art = Image.new("RGB", (64, 36), tuple(int(c * 0.22) for c in self.accent))
            else:
                raise ValueError(f"no picture of {spec.get('name')}")
        ch = h * (0.62 if self.cut is not None else 0.72)
        cw = min(w * 0.80, max(ch * 0.95, ch * art.width / art.height)) if self.cut is None else w * 0.74
        self.cw, self.ch = int(cw), int(ch)
        self.art = fit(art, int(cw * 1.08), int(ch * 1.08))
        if self.cut is not None:
            self.art = grade(self.art, h)
        k = 0.06 * ch
        lean = 1 if self.right else -1
        self.quad = [(0, k if lean > 0 else 0), (cw, 0 if lean > 0 else k), (cw, ch - (0 if lean > 0 else k)), (0, ch - (k if lean > 0 else 0))]
        self.coeffs = solve_perspective([(0, 0), (cw, 0), (cw, ch), (0, ch)], self.quad)
        self.mask = Image.new("L", (self.cw, self.ch), 0)
        ImageDraw.Draw(self.mask).polygon(self.quad, fill=255)
        self.edge = Image.new("RGBA", (self.cw, self.ch), (0, 0, 0, 0))
        ImageDraw.Draw(self.edge).line(self.quad + [self.quad[0]], fill=(235, 235, 235, 200), width=max(2, h // 400))
        self.home = (w - self.cw) / 2
        self.top = (h - self.ch) / 2 + (h * 0.05 if self.cut is not None else -h * 0.03)
        self.drawn = drawn
        if self.cut is not None:
            # Every character the same height, with a soft shadow cast on the card behind.
            fh = int(h * 0.86)
            s = fh / self.cut.height
            self.cut = self.cut.resize((max(1, round(self.cut.width * s)), fh), Image.LANCZOS)
            if self.cut.width > w * 0.48:
                s2 = w * 0.48 / self.cut.width
                self.cut = self.cut.resize((round(self.cut.width * s2), round(self.cut.height * s2)), Image.LANCZOS)
            a = self.cut.getchannel("A")
            sh = Image.new("RGBA", self.cut.size, (0, 0, 0, 0))
            sh.putalpha(a.point(lambda v: int(v * (0.62 if drawn else 0.55))))
            pad = h // 24
            self.shadow = Image.new("RGBA", (self.cut.width + 2 * pad, self.cut.height + 2 * pad), (0, 0, 0, 0))
            self.shadow.alpha_composite(sh, (pad, pad))
            self.shadow = self.shadow.filter(ImageFilter.GaussianBlur(h / (70 if drawn else 90)))
            self.shadow_pad = pad
        self.place_profile(spec)

    def figure_x(self, u):
        """The character's left edge in the frame at rest, `u` (0..1) through the card (it drifts
        towards the card's other side as the card goes on)."""
        side = 1 if self.right else -1
        return self.w * (0.70 if self.right else 0.30) - self.cut.width / 2 - side * self.w * 0.015 * u

    def place_profile(self, spec):
        """Lays the character's profile out low on the card's other side and the shade behind it:
        sets self.info (Profile), self.info_xy (its top-left in the frame, the card at rest) and
        self.scrim / self.scrim_box (the shade's strength over the card, 0..1, and where it is).
        A panel card with room beside it puts the profile on the black there instead (no shade)."""
        w, h, cw, ch = self.w, self.h, self.cw, self.ch
        k = 0.06 * ch
        left = self.right  # the profile goes on the side away from the character
        inside = self.cut is not None or self.home - 0.055 * w < 0.34 * h
        mx, mb, mt, gap = 0.052 * h, 0.05 * h, 0.04 * h, 0.06 * h
        if inside:
            xl = mx if left else cw - mx  # the outer edge on the card
            x_out = self.home + xl
            # The bottom edge is highest at the card's far (outer) side.
            bottom = self.top + (ch - k + k * xl / cw if self.right else ch - k * xl / cw) - mb
            top = self.top + k + mt
        else:
            x_out = 0.055 * w if left else w - 0.055 * w
            bottom, top = self.top + ch - k - mb, self.top + mt
        prof = profile_of(spec)

        def room_for(band):
            """How wide the profile's column can be with the profile `band` tall: up to a gap
            short of the character where it comes nearest while the profile shows (at the card's
            end), in the rows the profile takes (and a little around them)."""
            if self.cut is not None:
                fx = self.figure_x(1.0)
                cy = self.top + ch - self.cut.height + 0.06 * h
                r0 = int(min(max(0, bottom - band - 0.03 * h - cy), self.cut.height))
                r1 = int(min(max(0, bottom + 0.03 * h - cy), self.cut.height))
                a = self.cut.getchannel("A")
                bb = a.crop((0, r0, a.width, r1)).point(lambda v: 255 if v > 24 else 0).getbbox() if r1 > r0 else None
                if bb is None:
                    room = cw - 2 * mx
                else:
                    room = fx + bb[0] - gap - x_out if left else x_out - (fx + bb[2] + gap)
            elif inside:
                room = 0.42 * cw
            else:
                room = (self.home - 0.04 * h) - x_out if left else x_out - (self.home + cw + 0.04 * h)
            return min(room, 0.8 * h)

        def lay(col):
            return Profile(spec.get("name"), prof, self.accent, h, col, bottom - top, "left" if left else "right")

        # The column for a profile of the usual height; when the profile comes out taller and the
        # character comes nearer higher up, narrower (and the profile laid out again).
        band, col = 0.3 * h, None
        for _ in range(4):
            room = room_for(band)
            if col is not None and room >= col - 0.5:
                break
            col = room
            info = lay(col)
            if info.height <= band + 0.5:
                break
            band = info.height
        else:
            # Not settled: the room over all the height the profile may take.
            col = min(col, room_for(bottom - top))
            info = lay(col)
        self.info = info
        self.info_xy = (x_out if left else x_out - info.width, bottom - info.height)
        self.scrim, self.scrim_box = None, None
        if inside:
            self.scrim, self.scrim_box = self._shade(left, bottom)

    def _shade(self, left, bottom):
        """The shade behind the profile: that lower corner of the card darkened, falling off softly
        towards the card's middle and top, as strong as the picture there needs for white text to
        read (more on a bright picture than on a dark one)."""
        h, cw, ch = self.h, self.cw, self.ch
        ix, iy = self.info_xy
        x0, y0 = ix - self.home, iy - self.top
        x1 = x0 + self.info.width
        reach, up = 0.07 * h, 0.075 * h
        m = Image.new("L", (cw, ch), 0)
        box = (-cw, y0 - up, x1 + reach, 2 * ch) if left else (x0 - reach, y0 - up, 2 * cw, 2 * ch)
        ImageDraw.Draw(m).rounded_rectangle(box, radius=0.08 * h, fill=255)
        m = m.filter(ImageFilter.GaussianBlur(0.075 * h))
        shade = np.asarray(m, np.float32) / 255 * (np.asarray(self.mask, np.float32) / 255)
        # How bright the picture is behind the text (its brighter parts, as the background drifts).
        tx0, tx1 = int(max(0, x0)), int(min(cw, x1))
        ty0, ty1 = int(max(0, y0)), int(min(ch, bottom - self.top))
        bright = 0.0
        if tx1 > tx0 + 4 and ty1 > ty0 + 4:
            for u in (0.25, 0.75):
                px = np.asarray(self.picture(u).convert("RGB").crop((tx0, ty0, tx1, ty1)), np.float32) / 255
                luma = px @ np.array([0.2126, 0.7152, 0.0722], np.float32)
                bright = max(bright, float(np.percentile(luma, 85)))
        strength = min(0.82, max(0.45, 1 - 0.26 / max(bright, 1e-3)))
        shade *= strength
        ys, xs = np.nonzero(shade > 1 / 255)
        if not len(xs):
            return None, None
        bx = (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1)
        return shade[bx[1]:bx[3], bx[0]:bx[2]], bx

    def picture(self, u, shade=0.0):
        """The tilted card at progress `u` (0..1) of its time (RGBA, card-sized), its profile's
        shade drawn `shade` (0..1) strong."""
        dx = (self.art.width - self.cw) * (u if self.right else 1 - u)
        dy = (self.art.height - self.ch) * 0.5
        flat = self.art.crop((round(dx), round(dy), round(dx) + self.cw, round(dy) + self.ch))
        warped = flat.transform((self.cw, self.ch), Image.PERSPECTIVE, self.coeffs, Image.BICUBIC).convert("RGBA")
        warped.putalpha(self.mask)
        if shade > 0.004 and getattr(self, "scrim", None) is not None:
            x0, y0, x1, y1 = self.scrim_box
            px = np.asarray(warped.crop(self.scrim_box)).copy()
            px[..., :3] = (px[..., :3] * (1 - self.scrim * min(1.0, shade))[..., None] + 0.5).astype(np.uint8)
            warped.paste(Image.fromarray(px), (x0, y0))
        warped.alpha_composite(self.edge)
        return warped

    def layout(self, t, leaves=True):
        """Where everything is at `t` into this card: (card x, card y, character x, character y,
        how far it has left, the profile's x and y (its top-left), how far the profile has gone)."""
        w, h = self.w, self.h
        side = 1 if self.right else -1
        enter = ease_out(t / SLIDE_SEC)
        out = ease_in_out(1 + (t - CARD_SEC) / SLIDE_SEC)  # over the card's last SLIDE_SEC, exactly 1 at its end
        leave = out if leaves else 0.0
        home = self.home
        travel = lambda e, l: (1 - e) * (w - home + self.cw * 0.2) * side - l * (home + self.cw) * side
        x = home + travel(enter, leave)
        y = self.top
        cx = cy = None
        if self.cut is not None:
            # The character, nearer the camera: arrives a beat later, drifts against the scene.
            e2 = ease_out((t - 0.06) / (SLIDE_SEC + 0.12))
            u = t / CARD_SEC
            cx = (w * (0.70 if self.right else 0.30)) - self.cut.width / 2 + travel(e2, leave) * 1.15 - side * w * 0.015 * u
            cy = y + self.ch - self.cut.height + h * 0.06
        # The profile builds up after the card lands and goes as the card ends: riding out with
        # it, or before the last card turns into the transition.
        ix, iy = self.info_xy
        return x, y, cx, cy, leave, ix - leave * (home + self.cw) * side, iy, out

    def frame(self, t, leaves=True):
        """The frame (RGBA) at `t` into this card; in its last SLIDE_SEC it slides out (`leaves`)."""
        w, h = self.w, self.h
        x, y, cx, cy, leave, ix, iy, gone = self.layout(t, leaves)
        shade = self.info.shown(t) * (1 - gone)
        img = Image.new("RGBA", (w, h), (0, 0, 0, 255))
        img.alpha_composite(self.picture(t / CARD_SEC, shade), (round(x), round(y)))
        # On the card, so under the character (who only ever passes over it as the card leaves).
        self.info.draw(img, t, ix, iy, gone)
        if self.cut is not None:
            side = 1 if self.right else -1
            sp = self.shadow_pad
            off = h * (0.02 if self.drawn else 0.012)
            img.alpha_composite(self.shadow, (round(cx - sp - side * off), round(cy - sp + h * 0.012)))
            img.alpha_composite(self.cut, (round(cx), round(cy)))
        return img

    def surface(self, t):
        """What the transition turns into light at `t` (the card staying): the tilted card and the
        character, as a mask (L, frame-sized)."""
        x, y, cx, cy, _, _, _, _ = self.layout(t, leaves=False)
        m = Image.new("L", (self.w, self.h), 0)
        m.paste(self.mask, (round(x), round(y)), self.mask)
        if self.cut is not None:
            a = self.cut.getchannel("A")
            m.paste(a, (round(cx), round(cy)), a)
        return m


# ─────────────────────────────────────────────────────────────── colours

def _pixels(img, alpha=None, side=72):
    """`img`'s pixels (RGB 0..1, N x 3), small; only the opaque ones when it has alpha."""
    im = img.copy()
    im.thumbnail((side, side))
    a = np.asarray(im.convert("RGBA"), np.float32) / 255
    keep = a[..., 3] > 0.6
    return a[..., :3][keep]


def _hls(px):
    """Hue (0..1), lightness and saturation of RGB pixels (0..1, N x 3)."""
    mx, mn = px.max(axis=1), px.min(axis=1)
    l = (mx + mn) / 2
    d = mx - mn
    s = np.where(d < 1e-6, 0, d / np.maximum(1e-6, np.where(l < 0.5, mx + mn, 2 - mx - mn)))
    r, g, b = px[:, 0], px[:, 1], px[:, 2]
    safe = np.maximum(d, 1e-6)
    h = np.where(mx == r, ((g - b) / safe) % 6, np.where(mx == g, (b - r) / safe + 2, (r - g) / safe + 4)) / 6
    return np.where(d < 1e-6, 0, h), l, s


def card_palette(specs):
    """The intro's colours from its own cards (the backgrounds and the characters, the last card
    weighted most): {primary, secondary, accent, glow, light, dark} as hex. The transition widens
    the saturated ones into its spectrum; `accent` is the names' bar. Only the cards are looked at."""
    groups = []
    n = len(specs)
    for i, s in enumerate(specs):
        wt = 2.5 if i == n - 1 else 1.0
        parts = []
        try:
            bg = backdrop_image(s)
            if bg is not None:
                parts.append((_pixels(bg), 0.45))
        except (OSError, ValueError):
            pass
        for key in ("cutout", "image"):
            p = s.get(key)
            if p and os.path.isfile(p):
                try:
                    with Image.open(p) as im:
                        src = im if key == "cutout" else open_crop(p, s.get("crop"))
                        parts.append((_pixels(src), 0.55))
                except (OSError, ValueError):
                    pass
                break
        for px, share in parts:
            if len(px):
                groups.append((px, wt * share / len(px)))
    if not groups:
        return {}
    px = np.concatenate([g[0] for g in groups])
    wts = np.concatenate([np.full(len(g[0]), g[1]) for g in groups])
    h, l, s = _hls(px)
    chroma = (s > 0.22) & (l > 0.12) & (l < 0.88)
    pal = {}
    if chroma.any():
        bins = 36
        hist = np.bincount((h[chroma] * bins).astype(int) % bins, weights=(wts * s * (1 - np.abs(2 * l - 1)))[chroma], minlength=bins)
        hist = np.convolve(np.concatenate([hist[-2:], hist, hist[:2]]), [0.15, 0.2, 0.3, 0.2, 0.15], mode="valid")
        peaks = []
        for k in np.argsort(-hist):
            if hist[k] <= 0:
                break
            if all(min(abs(k - p), bins - abs(k - p)) >= 3 for p in peaks):
                peaks.append(int(k))
            if len(peaks) == 3:
                break

        def colour(k, light, sat):
            sel = chroma & (np.abs(((h - (k + 0.5) / bins) + 0.5) % 1 - 0.5) < 1.5 / bins)
            ww = wts[sel] * s[sel]
            ang = np.angle(np.sum(ww * np.exp(2j * np.pi * h[sel]))) / (2 * np.pi) % 1 if sel.any() else (k + 0.5) / bins
            ls = float(np.average(l[sel], weights=ww)) if sel.any() else 0.5
            ss = float(np.average(s[sel], weights=ww)) if sel.any() else 0.7
            return tr.rgb_hex([v * 255 for v in colorsys.hls_to_rgb(ang, light if light is not None else min(0.6, max(0.4, ls)), max(sat, min(0.95, ss)))])

        top = hist[peaks[0]]
        pal["primary"] = colour(peaks[0], None, 0.7)
        pal["glow"] = colour(peaks[0], 0.64, 0.8)
        second = [p for p in peaks[1:] if hist[p] >= 0.3 * top]
        pal["secondary"] = colour(second[0], None, 0.65) if second else colour(peaks[0], 0.42, 0.75)
        # The names' bar: the leading colour, bright enough on black.
        pal["accent"] = colour(peaks[0], 0.56, 0.82)
    bright = l > 0.72
    if bright.any():
        c = np.average(px[bright], axis=0, weights=wts[bright])
        hh, ll, ss = colorsys.rgb_to_hls(*c)
        pal["light"] = tr.rgb_hex([v * 255 for v in colorsys.hls_to_rgb(hh, max(0.8, ll), min(0.45, ss))])
    dark = l < 0.22
    if dark.any():
        c = np.average(px[dark], axis=0, weights=wts[dark])
        hh, ll, ss = colorsys.rgb_to_hls(*c)
        pal["dark"] = tr.rgb_hex([v * 255 for v in colorsys.hls_to_rgb(hh, min(0.07, ll), min(0.5, ss))])
    return pal


def accent_of(pal, style):
    """The names' accent bar (RGB)."""
    try:
        return tr.hex_rgb(pal["accent"])
    except (KeyError, ValueError, TypeError, AttributeError, IndexError):
        return STYLE_COLORS.get(style, STYLE_COLORS["epic"])[0]


# ─────────────────────────────────────────────────────────────── sound

def decode_stereo(path, seconds, start=0.0):
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error"]
    if start > 0:
        cmd += ["-ss", f"{start:.3f}"]
    cmd += ["-i", path, "-t", f"{seconds:.3f}", "-f", "f32le", "-ac", "2", "-ar", str(SR), "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True, creationflags=_NO_WINDOW).stdout
    return np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(np.float64)


# ITU-R BS.1770's K-weighting at 48 kHz: a high shelf, then the RLB high-pass (biquads b, a).
K_SHELF = ((1.53512485958697, -2.69169618940638, 1.19839281085285), (1.0, -1.69065929318241, 0.73248077421585))
K_HIGHPASS = ((1.0, -2.0, 1.0), (1.0, -1.99004745483398, 0.99007225036621))


def _k_weight(x):
    """x (n x 2) through the K-weighting filters of ITU-R BS.1770 (their exact magnitude, applied
    in the frequency domain: loudness only needs the energy)."""
    n = len(x)
    z = np.exp(-1j * 2 * np.pi * np.fft.rfftfreq(n, 1 / SR) / SR)
    gain = np.ones(len(z))
    for b, a in (K_SHELF, K_HIGHPASS):
        gain *= np.abs((b[0] + b[1] * z + b[2] * z * z) / (a[0] + a[1] * z + a[2] * z * z))
    return np.fft.irfft(np.fft.rfft(x, axis=0) * gain[:, None], n=n, axis=0)


def loudness(x):
    """Integrated loudness (LUFS, gated) of a stereo signal at SR, or None when it is silent."""
    if len(x) < int(0.4 * SR):
        return None
    k = _k_weight(x)
    step, size = int(0.1 * SR), int(0.4 * SR)
    ms = np.array([np.sum(np.mean(k[i:i + size] ** 2, axis=0)) for i in range(0, len(k) - size + 1, step)])
    lk = -0.691 + 10 * np.log10(np.maximum(ms, 1e-12))
    gated = ms[lk > -70]
    if not len(gated):
        return None
    rel = -0.691 + 10 * np.log10(np.mean(gated)) - 10
    gated = ms[(lk > -70) & (lk > rel)]
    return float(-0.691 + 10 * np.log10(np.mean(gated))) if len(gated) else None


def ease(x):
    """A 0..1 ramp that starts and ends gently (raised cosine): fades without a click or a step."""
    x = np.clip(x, 0.0, 1.0)
    return 0.5 - 0.5 * np.cos(np.pi * x)


def card_swish(seed=5):
    """The soft rush of air as a card slides in: band-limited noise (120 Hz to 1.1 kHz) swelling
    to its peak at CARD_SWISH_PEAK and fading, mono, peaking at 1. The shared effects' whoosh is a
    resonant sweep that peaks near 2.3 kHz; under the cards' music it rang like a "ting" on every
    card, so the cards have their own, darker and unpitched."""
    rng = np.random.default_rng(seed)
    n = int(CARD_SWISH_SEC * SR)
    t = np.arange(n) / SR
    env = np.where(t < CARD_SWISH_PEAK, (t / CARD_SWISH_PEAK) ** 2, np.exp(-(t - CARD_SWISH_PEAK) / 0.11))
    x = tr.fft_filter(rng.standard_normal(n), lo=120, hi=1100) * env
    x[-int(0.02 * SR):] *= np.linspace(1.0, 0.0, int(0.02 * SR))
    return x / (np.abs(x).max() + 1e-9)


def music_gain(t, total, hit):
    """The music's gain at times `t` (seconds) in an intro `total` seconds long whose big moment
    is at `hit`: it fades in over MUSIC_IN_SEC, rings on MUSIC_HOLD_SEC after the big moment, then
    fades out to silence by the end (never over less than MUSIC_OUT_MIN_SEC), so it never stops
    on a held note when the recap starts."""
    out_from = max(0.0, min(hit + MUSIC_HOLD_SEC, total - MUSIC_OUT_MIN_SEC))
    return ease(t / MUSIC_IN_SEC) * (1.0 - ease((t - out_from) / max(total - out_from, 1e-6)))


def soundtrack(n_cards, music, style=None):
    """The cards' music (read from where its big moment lands as the camera passes through the
    ribbons) and their whooshes, then the transition's own sound; about LOUDNESS LUFS."""
    total = total_seconds(n_cards)
    n = int(round(total * SR))
    mix = np.zeros((n, 2))
    trans_at = n_cards * CARD_SEC
    if music and os.path.isfile(music):
        try:
            m = decode_stereo(music, total, (MAX_CARDS - n_cards) * CARD_SEC)[:n]
        except (OSError, subprocess.CalledProcessError, ValueError):
            m = np.zeros((0, 2))
        t = np.arange(len(m)) / SR
        # Faded in, carried through the dive, faded out under the recap settling in.
        env = music_gain(t, total, trans_at + tr.PASS_SEC)
        mix[: len(m)] += m * env[:, None] * 0.75

    def place(x, at_sec, gain):
        at = int(round(at_sec * SR))
        if at < 0:
            x, at = x[-at:], 0
        x = x[: max(0, n - at)] * gain
        mix[at: at + len(x)] += x if x.ndim == 2 else x[:, None]

    swish = card_swish()
    for i in range(n_cards):
        place(swish, i * CARD_SEC + SLIDE_SEC * 0.5 - CARD_SWISH_PEAK, CARD_SWISH_GAIN)  # peaks as the card slides in
    # The shimmer in the music's key: C major for lo-fi, A minor for the rest.
    major = style == "lofi"
    place(tr.sound(root=65.41 if major else 55.0, minor=not major), trans_at, 1.0)
    # Everything (the transition's airy release too) ends in silence, not on a cut.
    i = np.arange(n)
    fade = np.minimum(1, i / (0.01 * SR)) * (1.0 - ease((i - (n - END_FADE_SEC * SR)) / (END_FADE_SEC * SR)))
    mix *= fade[:, None]
    # To LOUDNESS, a soft limit keeping the peaks under -1 dBFS (measured again after the limit,
    # which takes a little off the loudest moments).
    lim = 10 ** (-1.0 / 20)
    gain = 0.0
    out = mix
    for _ in range(3):
        out = np.tanh(mix * 10 ** (gain / 20) / lim) * lim
        lu = loudness(out)
        if lu is None or abs(lu - LOUDNESS) < 0.2:
            break
        gain += LOUDNESS - lu
    return out


def first_frame(video, w, h):
    """The recap's first frame (what the intro opens onto), or None."""
    if not video or not os.path.isfile(video):
        return None
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-ss", "0.05", "-i", video, "-frames:v", "1", "-vf", f"scale={w}:{h}", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    try:
        raw = subprocess.run(cmd, capture_output=True, check=True, creationflags=_NO_WINDOW).stdout
        return Image.frombytes("RGB", (w, h), raw[: w * h * 3]) if len(raw) >= w * h * 3 else None
    except (OSError, subprocess.CalledProcessError):
        return None


def write_wav_stereo(path, x):
    data = (np.clip(x, -1, 1) * 32767).astype("<i2").tobytes()
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(data)


# ─────────────────────────────────────────────────────────────── casting

# Words in the reader's panel descriptions that make a good backdrop, and ones that rule it out.
SCENE_GOOD = ("city", "skyline", "building", "street", "tower", "sky", "night", "moon", "room", "hall", "corridor", "school",
              "forest", "mountain", "sea", "ocean", "river", "bridge", "rooftop", "roof", "alley", "town", "village", "castle",
              "temple", "landscape", "exterior", "interior", "establishing", "view", "office", "shop", "station", "train",
              "road", "field", "clouds", "sunset", "window", "palace", "garden", "desert", "ruins", "arena")
PEOPLE = re.compile(r"\b(people|person|persons|crowd|students?|man|men|woman|women|boy|boys|girl|girls|figures?|monks?|children|kids?|soldiers?|"
                    r"guards?|he|she|his|her|they|someone|character|characters|silhouette|faces?|couple|group|townsfolk|villagers?)\b")
SCENE_BAD = ("blood", "naked", "nude", "corpse", "gore", "dead", "body", "bodies", "flesh", "wound", "tentacle", "text",
             "speech", "black panel", "blank", "white panel", "close-up", "closeup", "face", "eyes", "hand", "sex", "breast")
MAX_RESERVES = 2
# A body shot whose panel description speaks of the other sex than the character's description is
# someone else (a wrong merge); gore is no picture for a card; a tiny or turned-away figure shows
# little of the character.
MALE = re.compile(r"\b(man|men|boy|boys|male|guy|he|his|him|himself|father|brother|son|husband|king|prince|monk|gentleman|butler)\b", re.I)
FEMALE = re.compile(r"\b(woman|women|girl|girls|female|lady|she|her|hers|herself|mother|sister|daughter|wife|queen|princess|maid)\b", re.I)
GORE = re.compile(r"\b(gor[ey]|blood|bloody|bloodied|bloodstained|blood-stained|corpses?|severed|entrails|guts|naked|nude)\b", re.I)
FAR = re.compile(r"\b(tiny|distant|far away|far-off|in the distance|from behind|seen from behind|back view|from the back|silhouetted|upside-down|upside down)\b", re.I)
# A close-up is no full-body shot (however much panel there is below the face).
CLOSE = re.compile(r"\b(close-up|closeup|close up|extreme close|portrait|bust shot|head and shoulders)\b", re.I)
# Words that describe a look (hair, colours, clothes, marks), to tell whom a panel shows.
LOOK_WORDS = set("""hair bangs ponytail bob bun braid bald shaved spiky messy curly wavy straight slicked swept tousled long short
black white grey red blue navy brown blond silver green yellow orange purple pink crimson golden pale dark light
shirt tie blazer jacket hoodie turtleneck coat suit robe sweater vest uniform track skirt dress cap hat helmet apron scarf
tank shorts pants jeans trousers boots sneakers sash gloves cloak armor armour lab collar kimono hanbok tracksuit
glasses sunglasses mask respirator earring earrings bandage scar scars tattoo tattoos mole fangs horns antennae beard
stubble moustache mustache freckles muscular heavyset chubby slim lean tall""".split())
LOOK_SAME = {"haired": "hair", "tattooed": "tattoo", "tattoos": "tattoo", "gray": "grey", "blonde": "blond", "bespectacled": "glasses",
             "scarred": "scar", "scars": "scar", "masked": "mask", "earrings": "earring", "bandaged": "bandage", "armour": "armor"}


def hair(text):
    """'bald', 'short' or 'long' when `text` says how the hair is, else None."""
    t = (text or "").lower()
    if re.search(r"\b(bald|shaved head|shaven head|shaved-head|shaved)\b", t):
        return "bald"
    if re.search(r"\b(ponytail|pigtails|long-haired|long(?: [\w-]+){0,3} hair)\b", t):
        return "long"
    if re.search(r"\b(short-haired|bob|buzz cut|crew cut|short(?: [\w-]+){0,3} hair)\b", t):
        return "short"
    return None


def looks(text):
    """The look a description gives: its look words and their pairs ("track jacket", "red tie")."""
    words = [LOOK_SAME.get(w, w) for w in re.findall(r"[a-z]+", (text or "").lower())]
    marks = {w for w in words if w in LOOK_WORDS}
    pairs = {f"{a} {b}" for a, b in zip(words, words[1:]) if a in LOOK_WORDS and b in LOOK_WORDS}
    return marks, pairs


def look_match(a, b):
    return len(a[0] & b[0]) + 2 * len(a[1] & b[1])


def gender(text):
    """'m' or 'f' when `text` speaks of only one sex, else None."""
    m, f = bool(MALE.search(text or "")), bool(FEMALE.search(text or ""))
    return "m" if m and not f else "f" if f and not m else None


def norm_name(s):
    """A name for matching: lower-case letters and digits only ("Jung Da-hyun" is "Jung Dahyun")."""
    return "".join(ch for ch in (s or "").lower() if ch.isalnum())


def cast_cards(a, drawable=None):
    """The cards from the chapters' reads (`chapters`: [{read, pages}]) and the cast (`cast`:
    [{name, aliases, id, description, faces}], the most seen first): for each character, up to
    five body shots (best first: where the reader called the character by its own name, before an
    alias, which can be a wrong merge; then a lone figure, standing whole in the panel, a good
    size, no dialogue over it), and a backdrop panel (an establishing shot with no one in it,
    neither black nor blank). A character with face crops but no body shot still gets a card
    when `drawable(character)` says a drawing of it can be shown. At most MAX_CARDS cards in the
    cast's order, then up to MAX_RESERVES reserves (with body shots; "reserve": true) that stand
    in for a card left without a picture. The pages read are indexed in a["_pages"] ({"chapter:
    page": {image, summary, scene, panels}}) for painting the characters' backgrounds."""
    cast = a.get("cast") or []
    own, alias = {}, {}
    for i, c in enumerate(cast):
        own.setdefault(norm_name(c.get("name")), i)
    for i, c in enumerate(cast):
        for n in c.get("aliases") or []:
            k = norm_name(n)
            if k and k not in own:
                alias.setdefault(k, i)
    shots = [[] for _ in cast]
    sexes = [gender(c.get("description")) for c in cast]
    cast_looks = [looks(c.get("description")) for c in cast]
    cast_hair = [hair(c.get("description")) for c in cast]
    scenes = []
    index = {}
    for ci, ch in enumerate(a.get("chapters") or []):
        try:
            with open(ch["read"], encoding="utf-8") as f:
                rd = json.load(f)
        except (OSError, ValueError, KeyError, TypeError):
            continue
        pages = ch.get("pages") or []
        for p in rd.get("pages", []):
            pi = p.get("index", 0)
            if not isinstance(pi, int) or pi >= len(pages):
                continue
            img = pages[pi]
            index[f"{ci}:{pi}"] = {"image": img, "summary": p.get("summary") or "", "scene": p.get("scene") or {},
                                   "panels": [{"bbox": pn.get("bbox") or [0, 0, 1, 1], "description": pn.get("description") or "",
                                               "people": len(pn.get("characters") or []), "talk": len(pn.get("lines") or []),
                                               "names": [str(w.get("name") or "") for w in pn.get("characters") or [] if isinstance(w, dict)]}
                                              for pn in p.get("panels", [])]}
            for pn in p.get("panels", []):
                P = pn.get("bbox") or [0, 0, 1, 1]
                pw, ph = P[2] - P[0], P[3] - P[1]
                people = pn.get("characters") or []
                talk = bool(pn.get("lines"))
                if not people and not talk and pw * ph > 0.08 and pw * 0.7 / max(ph, 1e-3) > 0.9:
                    desc = (pn.get("description") or "").lower()
                    good = sum(k in desc for k in SCENE_GOOD)
                    if good and not any(k in desc for k in SCENE_BAD) and not PEOPLE.search(desc):
                        scenes.append((good + pw * ph * 2 + min(1.0, pw * 0.7 / ph / 1.6), img, P))
                pdesc = pn.get("description") or ""
                plook = looks(pdesc)
                for who in people:
                    k = norm_name(who.get("name"))
                    if k in own:
                        i, by_own = own[k], True
                    elif k in alias:
                        i, by_own = alias[k], False
                    else:
                        continue
                    F = who.get("face")
                    if not F:
                        continue
                    fh = F[3] - F[1]
                    if fh < 0.025:
                        continue
                    bot = min(P[3], F[1] + 8.0 * fh)
                    body = (bot - F[1]) / fh
                    if body < 3.5:
                        continue
                    sex = gender(pdesc)
                    if (sexes[i] and sex and sex != sexes[i]) or GORE.search(pdesc):
                        continue
                    # The panel's description fits another of the cast far better: it shows them.
                    mine = look_match(plook, cast_looks[i])
                    other = max((look_match(plook, cast_looks[j]) for j in range(len(cast)) if j != i), default=0)
                    if other >= 4 and other >= mine + 2:
                        continue
                    worn = hair(pdesc)
                    if cast_hair[i] and worn and worn != cast_hair[i] and len(people) == 1:
                        continue
                    score = (min(body, 7.5) + fh * 20 + (3.0 if len(people) == 1 else 0) - (12.0 if talk else 0) - (6.0 if FAR.search(pdesc) else 0)
                             - (8.0 if CLOSE.search(pdesc) else 0))
                    others = [o.get("face") for o in people if o is not who and o.get("face")]
                    shots[i].append((by_own, score, img, P, F, bot, others, f"{ci}:{pi}", pdesc))
    a["_pages"] = index
    scenes.sort(key=lambda e: -e[0])
    ok = []
    for e in scenes[:40]:
        try:
            im = Image.open(e[1]).convert("L")
        except OSError:
            continue
        W, H = im.size
        c = im.crop((int(e[2][0] * W), int(e[2][1] * H), int(e[2][2] * W), int(e[2][3] * H))).resize((96, 64))
        st = ImageStat.Stat(c)
        if 40 < st.mean[0] < 215 and st.stddev[0] > 38 and all(e[1] != o[1] or e[2] != o[2] for o in ok):
            ok.append(e)
        if len(ok) >= MAX_CARDS + MAX_RESERVES:
            break
    cards, reserves = [], []
    sizes = {}
    for i, c in enumerate(cast):
        cands = sorted(shots[i], key=lambda b: (not b[0], -b[1]))[:5]
        faces = [f for f in (c.get("faces") or []) if isinstance(f, str) and os.path.isfile(f)]
        full = len(cards) >= MAX_CARDS
        if not cands and (full or not (faces and drawable and drawable(c))):
            continue
        jobs = []
        for by_own, score, img, P, F, bot, others, page, pdesc in cands:
            if img not in sizes:
                try:
                    with Image.open(img) as pg:
                        sizes[img] = pg.size
                except OSError:
                    continue
            W, H = sizes[img]
            fh = F[3] - F[1]
            cx = (F[0] + F[2]) / 2
            box = [max(P[0], cx - 2.6 * fh * H / W) * W, max(P[1], F[1] - 0.6 * fh) * H, min(P[2], cx + 2.6 * fh * H / W) * W, bot * H]
            jobs.append({"image": img, "box": box, "point": [cx * W, (F[1] + F[3]) / 2 * H], "face_h": fh * H, "crop": [box[0] / W, box[1] / H, box[2] / W, box[3] / H],
                         "score": round(score, 2), "own_name": by_own,
                         "chest": [cx * W, min(bot, F[3] + 1.6 * fh) * H],
                         "not": [[(o[0] + o[2]) / 2 * W, (o[1] + o[3]) / 2 * H] for o in others],
                         "panel": list(P), "page": page, "description": pdesc})
        if not jobs and (full or not (faces and drawable and drawable(c))):
            continue
        card = {"name": c["name"].strip(), "id": c.get("id"), "description": c.get("description") or "", "faces": faces, "shots": jobs,
                "image": jobs[0]["image"] if jobs else None, "crop": jobs[0]["crop"] if jobs else None,
                "profile": c.get("profile"), "traits": c.get("traits")}
        if not full:
            cards.append(card)
        elif jobs:
            card["reserve"] = True
            reserves.append(card)
            if len(reserves) >= MAX_RESERVES:
                break
    out = cards + reserves
    for k, card in enumerate(out):
        e = ok[k % len(ok)] if ok else None
        card["env"], card["env_crop"] = (e[1], e[2]) if e else (None, None)
    return out


def _names(cs):
    n = [c["name"] for c in cs]
    return n[0] if len(n) == 1 else ", ".join(n[:-1]) + " and " + n[-1]


def cut_out(cards, a, work):
    """Each card's character cut out of its best shot (intro_roto.py in the layered-motion pack's
    venv, `roto_python`). Cards keep their panel when there is no pack or no clean cut."""
    py = a.get("roto_python")
    if not py or not os.path.isfile(py) or not cards:
        return
    jobs = [{"candidates": c["shots"], "out": os.path.join(work, f"cut_{i}_{intro_cast.slug(c.get('id') or c['name'])}.png")} for i, c in enumerate(cards)]
    jf = os.path.join(work, "roto_jobs.json")
    with open(jf, "w", encoding="utf-8") as f:
        json.dump(jobs, f)
    try:
        env = dict(os.environ)
        if a.get("roto_data"):
            env["MANGAVOX_DATA"] = a["roto_data"]
        r = subprocess.run([py, os.path.join(HERE, "intro_roto.py"), jf], capture_output=True, text=True, timeout=600, env=env, creationflags=_NO_WINDOW)
        last = (r.stdout.strip().splitlines() or ["{}"])[-1]
        done = set(json.loads(last).get("done") or [])
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return
    for c, j in zip(cards, jobs):
        if j["out"] in done:
            c["cutout"] = j["out"]


def choose_cards(cards, drawer, notes):
    """The cards to show, in the cast's order: those with a picture (a drawing, or a body shot to
    cut out or show), then reserves for any left without one; at most MAX_CARDS. Adds a note for
    each character left out."""
    final = []
    for c in cards:
        if c.get("reserve"):
            continue
        if c.get("cutout") or c.get("shots"):
            final.append(c)
        else:
            why = c.get("draw_fail") or (drawer.why_not if drawer is not None else None) or "it could not be drawn"
            notes.append(f"{c['name']}: {why}, and the manga has no full-body panel of them, so they were left out")
    for c in cards:
        if c.get("reserve") and len(final) < MAX_CARDS:
            final.append(c)
    return final[:MAX_CARDS]


# ─────────────────────────────────────────────────────────────── render

def stages(weights):
    """The progress bar's spans: each stage's share of 0..1 by its weight, in order."""
    total = float(sum(weights.values())) or 1.0
    spans, at = {}, 0.0
    for k, v in weights.items():
        spans[k] = (at, at + v / total)
        at += v / total
    return spans


def make_cards(a, report, prog, weights):
    """Casts the intro (with a `cast`): picks the cards, gives each character its kept drawing or
    a new one (intro_cast), cuts the others out of a panel, then gives each a painted background
    of its place (kept or new); sets a["cards"]. Credits and notes go to `report`. Returns the
    progress spans (`weights` updated once it is known what is made) and the specs of the cards
    to show."""
    span = stages(weights)
    if a.get("cast"):
        prog.set(0.0, "Choosing the cast", "cast")
        drawer = intro_cast.Drawer(a)
        try:
            cards = cast_cards(a, drawable=drawer.drawable)
            main = [c for c in cards if not c.get("reserve")]
            drawing = drawer.can and any(not drawer.kept(c) and not drawer.given_up(c) for c in main)
            # Cutting out of panels: certain for those that can't be drawn, a fallback otherwise.
            cutting = a.get("roto_python") and any(c.get("shots") and not drawer.kept(c) and (not drawer.can or drawer.given_up(c)) for c in main)
            painting = drawer.can and any(not drawer.kept_backdrop(c) and not drawer.backdrop_given_up(c) for c in main[:MAX_CARDS])
            weights.update({"draw": 40 if drawing else 1, "cutout": 8 if cutting else (2 if drawing and a.get("roto_python") else 0.5),
                            "backdrop": 40 if painting else 1})
            span = stages(weights)
            prog.set(span["cast"][1])
            drawer.draw(cards, prog, span["draw"])
            final = choose_cards(cards, drawer, report["notes"])
            need = [c for c in final if not c.get("cutout") and c.get("shots")]
            if need and a.get("roto_python") and os.path.isfile(a["roto_python"]):
                prog.set(span["cutout"][0], f"Cutting {_names(need)} out of {'a panel' if len(need) == 1 else 'the manga panels'}", "cutout")
                cut_out(need, a, a["work_dir"])
            prog.set(span["cutout"][1])
            for c in final:
                if not c.get("drawn") and c.get("draw_fail"):
                    how = "cut from a panel instead" if c.get("cutout") else "shown in its panel instead"
                    report["notes"].append(f"{c['name']}: {c['draw_fail']}, {how}")
            if drawer.why_not == "no Magnific API key" and any(not c.get("drawn") for c in final):
                report["notes"].append("No Magnific API key in Studio, so the characters were cut from the manga's panels")
            for i, c in enumerate(final):
                c["side"] = "right" if stands_right(c, i) else "left"
            drawer.paint([c for c in final if c.get("cutout")], a.get("_pages") or {}, prog, span["backdrop"])
            for c in final:
                if c.get("cutout") and not c.get("backdrop") and c.get("backdrop_fail"):
                    report["notes"].append(f"{c['name']}: {c['backdrop_fail']}, so a panel of the manga is the background")
        finally:
            report["credits"].extend(drawer.credits)
        a["cards"] = final
    specs = [c for c in a.get("cards", []) if c.get("name") and ((c.get("cutout") and os.path.isfile(c["cutout"]))
                                                               or (c.get("image") and os.path.isfile(c["image"])))][:MAX_CARDS]
    return span, specs


def render(a, report=None):
    """Renders the intro to a["out"]; returns (path, seconds). `report` ({"credits": [], "notes":
    []}) collects the credits spent and the notes for the app, also when this fails part-way."""
    report = report if report is not None else {"credits": [], "notes": []}
    report.setdefault("credits", [])
    report.setdefault("notes", [])
    prog = intro_cast.Progress(a.get("progress_file"))
    w, h = int(a["width"]) & ~1, int(a["height"]) & ~1
    fps_s = str(a.get("fps") or "30")
    fps = parse_fps(fps_s)
    style = a.get("style") or "epic"
    work = a["work_dir"]
    os.makedirs(work, exist_ok=True)
    weights = {"cast": 1, "draw": 1, "cutout": 1, "backdrop": 1, "cards": 8, "transition": 12, "encode": 2}
    span, specs = make_cards(a, report, prog, weights)
    if not specs:
        raise ValueError("no character has a picture yet")
    pal = card_palette(specs)
    accent = accent_of(pal, style)
    if "dark" not in pal:
        pal["dark"] = tr.rgb_hex(STYLE_COLORS.get(style, STYLE_COLORS["epic"])[1])
    cards = [Card(s, w, h, accent, i) for i, s in enumerate(specs)]
    reveal = first_frame(a.get("reveal"), w, h)
    n = len(cards)
    total = total_seconds(n)
    prog.set(span["cards"][0], "Mixing the intro's sound", "cards")
    wav = os.path.join(work, "intro.wav")
    write_wav_stereo(wav, soundtrack(n, a.get("music"), style))
    out = a["out"]
    tmp = out + ".part.mp4"
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{w}x{h}", "-r", fps_s, "-i", "-",
           "-i", wav, "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-preset", "medium", "-crf", "16", "-pix_fmt", "yuv420p",
           "-c:a", "aac", "-b:a", "256k", "-ar", str(SR), "-ac", "2", "-shortest", "-movflags", "+faststart", "-f", "mp4", tmp]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=_NO_WINDOW)
    all_n, card_n = frame_counts(n, fps)
    tail_n = all_n - card_n
    per_card = CARD_SEC * fps
    card_p, tail_p = prog.part(*span["cards"]), prog.part(*span["transition"])
    try:
        card_p(0.05, "Rendering the cast cards", "cards")
        for k in range(card_n):
            i = min(n - 1, int(math.floor(k / per_card + 1e-9)))
            t = max(0.0, k / fps - i * CARD_SEC)
            p.stdin.write(cards[i].frame(t, leaves=i < n - 1).convert("RGB").tobytes())
            card_p(0.05 + 0.95 * (k + 1) / card_n)
        # The last card, held where it ends, turns into the ribbons.
        last = cards[-1]
        src = np.asarray(last.frame(CARD_SEC, leaves=False).convert("RGB"))
        mask = np.asarray(last.surface(CARD_SEC), np.float32) / 255
        tail_p(0.0, f"Rendering the transition (frame 1 of {tail_n})", "transition")
        dive = tr.RibbonDive(w, h, src, mask, pal, reveal, fps)
        for k, img in enumerate(dive.frames(tail_n)):
            p.stdin.write(np.ascontiguousarray(img).tobytes())
            tail_p((k + 1) / tail_n, f"Rendering the transition (frame {k + 1} of {tail_n})", minor=True)
        prog.set(span["encode"][0], "Saving the intro", "encode")
        p.stdin.close()
    except BrokenPipeError:
        pass
    err = p.stderr.read().decode("utf-8", "replace")
    p.stderr.close()
    if p.wait() != 0:
        raise RuntimeError("ffmpeg: " + (err.strip().splitlines() or ["failed"])[-1])
    os.replace(tmp, out)
    prog.set(1.0, "Saving the intro", "encode")
    prog.flush()
    return out, total


def result_line(report, path=None, seconds=None, error=None):
    """The final JSON line for the app."""
    if error is None:
        v = {"ok": True, "path": path, "seconds": round(seconds, 3)}
    else:
        v = {"ok": False, "message": str(error) or type(error).__name__}
    v["credits"] = list(report.get("credits") or [])
    v["notes"] = list(report.get("notes") or [])
    return json.dumps(v)


def main():
    report = {"credits": [], "notes": []}
    try:
        with open(sys.argv[1], encoding="utf-8") as f:
            a = json.load(f)
        path, secs = render(a, report)
        print(result_line(report, path, secs))
    except Exception as e:  # noqa: BLE001 - reported to the app
        traceback.print_exc(file=sys.stderr)
        print(result_line(report, error=e))


if __name__ == "__main__":
    main()
