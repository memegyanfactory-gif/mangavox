#!/usr/bin/env python3
"""
Expressive narration, engine-neutral: the emotion plan of a chapter, how each engine
voices it, text chunking, and the audio post-processing every engine shares.

  * Emotion plan: each page's scene {kind, mood, intensity} from `<output_dir>/studio/read.json`
    (per-beat kind/mood/intensity in `narration.studio.json` win when present), intensity
    smoothed over neighbours (3-unit moving average, at most 3 points per step).
  * Engine adapters: `chatterbox_params` (exaggeration / cfg_weight / temperature, per the
    research mapping) and `kokoro_speed` (Kokoro and Piper only get a speed).
  * Text: 1-3 sentences or <= 250 characters per chunk (Chatterbox caps at ~40 s), shouts
    on their own, silence decided here and inserted in post instead of by the model.
  * Audio: WAV in/out at the engine's native rate, silence trimming, chapter loudness
    normalisation to about -16 LUFS (ITU-R BS.1770 K-weighting with gating), peak limiting at -1 dBFS.

numpy is used when it is importable (it is in the voice environment and the engine venvs);
without it the loudness pass falls back to an unweighted, gated RMS estimate.

CLI (for interpreters without numpy):
  python narration_tts.py loudnorm <args.json>   {"files": [...], "target": -16, "gain_db"?: dB, "rate"?: Hz}
  python narration_tts.py plan <output_dir>       prints the emotion plan of a chapter
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import re
import struct
import sys
import wave
import zlib

try:  # numpy is optional; every function has a stdlib path.
    import numpy as np
except Exception:  # pragma: no cover - depends on the interpreter
    np = None

TARGET_LUFS = -16.0
CEILING = 0.89  # about -1 dBFS
DEFAULT_KIND = "other"
DEFAULT_INTENSITY = 3.0

# ----------------------------------------------------------------------------- scene kinds

KINDS = ("fight", "action", "dialogue", "emotional", "comedy", "reveal", "calm", "flashback", "romance", "horror", "other")
_KIND_ALIASES = {
    "battle": "fight", "combat": "fight", "chase": "action", "sad": "emotional", "tragic": "emotional",
    "drama": "emotional", "dramatic": "emotional", "funny": "comedy", "comedic": "comedy", "humor": "comedy",
    "twist": "reveal", "mystery": "reveal", "shock": "reveal", "peaceful": "calm", "quiet": "calm",
    "memory": "flashback", "romantic": "romance", "love": "romance", "scary": "horror", "dread": "horror",
    "conversation": "dialogue", "talk": "dialogue",
}

# Research mapping (tts-research.json, emotion_mapping §1).
KIND_BIAS = {
    "calm": -0.10, "flashback": -0.05, "dialogue": 0.0, "romance": 0.0, "horror": 0.05,
    "comedy": 0.10, "emotional": 0.10, "reveal": 0.15, "fight": 0.20, "action": 0.20, "other": 0.0,
}
FAST_KINDS = frozenset({"fight", "action", "comedy"})
SLOW_KINDS = frozenset({"emotional", "horror", "reveal", "flashback", "romance", "calm"})


def normalize_kind(kind) -> str:
    k = str(kind or "").strip().lower()
    k = _KIND_ALIASES.get(k, k)
    return k if k in KINDS else DEFAULT_KIND


def clamp(v: float, lo: float, hi: float) -> float:
    return lo if v < lo else hi if v > hi else v


def _num(v, default=None):
    try:
        f = float(v)
        return f if math.isfinite(f) else default
    except (TypeError, ValueError):
        return default


# ----------------------------------------------------------------------------- engine adapters

def chatterbox_params(kind, intensity, line_kind: str = "narration", strength: float = 1.0) -> dict:
    """Chatterbox generate() settings for a scene, per the research mapping:

    exaggeration = clamp(0.35 + 0.075*i + kind_bias, 0.30, 1.20)
    cfg_weight   = 0.5 for fight/action/comedy; 0.35..0.30 for emotional/horror/reveal/
                   flashback/romance/calm; else max(0.3, 0.5 - 0.02*i)
    temperature  = 0.90 at i >= 8, 0.70 for calm/flashback and quiet exposition, else 0.80
    Thoughts get exaggeration -0.1. `strength` (the chapter's emotion strength, 0..2) scales
    the intensity and the kind bias."""
    kind = normalize_kind(kind)
    s = clamp(_num(strength, 1.0), 0.0, 2.0)
    i = clamp(_num(intensity, DEFAULT_INTENSITY) * s, 0.0, 10.0)
    bias = KIND_BIAS.get(kind, 0.0) * min(s, 1.0)
    exaggeration = clamp(0.35 + 0.075 * i + bias, 0.30, 1.20)
    if line_kind == "thought":
        exaggeration = clamp(exaggeration - 0.10, 0.30, 1.20)
    if kind in FAST_KINDS:
        cfg = 0.50
    elif kind in SLOW_KINDS:
        cfg = 0.35 - 0.005 * i
    else:
        cfg = max(0.30, 0.50 - 0.02 * i)
    if i >= 8:
        temperature = 0.90
    elif kind in ("calm", "flashback") or (kind in ("other", "dialogue") and i <= 2):
        temperature = 0.70
    else:
        temperature = 0.80
    return {
        "exaggeration": round(exaggeration, 3),
        "cfg_weight": round(clamp(cfg, 0.30, 0.50), 3),
        "temperature": round(temperature, 2),
        "repetition_penalty": 1.2,
        "top_p": 1.0,
        "min_p": 0.05,
    }


def chatterbox_tempo(kind) -> float:
    """Pitch-preserving tempo in post: slower for emotional/flashback, faster for fights."""
    kind = normalize_kind(kind)
    if kind in ("emotional", "flashback"):
        return 0.95
    if kind in ("fight", "action"):
        return 1.05
    return 1.0


def kokoro_speed(kind, intensity, strength: float = 1.0) -> float:
    """Kokoro/Piper have no emotion control: intensity maps to speed only, 0.92 for calm or
    emotional scenes up to 1.08 for fights."""
    kind = normalize_kind(kind)
    s = clamp(_num(strength, 1.0), 0.0, 2.0)
    if s <= 0:
        return 1.0  # emotion strength 0: flat, the user's speed as it is
    i = clamp(_num(intensity, DEFAULT_INTENSITY) * s, 0.0, 10.0)
    if kind in FAST_KINDS - {"comedy"}:
        base = 1.0
    elif kind in SLOW_KINDS:
        base = 0.92
    else:
        base = 0.96
    return round(clamp(base + 0.008 * i, 0.90, 1.10), 3)


def sentence_gap_ms(kind, intensity) -> int:
    """500-900 ms between sentences: calmer and slower scenes breathe longer."""
    kind = normalize_kind(kind)
    gap = 900 - 40 * clamp(_num(intensity, DEFAULT_INTENSITY), 0, 10)
    if kind in FAST_KINDS:
        gap -= 100
    elif kind in SLOW_KINDS:
        gap += 100
    return int(clamp(gap, 500, 900))


def minor_gap_ms(kind, boundary: str) -> int:
    """250-350 ms at commas and ellipses."""
    if boundary == "ellipsis":
        return 350
    return 300 if normalize_kind(kind) in SLOW_KINDS else 250


REVEAL_LEAD_MS = 300
PAGE_LEAD_MS = 120
PAGE_TAIL_MS = 300


def segment_seed(page: int, index: int, text: str, take: int = 0) -> int:
    """Fixed seed per segment, so a re-render of the same text sounds the same. A page's take
    (Studio's "New take", `audio/takes.json`) is mixed in, so asking again gives a new
    performance and a new chunk-cache key; take 0 is the original seed."""
    key = f"{page}:{index}:{text}"
    if take:
        key += f":take{int(take)}"
    return zlib.crc32(key.encode("utf-8")) & 0x7FFFFFFF


def load_takes(output_dir: str) -> dict:
    """{page index: take} from `<output_dir>/audio/takes.json` (mv_studio::voices::new_take)."""
    data = _read_json(os.path.join(output_dir, "audio", "takes.json"))
    out: dict = {}
    if isinstance(data, dict):
        for k, v in data.items():
            try:
                page, take = int(k), int(v)
            except (TypeError, ValueError):
                continue
            if take > 0:
                out[page] = take
    return out


def cache_key(engine: str, voice: str, text: str, params: dict, seed: int, extra: str = "") -> str:
    """hash(text, style, engine, seed): the file name of a cached chunk."""
    blob = json.dumps({"e": engine, "v": voice, "t": text, "p": params, "s": seed, "x": extra}, sort_keys=True, ensure_ascii=False)
    return hashlib.sha1(blob.encode("utf-8")).hexdigest()[:24]


# ----------------------------------------------------------------------------- emotion plan

def _read_json(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return None


def load_scenes(output_dir: str) -> dict:
    """{page index: {kind, mood, intensity, summary}} from `<output_dir>/studio/read.json`."""
    data = _read_json(os.path.join(output_dir, "studio", "read.json"))
    out: dict = {}
    if not isinstance(data, dict):
        return out
    for i, p in enumerate(data.get("pages") or []):
        if not isinstance(p, dict):
            continue
        idx = p.get("index", i)
        scene = p.get("scene") or {}
        if not isinstance(idx, int) or not isinstance(scene, dict):
            continue
        out[idx] = {
            "kind": normalize_kind(scene.get("kind")),
            "mood": str(scene.get("mood") or ""),
            "intensity": _num(scene.get("intensity")),
            "summary": str(p.get("summary") or ""),
            "line_kinds": [
                str(l.get("kind") or "")
                for panel in (p.get("panels") or []) if isinstance(panel, dict)
                for l in (panel.get("lines") or []) if isinstance(l, dict)
            ],
        }
    return out


def load_beats(output_dir: str) -> dict:
    """{page: [beat, ...]} from `narration.studio.json`, keeping any per-beat style."""
    data = _read_json(os.path.join(output_dir, "narration.studio.json"))
    out: dict = {}
    beats = data.get("beats") if isinstance(data, dict) else None
    for b in beats or []:
        if isinstance(b, dict) and isinstance(b.get("page"), int):
            out.setdefault(b["page"], []).append(b)
    return out


def _squash(text: str) -> str:
    return re.sub(r"\s+", " ", text or "").strip()


def smooth(values: list, window: int = 3, max_step: float = 3.0) -> list:
    """Moving average over `window` neighbours, then at most `max_step` change per step."""
    if not values:
        return []
    known = [v for v in values if v is not None]
    fill = sum(known) / len(known) if known else DEFAULT_INTENSITY
    vals = [fill if v is None else float(v) for v in values]
    half = window // 2
    avg = []
    for i in range(len(vals)):
        part = vals[max(0, i - half): i + half + 1]
        avg.append(sum(part) / len(part))
    out = [avg[0]]
    for v in avg[1:]:
        prev = out[-1]
        out.append(clamp(v, prev - max_step, prev + max_step))
    return [round(clamp(v, 0.0, 10.0), 2) for v in out]


def build_plan(segments: list, output_dir: str, strength: float = 1.0) -> list:
    """The emotion plan of a chapter: one entry per script segment, in order:

    {page, text, units: [{text, kind, mood, intensity, raw_intensity, line_kind, source}]}

    A page is one unit (its read.json scene) unless `narration.studio.json` has beats for it
    whose texts make up the page text and carry their own kind/mood/intensity; then each beat
    is a unit. Intensity is smoothed across all units of the chapter."""
    scenes = load_scenes(output_dir)
    beats = load_beats(output_dir)
    plan = []
    for seg in segments:
        page = int(seg.get("page", 0) or 0)
        text = (seg.get("text") or "").strip()
        scene = scenes.get(page) or {}
        base = {
            "kind": scene.get("kind", DEFAULT_KIND),
            "mood": scene.get("mood", ""),
            "intensity": scene.get("intensity"),
            "line_kind": "narration",
            "source": "read.json" if scene else "default",
        }
        units = []
        page_beats = [b for b in beats.get(page, []) if _squash(b.get("text"))]
        styled = any(any(k in b for k in ("kind", "mood", "intensity")) for b in page_beats)
        if text and styled and _squash(" ".join(b["text"] for b in page_beats)) == _squash(text):
            for b in page_beats:
                units.append({
                    "text": b["text"].strip(),
                    "kind": normalize_kind(b.get("kind")) if b.get("kind") else base["kind"],
                    "mood": str(b.get("mood") or base["mood"]),
                    "intensity": _num(b.get("intensity"), base["intensity"]),
                    "line_kind": str(b.get("line_kind") or b.get("lineKind") or "narration"),
                    # A beat carries a kind only when the user picked the passage's delivery in
                    # Studio (mv_recap::export_for_pipeline).
                    "source": "delivery" if b.get("kind") else "narration.studio.json",
                })
        elif text:
            units.append({**base, "text": text})
        plan.append({"page": page, "text": text, "units": units})
    flat = [u for p in plan for u in p["units"]]
    smoothed = smooth([u["intensity"] for u in flat])
    for u, s in zip(flat, smoothed):
        u["raw_intensity"] = u["intensity"]
        # The user's own direction is said as picked ("Intense · 9" stays 9); its neighbours
        # still ramp towards it through the smoothing.
        u["intensity"] = round(clamp(_num(u["intensity"], s), 0.0, 10.0), 2) if u["source"] == "delivery" else s
        u["strength"] = strength
    return plan


def page_style(units: list) -> dict:
    """The dominant (longest-text) unit's style, for page-wide choices such as tempo."""
    if not units:
        return {"kind": DEFAULT_KIND, "mood": "", "intensity": DEFAULT_INTENSITY}
    return max(units, key=lambda u: len(u.get("text") or ""))


# ----------------------------------------------------------------------------- text chunking

# Whitespace after . ! ? । … (or after one closing quote/bracket that follows them).
_SENT_END = re.compile(r"(?:(?<=[.!?।…])|(?<=[.!?।…][\"'”’)\]]))\s+")
_ENDS_ELLIPSIS = re.compile(r"(\.\.\.|…)[\"'”’)\]]*$")
_MINOR = re.compile(r"(?<=[,;:—])\s+|\s+(?=—)")


def _is_shout(s: str) -> bool:
    letters = [c for c in s if c.isalpha() and c.isascii()]
    if len(letters) < 3:
        return False
    return sum(c.isupper() for c in letters) / len(letters) >= 0.7


def _sentence_case(s: str) -> str:
    low = s.lower()
    low = re.sub(r"\bi\b", "I", low)
    return re.sub(r"^(\W*)(\w)", lambda m: m.group(1) + m.group(2).upper(), low)


def _split_long(s: str, max_chars: int) -> list:
    """A sentence over `max_chars`: split at commas/semicolons/dashes, then at spaces."""
    parts, cur = [], ""
    for piece in _MINOR.split(s):
        piece = piece.strip()
        if not piece:
            continue
        if cur and len(cur) + 1 + len(piece) > max_chars:
            parts.append(cur)
            cur = piece
        else:
            cur = f"{cur} {piece}".strip()
    if cur:
        parts.append(cur)
    out = []
    for p in parts:
        while len(p) > max_chars:
            cut = p.rfind(" ", 0, max_chars)
            cut = cut if cut > max_chars // 3 else max_chars
            out.append(p[:cut].strip())
            p = p[cut:].strip()
        if p:
            out.append(p)
    return out


def split_chunks(text: str, max_chars: int = 250, max_sentences: int = 3) -> list:
    """Chunks of 1-3 sentences (<= max_chars), each {text, boundary, shout}: `boundary` is
    what ends the chunk (sentence | ellipsis | comma | end) and decides the silence after it.
    Ellipses and dashes from the source text stay in place; shouts get their own chunk."""
    text = _squash(text)
    if not text:
        return []
    pieces = []  # (text, boundary)
    for sent in _SENT_END.split(text):
        t = sent.strip()
        if not t:
            continue
        # "Wait... " splits like a sentence end but only earns the short ellipsis pause.
        b = "ellipsis" if _ENDS_ELLIPSIS.search(t) else "sentence"
        if len(t) > max_chars:
            long = _split_long(t, max_chars)
            pieces += [(p, "comma") for p in long[:-1]] + [(long[-1], b)]
        else:
            pieces.append((t, b))
    chunks: list = []
    cur: list = []
    for t, b in pieces:
        shout = _is_shout(t)
        if shout:
            if cur:
                chunks.append(cur)
                cur = []
            chunks.append([(t, b, True)])
            continue
        joined = " ".join(x[0] for x in cur)
        fits = cur and len(cur) < max_sentences and len(joined) + 1 + len(t) <= max_chars
        # Keep sentences apart (so the gap between them is ours) unless they are short.
        short = cur and (len(joined) < 40 or len(t) < 40)
        if fits and short and cur[-1][1] == "sentence":
            cur.append((t, b, False))
        else:
            if cur:
                chunks.append(cur)
            cur = [(t, b, False)]
    if cur:
        chunks.append(cur)
    out = []
    for c in chunks:
        t = " ".join(x[0] for x in c)
        shout = c[0][2]
        out.append({"text": t, "synth_text": _sentence_case(t) if shout else t, "boundary": c[-1][1], "shout": shout})
    if out:
        out[-1]["boundary"] = "end"
    return out


def plan_chunks(units: list, max_chars: int = 250) -> list:
    """Every chunk of a page with its style and the silence before it (ms)."""
    out = []
    prev = None
    for ui, u in enumerate(units):
        chunks = split_chunks(u["text"], max_chars=max_chars)
        for ci, c in enumerate(chunks):
            if prev is None:
                pause = 0
            elif prev["boundary"] in ("ellipsis", "comma"):
                pause = minor_gap_ms(u["kind"], prev["boundary"])
            else:
                pause = sentence_gap_ms(u["kind"], u["intensity"])
            if ci == 0 and normalize_kind(u["kind"]) == "reveal":
                pause += REVEAL_LEAD_MS
            intensity = u["intensity"]
            if c["shout"]:
                intensity = min(10.0, intensity + 2.0)
            out.append({**c, "unit": ui, "kind": u["kind"], "mood": u.get("mood", ""), "intensity": intensity,
                        "line_kind": u.get("line_kind", "narration"), "pause_before_ms": int(pause)})
            prev = c
    return out


# ----------------------------------------------------------------------------- WAV I/O

def read_wav(path: str):
    """(samples, sample_rate): float32 mono numpy array, or a list of floats without numpy."""
    with wave.open(path, "rb") as wf:
        ch, width, sr, n = wf.getnchannels(), wf.getsampwidth(), wf.getframerate(), wf.getnframes()
        raw = wf.readframes(n)
    if np is not None:
        if width == 2:
            x = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
        elif width == 4:
            x = np.frombuffer(raw, dtype="<i4").astype(np.float32) / 2147483648.0
        elif width == 1:
            x = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128.0) / 128.0
        else:
            raise ValueError(f"unsupported WAV sample width {width}")
        if ch > 1:
            x = x.reshape(-1, ch).mean(axis=1)
        return x, sr
    if width != 2:
        raise ValueError("16-bit WAV needed without numpy")
    vals = struct.unpack(f"<{len(raw) // 2}h", raw)
    if ch > 1:
        vals = [sum(vals[i:i + ch]) / ch for i in range(0, len(vals), ch)]
    return [v / 32768.0 for v in vals], sr


def write_wav(path: str, samples, sr: int) -> None:
    """16-bit mono WAV at `sr` (no resampling, no normalisation; values beyond 1 are clipped)."""
    os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
    if np is not None:
        x = np.clip(np.asarray(samples, dtype=np.float32), -1.0, 1.0)
        data = (x * 32767.0).astype("<i2").tobytes()
    else:
        data = struct.pack(f"<{len(samples)}h", *(int(max(-1.0, min(1.0, v)) * 32767) for v in samples))
    tmp = path + ".tmp"
    with wave.open(tmp, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(int(sr))
        wf.writeframes(data)
    os.replace(tmp, path)


def silence(ms: float, sr: int):
    n = max(0, int(sr * ms / 1000.0))
    return np.zeros(n, dtype=np.float32) if np is not None else [0.0] * n


def trim_silence(x, sr: int, floor_db: float = -42.0, pad_start_ms: int = 40, pad_end_ms: int = 90):
    """Drops leading/trailing silence (frames 40 dB under the peak), keeping a short pad."""
    if np is None or len(x) == 0:
        return x
    peak = float(np.max(np.abs(x))) or 1e-9
    thr = max(peak * 10 ** (floor_db / 20.0), 1e-4)
    hop = max(1, sr // 100)
    n = len(x) // hop
    if n == 0:
        return x
    frames = np.sqrt(np.mean(x[: n * hop].reshape(n, hop) ** 2, axis=1))
    loud = np.nonzero(frames > thr)[0]
    if len(loud) == 0:
        return x[:0]
    start = max(0, loud[0] * hop - sr * pad_start_ms // 1000)
    end = min(len(x), (loud[-1] + 1) * hop + sr * pad_end_ms // 1000)
    return x[start:end]


def concat(parts):
    if np is not None:
        parts = [np.asarray(p, dtype=np.float32) for p in parts if len(p)]
        return np.concatenate(parts) if parts else np.zeros(0, dtype=np.float32)
    out = []
    for p in parts:
        out.extend(p)
    return out


def assemble_page(chunks: list, sr: int, lead_ms: float = 0, tail_ms: float = 0):
    """A page from its chunks [(samples, pause_before_ms)]: each chunk trimmed of the
    model's own leading/trailing silence, then our silences in between."""
    parts = [silence(lead_ms, sr)]
    for x, pause in chunks:
        parts.append(silence(pause, sr))
        parts.append(trim_silence(x, sr))
    parts.append(silence(tail_ms, sr))
    return concat(parts)


def ffmpeg_binary() -> str | None:
    base = os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")
    local = os.path.join(base, "bin", "ffmpeg.exe" if os.name == "nt" else "ffmpeg")
    if os.path.isfile(local):
        return local
    import shutil
    return shutil.which("ffmpeg")


def apply_tempo(path: str, tempo: float, sr: int) -> float | None:
    """Pitch-preserving tempo change in place (ffmpeg atempo). Returns the tempo applied,
    or None when it was ~1 or ffmpeg is missing."""
    if abs(float(tempo or 1.0) - 1.0) <= 0.01:
        return None
    ff = ffmpeg_binary()
    if not ff:
        return None
    import subprocess
    tmp = path + ".tempo.wav"
    r = subprocess.run([ff, "-y", "-v", "error", "-i", path, "-filter:a", f"atempo={tempo:.3f}", "-ac", "1", "-ar", str(sr),
                        "-c:a", "pcm_s16le", tmp], capture_output=True, timeout=120,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    if r.returncode == 0 and os.path.isfile(tmp):
        os.replace(tmp, path)
        return float(tempo)
    try:
        os.remove(tmp)
    except OSError:
        pass
    return None


def resample(x, sr: int, target: int):
    """Linear resampling, only used to bring a stray page to the chapter's rate."""
    if sr == target or np is None or len(x) == 0:
        return x
    n = max(1, int(round(len(x) * target / sr)))
    return np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x).astype(np.float32)


# ----------------------------------------------------------------------------- loudness

def _biquad(kind: str, fs: float):
    """RBJ biquads for the BS.1770 K-weighting (as pyloudnorm builds them for any rate)."""
    if kind == "shelf":
        G, Q, fc = 4.0, 1 / math.sqrt(2), 1500.0
        A = 10 ** (G / 40)
        w0 = 2 * math.pi * fc / fs
        alpha = math.sin(w0) / (2 * Q)
        c = math.cos(w0)
        sa = 2 * math.sqrt(A) * alpha
        b = [A * ((A + 1) + (A - 1) * c + sa), -2 * A * ((A - 1) + (A + 1) * c), A * ((A + 1) + (A - 1) * c - sa)]
        a = [(A + 1) - (A - 1) * c + sa, 2 * ((A - 1) - (A + 1) * c), (A + 1) - (A - 1) * c - sa]
    else:
        Q, fc = 0.5, 38.0
        w0 = 2 * math.pi * fc / fs
        alpha = math.sin(w0) / (2 * Q)
        c = math.cos(w0)
        b = [(1 + c) / 2, -(1 + c), (1 + c) / 2]
        a = [1 + alpha, -2 * c, 1 - alpha]
    return b, a


def _k_weight(x, sr: int):
    """K-weighted signal by zero-phase FFT filtering (only the magnitude matters for energy)."""
    n = len(x)
    size = 1 << max(10, (n - 1).bit_length())
    X = np.fft.rfft(x, size)
    w = np.linspace(0, math.pi, len(X))
    z1, z2 = np.exp(-1j * w), np.exp(-2j * w)
    H = np.ones(len(X), dtype=np.complex128)
    for kind in ("shelf", "hpf"):
        b, a = _biquad(kind, sr)
        H *= (b[0] + b[1] * z1 + b[2] * z2) / (a[0] + a[1] * z1 + a[2] * z2)
    return np.fft.irfft(X * np.abs(H), size)[:n]


def block_energies(x, sr: int, weighted: bool = True) -> list:
    """Mean-square energy of 400 ms blocks with 75 % overlap (BS.1770)."""
    if np is not None:
        x = np.asarray(x, dtype=np.float64)
        if len(x) == 0:
            return []
        y = _k_weight(x, sr) if weighted else x
        block, hop = int(0.4 * sr), int(0.1 * sr)
        if len(y) < block:
            return [float(np.mean(y ** 2))]
        c = np.concatenate([[0.0], np.cumsum(y ** 2)])
        starts = np.arange(0, len(y) - block + 1, hop)
        return list(((c[starts + block] - c[starts]) / block).astype(float))
    # stdlib: unweighted, every 4th sample.
    step = 4
    block, hop = max(1, int(0.4 * sr / step)), max(1, int(0.1 * sr / step))
    y = x[::step]
    out = []
    for s in range(0, max(1, len(y) - block + 1), hop):
        seg = y[s:s + block]
        if seg:
            out.append(sum(v * v for v in seg) / len(seg))
    return out


def integrated_loudness(energies: list) -> float | None:
    """Gated integrated loudness (LUFS) of a list of block energies."""
    def lk(z):
        return -0.691 + 10 * math.log10(z) if z > 0 else -float("inf")
    blocks = [z for z in energies if lk(z) > -70.0]
    if not blocks:
        return None
    rel = lk(sum(blocks) / len(blocks)) - 10.0
    gated = [z for z in blocks if lk(z) > rel]
    if not gated:
        return None
    return lk(sum(gated) / len(gated))


def limit_peaks(x, sr: int, ceiling: float = CEILING, block_ms: float = 10.0, hold: int = 2):
    """Look-ahead peak limiter (numpy): a smooth gain that keeps every sample at or under
    `ceiling`. The gain each 10 ms block needs is spread to its `hold` neighbours on both
    sides and interpolated between block centres, so it ramps down before a peak and back up
    after it instead of bending the waveform (which `soft_limit` does)."""
    x = np.asarray(x, dtype=np.float32)
    a = np.abs(x)
    if len(x) == 0 or float(a.max()) <= ceiling:
        return x
    b = max(1, int(sr * block_ms / 1000.0))
    n = -(-len(x) // b)
    padded = np.zeros(n * b, dtype=np.float32)
    padded[: len(x)] = a
    need = np.minimum(1.0, ceiling / np.maximum(padded.reshape(n, b).max(axis=1), 1e-9))
    g = need.copy()
    for k in range(1, min(hold, n - 1) + 1):
        g[k:] = np.minimum(g[k:], need[:-k])
        g[:-k] = np.minimum(g[:-k], need[k:])
    # Every sample of block k lies between the centres of k and k±1; both gains are at most
    # need[k], so the interpolated gain never lets a sample over the ceiling.
    gain = np.interp(np.arange(len(x)), (np.arange(n) + 0.5) * b, g).astype(np.float32)
    return x * gain


def soft_limit(x, ceiling: float = CEILING):
    """Leaves everything under `ceiling` alone and bends the rest smoothly under 1.0."""
    if np is not None:
        x = np.asarray(x, dtype=np.float32)
        a = np.abs(x)
        over = a > ceiling
        if np.any(over):
            room = 1.0 - ceiling
            a2 = ceiling + room * np.tanh((a[over] - ceiling) / room)
            x = x.copy()
            x[over] = np.sign(x[over]) * a2
        return x
    room = 1.0 - ceiling
    return [v if abs(v) <= ceiling else math.copysign(ceiling + room * math.tanh((abs(v) - ceiling) / room), v) for v in x]


def normalize_chapter(files: list, target: float = TARGET_LUFS, max_gain_db: float = 24.0,
                      gain_db: float | None = None, rate: int | None = None) -> dict:
    """One gain for the whole chapter so its integrated loudness is about `target` LUFS
    (pages keep their relative levels), peak-limited at -1 dBFS. Pages at another sample
    rate than most pages (or than `rate`) are resampled to it, so ffmpeg can concatenate them.
    `gain_db` applies a known gain instead of measuring (pages added to a chapter that was
    normalised before, so they match it)."""
    pages = []
    for f in files:
        try:
            x, sr = read_wav(f)
            pages.append((f, x, sr))
        except Exception:
            continue
    if not pages:
        return {"ok": False, "message": "no readable audio"}
    rates: dict = {}
    for _, x, sr in pages:
        rates[sr] = rates.get(sr, 0) + len(x)
    rate = int(rate) if rate else max(rates, key=rates.get)
    before = None
    if gain_db is None:
        energies: list = []
        for _, x, sr in pages:
            energies += block_energies(x, sr)
        before = integrated_loudness(energies)
        gain_db = 0.0 if before is None else clamp(target - before, -max_gain_db, max_gain_db)
    g = 10 ** (float(gain_db) / 20.0)
    limited = 0
    resampled = 0
    for f, x, sr in pages:
        if np is not None:
            y = np.asarray(x, dtype=np.float32) * g
            if len(y) and float(np.max(np.abs(y))) > CEILING:
                limited += 1
                y = limit_peaks(y, sr)
            y = soft_limit(y)  # only float rounding is left over the ceiling here
        else:
            y = soft_limit([v * g for v in x])
        if sr != rate and np is not None:
            y = resample(y, sr, rate)
            resampled += 1
        write_wav(f, y, rate if np is not None else sr)
    return {
        "ok": True,
        "lufs_before": None if before is None else round(before, 2),
        "gain_db": round(gain_db, 2),
        "target": target,
        "sample_rate": rate,
        "pages": len(pages),
        "limited_pages": limited,
        "resampled_pages": resampled,
        "weighting": "K (BS.1770)" if np is not None else "unweighted RMS (no numpy)",
    }


def wav_duration(path: str) -> float:
    try:
        with wave.open(path, "rb") as wf:
            return wf.getnframes() / float(wf.getframerate() or 1)
    except Exception:
        return 0.0


# ----------------------------------------------------------------------------- CLI

def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    cmd = sys.argv[1]
    if cmd == "loudnorm":
        with open(sys.argv[2], "r", encoding="utf-8") as f:
            args = json.load(f)
        print(json.dumps(normalize_chapter(args.get("files") or [], float(args.get("target", TARGET_LUFS)),
                                           gain_db=args.get("gain_db"), rate=args.get("rate"))), flush=True)
        return 0
    if cmd == "plan":
        out_dir = sys.argv[2]
        script = _read_json(os.path.join(out_dir, "script_edited.json")) or []
        plan = build_plan(script, out_dir)
        for p in plan:
            p["chunks"] = plan_chunks(p["units"])
            for c in p["chunks"]:
                c["params"] = chatterbox_params(c["kind"], c["intensity"], c["line_kind"])
        print(json.dumps(plan, indent=1, ensure_ascii=False))
        return 0
    print(json.dumps({"ok": False, "message": f"unknown command {cmd}"}))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
