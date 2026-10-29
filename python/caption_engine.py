#!/usr/bin/env python3
"""Styled, word-timed captions for MangaVox videos.

The caption templates are the Watchfiwn ones (``caption_styles.json``: font, size, colours,
stroke, shadow / glow layers, background chip, word-highlight rule, entrance animation, words per
line). This module:

1. times every narrated word (``time_words``): the words of a segment are spread over the voiced
   part of its audio, with sentence / comma breaks snapped to the real pauses found in the WAV;
2. groups the words into short caption cues (``build_cues``);
3. writes an ASS script (``write_ass``) that libass (ffmpeg's ``subtitles`` filter) burns in.
   Every word is its own positioned event, so pop / rise / bounce / blur entrances, karaoke
   colour and scale, highlight boxes, hero-word layouts, glow and drop shadows all render
   exactly where the layout put them. Animations are sampled into short linear pieces
   (``\\move`` + ``\\t``), which reproduces any easing curve (overshoot, bounce, spring).

Settings (``resolve_settings``) come from the job's ``video_config["subtitles"]``, else the
chapter's ``subtitles.json``, else the Studio default (``<data>/studio/subtitles.json``).
"""
from __future__ import annotations

import json
import math
import os
import re
import struct
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
CATALOG_PATH = os.path.join(HERE, "caption_styles.json")
FONTS_DIR = os.path.join(HERE, "fonts")

# ----------------------------------------------------------------------------- catalog

_CATALOG = None


def catalog() -> dict:
    global _CATALOG
    if _CATALOG is None:
        with open(CATALOG_PATH, "r", encoding="utf-8") as fh:
            _CATALOG = json.load(fh)
    return _CATALOG


def style_ids() -> list[str]:
    return [s["id"] for s in catalog()["styles"]]


def get_style(style_id: str | None) -> dict:
    cat = catalog()
    for s in cat["styles"]:
        if s["id"] == style_id:
            return json.loads(json.dumps(s))
    for s in cat["styles"]:
        if s["id"] == cat.get("default"):
            return json.loads(json.dumps(s))
    return json.loads(json.dumps(cat["styles"][0]))


# ----------------------------------------------------------------------------- settings

DEFAULT_SETTINGS = {
    "enabled": True,
    "style": None,          # catalog id; None = catalog default
    "size": 1.0,            # multiplier on the style's size
    "posX": None,           # caption centre, % of the frame (None = the style's own)
    "posY": None,
    "wordsPerLine": 0,      # 0 = the style's own
    "lines": 0,             # 0 = the style's own
    "uppercase": None,      # None = the style's own
    "color": None,          # primary text colour override
    "highlightColor": None, # active-word colour override
    "emphasis": True,       # word highlight / pop / hero word on or off
}


def normalize_settings(raw) -> dict:
    s = dict(DEFAULT_SETTINGS)
    if isinstance(raw, str):
        raw = {"style": raw}
    if not isinstance(raw, dict):
        return s
    for k in DEFAULT_SETTINGS:
        if k in raw and raw[k] is not None:
            s[k] = raw[k]
    try:
        s["size"] = min(2.5, max(0.4, float(s["size"])))
    except (TypeError, ValueError):
        s["size"] = 1.0
    for k in ("posX", "posY"):
        if s[k] is not None:
            try:
                s[k] = min(100.0, max(0.0, float(s[k])))
            except (TypeError, ValueError):
                s[k] = None
    for k in ("wordsPerLine", "lines"):
        try:
            s[k] = max(0, min(12, int(s[k] or 0)))
        except (TypeError, ValueError):
            s[k] = 0
    s["enabled"] = bool(s["enabled"])
    s["emphasis"] = bool(s["emphasis"])
    return s


def _read_json(path):
    try:
        with open(path, "r", encoding="utf-8") as fh:
            return json.load(fh)
    except Exception:
        return None


def studio_default(output_dir: str | None):
    """The Studio-wide default (``<data>/studio/subtitles.json`` -> ``default``)."""
    candidates = []
    if output_dir:
        p = os.path.abspath(output_dir)
        for _ in range(6):
            parent = os.path.dirname(p)
            if parent == p:
                break
            p = parent
            if os.path.basename(p).lower() == "studio":
                candidates.append(os.path.join(p, "subtitles.json"))
    # MANGAVOX_DATA is the folder the app uses (set for every child; follows a moved data folder).
    home = os.environ.get("MANGAVOX_DATA") or os.environ.get("MANGAVOX_HOME") or os.path.join(os.path.expanduser("~"), ".mangavox")
    candidates.append(os.path.join(home, "studio", "subtitles.json"))
    for c in candidates:
        data = _read_json(c)
        if isinstance(data, dict):
            return data.get("default") if isinstance(data.get("default"), dict) else data
    return None


def resolve_settings(config: dict | None, output_dir: str | None) -> dict:
    config = config or {}
    raw = config.get("subtitles")
    if raw is None and output_dir:
        raw = _read_json(os.path.join(output_dir, "subtitles.json"))
    if raw is None:
        raw = studio_default(output_dir)
    s = normalize_settings(raw)
    if config.get("captions") is False:
        s["enabled"] = False
    return s


def apply_settings(style: dict, s: dict) -> dict:
    st = json.loads(json.dumps(style))
    st["size"] = float(st.get("size", 4.6)) * float(s.get("size") or 1.0)
    if s.get("posX") is not None:
        st["posX"] = float(s["posX"])
    if s.get("posY") is not None:
        st["posY"] = float(s["posY"])
    if s.get("wordsPerLine"):
        st["wordsPerLine"] = int(s["wordsPerLine"])
    if s.get("lines"):
        st["maxLines"] = int(s["lines"])
    if s.get("uppercase") is not None:
        st["uppercase"] = bool(s["uppercase"])
    dyn = st.get("dynamic")
    if s.get("color"):
        st["color"] = s["color"]
        if dyn:
            dyn["ink"] = s["color"]
    if s.get("highlightColor"):
        hc = s["highlightColor"]
        st["highlightColor"] = hc
        if st.get("highlightBox"):
            st["highlightBoxColor"] = hc
        if dyn:
            if dyn.get("fx") in ("marker", "box", "underline"):
                dyn["accent"] = hc
            else:
                dyn["hero"] = hc
    if not s.get("emphasis", True):
        for k in ("highlightWords", "karaokePop", "highlightBox", "progressiveHighlight"):
            st[k] = False
        st["highlightFont"] = None
        st["highlightScale"] = 1.0
        if dyn:
            dyn["heroScale"] = 1.0
            dyn["hero"] = dyn.get("ink")
            if dyn.get("fx") in ("marker", "box", "underline", "focus"):
                dyn["fx"] = None
    return st


# ----------------------------------------------------------------------------- colours


def parse_color(c, default=(255, 255, 255, 1.0)):
    """'#rgb' / '#rrggbb' / '#rrggbbaa' / 'rgb(a)(..)' / 'transparent' -> (r, g, b, a 0..1)."""
    if not c or not isinstance(c, str):
        return default
    c = c.strip().lower()
    if c == "transparent":
        return (0, 0, 0, 0.0)
    if c.startswith("#"):
        h = c[1:]
        if len(h) == 3:
            h = "".join(ch * 2 for ch in h)
        try:
            r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
            a = int(h[6:8], 16) / 255 if len(h) >= 8 else 1.0
            return (r, g, b, a)
        except ValueError:
            return default
    m = re.match(r"rgba?\(([^)]*)\)", c)
    if m:
        parts = [p.strip() for p in m.group(1).split(",")]
        try:
            r, g, b = (int(float(p)) for p in parts[:3])
            a = float(parts[3]) if len(parts) > 3 else 1.0
            return (r, g, b, max(0.0, min(1.0, a)))
        except ValueError:
            return default
    return default


def lerp_color(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(4))


def ass_bgr(c) -> str:
    r, g, b = (int(round(max(0, min(255, v)))) for v in c[:3])
    return f"&H{b:02X}{g:02X}{r:02X}&"


def ass_alpha(opacity: float) -> str:
    return f"&H{255 - int(round(255 * max(0.0, min(1.0, opacity)))):02X}&"


# ----------------------------------------------------------------------------- fonts


class Font:
    def __init__(self, key: str, spec: dict, italic: bool):
        self.key = key
        file = spec.get("italicFile") if italic and spec.get("italicFile") else spec["file"]
        self.path = os.path.join(FONTS_DIR, file)
        self.ass_name = spec.get("ass", spec.get("label", "Arial"))
        self.bold = bool(spec.get("assBold"))
        # libass sizes a font so that winAscent + winDescent == \fs (VSFilter rule).
        self.fs_per_em = 1.0
        upem, asc, desc = _ttf_metrics(self.path)
        if upem and asc + desc > 0:
            self.fs_per_em = (asc + desc) / upem
        self._pil = None
        self._widths = {}

    def width(self, text: str, em: float) -> float:
        """Advance width of `text` at an em size of `em` pixels."""
        if text not in self._widths:
            w = None
            try:
                if self._pil is None:
                    from PIL import ImageFont
                    self._pil = ImageFont.truetype(self.path, 200)
                w = self._pil.getlength(text) / 200.0
            except Exception:
                w = None
            if w is None:
                w = sum(0.32 if ch == " " else (0.62 if ch.isupper() else 0.54) for ch in text)
            self._widths[text] = w
        return self._widths[text] * em


_FONTS: dict = {}


def font(key: str | None, italic: bool = False) -> Font:
    specs = catalog()["fonts"]
    if key not in specs:
        key = "inter-bold"
    k = (key, bool(italic))
    if k not in _FONTS:
        _FONTS[k] = Font(key, specs[key], italic)
    return _FONTS[k]


def _ttf_metrics(path):
    """(unitsPerEm, usWinAscent, usWinDescent) read straight from the font's tables."""
    try:
        with open(path, "rb") as fh:
            data = fh.read()
        num = struct.unpack(">H", data[4:6])[0]
        tables = {}
        for i in range(num):
            rec = data[12 + 16 * i: 28 + 16 * i]
            tag = rec[:4].decode("latin-1")
            off = struct.unpack(">I", rec[8:12])[0]
            tables[tag] = off
        upem = struct.unpack(">H", data[tables["head"] + 18: tables["head"] + 20])[0]
        os2 = tables.get("OS/2")
        if os2 is not None:
            asc, desc = struct.unpack(">HH", data[os2 + 74: os2 + 78])
        else:
            hh = tables["hhea"]
            asc, desc = struct.unpack(">hh", data[hh + 4: hh + 8])
            desc = -desc
        return upem, asc, desc
    except Exception:
        return 0, 0, 0


# ----------------------------------------------------------------------------- easing + poses


def _clamp(v, a, b):
    return a if v < a else b if v > b else v


def ease(name: str, t: float, k: float = 1.0) -> float:
    t = _clamp(t, 0.0, 1.0)
    if name == "linear":
        return t
    if name == "hold":
        return 1.0 if t >= 1 else 0.0
    if name == "smooth":
        a = 1.2 + k * 0.9
        p = t ** a
        q = (1 - t) ** a
        return p / (p + q) if p + q > 0 else t
    if name == "slowdown":
        return 1 - (1 - t) ** (1.6 + k * 1.6)
    if name == "accelerate":
        return t ** (1.6 + k * 1.6)
    if name == "overshoot":
        s = 1.70158 * _clamp(k, 0.2, 3)
        u = t - 1
        return u * u * ((s + 1) * u + s) + 1
    if name == "elastic":
        if t <= 0:
            return 0.0
        if t >= 1:
            return 1.0
        p = _clamp(0.55 - 0.13 * k, 0.18, 0.8)
        return 2 ** (-9.5 * t) * math.sin((t - p / 4) * (2 * math.pi) / p) + 1
    if name == "bounce":
        n1, d1 = 7.5625, 2.75
        if t < 1 / d1:
            b = n1 * t * t
        elif t < 2 / d1:
            u = t - 1.5 / d1
            b = n1 * u * u + 0.75
        elif t < 2.5 / d1:
            u = t - 2.25 / d1
            b = n1 * u * u + 0.9375
        else:
            u = t - 2.625 / d1
            b = n1 * u * u + 0.984375
        return 1 - (1 - b) * _clamp(k, 0.2, 2.5)
    if name == "spring":
        if t <= 0:
            return 0.0
        if t >= 1:
            return 1.0
        zeta = _clamp(k, 0.05, 1.6) * 0.55
        wn = 7.2
        if zeta < 1:
            wd = wn * math.sqrt(1 - zeta * zeta)
            return 1 - math.exp(-zeta * wn * t) * (math.cos(wd * t) + (zeta * wn / wd) * math.sin(wd * t))
        return 1 - math.exp(-zeta * wn * t) * (1 + zeta * wn * t)
    # default: smooth
    return ease("smooth", t, k)


def _seeded(i, salt=1):
    x = math.sin(i * 127.1 + salt * 311.7) * 43758.5453
    return x - math.floor(x)


# Watchfiwn IN_PRESETS: the pose at progress 0; lerped to rest as the eased progress goes to 1.
_FROM = {
    "fade": lambda k: {"opacity": 0},
    "rise": lambda k: {"opacity": 0, "dy": 0.9 * k},
    "drop": lambda k: {"opacity": 0, "dy": -0.9 * k},
    "bounce": lambda k: {"opacity": 0, "dy": -0.9 * k},
    "slide-l": lambda k: {"opacity": 0, "dx": 1.6 * k},
    "slide-r": lambda k: {"opacity": 0, "dx": -1.6 * k},
    "pop": lambda k: {"opacity": 0, "scale": max(0.0, 1 - 0.85 * k)},
    "balloon": lambda k: {"opacity": 0, "scale": max(0.0, 1 - 0.85 * k)},
    "inflate": lambda k: {"opacity": 0, "scale": 0.01, "rot": -4 * k},
    "blur": lambda k: {"opacity": 0, "blur": 14 * k, "scale": 1 + 0.06 * k},
    "mask-up": lambda k: {"dy": 1.12, "clip": "line"},
    "reveal": lambda k: {"clipX": 0.0, "origin": "left"},
    "wipe-right": lambda k: {"clipX": 0.0, "origin": "left"},
    "wipe-left": lambda k: {"clipX": 0.0, "origin": "right"},
    "wipe-up": lambda k: {"clipY": 0.0, "origin": "bottom"},
    "wipe-down": lambda k: {"clipY": 0.0, "origin": "top"},
    "spin": lambda k: {"opacity": 0, "rot": -150 * k, "scale": 0.3},
    "skew": lambda k: {"opacity": 0, "dx": 1.7 * k, "rot": 9 * k},
    "stretch": lambda k: {"opacity": 0, "scaleX": 1 + 1.4 * k, "blur": 3 * k},
    "flip": lambda k: {"opacity": 0, "scaleX": 0.01, "dx": 0.15 * k},
    "zoom-out": lambda k: {"opacity": 0, "scale": 1 + 1.3 * k},
    "slam": lambda k: {"opacity": 0, "scale": 1 + 1.7 * k, "blur": 20 * k},
}

REST = {"dx": 0.0, "dy": 0.0, "scale": 1.0, "scaleX": 1.0, "scaleY": 1.0, "rot": 0.0, "opacity": 1.0,
        "blur": 0.0, "clipX": None, "clipY": None, "origin": None, "clip": None}


def preset_pose(name: str, e: float, k: float, i: int) -> dict:
    """Pose of an entrance preset at eased progress `e` (0 = start pose, 1 = rest)."""
    p = dict(REST)
    if name in _FROM:
        f = _FROM[name](k)
        for key in ("dx", "dy", "rot", "blur"):
            if key in f:
                p[key] = f[key] * (1 - e)
        if "opacity" in f:
            p["opacity"] = f["opacity"] + (1 - f["opacity"]) * min(e * 1.45, 1)
        for key in ("scale", "scaleX", "scaleY"):
            if key in f:
                p[key] = f[key] + (1 - f[key]) * e
        if "clipX" in f:
            p["clipX"] = _clamp(e, 0, 1)
            p["origin"] = f["origin"]
        if "clipY" in f:
            p["clipY"] = _clamp(e, 0, 1)
            p["origin"] = f["origin"]
        if f.get("clip") and e < 1:
            p["clip"] = "line"
        return p
    if name == "wave-in":
        if e < 1:
            p["opacity"] = min(1.0, e * 1.6)
            p["dy"] = -math.sin(_clamp(e, 0, 1) * math.pi) * 0.55 * k
            p["scale"] = 0.7 + 0.3 * e
        return p
    if name == "glitch":
        if e < 1:
            g = 1 - e
            on = _seeded(math.floor(e * 24) + i * 13) > 0.35 - g * 0.45
            p["opacity"] = 1.0 if on else 0.08
            p["dx"] = (_seeded(i, math.floor(e * 18)) - 0.5) * g * 0.7 * k
            p["dy"] = (_seeded(i, math.floor(e * 18) + 31) - 0.5) * g * 0.35 * k
        return p
    return p


# ----------------------------------------------------------------------------- word timing

_VOWELS = re.compile(r"[aeiouyàáâäèéêëìíîïòóôöùúûü]+", re.I)
_SENT_END = re.compile(r"[.!?…]+[\"'”’)\]]*$")
_CLAUSE_END = re.compile(r"[,;:—–-]+[\"'”’)\]]*$")


def _syllables(word: str) -> int:
    w = re.sub(r"[^a-zA-ZÀ-ɏ]", "", word)
    if not w:
        return max(1, len(word) // 3)
    n = len(_VOWELS.findall(w))
    if w.lower().endswith("e") and n > 1:
        n -= 1
    return max(1, n)


def _pause_weight(word: str) -> float:
    if _SENT_END.search(word):
        return 1.6
    if _CLAUSE_END.search(word):
        return 0.8
    return 0.0


def tokenize(text: str) -> list[str]:
    out: list[str] = []
    for tok in (text or "").split():
        if out and not re.search(r"[0-9A-Za-zÀ-￿]", tok):
            out[-1] += tok  # a stray "—" or "..." rides on the previous word
        else:
            out.append(tok)
    return out


_WAV_CACHE: dict = {}


def _read_wav(path):
    if path in _WAV_CACHE:
        return _WAV_CACHE[path]
    res = None
    try:
        import numpy as np
        with wave.open(path, "rb") as wf:
            sr = wf.getframerate()
            ch = wf.getnchannels()
            sw = wf.getsampwidth()
            raw = wf.readframes(wf.getnframes())
        if sw == 2:
            x = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
        elif sw == 4:
            x = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2147483648.0
        elif sw == 1:
            x = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128) / 128.0
        else:
            x = None
        if x is not None:
            if ch > 1:
                x = x.reshape(-1, ch).mean(axis=1)
            res = (x, sr)
    except Exception:
        res = None
    if len(_WAV_CACHE) > 8:
        _WAV_CACHE.clear()
    _WAV_CACHE[path] = res
    return res


def voiced_spans(path: str, offset: float, duration: float) -> list[tuple[float, float]]:
    """Speech regions (seconds, relative to `offset`) inside the audio slice."""
    w = _read_wav(path) if path else None
    if not w:
        return []
    import numpy as np
    x, sr = w
    a = int(max(0.0, offset) * sr)
    b = min(len(x), int((offset + duration) * sr))
    seg = x[a:b]
    hop = max(1, int(sr * 0.02))
    n = len(seg) // hop
    if n < 3:
        return []
    frames = seg[: n * hop].reshape(n, hop)
    rms = np.sqrt((frames ** 2).mean(axis=1) + 1e-12)
    db = 20 * np.log10(rms)
    peak = float(np.percentile(db, 95))
    thr = max(-48.0, peak - 28.0)
    voiced = db > thr
    spans = []
    i = 0
    while i < n:
        if voiced[i]:
            j = i
            while j < n and voiced[j]:
                j += 1
            spans.append([i * 0.02, j * 0.02])
            i = j
        else:
            i += 1
    # Merge blips across tiny gaps (stop consonants), drop clicks.
    merged = []
    for s in spans:
        if merged and s[0] - merged[-1][1] < 0.12:
            merged[-1][1] = s[1]
        else:
            merged.append(s)
    return [(s, e) for s, e in merged if e - s >= 0.06]


def _distribute(words, weights, a, b):
    total = sum(weights) or 1.0
    out = []
    t = a
    for w, wt in zip(words, weights):
        d = (b - a) * wt / total
        out.append([w, t, t + d])
        t += d
    return out


def time_words(text: str, start: float, end: float, audio_path: str | None = None, audio_offset: float = 0.0) -> list[dict]:
    """Word timings (absolute seconds) for a narrated segment.

    Words are weighted by syllables (plus a pause after punctuation) and spread over the voiced
    part of the audio slice. Phrase breaks are snapped to the longest real pauses in the WAV.
    """
    words = tokenize(text)
    if not words:
        return []
    dur = max(0.05, end - start)
    spans = voiced_spans(audio_path, audio_offset, dur) if audio_path else []
    lo, hi = 0.0, dur
    if spans:
        lo, hi = spans[0][0], spans[-1][1]
        if hi - lo < dur * 0.3:
            lo, hi = 0.0, dur
    syl = [0.5 + _syllables(w) for w in words]
    pause = [_pause_weight(w) for w in words]

    timed = None
    gaps = [(spans[i][1], spans[i + 1][0]) for i in range(len(spans) - 1) if spans[i + 1][0] - spans[i][1] >= 0.16 and lo <= spans[i][1] and spans[i + 1][0] <= hi]
    breaks = [i for i in range(len(words) - 1) if pause[i] > 0]
    if gaps and breaks:
        # Expected time of each break if speech were uniform, then match pauses to breaks in order.
        tot = sum(syl) + sum(pause[:-1])
        acc, expect = 0.0, {}
        for i in range(len(words) - 1):
            acc += syl[i] + pause[i]
            if i in breaks:
                expect[i] = lo + (hi - lo) * acc / tot
        biggest = sorted(gaps, key=lambda g: g[1] - g[0], reverse=True)[: len(breaks)]
        biggest.sort()
        anchors = []  # (word index ending the phrase, pause start, pause end)
        used = -1
        for g0, g1 in biggest:
            mid = (g0 + g1) / 2
            cands = [i for i in breaks if i > used]
            if not cands:
                break
            best = min(cands, key=lambda i: abs(expect[i] - mid) / (1.0 + pause[i] * 0.25))
            if abs(expect[best] - mid) <= max(0.6, (hi - lo) * 0.22):
                anchors.append((best, g0, g1))
                used = best
        if anchors:
            timed = []
            first, t0 = 0, lo
            for idx, g0, g1 in anchors:
                timed += _distribute(words[first: idx + 1], syl[first: idx + 1], t0, g0)
                first, t0 = idx + 1, g1
            timed += _distribute(words[first:], syl[first:], t0, hi)
    if timed is None:
        weights = [syl[i] + (pause[i] if i < len(words) - 1 else 0) for i in range(len(words))]
        timed = _distribute(words, weights, lo, hi)
        # The pause weight belongs after the word: pull each end back so the gap is silent.
        for i, row in enumerate(timed):
            if i < len(words) - 1 and pause[i] > 0:
                span = row[2] - row[1]
                row[2] = row[1] + span * syl[i] / (syl[i] + pause[i])
    return [{"text": w, "start": round(start + a, 3), "end": round(start + b, 3)} for w, a, b in timed]


# ----------------------------------------------------------------------------- cues


def build_cues(segments: list[dict], style: dict) -> list[dict]:
    """Caption cues (a few words each) from timed segments.

    Each segment: {"text", "start_ms", "end_ms", optional "audio_path", "audio_offset_ms"}.
    Cues never cross a segment (panel) boundary or a sentence end, and long sentences are split
    into even chunks so no cue is left with a lone word.
    """
    wpl = max(1, int(style.get("wordsPerLine") or 4))
    lines = max(1, int(style.get("maxLines") or 2))
    dyn = style.get("dynamic")
    per_cue = min(wpl * lines, int(dyn["maxWords"])) if dyn and not style.get("_userWords") else wpl * lines
    per_cue = max(1, per_cue)
    cues = []
    for seg in segments or []:
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        s = float(seg.get("start_ms", 0)) / 1000.0
        e = float(seg.get("end_ms", 0)) / 1000.0
        if e <= s:
            continue
        words = time_words(text, s, e, seg.get("audio_path"), float(seg.get("audio_offset_ms", 0) or 0) / 1000.0)
        if not words:
            continue
        # Sentences, then even chunks of at most per_cue words.
        sentences, cur = [], []
        for w in words:
            cur.append(w)
            if _SENT_END.search(w["text"]):
                sentences.append(cur)
                cur = []
        if cur:
            sentences.append(cur)
        chunks = []
        for sent in sentences:
            k = max(1, math.ceil(len(sent) / per_cue))
            size = math.ceil(len(sent) / k)
            for i in range(0, len(sent), size):
                chunks.append(sent[i: i + size])
        # Merge a trailing one-word sentence into the previous chunk when it still fits.
        merged = []
        for ch in chunks:
            if merged and len(ch) == 1 and len(merged[-1]) < per_cue:
                merged[-1] = merged[-1] + ch
            else:
                merged.append(ch)
        for i, ch in enumerate(merged):
            start = ch[0]["start"] if i else s
            end = merged[i + 1][0]["start"] if i + 1 < len(merged) else e
            # Each word stays active until the next one starts (no flicker in pauses).
            ws = []
            for j, w in enumerate(ch):
                nxt = ch[j + 1]["start"] if j + 1 < len(ch) else end
                ws.append({"text": w["text"], "start": max(start, w["start"]), "end": max(w["start"] + 0.01, nxt)})
            if end - start < 0.25:
                end = start + 0.25
            cues.append({"start": start, "end": end, "words": ws})
    # Keep cues ordered and non-overlapping.
    for a, b in zip(cues, cues[1:]):
        if a["end"] > b["start"]:
            a["end"] = b["start"]
    return [c for c in cues if c["end"] - c["start"] > 0.04]


# ----------------------------------------------------------------------------- layout

FILLERS = {
    "the", "a", "an", "is", "are", "to", "of", "and", "or", "but", "in", "on", "it", "i", "you", "we", "they",
    "he", "she", "was", "were", "be", "been", "am", "for", "with", "as", "at", "this", "that", "my", "your",
    "our", "me", "im", "i'm", "dont", "don't", "just", "so", "if", "not", "do", "did", "have", "has", "had",
    "from", "by", "its", "it's", "oh", "uh", "um", "like", "yeah", "ok", "okay", "well", "then",
}
PUNCH = {
    "need", "want", "make", "stop", "wait", "look", "love", "hate", "know", "think", "feel", "come", "take",
    "give", "keep", "try", "use", "find", "break", "ask", "way", "now", "never", "always", "more", "best",
    "free", "new", "big", "down", "hard", "real", "matters", "build", "story", "moment", "focus", "bold",
    "scream", "chase", "dreams", "power", "fight", "death", "dead", "kill", "blood", "fear", "war", "truth",
}
CANDY = ["#ff499e", "#ffe600", "#70e000", "#00f5d4", "#9d4edd"]
CARD_THEMES = [("#ccff00", "#000000", -2.2), ("#ffffff", "#0b0c10", 1.8), ("#7928ca", "#ffffff", -1.2)]


def _bare(word: str) -> str:
    return re.sub(r"[^a-z0-9']", "", word.lower())


class Layout:
    """Token centres / sizes for one state of a cue (which word is active)."""

    def __init__(self, tokens, block, lines):
        self.tokens = tokens   # [{x, y, em, w, line}]
        self.block = block     # (cx, cy, w, h)
        self.lines = lines     # [(cx, cy, w, em)]


def _line_groups(n: int, style: dict) -> list[list[int]]:
    dyn = style.get("dynamic")
    if dyn and dyn.get("stack") and not style.get("_userWords"):
        return [[i] for i in range(n)]
    wpl = max(1, int(style.get("wordsPerLine") or 4))
    if dyn and not style.get("_userWords"):
        if n <= 2:
            return [list(range(n))]
        half = math.ceil(n / 2)
        return [list(range(half)), list(range(half, n))]
    rows = math.ceil(n / wpl)
    size = math.ceil(n / rows)
    return [list(range(i, min(n, i + size))) for i in range(0, n, size)]


def layout_cue(cue: dict, style: dict, W: int, H: int, active: int, fit: float = 1.0, upto: int | None = None) -> Layout:
    """Lay out the cue's words; `active` is the hero word (dynamic styles) and `upto` limits the
    layout to the first words (a rolling window shows only what has been said)."""
    words = cue["_display"][: upto if upto else None]
    n = len(words)
    base_em = style["size"] / 100.0 * H * fit
    dyn = style.get("dynamic")
    hero = float(dyn.get("heroScale", 1.0)) if dyn else 1.0
    fnt = font(style["font"], style.get("italic"))
    ems = [base_em * (hero if (dyn and i == active) else 1.0) for i in range(n)]
    widths = [fnt.width(words[i], ems[i]) for i in range(n)]
    space = fnt.width(" ", base_em)
    # Each word is its own event, so a stroke or a karaoke pop must not eat the gap.
    if style.get("outline"):
        space += max(1.5, base_em * 0.1 * float(style.get("outlineW", 1) or 1)) * 0.8
    if style.get("karaokePop"):
        space += base_em * 0.08
    if style.get("highlightBox") or (dyn and dyn.get("fx") in ("marker", "box")):
        space += base_em * 0.18
    lh = float(style.get("lineHeight") or 1.3)
    if dyn and dyn.get("fx") in ("cards", "stickers"):
        lh = max(lh, 1.55)
        space *= 1.8
    groups = _line_groups(n, style)
    line_dims = []
    for g in groups:
        lw = sum(widths[i] for i in g) + space * (len(g) - 1)
        lem = max(ems[i] for i in g)
        line_dims.append((lw, lem * lh))
    block_w = max(d[0] for d in line_dims)
    block_h = sum(d[1] for d in line_dims)
    cx = W * _clamp(float(style.get("posX", 50)), 0, 100) / 100.0
    cy = H * _clamp(float(style.get("posY", 76)), 0, 100) / 100.0
    mx, my = W * 0.04, H * 0.04
    cx = _clamp(cx, mx + block_w / 2, W - mx - block_w / 2) if block_w < W - 2 * mx else W / 2
    cy = _clamp(cy, my + block_h / 2, H - my - block_h / 2) if block_h < H - 2 * my else H / 2
    tokens = [None] * len(cue["_display"])
    lines = []
    y = cy - block_h / 2
    for g, (lw, lhh) in zip(groups, line_dims):
        ly = y + lhh / 2
        x = cx - lw / 2
        for i in g:
            tokens[i] = {"x": x + widths[i] / 2, "y": ly, "em": ems[i], "w": widths[i]}
            x += widths[i] + space
        lines.append((cx, ly, lw, max(ems[i] for i in g)))
        y += lhh
    return Layout(tokens, (cx, cy, block_w, block_h), lines)


def _fit_factor(cue, style, W, H) -> float:
    """Shrink a cue whose widest state would overflow maxWidth (Watchfiwn keeps every word)."""
    maxw = W * _clamp(float(style.get("maxWidth") or 0.82), 0.5, 0.94)
    n = len(cue["words"])
    states = range(n) if style.get("dynamic") else [-1]
    worst = 0.0
    for a in states:
        lay = layout_cue(cue, style, W, H, a, 1.0)
        worst = max(worst, max(l[2] for l in lay.lines) / maxw, lay.block[3] / (H * 0.6))
    return min(1.0, 1.0 / worst) if worst > 0 else 1.0


# ----------------------------------------------------------------------------- ASS writer


def _cs(t: float) -> int:
    return int(round(t * 100))


def _ts(cs: int) -> str:
    cs = max(0, cs)
    h = cs // 360000
    m = (cs % 360000) // 6000
    s = (cs % 6000) // 100
    return f"{h}:{m:02d}:{s:02d}.{cs % 100:02d}"


def _f(v: float) -> str:
    s = f"{v:.1f}"
    return s[:-2] if s.endswith(".0") else s


def _rrect(w: float, h: float, r: float) -> str:
    r = max(0.0, min(r, w / 2, h / 2))
    k = r * 0.4477
    p = [f"m {_f(r)} 0", f"l {_f(w - r)} 0", f"b {_f(w - k)} 0 {_f(w)} {_f(k)} {_f(w)} {_f(r)}",
         f"l {_f(w)} {_f(h - r)}", f"b {_f(w)} {_f(h - k)} {_f(w - k)} {_f(h)} {_f(w - r)} {_f(h)}",
         f"l {_f(r)} {_f(h)}", f"b {_f(k)} {_f(h)} 0 {_f(h - k)} 0 {_f(h - r)}",
         f"l 0 {_f(r)}", f"b 0 {_f(k)} {_f(k)} 0 {_f(r)} 0"]
    return " ".join(p)


def _esc(text: str) -> str:
    return text.replace("\\", "\\\\").replace("{", "(").replace("}", ")").replace("\n", " ")


class _Writer:
    def __init__(self, W, H):
        self.W, self.H = W, H
        self.events = []

    def add(self, layer, a_cs, b_cs, tags, text):
        if b_cs <= a_cs:
            return
        self.events.append((layer, a_cs, b_cs, tags, text))

    def write(self, path, style):
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("[Script Info]\n; MangaVox styled captions\n")
            fh.write(f"Title: MangaVox captions ({style.get('label', style.get('id'))})\n")
            fh.write("ScriptType: v4+\nWrapStyle: 2\nScaledBorderAndShadow: yes\nYCbCr Matrix: TV.709\n")
            fh.write(f"PlayResX: {self.W}\nPlayResY: {self.H}\n\n")
            fh.write("[V4+ Styles]\n")
            fh.write("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n")
            fh.write("Style: W,Arial,48,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,5,0,0,0,1\n\n")
            fh.write("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n")
            self.events.sort(key=lambda e: (e[1], e[0]))
            for layer, a, b, tags, text in self.events:
                fh.write(f"Dialogue: {layer},{_ts(a)},{_ts(b)},W,,0,0,0,,{{{tags}}}{text}\n")


L_CHIP, L_CARD, L_3D, L_SHADOW, L_GLOW, L_TEXT, L_BAND = 0, 1, 2, 3, 4, 5, 6


class _CueRenderer:
    def __init__(self, cue, style, W, H, out: _Writer):
        self.cue, self.st, self.W, self.H, self.out = cue, style, W, H, out
        st = style
        upper = st.get("uppercase")
        cue["_display"] = [w["text"].upper() if upper else w["text"] for w in cue["words"]]
        self.n = len(cue["words"])
        self.anim = st.get("anim") or {}
        preset = self.anim.get("preset") or "none"
        self.unit = self.anim.get("unit") or "word"
        if preset == "type":
            self.unit = "letter"
        elif self.unit == "letter":
            self.unit = "word"   # wave-in / glitch letters ride per word here
        self.preset = preset
        self.dur = max(0.05, float(self.anim.get("duration") or 0.3))
        self.k = float(self.anim.get("intensity") or 1.0)
        self.easing = self.anim.get("ease") or "overshoot"
        self.dyn = st.get("dynamic")
        # Rolling dynamic styles lay out only the words said so far, so the line builds up
        # around the hero word instead of leaving holes for words still to come.
        self.rolling = bool(self.dyn and self.dyn.get("window") == "rolling" and preset != "none")
        self.fit = _fit_factor(cue, st, W, H)
        self.layouts = {}
        self.scale1080 = H / 1080.0
        self.stateful = bool(self.dyn or st.get("highlightWords") or st.get("karaokePop") or st.get("highlightBox")
                             or st.get("progressiveHighlight") or st.get("highlightFont"))

    # -- state -------------------------------------------------------------
    def active_at(self, t):
        for i, w in enumerate(self.cue["words"]):
            if w["start"] <= t < w["end"]:
                return i
        if t >= self.cue["words"][-1]["end"]:
            return self.n - 1
        return -1

    def layout(self, active):
        key = active if self.dyn else -1
        if key not in self.layouts:
            upto = key + 1 if (self.rolling and key >= 0) else None
            self.layouts[key] = layout_cue(self.cue, self.st, self.W, self.H, key, self.fit, upto)
        return self.layouts[key]

    def appear(self, i):
        if self.preset == "none" or self.unit == "none":
            return self.cue["start"]
        if self.dyn and self.dyn.get("window") == "phrase":
            return self.cue["start"] + i * 0.06
        return max(self.cue["start"], self.cue["words"][i]["start"])

    # -- poses ---------------------------------------------------------------
    def token_geo(self, i, t):
        """(x, y, em) of token i at time t, gliding between hero layouts."""
        act = self.active_at(t)
        cur = self.layout(act).tokens[i] or self.layout(-1).tokens[i]
        if not self.dyn:
            return cur["x"], cur["y"], cur["em"], cur["w"]
        w = self.cue["words"]
        ts = w[act]["start"] if act >= 0 else self.cue["start"]
        prev_act = act - 1
        if act <= 0 or t - ts >= 0.12:
            return cur["x"], cur["y"], cur["em"], cur["w"]
        prv = self.layout(prev_act).tokens[i]
        if prv is None:
            return cur["x"], cur["y"], cur["em"], cur["w"]
        e = ease("slowdown", (t - ts) / 0.12, 1)
        return (prv["x"] + (cur["x"] - prv["x"]) * e, prv["y"] + (cur["y"] - prv["y"]) * e,
                prv["em"] + (cur["em"] - prv["em"]) * e, prv["w"] + (cur["w"] - prv["w"]) * e)

    def pose(self, i, t):
        st = self.st
        t0 = self.appear(i)
        if t < t0 - 1e-6:
            return None
        x, y, em, w = self.token_geo(i, t)
        p = dict(REST)
        if self.preset != "none" and self.unit != "letter":
            prog = (t - t0) / self.dur
            if prog < 1:
                e = ease(self.easing, prog, self.k)
                p = preset_pose(self.preset, e, self.k, i)
        if self.unit == "none" and (p["scale"] != 1 or p["dx"] or p["dy"]):
            bx, by = self.layout(-1).block[0], self.layout(-1).block[1]
            x = bx + (x - bx) * p["scale"]
            y = by + (y - by) * p["scale"]
        word = self.cue["words"][i]
        act = word["start"] <= t < word["end"]
        scale = p["scale"]
        if st.get("karaokePop") and act:
            ramp = 0.08
            since = _clamp((t - word["start"]) / ramp, 0, 1)
            until = _clamp((word["end"] - t) / ramp, 0, 1)
            scale *= 1 + 0.09 * min(ease("overshoot", since, 1), ease("overshoot", until, 1))
        if self.dyn and act and self.dyn.get("heroScale", 1) > 1.01:
            since = _clamp((t - word["start"]) / 0.14, 0, 1)
            scale *= 0.82 + 0.18 * ease("overshoot", since, 1.4)
        hl = self.highlighted(i, t)
        if hl and st.get("highlightScale", 1) and abs(float(st.get("highlightScale", 1)) - 1) > 0.01:
            scale *= _clamp(float(st["highlightScale"]), 0.7, 1.5)
        opacity = p["opacity"]
        rem = self.cue["end"] - t
        if rem < 0.12:
            opacity *= ease("slowdown", _clamp(rem / 0.12, 0, 1), 1)
        blur = p["blur"]
        if self.dyn and self.dyn.get("fx") == "focus" and not act:
            blur += 2.2
            opacity *= 0.6
        rot = p["rot"] + (self.dyn.get("rot", 0) if self.dyn else 0)
        if self.dyn and self.dyn.get("fx") in ("stickers",):
            rot += ((i % 3) - 1) * 3.8
        if self.dyn and self.dyn.get("fx") == "cycle":
            rot += 3.5 if i % 2 else -3.5
        if self.dyn and self.dyn.get("fx") == "cards":
            rot += CARD_THEMES[i % 3][2]
        return {
            "x": x + p["dx"] * em, "y": y + p["dy"] * em, "em": em, "w": w,
            "sx": scale * p["scaleX"], "sy": scale * p["scaleY"], "rot": rot,
            "op": _clamp(opacity, 0, 1), "blur": blur * self.scale1080 * 0.8,
            "clipX": p["clipX"], "clipY": p["clipY"], "origin": p["origin"], "clip": p["clip"],
            "restY": y,
        }

    def highlighted(self, i, t):
        w = self.cue["words"][i]
        if self.st.get("progressiveHighlight"):
            return t >= w["start"]
        return w["start"] <= t < w["end"]

    # -- look (colours per state) ---------------------------------------------
    def look(self, i, t):
        st = self.st
        w = self.cue["words"][i]
        act = w["start"] <= t < w["end"]
        hl = self.highlighted(i, t)
        base = parse_color(st.get("color"))
        fill = base
        grad = None
        box = None       # (colour) for highlight box
        fnt_key, italic = st["font"], bool(st.get("italic"))
        if self.dyn:
            d = self.dyn
            raw = self.cue["_display"][i]
            fx = d.get("fx")
            if act:
                fill = parse_color(d["hero"])
            elif _bare(raw) in FILLERS:
                fill = parse_color(d["mute"])
            elif _bare(raw) in PUNCH:
                fill = parse_color(d["accent"])
            elif fx == "cycle":
                fill = parse_color([d["ink"], d["accent"], d["hero"]][i % 3])
            elif fx == "candy":
                fill = parse_color(CANDY[i % len(CANDY)])
            else:
                fill = parse_color(d["ink"])
            if fx in ("marker", "box") and act:
                box = parse_color(d["accent"])
                fill = parse_color(st.get("highlightTextColor") or d["hero"])
            if fx == "cards":
                bg, ink, _ = CARD_THEMES[i % 3]
                box = parse_color(bg)
                fill = parse_color(ink if not act else "#000000")
                if act:
                    box = parse_color(d["hero"])
            if fx == "stickers":
                box = parse_color(d["accent"] if act else "#ffffff")
                fill = parse_color(d["ink"])
            if st.get("gradient") and not act and fx is None:
                grad = (parse_color(d["ink"]), parse_color(d["accent"]))
        else:
            if st.get("highlightBox") and act:
                box = parse_color(st.get("highlightBoxColor"))
                fill = parse_color(st.get("highlightTextColor") or "#0a0d05")
            elif st.get("wordAlt"):
                fill = parse_color(st.get("color2")) if i % 2 else base
            elif st.get("highlightWords") and hl:
                fill = parse_color(st.get("highlightColor"))
            elif st.get("gradient") and st.get("color2"):
                grad = (base, parse_color(st.get("color2")))
        if hl and st.get("highlightFontKey") and st.get("highlightFont"):
            fnt_key, italic = st["highlightFontKey"], bool(st.get("highlightItalic"))
        return {"fill": fill, "grad": grad, "box": box, "font": fnt_key, "italic": italic,
                "underline": bool(self.dyn and self.dyn.get("fx") == "underline" and act)}

    # -- breakpoints -----------------------------------------------------------
    def times(self, i):
        cue = self.cue
        t0 = self.appear(i)
        ts = {t0, cue["end"]}
        if self.preset != "none" and self.unit != "letter":
            # Enough samples to trace the easing: springy curves need more than an ease-out.
            if self.preset == "glitch":
                steps = 8
            elif self.easing in ("overshoot", "bounce", "elastic", "spring"):
                steps = 5
            elif self.easing == "linear":
                steps = 1
            else:
                steps = 3
            for s in range(1, steps + 1):
                ts.add(t0 + self.dur * s / steps)
        if self.unit == "letter":
            stag = float(self.anim.get("stagger") or 0.045)
            for j in range(len(self.cue["_display"][i])):
                ts.add(t0 + j * stag)
        w = cue["words"][i]
        if self.dyn:
            # The hero word changes every token's size and place.
            for o in cue["words"]:
                ts.update((o["start"], o["start"] + 0.04, o["start"] + 0.08, o["start"] + 0.12))
        elif self.stateful:
            # Only this word's own look changes (active / spoken).
            ts.update((w["start"], w["end"]))
        if self.st.get("karaokePop"):
            ts.update((w["start"] + 0.03, w["start"] + 0.06, w["start"] + 0.08, w["end"] - 0.08, w["end"] - 0.04, w["end"]))
        ts.update((cue["end"] - 0.12, cue["end"] - 0.06))
        cs = sorted({_cs(t) for t in ts if t0 - 1e-6 <= t <= cue["end"] + 1e-6})
        return cs

    # -- emit ----------------------------------------------------------------
    def render(self):
        for i in range(self.n):
            self.render_token(i)
        self.render_chips()

    def render_token(self, i):
        cs = self.times(i)
        pieces = []
        for a, b in zip(cs, cs[1:]):
            ta, tb = a / 100.0, b / 100.0
            pa = self.pose(i, ta + 1e-4)
            pb = self.pose(i, tb - 1e-4)
            if pa is None or pb is None:
                continue
            lk = self.look(i, ta + 1e-4)
            vis = None
            if self.unit == "letter":
                stag = float(self.anim.get("stagger") or 0.045)
                t0 = self.appear(i)
                vis = sum(1 for j in range(len(self.cue["_display"][i])) if t0 + j * stag <= ta + 1e-4)
            # Merge static pieces with the previous one.
            if pieces and pieces[-1][5] == vis and pieces[-1][4] == lk and pieces[-1][3] == pa and pa == pb and pieces[-1][2] == pieces[-1][3]:
                pieces[-1][1] = b
                continue
            pieces.append([a, b, pa, pb, lk, vis])
        text = self.cue["_display"][i]
        for a, b, pa, pb, lk, vis in pieces:
            self.emit_word(i, a, b, pa, pb, lk, text, vis)

    def _xform(self, pa, pb, dx=0.0, dy=0.0, extra_scale=1.0, em_ref=None):
        """\\move + scale/rotation/blur start values and the \\t to the end values."""
        em_ref = em_ref or pb["em"]
        xa, ya, xb, yb = pa["x"] + dx * pa["sx"], pa["y"] + dy * pa["sy"], pb["x"] + dx * pb["sx"], pb["y"] + dy * pb["sy"]
        sxa = 100 * pa["sx"] * pa["em"] / em_ref * extra_scale
        sya = 100 * pa["sy"] * pa["em"] / em_ref * extra_scale
        sxb = 100 * pb["sx"] * pb["em"] / em_ref * extra_scale
        syb = 100 * pb["sy"] * pb["em"] / em_ref * extra_scale
        if abs(xa - xb) < 0.05 and abs(ya - yb) < 0.05:
            pos = f"\\pos({_f(xa)},{_f(ya)})"
        else:
            pos = f"\\move({_f(xa)},{_f(ya)},{_f(xb)},{_f(yb)})"
        start = f"\\fscx{_f(sxa)}\\fscy{_f(sya)}\\frz{_f(-pa['rot'])}"
        end = f"\\fscx{_f(sxb)}\\fscy{_f(syb)}\\frz{_f(-pb['rot'])}"
        return pos, start, end, (sxa, sya, sxb, syb)

    def _clip(self, p, w, em):
        """Rect clip for wipes / mask-up in screen space, or ''."""
        hw = w / 2 * p["sx"] + em * 0.3
        hh = em * 0.75 * p["sy"]
        x0, x1 = p["x"] - hw, p["x"] + hw
        y0, y1 = p["y"] - hh, p["y"] + hh
        if p["clipX"] is not None:
            if p["origin"] == "right":
                x0 = x1 - (x1 - x0) * p["clipX"]
            else:
                x1 = x0 + (x1 - x0) * p["clipX"]
        elif p["clipY"] is not None:
            if p["origin"] == "bottom":
                y0 = y1 - (y1 - y0) * p["clipY"]
            else:
                y1 = y0 + (y1 - y0) * p["clipY"]
        elif p["clip"] == "line":
            y0, y1 = p["restY"] - em * 0.75, p["restY"] + em * 0.75
        else:
            return None
        return (x0, y0, x1, y1)

    def emit_word(self, i, a, b, pa, pb, lk, text, vis):
        st = self.st
        em = pb["em"]
        f = font(lk["font"], lk["italic"])
        fs = em * f.fs_per_em
        ital = "\\i1" if lk["italic"] and not catalog()["fonts"].get(lk["font"], {}).get("italicFile") else "\\i0"
        head = f"\\an5\\fn{f.ass_name}\\fs{_f(fs)}\\b{1 if f.bold else 0}{ital}\\shad0"
        pos, s0, s1, scales = self._xform(pa, pb)
        smin_a = min(scales[0], scales[1]) / 100.0
        smin_b = min(scales[2], scales[3]) / 100.0
        clip_a = self._clip(pa, pa["w"], em)
        clip_b = self._clip(pb, pb["w"], em)
        clip0 = clip1 = ""
        if clip_a and clip_b:
            clip0 = "\\clip(" + ",".join(_f(v) for v in clip_a) + ")"
            clip1 = "\\clip(" + ",".join(_f(v) for v in clip_b) + ")"
        if vis is not None:
            shown, hidden = text[:vis], text[vis:]
            if not shown:
                return
            body = _esc(shown) + ("{\\alpha&HFF&}" + _esc(hidden) if hidden else "")
        else:
            body = _esc(text)
        op_a, op_b = pa["op"], pb["op"]
        bl_a, bl_b = pa["blur"], pb["blur"]
        outline = st.get("outline") and float(st.get("outlineW", 1) or 0) > 0.05
        bord = max(1.5, em * 0.1 * float(st.get("outlineW", 1) or 1)) / 2 if outline else 0.0
        oc = parse_color(st.get("outlineColor"), (0, 0, 0, 1))

        def piece(layer, c1, c3, bw, extra_blur=0.0, dx=0.0, dy=0.0, a1=1.0, a3=1.0, clipx=None):
            p_pos, p_s0, p_s1, _ = self._xform(pa, pb, dx, dy)
            ca, cb = (clipx if clipx else (clip0, clip1))
            tags = (f"{head}{p_pos}\\1c{ass_bgr(c1)}\\3c{ass_bgr(c3)}"
                    f"\\bord{_f(bw * smin_a)}\\blur{_f(bl_a + extra_blur)}"
                    f"\\1a{ass_alpha(op_a * c1[3] * a1)}\\3a{ass_alpha(op_a * c3[3] * a3)}{p_s0}{ca}"
                    f"\\t(\\bord{_f(bw * smin_b)}\\blur{_f(bl_b + extra_blur)}"
                    f"\\1a{ass_alpha(op_b * c1[3] * a1)}\\3a{ass_alpha(op_b * c3[3] * a3)}{p_s1}{cb})")
            self.out.add(layer, a, b, tags, body)

        # Highlight box / marker / cards / stickers behind the word.
        if lk["box"] is not None:
            fxd = self.dyn.get("fx") if self.dyn else None
            if fxd in ("cards", "stickers"):
                bw_, bh_, rad = pb["w"] + em * 0.5, em * 1.32, em * 0.16
            else:
                bw_, bh_, rad = pb["w"] + em * 0.52, em * 1.36, em * 0.42
            if fxd == "marker":
                rad = em * 0.12
            self._drawing(L_CARD, a, b, pa, pb, bw_, bh_, rad, lk["box"], 0.0)
        if lk["underline"]:
            w0 = pb["w"]
            sinceA = _clamp((a / 100.0 - self.cue["words"][i]["start"]) / 0.2, 0, 1)
            sinceB = _clamp((b / 100.0 - self.cue["words"][i]["start"]) / 0.2, 0, 1)
            self._drawing(L_CARD, a, b, pa, pb, w0 + em * 0.2, em * 0.12, em * 0.06,
                          parse_color(self.dyn.get("accent")), em * 0.56,
                          xscale=(ease("slowdown", sinceA, 1), ease("slowdown", sinceB, 1)))
        # Tetriz 3D extrusion.
        if st.get("pixel3d"):
            depth = max(2.0, em * 0.13)
            black = max(2.0, em * 0.18) / 2
            rim = max(1.5, em * 0.045)
            piece(L_3D, (21, 21, 21, 1), (245, 245, 232, 1), black + rim, dx=depth / max(pb["sx"], 0.01), dy=depth / max(pb["sy"], 0.01))
            piece(L_3D, (21, 21, 21, 1), (5, 5, 5, 1), black, dx=depth / max(pb["sx"], 0.01), dy=depth / max(pb["sy"], 0.01))
        # Drop shadow (Watchfiwn: only without glow).
        if st.get("dropShadow") and not st.get("glow"):
            sh = (0, 0, 0, 0.85)
            piece(L_SHADOW, sh, sh, bord, extra_blur=em * 0.08, dx=em * 0.045, dy=em * 0.11)
        # Glow: a blurred copy of glyph + stroke in the glow colour.
        if st.get("glow"):
            gc = parse_color(st.get("glowColor"), (183, 255, 25, 1))
            sigma = float(st.get("glowSize") or 18) * self.scale1080 * 0.8
            piece(L_GLOW, gc, gc, bord + sigma * 0.35, extra_blur=sigma)
            if sigma > 22:
                piece(L_GLOW, gc, gc, bord + sigma * 0.12, extra_blur=sigma * 0.45, a1=0.8, a3=0.8)
        # Outline (+ Tetriz white rim) and fill.
        if st.get("pixel3d") and outline:
            piece(L_TEXT, lk["fill"], (245, 245, 232, 1), bord + max(1.5, em * 0.045))
        fill = lk["grad"][1] if lk["grad"] else lk["fill"]
        piece(L_TEXT + (1 if st.get("pixel3d") else 0), fill, oc, bord)
        # Gradient: horizontal bands from colour (top) to colour2 (bottom), each clipped.
        if lk["grad"] and not clip0:
            c_top, c_bot = lk["grad"]
            bands = 3
            for k_ in range(bands):
                col = lerp_color(c_top, c_bot, (k_ + 0.5) / bands)

                def band(p):
                    hh = em * 0.62 * p["sy"]
                    hw = p["w"] / 2 * p["sx"] + em * 0.3
                    top = p["y"] - hh
                    h = 2 * hh / bands
                    y0 = top + h * k_ - (0 if k_ == 0 else 0.5)
                    y1 = top + h * (k_ + 1) + (0.5 if k_ < bands - 1 else hh)
                    if k_ == 0:
                        y0 = p["y"] - em * 2
                    return f"\\clip({_f(p['x'] - hw)},{_f(y0)},{_f(p['x'] + hw)},{_f(y1)})"

                piece(L_BAND + 1, col, col, 0.0, clipx=(band(pa), band(pb)))

    def _drawing(self, layer, a, b, pa, pb, w, h, r, col, dy, xscale=None):
        """A rounded rectangle centred on the word (follows its pose)."""
        path = _rrect(w, h, r)
        sa, sb = pa["sx"], pb["sx"]
        sya, syb = pa["sy"], pb["sy"]
        xa_scale = 100 * sa * (xscale[0] if xscale else 1)
        xb_scale = 100 * sb * (xscale[1] if xscale else 1)
        xa, ya, xb, yb = pa["x"], pa["y"] + dy * sya, pb["x"], pb["y"] + dy * syb
        pos = f"\\pos({_f(xa)},{_f(ya)})" if abs(xa - xb) < .05 and abs(ya - yb) < .05 else f"\\move({_f(xa)},{_f(ya)},{_f(xb)},{_f(yb)})"
        tags = (f"\\an5{pos}\\p1\\bord0\\shad0\\blur0.6\\1c{ass_bgr(col)}\\1a{ass_alpha(pa['op'] * col[3])}"
                f"\\fscx{_f(xa_scale)}\\fscy{_f(100 * sya)}\\frz{_f(-pa['rot'])}"
                f"\\t(\\1a{ass_alpha(pb['op'] * col[3])}\\fscx{_f(xb_scale)}\\fscy{_f(100 * syb)}\\frz{_f(-pb['rot'])})")
        self.out.add(layer, a, b, tags, path)

    def render_chips(self):
        st = self.st
        if not (st.get("bgOn") and st.get("bg") and st.get("bg") != "transparent"):
            return
        col = parse_color(st.get("bg"))
        if col[3] <= 0:
            return
        cue = self.cue
        # One chip per line per layout state (static unless a hero word resizes the line).
        bounds = [cue["start"]] + ([w["start"] for w in cue["words"]] if self.dyn else []) + [cue["end"]]
        bounds = sorted({_cs(t) for t in bounds if cue["start"] <= t <= cue["end"]})
        fade_in = 8
        groups = _line_groups(self.n, st)
        for a, b in zip(bounds, bounds[1:]):
            lay = self.layout(self.active_at(a / 100.0 + 1e-4))
            for li, (lx, ly, lw, lem) in enumerate(lay.lines):
                first = groups[li][0] if li < len(groups) else 0
                a_line = max(a, _cs(self.appear(first)))
                if a_line >= b:
                    continue
                padx, pady = lem * 0.42, lem * 0.26
                w, h = lw + padx * 2, lem + pady * 2
                path = _rrect(w, h, lem * float(st.get("bgRadius", 0.22) if st.get("bgRadius") is not None else 0.22))
                op = col[3]
                fin = f"\\fad({fade_in if (a == bounds[0] or a_line > a) else 0},{120 if b == bounds[-1] else 0})"
                self.out.add(L_CHIP, a_line, b, f"\\an5\\pos({_f(lx)},{_f(ly)})\\p1\\bord0\\shad0\\blur0.6\\1c{ass_bgr(col)}\\1a{ass_alpha(op)}{fin}", path)


def write_ass(segments: list[dict], path: str, width: int, height: int, settings: dict | None = None) -> str | None:
    """Write the styled caption track for `segments` to `path`. Returns the path or None."""
    s = normalize_settings(settings or {})
    if not s["enabled"]:
        return None
    style = apply_settings(get_style(s.get("style")), s)
    if s.get("wordsPerLine") or s.get("lines"):
        style["_userWords"] = True
    cues = build_cues(segments, style)
    if not cues:
        return None
    out = _Writer(width, height)
    for cue in cues:
        _CueRenderer(cue, style, width, height, out).render()
    out.write(path, style)
    return path


def ffmpeg_filter_path(p: str) -> str:
    """A path quoted for an ffmpeg filter option (Windows drive colons escaped)."""
    p = os.path.abspath(p).replace("\\", "/")
    p = p.replace(":", "\\:").replace("'", "\\'")
    return f"'{p}'"


def subtitles_filter(ass_path: str) -> str:
    f = f"subtitles={ffmpeg_filter_path(ass_path)}"
    if os.path.isdir(FONTS_DIR):
        f += f":fontsdir={ffmpeg_filter_path(FONTS_DIR)}"
    return f
