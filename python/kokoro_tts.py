#!/usr/bin/env python3
"""
Local Hindi/English TTS — open-source, no API, Apple Silicon friendly.

Engines (best → fallback):
  1. XTTS v2 — ultra-natural Hindi + English (ElevenLabs-class, ~2 GB)
  2. Kokoro-82M — 4 Hindi + 10+ English neural voices (~310 MB)
  3. Piper — Hindi (3) + English (4) voices (Python 3.9+)

Models download once to ~/.mangavox/models/

Usage:
  python kokoro_tts.py status
  python kokoro_tts.py install
  python kokoro_tts.py synthesize <args.json>
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import urllib.request
import wave
import struct

BASE_DIR = os.path.join(os.path.expanduser("~"), ".mangavox")
MODEL_DIR = os.environ.get("MANGAVOX_KOKORO_DIR", os.path.join(BASE_DIR, "models", "kokoro"))
PIPER_HI_DIR = os.path.join(BASE_DIR, "models", "piper", "hi_IN")
PIPER_EN_DIR = os.path.join(BASE_DIR, "models", "piper", "en_US")
# Back-compat alias
PIPER_DIR = PIPER_HI_DIR
VENV_DIR = os.path.join(BASE_DIR, "tts-venv")
# Windows venvs keep the interpreter under Scripts/, everything else under bin/.
VENV_PYTHON = os.path.join(VENV_DIR, "Scripts", "python.exe") if os.name == "nt" else os.path.join(VENV_DIR, "bin", "python3")
UV_CACHE_DIR = os.path.join(BASE_DIR, "uv-cache")
UV_PYTHON_INSTALL_DIR = os.path.join(BASE_DIR, "uv-python")

KOKORO_FILES = [
    ("kokoro-v1.0.onnx", "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx"),
    ("voices-v1.0.bin", "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin"),
]

PIPER_VOICES = {
    "pratham": [
        ("hi_IN-pratham-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/pratham/medium/hi_IN-pratham-medium.onnx"),
        ("hi_IN-pratham-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/pratham/medium/hi_IN-pratham-medium.onnx.json"),
    ],
    "priyamvada": [
        ("hi_IN-priyamvada-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/priyamvada/medium/hi_IN-priyamvada-medium.onnx"),
        ("hi_IN-priyamvada-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/hi/hi_IN/priyamvada/medium/hi_IN-priyamvada-medium.onnx.json"),
    ],
    "rohan": [
        ("hi_IN-rohan-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx"),
        ("hi_IN-rohan-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/main/hi/hi_IN/rohan/medium/hi_IN-rohan-medium.onnx.json"),
    ],
}

PIPER_EN_VOICES = {
    "lessac": [
        ("en_US-lessac-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx"),
        ("en_US-lessac-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"),
    ],
    "amy": [
        ("en_US-amy-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/amy/medium/en_US-amy-medium.onnx"),
        ("en_US-amy-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/amy/medium/en_US-amy-medium.onnx.json"),
    ],
    "kristin": [
        ("en_US-kristin-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/kristin/medium/en_US-kristin-medium.onnx"),
        ("en_US-kristin-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/kristin/medium/en_US-kristin-medium.onnx.json"),
    ],
    "ryan": [
        ("en_US-ryan-medium.onnx", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ryan/medium/en_US-ryan-medium.onnx"),
        ("en_US-ryan-medium.onnx.json", "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/ryan/medium/en_US-ryan-medium.onnx.json"),
    ],
}

VOICE_MAP = {
    # Kokoro Hindi — natural neural (4 voices)
    "hf_alpha": ("hf_alpha", "hi", "kokoro"),
    "hf_beta": ("hf_beta", "hi", "kokoro"),
    "hm_omega": ("hm_omega", "hi", "kokoro"),
    "hm_psi": ("hm_psi", "hi", "kokoro"),
    # XTTS ultra-natural (Hindi + English)
    "xtts_hi_female": ("xtts_hi_female", "hi", "xtts"),
    "xtts_hi_male": ("xtts_hi_male", "hi", "xtts"),
    "xtts_hi_storyteller": ("xtts_hi_storyteller", "hi", "xtts"),
    "xtts_hi_narrator": ("xtts_hi_narrator", "hi", "xtts"),
    "xtts_en_female": ("xtts_en_female", "en", "xtts"),
    "xtts_en_male": ("xtts_en_male", "en", "xtts"),
    "xtts_en_narrator": ("xtts_en_narrator", "en", "xtts"),
    "xtts_en_storyteller": ("xtts_en_storyteller", "en", "xtts"),
    "xtts_en_warm": ("xtts_en_warm", "en", "xtts"),
    "xtts_en_clear": ("xtts_en_clear", "en", "xtts"),
    # F5-TTS (State-of-the-art, most natural, recommended)
    "f5_hi_female": ("f5_hi_female", "hi", "f5-tts"),
    "f5_hi_male": ("f5_hi_male", "hi", "f5-tts"),
    "f5_hi_storyteller": ("f5_hi_storyteller", "hi", "f5-tts"),
    "f5_hi_narrator": ("f5_hi_narrator", "hi", "f5-tts"),
    "f5_en_female": ("f5_en_female", "en", "f5-tts"),
    "f5_en_male": ("f5_en_male", "en", "f5-tts"),
    "f5_en_narrator": ("f5_en_narrator", "en", "f5-tts"),
    "f5_en_storyteller": ("f5_en_storyteller", "en", "f5-tts"),
    # Parler-TTS (Fast, controllable, high quality)
    "parler_hi_female_clear": ("parler_hi_female_clear", "hi", "parler-tts"),
    "parler_hi_female_warm": ("parler_hi_female_warm", "hi", "parler-tts"),
    "parler_hi_male_deep": ("parler_hi_male_deep", "hi", "parler-tts"),
    "parler_hi_male_bright": ("parler_hi_male_bright", "hi", "parler-tts"),
    "parler_en_female_neutral": ("parler_en_female_neutral", "en", "parler-tts"),
    "parler_en_female_warm": ("parler_en_female_warm", "en", "parler-tts"),
    "parler_en_male_narrator": ("parler_en_male_narrator", "en", "parler-tts"),
    "parler_en_male_casual": ("parler_en_male_casual", "en", "parler-tts"),
    # Piper Hindi
    "hi_piper_f": ("hi_piper_f", "hi", "piper"),
    "hi_piper_m": ("hi_piper_m", "hi", "piper"),
    "hi_piper_rohan": ("hi_piper_rohan", "hi", "piper"),
    "hi-lekha": ("hf_alpha", "hi", "kokoro"),
    "hi-female": ("hf_alpha", "hi", "kokoro"),
    "hi-male": ("hm_omega", "hi", "kokoro"),
    "hi-male-aman": ("hm_omega", "hi", "kokoro"),
    "hi-male-deep": ("hm_omega", "hi", "kokoro"),
    # Kokoro English US
    "af_heart": ("af_heart", "en-us", "kokoro"),
    "af_bella": ("af_bella", "en-us", "kokoro"),
    "af_nova": ("af_nova", "en-us", "kokoro"),
    "af_sky": ("af_sky", "en-us", "kokoro"),
    "af_nicole": ("af_nicole", "en-us", "kokoro"),
    "af_jessica": ("af_jessica", "en-us", "kokoro"),
    "am_michael": ("am_michael", "en-us", "kokoro"),
    "am_adam": ("am_adam", "en-us", "kokoro"),
    "am_echo": ("am_echo", "en-us", "kokoro"),
    "am_eric": ("am_eric", "en-us", "kokoro"),
    "am_liam": ("am_liam", "en-us", "kokoro"),
    # Kokoro English UK
    "bf_emma": ("bf_emma", "en-gb", "kokoro"),
    "bf_isabella": ("bf_isabella", "en-gb", "kokoro"),
    "bm_george": ("bm_george", "en-gb", "kokoro"),
    # Piper English
    "en_piper_f": ("en_piper_f", "en", "piper"),
    "en_piper_m": ("en_piper_m", "en", "piper"),
    "en_piper_f2": ("en_piper_f2", "en", "piper"),
    "en_piper_m2": ("en_piper_m2", "en", "piper"),
    "say-default": ("af_heart", "en-us", "kokoro"),
}

PIPER_VOICE_MAP = {
    "hf_alpha": ("hi", "priyamvada"),
    "hf_beta": ("hi", "priyamvada"),
    "hm_omega": ("hi", "pratham"),
    "hm_psi": ("hi", "rohan"),
    "hi_piper_f": ("hi", "priyamvada"),
    "hi_piper_m": ("hi", "pratham"),
    "hi_piper_rohan": ("hi", "rohan"),
    "hi-lekha": ("hi", "priyamvada"),
    "hi-female": ("hi", "priyamvada"),
    "hi-male": ("hi", "pratham"),
    "hi-male-aman": ("hi", "pratham"),
    "hi-male-deep": ("hi", "pratham"),
    "af_heart": ("en", "amy"),
    "af_bella": ("en", "amy"),
    "af_nova": ("en", "amy"),
    "af_sky": ("en", "amy"),
    "af_nicole": ("en", "kristin"),
    "af_jessica": ("en", "kristin"),
    "am_michael": ("en", "lessac"),
    "am_adam": ("en", "lessac"),
    "am_echo": ("en", "ryan"),
    "am_eric": ("en", "ryan"),
    "am_liam": ("en", "ryan"),
    "bf_emma": ("en", "kristin"),
    "bf_isabella": ("en", "kristin"),
    "bm_george": ("en", "ryan"),
    "en_piper_f": ("en", "amy"),
    "en_piper_m": ("en", "lessac"),
    "en_piper_f2": ("en", "kristin"),
    "en_piper_m2": ("en", "ryan"),
    "say-default": ("en", "amy"),
}


def emit(obj: dict) -> None:
    print(json.dumps(obj), flush=True)


def _python_candidates() -> list[str]:
    out: list[str] = []
    if os.path.isfile(VENV_PYTHON):
        out.append(VENV_PYTHON)
    for cand in [
        "/opt/homebrew/opt/python@3.12/bin/python3.12",
        "/opt/homebrew/bin/python3.12",
        "/usr/local/opt/python@3.12/bin/python3.12",
        shutil.which("python3.12"),
        shutil.which("python3.11"),
        shutil.which("python3.10"),
        "/opt/homebrew/bin/python3.11",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3.12",
        *_windows_launcher_pythons(),
        sys.executable,
        shutil.which("python3"),
    ]:
        if cand and cand not in out and os.path.isfile(cand):
            out.append(cand)
    return out


_LAUNCHER_PYTHONS: list[str] | None = None


def _windows_launcher_pythons() -> list[str]:
    """Interpreters the Windows `py` launcher knows, best versions for the TTS wheels first."""
    global _LAUNCHER_PYTHONS
    if _LAUNCHER_PYTHONS is not None:
        return _LAUNCHER_PYTHONS
    found: list[str] = []
    py = shutil.which("py") if os.name == "nt" else None
    if py:
        for ver in ("3.12", "3.11", "3.13", "3.10"):
            try:
                r = subprocess.run([py, f"-{ver}", "-c", "import sys; print(sys.executable)"],
                                   capture_output=True, text=True, timeout=10,
                                   creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                exe = r.stdout.strip()
                if r.returncode == 0 and exe and exe not in found:
                    found.append(exe)
            except Exception:
                pass
    _LAUNCHER_PYTHONS = found
    return found


def _python_version_ok(path: str, min_minor: int = 10) -> bool:
    try:
        out = subprocess.run([path, "-c", "import sys; print(sys.version_info[:2])"],
                             capture_output=True, text=True, timeout=10)
        if out.returncode != 0:
            return False
        major, minor = eval(out.stdout.strip())
        return major == 3 and minor >= min_minor
    except Exception:
        return False


def resolve_python(prefer_venv: bool = True) -> str:
    if prefer_venv and os.path.isfile(VENV_PYTHON) and _python_version_ok(VENV_PYTHON, 10):
        return VENV_PYTHON
    for cand in _python_candidates():
        if cand == VENV_PYTHON and not _python_version_ok(cand, 10):
            continue
        if _python_version_ok(cand, 10):
            return cand
    return sys.executable


def kokoro_model_paths() -> tuple[str, str]:
    """Full-precision model when downloaded, else the int8 "Lite" pack from Settings → Audio."""
    full = os.path.join(MODEL_DIR, "kokoro-v1.0.onnx")
    lite = os.path.join(MODEL_DIR, "kokoro-v1.0.int8.onnx")
    onnx = lite if not os.path.isfile(full) and os.path.isfile(lite) else full
    return (onnx, os.path.join(MODEL_DIR, "voices-v1.0.bin"))


def _thread_cap() -> int | None:
    """Worker threads allowed by MangaVox's system resource limit (Settings → System)."""
    try:
        n = int(os.environ.get("MANGAVOX_THREADS", "0"))
        return n if n > 0 else None
    except ValueError:
        return None


def load_kokoro():
    """Kokoro with ONNX Runtime capped to the allowed thread count."""
    from kokoro_onnx import Kokoro

    onnx_path, voices_path = kokoro_model_paths()
    cap = _thread_cap()
    if cap:
        try:
            import onnxruntime as ort

            opts = ort.SessionOptions()
            opts.intra_op_num_threads = cap
            opts.inter_op_num_threads = 1
            session = ort.InferenceSession(onnx_path, sess_options=opts, providers=["CPUExecutionProvider"])
            return Kokoro.from_session(session, voices_path)
        except Exception:
            pass
    return Kokoro(onnx_path, voices_path)


def kokoro_models_ready() -> bool:
    onnx, voices = kokoro_model_paths()
    return os.path.isfile(onnx) and os.path.getsize(onnx) > 1_000_000 and os.path.isfile(voices)


def _piper_set_ready(voice_set: dict, dest_dir: str) -> bool:
    for files in voice_set.values():
        for name, _ in files:
            if not os.path.isfile(os.path.join(dest_dir, name)):
                return False
    return True


def piper_models_ready() -> bool:
    return piper_hi_ready() or piper_en_ready()


def piper_hi_ready() -> bool:
    return _piper_set_ready(PIPER_VOICES, PIPER_HI_DIR)


def piper_en_ready() -> bool:
    return _piper_set_ready(PIPER_EN_VOICES, PIPER_EN_DIR)


def _is_english_voice(voice_id: str) -> bool:
    v = (voice_id or "").lower()
    if v.startswith("xtts_en_") or v.startswith("en_piper"):
        return True
    return v.startswith(("af_", "am_", "bf_", "bm_")) or v in ("say-default",)


def _is_xtts_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("xtts_")


# Piper stand-in when XTTS v2 is not installed (~2 GB). Preview + generation still work.
XTTS_FALLBACK_VOICE: dict[str, str] = {
    "xtts_hi_female": "hi_piper_f",
    "xtts_hi_male": "hi_piper_m",
    "xtts_hi_storyteller": "hi_piper_f",
    "xtts_hi_narrator": "hi_piper_rohan",
    "xtts_en_female": "en_piper_f",
    "xtts_en_male": "en_piper_m",
    "xtts_en_narrator": "en_piper_f2",
    "xtts_en_storyteller": "en_piper_m",
    "xtts_en_warm": "en_piper_f2",
    "xtts_en_clear": "en_piper_m2",
}

STUDIO_FALLBACK_VOICE: dict[str, str] = {
    "f5_hi_female": "hi_piper_f",
    "f5_hi_male": "hi_piper_m",
    "f5_hi_storyteller": "hi_piper_rohan",
    "f5_hi_narrator": "hi_piper_m",
    "parler_hi_female_clear": "hi_piper_f",
    "parler_hi_female_warm": "hf_alpha",
    "parler_hi_male_deep": "hi_piper_m",
    "parler_hi_male_bright": "hi_piper_rohan",
    "f5_en_female": "en_piper_f",
    "f5_en_male": "en_piper_m",
    "f5_en_narrator": "en_piper_f2",
    "f5_en_storyteller": "en_piper_m2",
    "parler_en_female_neutral": "en_piper_f",
    "parler_en_female_warm": "en_piper_f2",
    "parler_en_male_narrator": "en_piper_m",
    "parler_en_male_casual": "en_piper_m2",
}



def _is_f5_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("f5_")


def _is_parler_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("parler_")


def xtts_fallback_voice(voice: str) -> str | None:
    return XTTS_FALLBACK_VOICE.get(voice or "")


def studio_fallback_voice(voice: str) -> str | None:
    return STUDIO_FALLBACK_VOICE.get(voice or "")


def _is_hindi_voice_id(voice_id: str) -> bool:
    v = voice_id or ""
    if v.startswith("xtts_hi_") or v.startswith("hi_piper") or v.startswith(("hf_", "hm_")):
        return True
    return v in ("hi-lekha", "hi-female", "hi-male", "hi-male-aman", "hi-male-deep")


def _voice_engine_hint(voice_id: str) -> str | None:
    if voice_id in VOICE_MAP:
        return VOICE_MAP[voice_id][2]
    if _is_xtts_voice(voice_id):
        return "xtts"
    if (voice_id or "").startswith(("hf_", "hm_")):
        return "kokoro"
    if (voice_id or "").startswith("hi_piper_"):
        return "piper"
    return None


def piper_voice_ready(voice: str) -> bool:
    if _is_xtts_voice(voice):
        return False
    lang, key = PIPER_VOICE_MAP.get(voice, ("hi", "pratham"))
    if lang == "en":
        if key not in PIPER_EN_VOICES:
            return piper_en_ready()
        return _piper_set_ready({key: PIPER_EN_VOICES[key]}, PIPER_EN_DIR)
    if key not in PIPER_VOICES:
        return piper_hi_ready()
    return _piper_set_ready({key: PIPER_VOICES[key]}, PIPER_HI_DIR)


def ensure_piper_models_for_voice(voice: str) -> bool:
    """Download Piper ONNX files for this voice if missing."""
    lang, key = PIPER_VOICE_MAP.get(voice, ("hi", "pratham"))
    if lang == "en":
        voice_set = {key: PIPER_EN_VOICES[key]} if key in PIPER_EN_VOICES else PIPER_EN_VOICES
        dest = PIPER_EN_DIR
    else:
        voice_set = {key: PIPER_VOICES[key]} if key in PIPER_VOICES else PIPER_VOICES
        dest = PIPER_HI_DIR

    if _piper_set_ready(voice_set, dest):
        return True

    label = "English" if lang == "en" else "Hindi"
    emit({"type": "info", "message": f"Downloading Piper {label} voice model…"})
    for _v, files in voice_set.items():
        for name, url in files:
            path = os.path.join(dest, name)
            if not os.path.isfile(path):
                _download_file(dest, name, url)
    return _piper_set_ready(voice_set, dest)


def resolve_engine_for_voice(
    voice: str,
    *,
    allow_download: bool = False,
    allow_fallback: bool = False,
) -> str | None:
    py = resolve_python()
    hint = _voice_engine_hint(voice)
    
    # Priority 1: F5-TTS (state-of-the-art, most natural)
    if hint == "f5-tts" or _is_f5_voice(voice):
        if f5_ready(py) and f5_deps_ready(py):
            return "f5-tts"
        if allow_fallback:
            fb = studio_fallback_voice(voice)
            if fb:
                return resolve_engine_for_voice(
                    fb, allow_download=allow_download, allow_fallback=False,
                )
    
    # Priority 2: Parler-TTS (fast, controllable, high quality)
    if hint == "parler-tts" or _is_parler_voice(voice):
        if parler_ready(py) and parler_deps_ready(py):
            return "parler-tts"
        if allow_fallback:
            fb = studio_fallback_voice(voice)
            if fb:
                return resolve_engine_for_voice(
                    fb, allow_download=allow_download, allow_fallback=False,
                )

    # Priority 3: XTTS (ElevenLabs-quality, voice cloning)
    if hint == "xtts" or _is_xtts_voice(voice):
        try:
            from xtts_tts import xtts_ready, xtts_voice_ref_ready
            if xtts_ready(py) and xtts_voice_ref_ready(voice):
                return "xtts"
        except ImportError:
            pass
        if allow_fallback:
            fb = xtts_fallback_voice(voice)
            if fb:
                return resolve_engine_for_voice(
                    fb, allow_download=allow_download, allow_fallback=False,
                )
        return None

    # Priority 4: Piper (standard, reliable)
    if hint == "piper":
        if piper_voice_ready(voice) and piper_deps_ready():
            return "piper"
        if allow_download and piper_deps_ready() and ensure_piper_models_for_voice(voice):
            return "piper"
        return None

    # Priority 5: Kokoro (small, efficient)
    if hint != "piper" and kokoro_models_ready() and kokoro_deps_ready(py):
        if hint == "kokoro" or (voice or "").startswith(("hf_", "hm_")) or _is_english_voice(voice):
            return "kokoro"

    # Fallback chain
    if piper_voice_ready(voice) and piper_deps_ready():
        return "piper"
    if allow_download and piper_deps_ready() and ensure_piper_models_for_voice(voice):
        return "piper"
    return None



def voice_ready(voice: str) -> dict:
    f5_ready_flag = False
    parler_ready_flag = False
    xtts_ready_flag = False
    try:
        f5_ready_flag = f5_ready(resolve_python()) and f5_deps_ready(resolve_python())
    except Exception:
        pass
    try:
        parler_ready_flag = parler_ready(resolve_python()) and parler_deps_ready(resolve_python())
    except Exception:
        pass
    try:
        from xtts_tts import xtts_ready
        xtts_ready_flag = xtts_ready(resolve_python())
    except ImportError:
        pass

    engine = resolve_engine_for_voice(voice, allow_download=False)
    preview_fallback = False
    preview_voice = voice
    if engine is None and _is_xtts_voice(voice):
        fb = xtts_fallback_voice(voice)
        if fb:
            fb_engine = resolve_engine_for_voice(fb, allow_download=False)
            if fb_engine:
                engine = fb_engine
                preview_fallback = True
                preview_voice = fb
    if engine is None and (_is_f5_voice(voice) or _is_parler_voice(voice)):
        fb = studio_fallback_voice(voice)
        if fb:
            fb_engine = resolve_engine_for_voice(fb, allow_download=False)
            if fb_engine:
                engine = fb_engine
                preview_fallback = True
                preview_voice = fb

    return {
        "voice": voice,
        "ready": engine is not None,
        "engine": engine or "none",
        "previewFallback": preview_fallback,
        "previewVoice": preview_voice,
        "f5Ready": f5_ready_flag,
        "parlerReady": parler_ready_flag,
        "xttsInstalled": xtts_ready_flag,
        "xttsReady": xtts_ready_flag,
        "kokoroReady": kokoro_models_ready() and kokoro_deps_ready(resolve_python()),
        "piperHiReady": piper_hi_ready() and piper_deps_ready(),
        "piperEnReady": piper_en_ready() and piper_deps_ready(),
    }


def kokoro_deps_ready(python: str | None = None) -> bool:
    py = python or resolve_python()
    try:
        r = subprocess.run([py, "-c", "import kokoro_onnx, soundfile"], capture_output=True, timeout=20)
        return r.returncode == 0
    except Exception:
        return False


def piper_deps_ready(python: str | None = None) -> bool:
    py = python or resolve_python()
    try:
        r = subprocess.run([py, "-c", "from piper import PiperVoice"], capture_output=True, timeout=20)
        return r.returncode == 0
    except Exception:
        return False



def f5_deps_ready(python: str | None = None) -> bool:
    """Check if F5-TTS dependencies are installed."""
    py = python or sys.executable
    try:
        r = subprocess.run(
            [py, "-c", "from f5_tts.api import F5TTS; import torch"],
            capture_output=True, timeout=20, cwd=BASE_DIR,
        )
        return r.returncode == 0
    except Exception:
        return False


def f5_ready(python: str | None = None) -> bool:
    """Check if F5-TTS model is ready."""
    py = python or sys.executable
    try:
        from f5_tts import f5_ready as f5_model_ready
        return f5_model_ready(py)
    except Exception:
        return False


def parler_deps_ready(python: str | None = None) -> bool:
    """Check if Parler-TTS dependencies are installed."""
    py = python or sys.executable
    try:
        r = subprocess.run(
            [py, "-c", "from parler_tts import ParlerTTSForConditionalGeneration; import torch"],
            capture_output=True, timeout=20, cwd=BASE_DIR,
        )
        return r.returncode == 0
    except Exception:
        return False


def parler_ready(python: str | None = None) -> bool:
    """Check if Parler-TTS model is ready."""
    py = python or sys.executable
    try:
        from parler_tts import parler_ready as parler_model_ready
        return parler_model_ready(py)
    except Exception:
        return False

def active_engine() -> str | None:
    py = resolve_python()
    # Priority: F5-TTS > Parler-TTS > XTTS > Kokoro > Piper
    if f5_ready(py) and f5_deps_ready(py):
        return "f5-tts"
    if parler_ready(py) and parler_deps_ready(py):
        return "parler-tts"
    if kokoro_models_ready() and kokoro_deps_ready(py):
        return "kokoro"
    try:
        from xtts_tts import xtts_ready
        if xtts_ready(py):
            return "xtts"
    except ImportError:
        pass
    if piper_models_ready() and piper_deps_ready():
        return "piper"
    return None


def status() -> dict:
    engine = active_engine()
    py = resolve_python()
    active_py_version = sys.version.split()[0]
    try:
        r = subprocess.run([py, "-c", "import sys; print(sys.version.split()[0])"], capture_output=True, text=True, timeout=10)
        if r.returncode == 0 and r.stdout.strip():
            active_py_version = r.stdout.strip()
    except Exception:
        pass
    total_mb = 0.0
    extra_dirs = []
    try:
        from xtts_tts import XTTS_DIR as _XTTS_DIR
        extra_dirs.append(_XTTS_DIR)
    except Exception:
        _XTTS_DIR = os.path.join(BASE_DIR, "models", "xtts")
    try:
        from f5_tts import F5_DIR as _F5_DIR
        extra_dirs.append(_F5_DIR)
    except Exception:
        _F5_DIR = os.path.join(BASE_DIR, "models", "f5-tts")
    try:
        from parler_tts import PARLER_DIR as _PARLER_DIR
        extra_dirs.append(_PARLER_DIR)
    except Exception:
        _PARLER_DIR = os.path.join(BASE_DIR, "models", "indic-parler-tts")

    for root in (MODEL_DIR, PIPER_HI_DIR, PIPER_EN_DIR, *extra_dirs):
        if os.path.isdir(root):
            for dirpath, _, files in os.walk(root):
                for f in files:
                    try:
                        total_mb += os.path.getsize(os.path.join(dirpath, f)) / (1024 * 1024)
                    except OSError:
                        pass
    
    f5_ready_flag = f5_ready(py) and f5_deps_ready(py)
    parler_ready_flag = parler_ready(py) and parler_deps_ready(py)
    xtts_ready_flag = False
    try:
        from xtts_tts import xtts_ready
        xtts_ready_flag = xtts_ready(py)
    except ImportError:
        pass
    kokoro_deps_flag = kokoro_deps_ready(py)
    piper_deps_flag = piper_deps_ready(py)

    st = {
        "installed": engine is not None or kokoro_models_ready() or piper_models_ready() or f5_ready_flag or parler_ready_flag,
        "kokoroDepsReady": kokoro_deps_flag,
        "piperDepsReady": piper_deps_flag,
        "engine": engine or "none",
        "f5Ready": f5_ready_flag,
        "parlerReady": parler_ready_flag,
        "kokoroReady": kokoro_models_ready(),
        "piperReady": piper_models_ready(),
        "piperHiReady": piper_hi_ready(),
        "piperEnReady": piper_en_ready(),
        "xttsReady": xtts_ready_flag,
        "depsReady": kokoro_deps_flag or piper_deps_flag or f5_ready_flag or parler_ready_flag,
        "modelsReady": kokoro_models_ready() or piper_models_ready() or f5_ready_flag or parler_ready_flag,
        "modelDir": MODEL_DIR,
        "piperDir": PIPER_DIR,
        "piperHiDir": PIPER_HI_DIR,
        "piperEnDir": PIPER_EN_DIR,
        "xttsDir": _XTTS_DIR,
        "f5Dir": _F5_DIR,
        "parlerDir": _PARLER_DIR,
        "baseDir": BASE_DIR,
        "activePython": py,
        "venvPython": VENV_PYTHON if os.path.isfile(VENV_PYTHON) else None,
        "sizeMb": round(total_mb, 1),
        "platform": sys.platform,
        "hindiVoices": [
            # F5-TTS (Recommended)
            "f5_hi_female", "f5_hi_male", "f5_hi_storyteller", "f5_hi_narrator",
            # Parler-TTS
            "parler_hi_female_clear", "parler_hi_female_warm", "parler_hi_male_deep", "parler_hi_male_bright",
            # XTTS
            "xtts_hi_female", "xtts_hi_male", "xtts_hi_storyteller", "xtts_hi_narrator",
            # Kokoro
            "hf_alpha", "hf_beta", "hm_omega", "hm_psi",
            # Piper
            "hi_piper_f", "hi_piper_m", "hi_piper_rohan",
        ],
        "englishVoices": [
            # F5-TTS (Recommended)
            "f5_en_female", "f5_en_male", "f5_en_narrator", "f5_en_storyteller",
            # Parler-TTS
            "parler_en_female_neutral", "parler_en_female_warm", "parler_en_male_narrator", "parler_en_male_casual",
            # XTTS
            "xtts_en_female", "xtts_en_male", "xtts_en_narrator", "xtts_en_storyteller",
            "xtts_en_warm", "xtts_en_clear",
            # Kokoro
            "af_heart", "af_bella", "af_nova", "af_sky", "af_nicole", "af_jessica",
            "am_michael", "am_adam", "am_echo", "am_eric", "am_liam",
            "bf_emma", "bf_isabella", "bm_george",
            # Piper
            "en_piper_f", "en_piper_m", "en_piper_f2", "en_piper_m2",
        ],
        "engines": {
            "f5-tts": {"ready": f5_ready_flag, "description": "State-of-the-art multilingual TTS"},
            "parler-tts": {"ready": parler_ready_flag, "description": "Fast, controllable high-quality TTS"},
            "xtts": {"ready": xtts_ready_flag, "description": "ElevenLabs-class quality with voice cloning"},
            "kokoro": {"ready": kokoro_models_ready() and kokoro_deps_ready(py), "description": "Efficient 82M parameter model"},
            "piper": {"ready": piper_models_ready() and piper_deps_ready(), "description": "Standard open-source TTS"},
        },
        "python": active_py_version,
    }
    return st


def download_all_voice_assets() -> None:
    """Download every Piper + Kokoro model file. Idempotent; no Python 3.12 required."""
    if not _piper_set_ready(PIPER_VOICES, PIPER_HI_DIR):
        emit({"type": "info", "message": "Downloading Piper Hindi voices (Pratham, Priyamvada, Rohan)…"})
        for _voice, files in PIPER_VOICES.items():
            for name, url in files:
                dest = os.path.join(PIPER_HI_DIR, name)
                if not os.path.isfile(dest):
                    _download_file(PIPER_HI_DIR, name, url)
    else:
        emit({"type": "info", "message": "Piper Hindi voices already present ✓"})

    if not _piper_set_ready(PIPER_EN_VOICES, PIPER_EN_DIR):
        emit({"type": "info", "message": "Downloading Piper English voices (Amy, Lessac, Kristin, Ryan)…"})
        for _voice, files in PIPER_EN_VOICES.items():
            for name, url in files:
                dest = os.path.join(PIPER_EN_DIR, name)
                if not os.path.isfile(dest):
                    _download_file(PIPER_EN_DIR, name, url)
    else:
        emit({"type": "info", "message": "Piper English voices already present ✓"})

    if not kokoro_models_ready():
        emit({"type": "info", "message": "Downloading Kokoro-82M model files (~310 MB)…"})
        for name, url in KOKORO_FILES:
            dest = os.path.join(MODEL_DIR, name)
            if not os.path.isfile(dest):
                _download_file(MODEL_DIR, name, url)
    else:
        emit({"type": "info", "message": "Kokoro model files already present ✓"})


def _download_file(dest_dir: str, name: str, url: str) -> None:
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, name)
    tmp = dest + ".part"
    emit({"type": "progress", "stage": "download", "file": name, "percent": 0})
    req = urllib.request.Request(url, headers={"User-Agent": "MangaVox/1.0"})
    with urllib.request.urlopen(req, timeout=600) as resp:
        total = int(resp.headers.get("Content-Length") or 0)
        done = 0
        with open(tmp, "wb") as out:
            while True:
                chunk = resp.read(1024 * 256)
                if not chunk:
                    break
                out.write(chunk)
                done += len(chunk)
                if total > 0:
                    emit({"type": "progress", "stage": "download", "file": name,
                          "percent": round(done / total * 100, 1), "completed": done, "total": total})
    os.replace(tmp, dest)
    emit({"type": "progress", "stage": "download", "file": name, "percent": 100})


def _ensure_venv() -> str | None:
    """Create ~/.mangavox/tts-venv with Python 3.10+ if possible."""
    if os.path.isfile(VENV_PYTHON):
        if _python_version_ok(VENV_PYTHON, 10):
            return VENV_PYTHON
        emit({
            "type": "info",
            "message": (
                "Existing TTS virtualenv uses Python < 3.10, so Studio voice install "
                "will use a newer Python if available."
            ),
        })
    for cand in _python_candidates():
        if cand == VENV_PYTHON:
            continue
        if cand == sys.executable and not _python_version_ok(cand, 10):
            continue
        if not _python_version_ok(cand, 10):
            continue
        emit({"type": "info", "message": f"Creating TTS virtualenv with {cand}…"})
        os.makedirs(BASE_DIR, exist_ok=True)
        subprocess.run([cand, "-m", "venv", VENV_DIR], check=True, timeout=120)
        if os.path.isfile(VENV_PYTHON):
            return VENV_PYTHON
    uv = shutil.which("uv")
    if uv:
        try:
            emit({"type": "info", "message": "Creating private Python 3.12 runtime for studio voices…"})
            os.makedirs(BASE_DIR, exist_ok=True)
            env = os.environ.copy()
            env["UV_CACHE_DIR"] = UV_CACHE_DIR
            env["UV_PYTHON_INSTALL_DIR"] = UV_PYTHON_INSTALL_DIR
            subprocess.run([uv, "python", "install", "3.12"], check=True, timeout=900, env=env)
            subprocess.run([uv, "venv", "--python", "3.12", VENV_DIR], check=True, timeout=300, env=env)
            if os.path.isfile(VENV_PYTHON):
                return VENV_PYTHON
        except Exception as e:
            emit({"type": "info", "message": f"Private Python setup skipped: {str(e)[:200]}"})
    return None


def _pip_install(python: str, packages: list[str]) -> None:
    subprocess.run(
        [python, "-m", "ensurepip", "--upgrade"],
        capture_output=True, text=True, timeout=180,
    )
    
    # Try upgrade pip
    res_pip = subprocess.run(
        [python, "-m", "pip", "install", "--upgrade", "pip"],
        capture_output=True, text=True, timeout=180
    )
    if res_pip.returncode != 0 and "externally-managed-environment" in (res_pip.stderr or ""):
        subprocess.run(
            [python, "-m", "pip", "install", "--break-system-packages", "--upgrade", "pip"],
            capture_output=True, text=True, timeout=180
        )

    # Try standard package install
    res = subprocess.run(
        [python, "-m", "pip", "install", *packages],
        capture_output=True, text=True, timeout=600,
    )
    if res.returncode != 0:
        if "externally-managed-environment" in (res.stderr or ""):
            res_break = subprocess.run(
                [python, "-m", "pip", "install", "--break-system-packages", *packages],
                capture_output=True, text=True, timeout=600,
            )
            if res_break.returncode != 0:
                raise RuntimeError(f"Pip install failed with --break-system-packages: {res_break.stderr}")
        else:
            raise RuntimeError(f"Pip install failed: {res.stderr}")


def install() -> int:
    emit({"type": "info", "message": "Installing all voice packs (Piper + Kokoro + Ultra)…"})
    installed_engine = None

    try:
        download_all_voice_assets()
    except Exception as e:
        emit({"type": "error", "message": f"Model download failed: {str(e)[:400]}"})
        return 1

    # ── Try Kokoro-82M first (best Hindi quality on Apple Silicon) ──
    venv_py = _ensure_venv()
    kokoro_py = venv_py or next((p for p in _python_candidates() if _python_version_ok(p, 10)), None)

    if kokoro_py:
        emit({"type": "info", "message": "Installing Kokoro-82M (recommended for M-series Mac)…"})
        emit({"type": "progress", "stage": "pip", "percent": 5})
        try:
            if not kokoro_deps_ready(kokoro_py):
                _pip_install(kokoro_py, [
                    "kokoro-onnx>=0.4.0", "soundfile>=0.12.0", "numpy>=2.0.0",
                    "misaki[en]>=0.9.0",
                ])
            emit({"type": "progress", "stage": "pip", "percent": 100})
            if not kokoro_models_ready():
                emit({"type": "info", "message": f"Downloading Kokoro model (~310 MB)…"})
                for name, url in KOKORO_FILES:
                    if not os.path.isfile(os.path.join(MODEL_DIR, name)):
                        _download_file(MODEL_DIR, name, url)
            if kokoro_models_ready() and kokoro_deps_ready(kokoro_py):
                installed_engine = "kokoro"
        except Exception as e:
            emit({"type": "info", "message": f"Kokoro install skipped: {str(e)[:200]}"})

    # ── Piper runtime first — XTTS uses Piper to build reference clips ──
    piper_py = venv_py or kokoro_py or resolve_python()
    if piper_py:
        emit({"type": "info", "message": "Installing Piper TTS runtime in the managed voice environment…"})
        emit({"type": "progress", "stage": "pip", "percent": 5})
        try:
            if not piper_deps_ready(piper_py):
                _pip_install(piper_py, ["piper-tts>=1.3.0", "onnxruntime>=1.19.0,<2"])
            emit({"type": "progress", "stage": "pip", "percent": 100})
            if piper_models_ready() and piper_deps_ready(piper_py):
                installed_engine = installed_engine or "piper"
        except Exception as e:
            emit({"type": "error", "message": f"Piper install failed: {str(e)[:400]}"})
            return 1

    # ── F5-TTS (State-of-the-art, most natural, recommended) ──
    if venv_py or kokoro_py:
        try:
            from f5_tts import install_f5, f5_ready
            if not f5_ready(venv_py or kokoro_py):
                emit({"type": "info", "message": "Installing F5-TTS (State-of-the-art multilingual TTS, ~1.5 GB)…"})
                install_f5(venv_py or kokoro_py, progress_cb=emit)
            if f5_ready(venv_py or kokoro_py) and not installed_engine:
                installed_engine = "f5-tts"
        except Exception as e:
            emit({"type": "info", "message": f"F5-TTS install skipped: {str(e)[:200]}"})

    # ── Parler-TTS (Fast, controllable, high quality) ──
    if venv_py or kokoro_py:
        try:
            from parler_tts import install_parler, parler_ready
            if not parler_ready(venv_py or kokoro_py):
                emit({"type": "info", "message": "Installing Parler-TTS (Fast controllable TTS, ~1 GB)…"})
                install_parler(venv_py or kokoro_py, progress_cb=emit)
            if parler_ready(venv_py or kokoro_py) and not installed_engine:
                installed_engine = "parler-tts"
        except Exception as e:
            emit({"type": "info", "message": f"Parler-TTS install skipped: {str(e)[:200]}"})

    # ── XTTS v2 ultra-natural Hindi (ElevenLabs-class, uses same Python 3.12 venv) ──
    xtts_py = venv_py or kokoro_py
    if xtts_py and (piper_hi_ready() or piper_en_ready()):
        try:
            from xtts_tts import install_xtts, xtts_ready
            if not xtts_ready(xtts_py):
                emit({"type": "info", "message": "Installing Ultra Natural Hindi + English (XTTS v2, ~2 GB)…"})
                install_xtts(xtts_py, progress_cb=emit)
            if xtts_ready(xtts_py) and not installed_engine:
                installed_engine = "xtts"
        except Exception as e:
            emit({"type": "info", "message": f"XTTS install skipped: {str(e)[:200]}"})

    if kokoro_models_ready() and not kokoro_py:
        emit({
            "type": "info",
            "message": (
                "Kokoro model files downloaded. Install Python 3.12 for true Kokoro voices: "
                "brew install python@3.12 — then click Download again."
            ),
        })

    st = status()
    if st["installed"]:
        labels = {
            "f5-tts": "F5-TTS State-of-the-Art",
            "parler-tts": "Parler-TTS High Quality",
            "xtts": "XTTS Ultra Natural",
            "kokoro": "Kokoro-82M Natural",
            "piper": "Piper AI"
        }
        label = labels.get(st["engine"], "Local AI")
        emit({
            "type": "done",
            "message": f"{label} ready — natural Hindi + English voices. Fully offline.",
            **st,
        })
        return 0

    emit({
        "type": "error",
        "message": (
            "Could not install TTS. Install Python 3.12 for best quality: "
            "brew install python@3.12 — then retry Download."
        ),
    })
    return 1


def resolve_voice(voice_id: str) -> tuple[str, str, str]:
    if voice_id in VOICE_MAP:
        return VOICE_MAP[voice_id]
    if voice_id.startswith(("hf_", "hm_")):
        return voice_id, "hi", "kokoro"
    if voice_id.startswith(("af_", "am_", "bf_", "bm_")):
        lang = "en-gb" if voice_id.startswith("bf_") or voice_id.startswith("bm_") else "en-us"
        return voice_id, lang, "kokoro"
    return "hf_alpha", "hi", "kokoro"


def _write_wav_pcm16(path: str, samples, sample_rate: int) -> None:
    import numpy as np
    arr = np.asarray(samples, dtype=np.float64)
    if arr.ndim > 1:
        arr = arr.mean(axis=1)
    peak = np.max(np.abs(arr)) or 1.0
    pcm = (arr / peak * 32767).astype(np.int16)
    target_rate = 22050
    if sample_rate != target_rate:
        target_len = max(1, int(len(pcm) * target_rate / sample_rate))
        x_old = np.linspace(0.0, 1.0, len(pcm), endpoint=False)
        x_new = np.linspace(0.0, 1.0, target_len, endpoint=False)
        pcm = np.interp(x_new, x_old, pcm.astype(np.float64)).astype(np.int16)
        sample_rate = target_rate
    os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
    with wave.open(path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm.tobytes())


def synthesize_kokoro(text: str, output_path: str, voice: str, speed: float) -> None:
    kokoro_voice, lang, _ = resolve_voice(voice)
    py = resolve_python()
    if not kokoro_deps_ready(py):
        raise RuntimeError("Kokoro deps missing")

    engine = load_kokoro()
    samples, sample_rate = engine.create(text, voice=kokoro_voice, speed=float(speed or 1.0), lang=lang)
    _write_wav_pcm16(output_path, samples, sample_rate)


def synthesize_piper(text: str, output_path: str, voice: str, speed: float) -> None:
    from piper import PiperVoice

    if not ensure_piper_models_for_voice(voice):
        lang, key = PIPER_VOICE_MAP.get(voice, ("hi", "pratham"))
        prefix = "en_US" if lang == "en" else "hi_IN"
        dest = PIPER_EN_DIR if lang == "en" else PIPER_HI_DIR
        onnx = os.path.join(dest, f"{prefix}-{key}-medium.onnx")
        raise RuntimeError(f"Piper model missing: {onnx} — click Download AI Voice Models in the Audio tab")

    lang, key = PIPER_VOICE_MAP.get(voice, ("hi", "pratham"))
    if lang == "en":
        prefix, dest = "en_US", PIPER_EN_DIR
    else:
        prefix, dest = "hi_IN", PIPER_HI_DIR

    onnx = os.path.join(dest, f"{prefix}-{key}-medium.onnx")
    cfg = os.path.join(dest, f"{prefix}-{key}-medium.onnx.json")
    if not os.path.isfile(onnx):
        raise RuntimeError(f"Piper model missing: {onnx} — click Download AI Voice Models in the Audio tab")

    piper = PiperVoice.load(onnx, config_path=cfg)
    chunks = list(piper.synthesize(text))
    if not chunks:
        raise RuntimeError("Piper produced no audio")

    sr = chunks[0].sample_rate
    audio_bytes = b"".join(c.audio_int16_bytes for c in chunks)
    os.makedirs(os.path.dirname(os.path.abspath(output_path)) or ".", exist_ok=True)
    with wave.open(output_path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sr)
        wf.writeframes(audio_bytes)


def synthesize_f5(text: str, output_path: str, voice: str, speed: float) -> None:
    """Synthesize using F5-TTS."""
    py = resolve_python()
    try:
        from f5_tts import synthesize_f5 as f5_synthesize
        f5_synthesize(text, output_path, voice, speed, py)
    except ImportError:
        raise RuntimeError("F5-TTS not installed")


def synthesize_parler(text: str, output_path: str, voice: str, speed: float) -> None:
    """Synthesize using Parler-TTS."""
    py = resolve_python()
    try:
        from parler_tts import synthesize_parler as parler_synthesize
        parler_synthesize(text, output_path, voice, speed, py)
    except ImportError:
        raise RuntimeError("Parler-TTS not installed")


def synthesize_to_file(text: str, output_path: str, voice: str = "hf_alpha", speed: float = 1.0) -> None:
    engine = resolve_engine_for_voice(voice, allow_download=True, allow_fallback=True)
    synth_voice = voice
    if _is_xtts_voice(voice) and engine != "xtts":
        synth_voice = xtts_fallback_voice(voice) or voice
    if (_is_f5_voice(voice) and engine != "f5-tts") or (_is_parler_voice(voice) and engine != "parler-tts"):
        synth_voice = studio_fallback_voice(voice) or voice
    py = resolve_python()
    
    if engine == "f5-tts":
        synthesize_f5(text, output_path, voice, speed)
    elif engine == "parler-tts":
        synthesize_parler(text, output_path, voice, speed)
    elif engine == "xtts":
        from xtts_tts import synthesize_xtts
        synthesize_xtts(text, output_path, voice, speed, py)
    elif engine == "kokoro":
        synthesize_kokoro(text, output_path, synth_voice, speed)
    elif engine == "piper":
        synthesize_piper(text, output_path, synth_voice, speed)
    else:
        lang = "English" if _is_english_voice(voice) else "Hindi"
        raise RuntimeError(
            f"No {lang} AI voice model installed — open Audio tab and click Download AI Voice Models"
        )


_loaded_kokoro_model = None

def get_kokoro_model():
    global _loaded_kokoro_model
    if _loaded_kokoro_model is None:
        _loaded_kokoro_model = load_kokoro()
    return _loaded_kokoro_model

from http.server import HTTPServer, BaseHTTPRequestHandler

class TTSServerHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        # Suppress server request log pollution on stdout
        pass

    def do_GET(self):
        if self.path == "/status":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"status": "running", "model": "kokoro"}).encode("utf-8"))
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == "/synthesize":
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length)
            try:
                args = json.loads(body.decode("utf-8"))
                text = args.get("text", "").strip()
                output_path = args.get("output_path")
                voice = args.get("voice", "hf_alpha")
                speed = float(args.get("speed", 1.0))
                
                if not text or not output_path:
                    self.send_response(400)
                    self.end_headers()
                    self.wfile.write(b"Missing text or output_path")
                    return
                
                kokoro_voice, lang, _ = resolve_voice(voice)
                engine = get_kokoro_model()
                samples, sample_rate = engine.create(text, voice=kokoro_voice, speed=speed, lang=lang)
                _write_wav_pcm16(output_path, samples, sample_rate)
                
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"ok": True, "path": output_path}).encode("utf-8"))
            except Exception as e:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(str(e).encode("utf-8"))
        else:
            self.send_response(404)
            self.end_headers()

def run_server(voice_id: str = "hf_alpha"):
    try:
        emit({"type": "info", "message": f"Preloading Kokoro model into memory for voice {voice_id}..."})
        get_kokoro_model()
        emit({"type": "info", "message": "Kokoro model loaded successfully!"})
    except Exception as e:
        emit({"type": "error", "message": f"Failed to preload model: {e}"})
        sys.exit(1)
        
    server = HTTPServer(("127.0.0.1", 5566), TTSServerHandler)
    emit({"type": "info", "message": "TTS HTTP Daemon Server running on http://127.0.0.1:5566"})
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


def cmd_synthesize(args_path: str) -> int:
    with open(args_path, "r", encoding="utf-8") as f:
        args = json.load(f)
    text = (args.get("text") or "").strip()
    if not text:
        emit({"type": "error", "message": "Empty text"})
        return 1
    try:
        synthesize_to_file(text, args["output_path"], args.get("voice", "hf_alpha"), args.get("speed", 1.0))
        emit({"type": "done", "path": args["output_path"], "engine": active_engine()})
        return 0
    except Exception as e:
        emit({"type": "error", "message": str(e)})
        return 1


ENGINE_PACKAGES = {
    "kokoro": ["kokoro-onnx>=0.4.0", "soundfile>=0.12.0", "numpy>=1.26"],
    "piper": ["piper-tts>=1.3.0", "onnxruntime>=1.19.0,<2"],
}


def install_engine(name: str) -> int:
    """Install one engine's Python packages into the managed voice environment.

    Model files are downloaded separately (Settings → Audio), so this only runs pip.
    Emits JSON lines: info / progress / done / error.
    """
    packages = ENGINE_PACKAGES.get(name)
    if not packages:
        emit({"type": "error", "message": f"Unknown engine: {name}"})
        return 2
    check = kokoro_deps_ready if name == "kokoro" else piper_deps_ready
    emit({"type": "progress", "stage": "venv", "current": 1, "total": 3, "message": "Preparing the voice environment…"})
    try:
        py = _ensure_venv()
    except Exception as e:
        emit({"type": "error", "message": f"Could not create the voice environment: {str(e)[:300]}"})
        return 1
    if not py:
        emit({"type": "error", "message": "Python 3.10 or newer is needed for voices. Install it (winget install -e --id Python.Python.3.12) and try again."})
        return 1
    if check(py):
        emit({"type": "done", "message": f"{name} is already installed", "python": py})
        return 0
    emit({"type": "progress", "stage": "pip", "current": 2, "total": 3, "message": f"Installing {', '.join(packages)}…"})
    try:
        _pip_install(py, packages)
    except Exception as e:
        emit({"type": "error", "message": str(e)[-600:]})
        return 1
    emit({"type": "progress", "stage": "verify", "current": 3, "total": 3, "message": "Checking the install…"})
    if not check(py):
        emit({"type": "error", "message": f"{name} installed but cannot be imported with {py}"})
        return 1
    emit({"type": "done", "message": f"{name} installed", "python": py})
    return 0


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    cmd = sys.argv[1].lower()
    if cmd == "status":
        emit(status())
        return 0
    if cmd in ("install", "install-all"):
        return install()
    if cmd == "install-engine":
        return install_engine(sys.argv[2] if len(sys.argv) > 2 else "kokoro")
    if cmd == "serve":
        voice_id = sys.argv[2] if len(sys.argv) > 2 else "hf_alpha"
        run_server(voice_id)
        return 0
    if cmd == "voice-ready":
        voice_id = sys.argv[2] if len(sys.argv) > 2 else "hf_alpha"
        emit(voice_ready(voice_id))
        return 0
    if cmd == "synthesize":
        if len(sys.argv) < 3:
            emit({"type": "error", "message": "Usage: kokoro_tts.py synthesize <args.json>"})
            return 2
        return cmd_synthesize(sys.argv[2])
    emit({"type": "error", "message": f"Unknown command: {cmd}"})
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
