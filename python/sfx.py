"""
Sound effects for MangaVox recap videos, and the music duck under the voice.

The story writer may cue a sound effect on a narration beat (sync_map beat keys `sfx`, one of
SOUNDS, and `sfx_strength` 1-3). The renderer turns those cues, plus a few of its own picture
moments (whip / zoom page turns, action punches), into one sparse SFX stem (`build_track`):

* Every sound is synthesised here with numpy (48 kHz, cached as WAVs in the fx cache,
  `%TEMP%/mangavox-fx/sfx/`), so nothing can be claimed or needs a licence. A user may drop their
  own licensed sounds in `<data>/studio/sfx/<name>*.wav|mp3|ogg|flac`; those win over the
  built-in one. No third-party sound is ever bundled.
* Placement is sparse (PLACEMENT constants): about one cue per SECONDS_PER_CUE seconds, never
  two within HARD_GAP, SPACING apart (SPACING_EARLY in the first EARLY_SEC seconds); the
  writer's cues outrank the renderer's own; hits land in the pause at the beat boundary;
  whooshes peak in the middle of the page turn; a riser ends exactly on its beat.
* Levels are loudness against loudness (LEVELS, by strength): a sound's loudest momentary LUFS
  sits that far under the narration's integrated LUFS.

`make_duck_gain` writes a gain curve that keeps the music DUCK_UNDER_DB under the voice while it
speaks (and never louder than GAP_UNDER_DB under it in the pauses), applied by ffmpeg amultiply.
It only ever turns the music down, so a score that is already ducked is left as it is.
"""
import glob
import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
import wave

try:
    import numpy as np
except ImportError:  # the renderer then mixes without effects (and ducks with ffmpeg alone)
    np = None

SR = 48000
# Bumped whenever a built-in sound changes, so cached WAVs are remade.
SYNTH_VERSION = 1
SOUNDS = (
    "impact", "whoosh", "riser", "boom", "sword_clang", "magic", "heartbeat", "glass",
    "crowd_gasp", "thunder", "footsteps_run", "door_slam",
)
USER_EXTS = (".wav", ".mp3", ".ogg", ".flac")

# ── Levels: dB relative to the narration (the sound's loudest 400 ms, K-weighted, against the
# voice's integrated loudness). Strength 1 / 2 / 3 -> low / middle / high end of the range.
CATEGORY = {
    "impact": "hit", "boom": "hit", "sword_clang": "hit", "door_slam": "hit", "thunder": "hit",
    "glass": "hit", "whoosh": "sweep", "riser": "sweep", "magic": "sweep", "footsteps_run": "sweep",
    "heartbeat": "bed", "crowd_gasp": "bed",
}
LEVELS = {"hit": (-18.0, -12.0), "sweep": (-25.0, -18.0), "bed": (-32.0, -24.0)}
# "Punchy" (Music step) lifts every effect a little.
PUNCHY_DB = 2.0
# Where a sound sits against its moment: "onset" starts on it, "peak" peaks on it, "end" ends on it.
ANCHOR = {"whoosh": "peak", "riser": "end"}

# ── Placement.
HARD_GAP = 1.2           # never two effects within this many seconds
SPACING = 8.0            # usual distance between two effects
SPACING_EARLY = 4.0      # ... in the opening EARLY_SEC seconds, which may be a little denser
SPACING_BIG = 4.0        # ... between two of the writer's strength-3 cues
EARLY_SEC = 30.0
SECONDS_PER_CUE = 14.0   # about one effect per this many seconds overall (10 when punchy)
GAP_SEARCH = (-0.35, 0.15)  # where a hit looks for a pause around its beat boundary (seconds)
MIN_PAUSE = 0.06         # shortest pause a hit may sit in
MASKED_DB = -3.0         # a writer's cue that cannot find a pause plays this much quieter
AUTO_MIN_INTENSITY = 6   # the renderer's own hits only on pages at least this intense
WHIP_TRANSITIONS = ("whip", "zoomblur")

# ── Music duck.
DUCK_UNDER_DB = 18.0     # music under the voice while it speaks
GAP_UNDER_DB = 6.0       # music in the pauses: never louder than this under the voice
DUCK_ATTACK = 0.02       # time to get (95 %) down when the voice starts
DUCK_RELEASE = 0.4       # ... and back up after the hold when it stops
DUCK_HOLD = 0.25         # bridges the gaps between words, so the music does not pump
DUCK_LOOKAHEAD = 0.03    # the dip lands just before the first syllable
DUCK_MAX_DB = 40.0
DUCK_RATE = 4800         # sample rate of the gain curve (ffmpeg resamples it)


# ─────────────────────────────────────────────────────────────────────────── helpers

def normalize_name(name):
    """A cue name from a beat (`"Sword clang"`, `"door-slam"`) as one of SOUNDS, or None."""
    if not isinstance(name, str):
        return None
    key = name.strip().lower().replace(" ", "_").replace("-", "_")
    return key if key in SOUNDS else None


def normalize_strength(v):
    try:
        s = float(v)
    except (TypeError, ValueError):
        return 2
    return int(min(3, max(1, round(s)))) if math.isfinite(s) else 2


def level_db(name, strength, punchy=False):
    """The cue's level relative to the narration (dB)."""
    lo, hi = LEVELS[CATEGORY[name]]
    db = lo + (hi - lo) * (normalize_strength(strength) - 1) / 2.0
    return db + (PUNCHY_DB if punchy else 0.0)


def mode_of(value):
    """The renderer's `sfx` setting: "off", "subtle" or "punchy" (on unless turned off)."""
    if isinstance(value, str):
        v = value.strip().lower()
        if v in ("off", "false", "0", "none", "no"):
            return "off"
        return "punchy" if v == "punchy" else "subtle"
    if value is False or value == 0:
        return "off"
    return "subtle"


def fx_cache_dir():
    """The shared fx cache (as the renderer's), or None when it cannot be made."""
    path = os.environ.get("MANGAVOX_FX_CACHE") or os.path.join(tempfile.gettempdir(), "mangavox-fx")
    try:
        os.makedirs(path, exist_ok=True)
        return path
    except OSError:
        return None


def _seed(name):
    return int(hashlib.md5(name.encode("utf-8")).hexdigest()[:8], 16)


def _t(dur):
    return np.arange(int(round(dur * SR))) / SR


def _attack(t, sec):
    return np.clip(t / max(sec, 1e-6), 0.0, 1.0)


def _shape(x, gain_fn):
    """Zero-phase filtering in the frequency domain (gain_fn maps Hz -> gain)."""
    n = len(x)
    size = 1 << (n + SR // 10 - 1).bit_length()
    spec = np.fft.rfft(x, size)
    f = np.fft.rfftfreq(size, 1.0 / SR)
    return np.fft.irfft(spec * gain_fn(f), size)[:n]


def _lp(x, fc, order=2):
    return _shape(x, lambda f: 1.0 / np.sqrt(1.0 + (f / fc) ** (2 * order)))


def _hp(x, fc, order=2):
    return _shape(x, lambda f: 1.0 / np.sqrt(1.0 + (fc / np.maximum(f, 1e-3)) ** (2 * order)))


def _bp(x, lo, hi, order=2):
    return _hp(_lp(x, hi, order), lo, order)


def _sweep(x, fc, q):
    """Band-pass with a moving centre `fc` (Hz per sample): a TPT state-variable filter."""
    g = np.tan(np.pi * np.clip(fc, 20.0, SR * 0.45) / SR)
    k = 1.0 / q
    a1 = 1.0 / (1.0 + g * (g + k))
    a2 = g * a1
    a3 = g * a2
    out = [0.0] * len(x)
    ic1 = ic2 = 0.0
    for i, (v0, b1, b2, b3) in enumerate(zip(x.tolist(), a1.tolist(), a2.tolist(), a3.tolist())):
        v3 = v0 - ic2
        v1 = b1 * ic1 + b2 * v3
        v2 = ic2 + b2 * ic1 + b3 * v3
        ic1 = 2.0 * v1 - ic1
        ic2 = 2.0 * v2 - ic2
        out[i] = k * v1
    return np.asarray(out)


def _phase(freq):
    return 2.0 * np.pi * np.cumsum(freq) / SR


def _unit(x):
    m = np.max(np.abs(x)) if len(x) else 0.0
    return x / m if m > 0 else x


def _sat(x, drive):
    return np.tanh(drive * x) / np.tanh(drive)


def _brown(n, rng):
    return _unit(_hp(np.cumsum(rng.standard_normal(n)), 20.0))


# ─────────────────────────────────────────────────────────────────────────── the sounds

def _whoosh(rng):
    dur, peak = 0.9, 0.36
    t = _t(dur)
    rise = np.clip(t / peak, 0, 1) ** 2.2
    fall = np.exp(-np.maximum(t - peak, 0) / 0.16)
    env = np.where(t < peak, rise, fall)
    # Pitch rises into the pass and falls away after it (a doppler-ish sweep).
    fc = np.where(t < peak, 450.0 * 5.5 ** (t / peak), 2475.0 * 0.3 ** ((t - peak) / (dur - peak)))
    body = _sweep(rng.standard_normal(len(t)), fc, 1.4)
    hiss = _sweep(rng.standard_normal(len(t)), fc * 2.2, 2.5)
    rush = _lp(rng.standard_normal(len(t)), 900.0)
    return (_unit(body) + 0.35 * _unit(hiss) + 0.45 * _unit(rush)) * env


def _impact(rng):
    t = _t(0.8)
    f = 45.0 + 75.0 * np.exp(-t / 0.03)
    thump = 0.8 * np.sin(_phase(f)) * np.exp(-t / 0.09) * _attack(t, 0.002)
    click = _unit(_hp(rng.standard_normal(len(t)), 2500.0)) * np.exp(-t / 0.004)
    crack = _unit(_bp(rng.standard_normal(len(t)), 150.0, 3000.0)) * np.exp(-t / 0.05)
    # A mid-range body, so the hit carries on small speakers too.
    body = np.sin(2 * np.pi * 180.0 * t) * np.exp(-t / 0.05) * _attack(t, 0.002)
    return _sat(thump + 0.4 * click + 1.3 * crack + 0.7 * body, 1.8)


def _riser(rng):
    dur = 2.2
    t = _t(dur)
    u = t / dur
    noise = _unit(_sweep(rng.standard_normal(len(t)), 300.0 * 20.0 ** u, 2.0))
    f = 180.0 * 2.0 ** (2.0 * u)
    tone = sum(np.sin(_phase(f * h)) / h + np.sin(_phase(f * h * 1.006)) / h for h in range(1, 6))
    return 0.75 * noise * u ** 2.5 + 0.3 * _unit(tone) * u ** 2.0


def _boom(rng):
    t = _t(2.6)
    f = 28.0 + 50.0 * np.exp(-t / 0.25)
    ph = _phase(f)
    sub = np.sin(ph) * np.exp(-t / 0.7) * _attack(t, 0.005)
    # A harmonic so small speakers hear it too.
    sub2 = 0.3 * np.sin(2 * ph) * np.exp(-t / 0.4) * _attack(t, 0.005)
    rumble = _unit(_lp(_brown(len(t), rng), 220.0)) * np.exp(-t / 0.9) * _attack(t, 0.02)
    hit = _unit(_lp(rng.standard_normal(len(t)), 2500.0)) * np.exp(-t / 0.05)
    crunch = _unit(_bp(rng.standard_normal(len(t)), 300.0, 1500.0)) * np.exp(-t / 0.15)
    return _sat(sub + sub2 + 0.6 * rumble + 0.8 * hit + 0.35 * crunch, 1.5)


def _sword_clang(rng):
    t = _t(1.6)
    f0 = 620.0
    # Free-bar modes plus a few stray partials: inharmonic, metallic.
    parts = [(1.0, 1.0, 1.1), (2.756, 0.8, 0.8), (3.17, 0.35, 0.6), (4.53, 0.3, 0.5),
             (5.404, 0.55, 0.55), (6.98, 0.25, 0.35), (8.933, 0.35, 0.35), (13.34, 0.2, 0.22)]
    ring = np.zeros(len(t))
    for ratio, amp, tau in parts:
        f = f0 * ratio
        beat = 1.0 + rng.uniform(0.0015, 0.004)
        ring += amp * (np.sin(2 * np.pi * f * t + rng.uniform(0, 6.28)) + 0.6 * np.sin(2 * np.pi * f * beat * t)) * np.exp(-t / tau)
    ring *= _attack(t, 0.001)
    hit = _unit(_hp(rng.standard_normal(len(t)), 3000.0)) * np.exp(-t / 0.006)
    scrape = _unit(_bp(rng.standard_normal(len(t)), 1500.0, 6000.0)) * np.exp(-t / 0.05)
    return _unit(ring) + 0.8 * hit + 0.25 * scrape


def _magic(rng):
    t = _t(1.8)
    x = np.zeros(len(t))
    for i, f in enumerate((1046.5, 1318.5, 1568.0, 1975.5)):
        tt = t - 0.07 * i
        on = tt >= 0
        for ratio, amp, tau in ((1.0, 1.0, 0.9), (2.0, 0.5, 0.6), (2.76, 0.35, 0.45), (4.07, 0.2, 0.3), (5.4, 0.12, 0.2)):
            x[on] += 0.8 ** i * amp * np.sin(2 * np.pi * f * ratio * tt[on]) * np.exp(-tt[on] / tau) * _attack(tt[on], 0.004)
    for _ in range(40):
        t0 = 1.2 * rng.random() ** 1.5
        tt = t - t0
        on = tt >= 0
        x[on] += rng.uniform(0.05, 0.15) * np.sin(2 * np.pi * rng.uniform(3000, 9000) * tt[on]) * np.exp(-tt[on] / rng.uniform(0.02, 0.08))
    shimmer = _unit(_hp(rng.standard_normal(len(t)), 5000.0)) * (0.5 + 0.5 * np.sin(2 * np.pi * 9 * t)) * _attack(t, 0.15) * np.exp(-t / 0.5)
    return (_unit(x) + 0.12 * shimmer) * _attack(t, 0.015)


def _heartbeat(rng):
    t = _t(1.0)
    x = np.zeros(len(t))
    thud = _unit(_lp(rng.standard_normal(len(t)), 500.0))
    for t0, amp in ((0.0, 1.0), (0.27, 0.72)):
        tt = t - t0
        on = tt >= 0
        f = 48.0 + 30.0 * np.exp(-tt[on] / 0.02)
        x[on] += amp * np.sin(_phase(f)) * np.exp(-tt[on] / 0.075) * _attack(tt[on], 0.006)
        x[on] += amp * 0.25 * thud[on] * np.exp(-tt[on] / 0.03)
    return _sat(_lp(x, 500.0), 2.0)


def _glass(rng):
    t = _t(1.5)
    x = _unit(_hp(rng.standard_normal(len(t)), 1500.0)) * np.exp(-t / 0.008)
    for _ in range(28):
        t0 = 0.6 * rng.random() ** 2
        tt = t - t0
        on = tt >= 0
        f = rng.uniform(2500, 9000)
        tau = rng.uniform(0.04, 0.25)
        amp = rng.uniform(0.1, 0.5) * math.exp(-t0 / 0.3)
        for ratio, a in ((1.0, 1.0), (1.47, 0.5), (2.09, 0.3)):
            x[on] += amp * a * np.sin(2 * np.pi * f * ratio * tt[on] + rng.uniform(0, 6.28)) * np.exp(-tt[on] / tau)
    tail = _unit(_hp(rng.standard_normal(len(t)), 3000.0)) * np.exp(-t / 0.12)
    return x + 0.15 * tail


def _crowd_gasp(rng):
    t = _t(1.4)
    n = len(t)
    x = np.zeros(n)
    for _ in range(7):
        t0 = 0.08 + rng.uniform(-0.06, 0.08)
        tt = t - t0
        env = np.where(tt < 0, 0.0, np.where(tt < 0.32, np.clip(tt / 0.32, 0, 1) ** 1.5, np.exp(-(tt - 0.32) / 0.35)))
        f1, f2 = rng.uniform(550, 850), rng.uniform(1000, 1500)
        voice = (0.6 * _unit(_bp(rng.standard_normal(n), f1 * 0.8, f1 * 1.25))
                 + 0.4 * _unit(_bp(rng.standard_normal(n), f2 * 0.85, f2 * 1.2))
                 + 0.5 * _unit(_bp(rng.standard_normal(n), 2000.0, 5000.0)))
        x += voice * env * rng.uniform(0.6, 1.0)
    return x


def _thunder(rng):
    t = _t(3.6)
    n = len(t)
    crack = np.zeros(n)
    for t0, amp in ((0.0, 1.0), (0.03, 0.7), (0.09, 0.5), (0.16, 0.35)):
        tt = t - t0
        on = tt >= 0
        crack[on] += amp * np.exp(-tt[on] / 0.05)
    crack *= _unit(_hp(rng.standard_normal(n), 800.0))
    lfo = _unit(_lp(rng.standard_normal(n), 4.0))
    rumble = _unit(_lp(_brown(n, rng), 180.0)) * (0.6 + 0.4 * lfo) * _attack(t, 0.12) * np.exp(-t / 1.3)
    low = np.sin(2 * np.pi * 40.0 * t) * np.exp(-t / 0.8) * _attack(t, 0.03)
    return 0.55 * crack + rumble + 0.3 * low


def _footsteps_run(rng):
    t = _t(1.6)
    n = len(t)
    x = np.zeros(n)
    grit = _unit(_bp(rng.standard_normal(n), 900.0, 4000.0))
    for i in range(6):
        t0 = 0.05 + 0.24 * i + rng.uniform(-0.015, 0.015)
        amp = (0.85 if i % 2 else 1.0) * (0.8 + 0.2 * math.sin(math.pi * i / 5))
        tt = t - t0
        on = tt >= 0
        x[on] += amp * 0.6 * np.sin(2 * np.pi * 85.0 * tt[on]) * np.exp(-tt[on] / 0.03) * _attack(tt[on], 0.002)
        x[on] += amp * 1.2 * grit[on] * np.exp(-tt[on] / 0.012)
        heel = tt - 0.03
        on2 = heel >= 0
        x[on2] += amp * 0.5 * grit[on2] * np.exp(-heel[on2] / 0.01)
    return x


def _door_slam(rng):
    t = _t(1.2)
    n = len(t)
    thump = np.sin(_phase(50.0 + 40.0 * np.exp(-t / 0.02))) * np.exp(-t / 0.1) * _attack(t, 0.002)
    wood = _unit(_bp(rng.standard_normal(n), 150.0, 700.0)) * np.exp(-t / 0.12)
    knock = np.sin(2 * np.pi * 160.0 * t) * np.exp(-t / 0.06) * _attack(t, 0.002)
    click = _unit(_hp(rng.standard_normal(n), 2500.0)) * np.exp(-t / 0.004)
    rattle = np.zeros(n)
    hiss = _unit(_bp(rng.standard_normal(n), 1500.0, 5000.0))
    for t0 in (0.07, 0.12):
        tt = t - t0
        on = tt >= 0
        rattle[on] += hiss[on] * np.exp(-tt[on] / 0.01)
    room = _unit(_lp(rng.standard_normal(n), 1200.0)) * np.exp(-t / 0.25) * _attack(t, 0.01)
    return _sat(thump + 1.0 * wood + 0.4 * knock + 0.5 * click + 0.25 * rattle + 0.12 * room, 1.6)


SYNTHS = {
    "impact": _impact, "whoosh": _whoosh, "riser": _riser, "boom": _boom, "sword_clang": _sword_clang,
    "magic": _magic, "heartbeat": _heartbeat, "glass": _glass, "crowd_gasp": _crowd_gasp,
    "thunder": _thunder, "footsteps_run": _footsteps_run, "door_slam": _door_slam,
}


def synthesize(name):
    """The built-in sound `name`: mono float at SR, DC-free, peaking at -1 dBFS, with short
    fades (a longer one into a riser, none needed out of it: it ends on its beat)."""
    rng = np.random.default_rng(_seed(name))
    x = _hp(SYNTHS[name](rng).astype(np.float64), 25.0)
    n = len(x)
    fin = int((0.05 if name == "riser" else 0.001) * SR)
    fout = int((0.025 if name == "riser" else 0.02) * SR)
    x[:fin] *= np.linspace(0.0, 1.0, fin, endpoint=False)
    x[n - fout:] *= np.cos(np.linspace(0.0, np.pi / 2, fout)) ** 2
    return (_unit(x) * 10 ** (-1.0 / 20)).astype(np.float32)


# ─────────────────────────────────────────────────────────────────────────── files

def write_wav(path, x, rate=SR):
    """Mono 16-bit WAV, written atomically."""
    data = (np.clip(x, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    tmp = path + ".tmp"
    with wave.open(tmp, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(data)
    os.replace(tmp, path)


def read_wav(path):
    """Mono float samples of a 16-bit WAV (channels averaged) and its rate."""
    with wave.open(path, "rb") as w:
        ch, width, rate = w.getnchannels(), w.getsampwidth(), w.getframerate()
        raw = w.readframes(w.getnframes())
    if width != 2:
        raise ValueError(f"{path}: {8 * width}-bit WAV")
    x = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x, rate


def decode(path, rate=SR, loop_to=None):
    """Any audio file as mono float32 at `rate` (ffmpeg), optionally looped to `loop_to` s."""
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error"]
    if loop_to:
        cmd += ["-stream_loop", "-1"]
    cmd += ["-i", path]
    if loop_to:
        cmd += ["-t", f"{loop_to:.3f}"]
    cmd += ["-map", "0:a:0", "-ac", "1", "-ar", str(rate), "-f", "f32le", "-"]
    out = subprocess.run(cmd, capture_output=True, timeout=1800, check=True).stdout
    return np.frombuffer(out, dtype="<f4").copy()


def user_sounds(name, user_dir):
    """The user's own files for `name` in `user_dir` (`<name>*.wav|mp3|ogg|flac`), sorted."""
    if not user_dir or not os.path.isdir(user_dir):
        return []
    found = []
    for p in glob.glob(os.path.join(glob.escape(user_dir), "*")):
        base = os.path.basename(p).lower()
        if base.startswith(name) and base.endswith(USER_EXTS) and os.path.isfile(p):
            # "impact.wav", "impact_2.wav", "impact (heavy).ogg" are impacts; "impactful.wav" is not.
            rest = base[len(name):].rsplit(".", 1)[0]
            if not rest or not rest[0].isalpha():
                found.append(p)
    return sorted(found)


def _user_sound(path, cache):
    st = os.stat(path)
    key = hashlib.md5(f"{os.path.abspath(path)}|{st.st_mtime_ns}|{st.st_size}".encode()).hexdigest()[:16]
    cached = os.path.join(cache, f"user_{key}.wav") if cache else None
    if cached and os.path.exists(cached):
        return read_wav(cached)[0]
    x = decode(path)
    if not len(x):
        raise ValueError(f"{path}: no audio")
    peak = float(np.max(np.abs(x)))
    if peak <= 0:
        raise ValueError(f"{path}: silent")
    # Leading silence trimmed, so the sound starts on its moment; peak at -1 dBFS.
    start = int(np.argmax(np.abs(x) > peak * 0.003))
    x = (x[start:] / peak * 10 ** (-1.0 / 20)).astype(np.float32)
    if cached:
        write_wav(cached, x)
    return x


def load_sound(name, user_dir=None, variant=0):
    """Samples of sound `name`: the user's own file when there is one (variant picks among
    several), else the built-in synthesis (cached)."""
    cache = fx_cache_dir()
    cache = os.path.join(cache, "sfx") if cache else None
    if cache:
        os.makedirs(cache, exist_ok=True)
    mine = user_sounds(name, user_dir)
    if mine:
        try:
            return _user_sound(mine[variant % len(mine)], cache)
        except Exception as e:
            print(f"[sfx] Could not use {mine[variant % len(mine)]}: {e}", file=sys.stderr)
    path = os.path.join(cache, f"{name}_v{SYNTH_VERSION}.wav") if cache else None
    if path and os.path.exists(path):
        try:
            return read_wav(path)[0]
        except Exception:
            pass
    x = synthesize(name)
    if path:
        try:
            write_wav(path, x)
        except OSError:
            pass
    return x


# ─────────────────────────────────────────────────────────────────────────── measuring

# ITU-R BS.1770 K-weighting at 48 kHz (high shelf, then the RLB high-pass) as biquads.
K_SHELF = ((1.53512485958697, -2.69169618940638, 1.19839281085285), (1.0, -1.69065929318241, 0.73248077421585))
K_RLB = ((1.0, -2.0, 1.0), (1.0, -1.99004745483398, 0.99007225036621))


def k_gain(f):
    """Magnitude of the K-weighting filter at frequencies `f` (Hz)."""
    z = np.exp(-1j * 2 * np.pi * np.asarray(f, dtype=np.float64) / SR)
    g = np.ones(len(z))
    for b, a in (K_SHELF, K_RLB):
        g = g * np.abs((b[0] + b[1] * z + b[2] * z * z) / (a[0] + a[1] * z + a[2] * z * z))
    return g


def body_lufs(x):
    """The loudest EBU momentary loudness of a sound (LUFS of its loudest 400 ms, K-weighted):
    what a loudness meter shows for it, comparable with the narration's integrated loudness."""
    return body_db(_shape(np.asarray(x, dtype=np.float64), k_gain)) - 0.691


def narration_lufs(path, volume=1.0):
    """Integrated loudness (LUFS) of the narration file (ffmpeg ebur128), or None."""
    try:
        r = subprocess.run(["ffmpeg", "-hide_banner", "-nostats", "-i", path, "-af", "ebur128", "-f", "null", "-"],
                           capture_output=True, text=True, timeout=600)
        found = [ln.split("I:")[1].split("LUFS")[0] for ln in (r.stderr or "").splitlines() if ln.strip().startswith("I:")]
        v = float(found[-1]) if found else None
        return v + 20 * math.log10(volume) if v is not None and math.isfinite(v) and v > -70 else None
    except Exception:
        return None


def body_db(x):
    """Loudest 400 ms of a sound (RMS in dBFS, a short sound padded with silence)."""
    win = int(0.4 * SR)
    if len(x) <= win:
        p = float(np.sum(np.square(x, dtype=np.float64))) / win
    else:
        c = np.concatenate([[0.0], np.cumsum(np.square(x, dtype=np.float64))])
        hop = int(0.05 * SR)
        idx = np.arange(0, len(x) - win + 1, hop)
        p = float(np.max(c[idx + win] - c[idx])) / win
    return 10 * math.log10(max(p, 1e-12))


def speech_db(x, rate=SR):
    """The voice's level: gated average power of 400 ms blocks (75 % overlap; absolute gate
    -70 dBFS, relative gate 10 dB under the mean), as EBU integrated loudness gates. None when silent."""
    win, hop = int(0.4 * rate), int(0.1 * rate)
    if len(x) < win:
        return None
    c = np.concatenate([[0.0], np.cumsum(np.square(x, dtype=np.float64))])
    idx = np.arange(0, len(x) - win + 1, hop)
    p = (c[idx + win] - c[idx]) / win
    p = p[p > 1e-7]
    if not len(p):
        return None
    p = p[p > np.mean(p) * 0.1]
    return 10 * math.log10(float(np.mean(p)))


def frame_db(x, rate, frame):
    """Power per `frame` seconds (dBFS)."""
    n = max(1, int(round(frame * rate)))
    m = len(x) // n
    if m == 0:
        return np.full(1, -120.0)
    p = np.mean(np.square(x[:m * n].reshape(m, n), dtype=np.float64), axis=1)
    return 10 * np.log10(np.maximum(p, 1e-12))


def speech_mask(nar_db, level, frame):
    """Frames where the narrator speaks: above `level` - 25 dB (30 ms smoothing)."""
    k = max(1, int(round(0.03 / frame)))
    p = np.convolve(10 ** (nar_db / 10), np.ones(k) / k, mode="same")
    return 10 * np.log10(np.maximum(p, 1e-12)) > level - 25.0


# ─────────────────────────────────────────────────────────────────────────── placement

def load_scenes(output_dir):
    """Page index -> {"intensity", "kind", "tags"} from the Read step (`studio/read.json`)."""
    out = {}
    try:
        with open(os.path.join(output_dir, "studio", "read.json"), encoding="utf-8") as f:
            pages = json.load(f).get("pages") or []
        for p in pages:
            sc = p.get("scene") or {}
            out[int(p.get("index", 0))] = {
                "intensity": int(sc.get("intensity") or 0),
                "kind": str(sc.get("kind") or ""),
                "tags": [str(t) for t in sc.get("tags") or []],
            }
    except Exception:
        pass
    return out


def candidates(items, clips, starts, trans, scenes=None):
    """Moments that may get a sound: the writer's cues on the clips' beats (strength nudged by
    the page's intensity), whooshes on whip / zoom page turns and hits on action punches.
    Each: {"name", "strength", "at" (the moment, s), "clip", "ai", "intensity"}."""
    scenes = scenes or {}
    out = []
    for k, c in enumerate(clips):
        seg = items[c["idx"]] if 0 <= c.get("idx", -1) < len(items) else {}
        scene = scenes.get(c.get("page"))
        intensity = scene["intensity"] if scene else None
        name = normalize_name(seg.get("sfx"))
        tr = trans[k] if k < len(trans) else None
        turn = starts[k] + (tr[1] / 2.0 if tr else 0.0)
        if name:
            s = normalize_strength(seg.get("sfx_strength"))
            if intensity is not None:
                s = min(3, s + 1) if intensity >= 8 else max(1, s - 1) if intensity <= 3 else s
            at = turn if name == "whoosh" else starts[k]
            out.append({"name": name, "strength": s, "at": at, "clip": k, "ai": True, "intensity": intensity or 0})
            continue
        if k > 0 and tr and str(tr[0]).startswith(WHIP_TRANSITIONS) and tr[1] > 0.05:
            out.append({"name": "whoosh", "strength": 1, "at": turn, "clip": k, "ai": False, "intensity": intensity or 0})
        if c.get("impact") and (intensity is None or intensity >= AUTO_MIN_INTENSITY):
            out.append({"name": "impact", "strength": 1, "at": starts[k], "clip": k, "ai": False, "intensity": intensity or 0})
    return out


def _pair(a, b):
    """A riser that ends on the beat another hit lands on: they belong together."""
    return a["clip"] == b["clip"] and (a["name"] == "riser") != (b["name"] == "riser") and \
        CATEGORY[b["name"] if a["name"] == "riser" else a["name"]] == "hit"


def select(cands, total, punchy=False):
    """The cues that play: the writer's first (strongest, most intense, earliest), then the
    renderer's own, each kept only when it is HARD_GAP from every other and SPACING from it
    (SPACING_EARLY in the opening, SPACING_BIG between two strength-3 cues of the writer), up
    to about one per SECONDS_PER_CUE. A riser and the hit it ends on count as one."""
    per = SECONDS_PER_CUE * (10.0 / 14.0 if punchy else 1.0)
    budget = max(2, math.ceil(total / per)) + (1 if total > EARLY_SEC else 0)
    order = sorted(cands, key=lambda c: (not c["ai"], -c["strength"], -c["intensity"], c["at"]))
    kept, used = [], 0
    for c in order:
        if c["at"] < 0 or c["at"] > total:
            continue
        paired, ok = False, True
        for k in kept:
            gap = abs(c["at"] - k["at"])
            if _pair(c, k):
                paired = True
                continue
            if gap < HARD_GAP:
                ok = False
                break
            need = SPACING_EARLY if min(c["at"], k["at"]) < EARLY_SEC else SPACING
            if c["ai"] and k["ai"] and c["strength"] == 3 and k["strength"] == 3:
                need = min(need, SPACING_BIG)
            if gap < need:
                ok = False
                break
        if not ok or (not paired and used >= budget):
            continue
        kept.append(c)
        used += 0 if paired else 1
    return sorted(kept, key=lambda c: c["at"])


def _pause_near(speaking, at, frame):
    """Start (s) of the pause in the narration nearest to `at` within GAP_SEARCH, or None."""
    n = len(speaking)
    a = max(0, int((at + GAP_SEARCH[0]) / frame))
    b = min(n, int((at + GAP_SEARCH[1]) / frame) + 1)
    best = None
    i = a
    while i < b:
        if speaking[i]:
            i += 1
            continue
        j = i
        while j < n and not speaking[j]:
            j += 1
        # The pause may run on past the window; it only has to start (or be) inside it.
        if (j - i) * frame >= MIN_PAUSE:
            start = i * frame
            # A pause that began before the window: sit just inside the window.
            if i == a and a > 0 and not speaking[a - 1]:
                start = a * frame
            if best is None or abs(start - at) < abs(best - at):
                best = start
        i = j
    return best


def place(selected, sounds, narration=None, narration_level=None, punchy=False, loudness=None):
    """Timed, levelled events {"name", "start", "gain", "at"} for the selected cues. Hits move
    into the pause at their beat (the narration's own gaps); a writer's cue that finds none plays
    MASKED_DB quieter, one of the renderer's is dropped. `sounds` maps name -> samples. Levels
    are set loudness against loudness: each sound's loudest momentary LUFS sits its LEVELS dB
    under the narration's integrated `loudness` (else its plain `narration_level`)."""
    frame = 0.01
    level = loudness if loudness is not None else narration_level if narration_level is not None else -20.0
    speaking = None
    if narration is not None and len(narration) and narration_level is not None:
        speaking = speech_mask(frame_db(narration, SR, frame), narration_level, frame)
    out = []
    for c in selected:
        x = sounds[c["name"]]
        dur = len(x) / SR
        anchor = ANCHOR.get(c["name"], "onset")
        trim_db = 0.0
        if anchor == "end":
            start = c["at"] - dur
        elif anchor == "peak":
            env = frame_db(x, SR, 0.01)
            start = c["at"] - float(np.argmax(env)) * 0.01
        else:
            start = c["at"]
            if speaking is not None and CATEGORY[c["name"]] == "hit":
                pause = _pause_near(speaking, c["at"], frame)
                if pause is not None:
                    start = pause + 0.02
                elif c["ai"]:
                    trim_db = MASKED_DB
                else:
                    continue
        rel = level_db(c["name"], c["strength"], punchy) + trim_db
        gain = 10 ** ((level + rel - body_lufs(x)) / 20.0)
        out.append({"name": c["name"], "start": round(start, 3), "at": round(c["at"], 3), "gain": gain,
                    "rel_db": round(rel, 1), "strength": c["strength"], "ai": c["ai"]})
    return out


def render(events, sounds, total, out_path):
    """The SFX stem (mono 16-bit WAV at SR, `total` seconds) with every event mixed in."""
    n = int(math.ceil(total * SR)) + 1
    track = np.zeros(n, dtype=np.float64)
    for e in events:
        x = sounds[e["name"]].astype(np.float64) * e["gain"]
        s = int(round(e["start"] * SR))
        if s < 0:
            # Starts before the video does (a riser on an early beat): its tail only, faded in.
            x = x[-s:].copy()
            f = min(len(x), int(0.05 * SR))
            x[:f] *= np.linspace(0.0, 1.0, f, endpoint=False)
            s = 0
        x = x[:max(0, n - s)].copy()
        if not len(x):
            continue
        f = min(len(x), int(0.01 * SR))
        x[len(x) - f:] *= np.linspace(1.0, 0.0, f)
        track[s:s + len(x)] += x
    write_wav(out_path, track)
    return out_path


def build_track(items, clips, starts, trans, total, narration_path, out_path, mode="subtle",
                user_dir=None, scenes=None, narration_vol=1.0):
    """Plans, places and renders the chapter's SFX stem. Returns the placed events (the stem is
    written only when there is at least one)."""
    if np is None or mode_of(mode) == "off" or not clips:
        return []
    punchy = mode_of(mode) == "punchy"
    chosen = select(candidates(items, clips, starts, trans, scenes), total, punchy)
    if not chosen:
        return []
    narration, level = None, None
    if narration_path and os.path.exists(narration_path) and narration_vol > 0:
        try:
            narration = read_wav(narration_path)[0] * float(narration_vol)
            level = speech_db(narration)
        except Exception as e:
            print(f"[sfx] Could not read the narration: {e}", file=sys.stderr)
    counts = {}
    sounds = {}
    for c in chosen:
        if c["name"] not in sounds:
            sounds[c["name"]] = load_sound(c["name"], user_dir, counts.get(c["name"], 0))
        counts[c["name"]] = counts.get(c["name"], 0) + 1
    loud = narration_lufs(narration_path, narration_vol) if narration is not None else None
    events = place(chosen, sounds, narration, level, punchy, loud)
    if events:
        render(events, sounds, total, out_path)
    return events


def user_dir_for(config, output_dir):
    """The user's own sounds: config `sfx_dir`, MANGAVOX_SFX_DIR, else `<data>/studio/sfx` found
    above the chapter's folder (`<data>/studio/projects/<manga>/<chapter>`)."""
    d = (config or {}).get("sfx_dir") or os.environ.get("MANGAVOX_SFX_DIR")
    if d:
        return d
    p = os.path.abspath(output_dir or "")
    for _ in range(4):
        p = os.path.dirname(p)
        if os.path.basename(p).lower() == "studio":
            return os.path.join(p, "sfx")
    return None


# ─────────────────────────────────────────────────────────────────────────── music duck

def duck_gain(narration, music, rate=SR, under_db=DUCK_UNDER_DB, gap_db=GAP_UNDER_DB, frame=0.005):
    """Gain (dB per `frame`) that keeps `music` `under_db` under the narration's speech level
    while it speaks and at least `gap_db` under it in the pauses; never a boost. Both mono float
    at `rate` (as power: see _decode_level). Returns (gain_db, speech_level) or (None, None)
    when the narration is silent."""
    level = speech_db(narration, rate)
    if level is None:
        return None, None
    nar = frame_db(narration, rate, frame)
    speaking = speech_mask(nar, level, frame)
    if speaking.any():
        # The voice's level while it speaks (pauses left out).
        level = 10 * math.log10(float(np.mean(10 ** (nar[speaking] / 10))))
    # Hold through the gaps between words, and dip a little before the first syllable.
    hold, ahead = int(round(DUCK_HOLD / frame)), int(round(DUCK_LOOKAHEAD / frame))
    idx = np.flatnonzero(speaking)
    held = np.zeros(len(speaking) + hold + 1, dtype=np.int32)
    np.add.at(held, np.maximum(idx - ahead, 0), 1)
    np.add.at(held, np.minimum(idx + hold + 1, len(held) - 1), -1)
    active = np.cumsum(held)[:len(speaking)] > 0
    # The music's level over ~3 s (follows its sections, not its beats), measured apart while the
    # voice speaks and in the pauses: a score that already dips under the voice is seen dipped.
    mus = frame_db(music, rate, frame)
    m = min(len(mus), len(active))
    on = active[:m].astype(np.float64)
    p = 10 ** (mus[:m] / 10)
    box = np.ones(max(1, int(round(3.0 / frame))))

    def level_where(mask):
        num = np.convolve(p * mask, box, mode="same")
        den = np.convolve(mask, box, mode="same")
        return 10 * np.log10(np.maximum(num / np.maximum(den, 1e-9), 1e-12))

    want = np.where(active[:m], level - under_db - level_where(on), level - gap_db - level_where(1.0 - on))
    want = np.clip(np.minimum(want, 0.0), -DUCK_MAX_DB, 0.0)
    # Fast down, slow up.
    att, rel = math.exp(-3 * frame / DUCK_ATTACK), math.exp(-3 * frame / DUCK_RELEASE)
    g, out = 0.0, [0.0] * m
    for i, w in enumerate(want.tolist()):
        a = att if w < g else rel
        g = w + (g - w) * a
        out[i] = g
    return np.asarray(out), level


# Analysis rate of the duck (levels only; speech and music carry little power above 8 kHz).
DUCK_ANALYSIS_RATE = 16000


def _decode_level(args, total, rate=DUCK_ANALYSIS_RATE):
    """An input as the mix hears it (ffmpeg turns it to stereo, as the mix graph does: a mono
    voice lands 3 dB down on each side), folded to one signal whose power is the channels'
    mean power."""
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", *args, "-t", f"{total:.3f}",
           "-map", "0:a:0", "-ac", "2", "-ar", str(rate), "-f", "f32le", "-"]
    raw = subprocess.run(cmd, capture_output=True, timeout=1800, check=True).stdout
    x = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(np.float64)
    return np.sqrt(0.5 * (x[:, 0] ** 2 + x[:, 1] ** 2))


def make_duck_gain(narration_path, music_args, music_vol, narration_vol, total, out_path):
    """Writes the music's gain curve (mono WAV at DUCK_RATE, `total` s, linear 0..1) for
    ffmpeg amultiply. `music_args` are the ffmpeg input arguments of the music (with
    -stream_loop for a looped track). Returns out_path, or None when it cannot be made."""
    if np is None or narration_vol <= 0 or music_vol <= 0:
        return None
    try:
        rate = DUCK_ANALYSIS_RATE
        narration = _decode_level(["-i", narration_path], total, rate) * float(narration_vol)
        music = _decode_level(list(music_args), total, rate) * float(music_vol)
        if not len(music):
            return None
        frame = 0.005
        gdb, _ = duck_gain(narration, music, rate, frame=frame)
        if gdb is None:
            return None
        n = int(math.ceil(total * DUCK_RATE)) + DUCK_RATE
        t = np.arange(n) / DUCK_RATE
        g = 10 ** (np.interp(t, (np.arange(len(gdb)) + 0.5) * frame, gdb, right=0.0) / 20.0)
        write_wav(out_path, np.minimum(g, 32767 / 32768), DUCK_RATE)
        return out_path
    except Exception as e:
        print(f"[sfx] Music duck unavailable ({e}); using the compressor", file=sys.stderr)
        return None
