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
  python kokoro_tts.py voices-ready          readiness of every catalog voice, one process
  python kokoro_tts.py voice-ready <voice>
  python kokoro_tts.py install-engine kokoro|piper|chatterbox
  python kokoro_tts.py install
  python kokoro_tts.py synthesize <args.json>

Model files are downloaded by the app (Settings → Audio). Synthesis never downloads.
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

# The app passes its data folder (Settings → Storage can move it).
BASE_DIR = os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")
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
# A uv the app placed in its data folder (preferred over one on PATH).
UV_BIN = os.path.join(BASE_DIR, "bin", "uv.exe" if os.name == "nt" else "uv")
# Markers the studio-grade engine installers write once their model is in place.
F5_MARKER = os.path.join(BASE_DIR, "models", "f5-tts", ".model_ready")
PARLER_MARKER = os.path.join(BASE_DIR, "models", "indic-parler-tts", ".model_ready")
XTTS_MARKER = os.path.join(BASE_DIR, "models", "xtts", ".model_ready")
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)

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
    # Chatterbox (Resemble AI): expressive narrators in their own engine (chatterbox_tts.py)
    "cb_narrator_f": ("cb_narrator_f", "en", "chatterbox"),
    "cb_narrator_m": ("cb_narrator_m", "en", "chatterbox"),
    "cb_storyteller_m": ("cb_storyteller_m", "en", "chatterbox"),
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
        *_uv_pythons(),
        sys.executable,
        shutil.which("python3"),
    ]:
        if cand and cand not in out and os.path.isfile(cand):
            out.append(cand)
    return out


def _uv_pythons() -> list[str]:
    """Interpreters uv installed into the data folder (`uv python install`), newest first."""
    found: list[str] = []
    try:
        names = sorted(os.listdir(UV_PYTHON_INSTALL_DIR), reverse=True)
    except OSError:
        return found
    for name in names:
        if not name.startswith("cpython-3."):
            continue
        home = os.path.join(UV_PYTHON_INSTALL_DIR, name)
        exe = os.path.join(home, "python.exe") if os.name == "nt" else os.path.join(home, "bin", "python3")
        if os.path.isfile(exe):
            found.append(exe)
    return found


def _uv_binary() -> str | None:
    return UV_BIN if os.path.isfile(UV_BIN) else shutil.which("uv")


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


_RESOLVED_PYTHON: str | None = None


def resolve_python(prefer_venv: bool = True) -> str:
    """Interpreter the voice engines run in: the managed venv, else the best Python 3.10+.

    Probing starts processes, so the answer is kept for the life of this process
    (`_ensure_venv` resets it after creating the venv)."""
    global _RESOLVED_PYTHON
    if prefer_venv and _RESOLVED_PYTHON:
        return _RESOLVED_PYTHON
    found = sys.executable
    if prefer_venv and os.path.isfile(VENV_PYTHON) and (_same_python(VENV_PYTHON, sys.executable) or _python_version_ok(VENV_PYTHON, 10)):
        found = VENV_PYTHON
    else:
        for cand in _python_candidates():
            if _python_version_ok(cand, 10):
                found = cand
                break
    if prefer_venv:
        _RESOLVED_PYTHON = found
    return found


def _same_python(a: str | None, b: str | None) -> bool:
    if not a or not b:
        return False
    try:
        return os.path.samefile(a, b)
    except OSError:
        return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


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


def _gpu_providers() -> list[str] | None:
    """ONNX Runtime providers for the graphics card, when the CUDA runtime is installed in the
    voice environment (Settings → Audio → Use graphics card) and MANGAVOX_TTS_GPU is not "0".
    On an RTX 3080 this speaks about 11x faster than the CPU."""
    if os.environ.get("MANGAVOX_TTS_GPU", "1") == "0":
        return None
    try:
        import onnxruntime as ort

        if "CUDAExecutionProvider" not in ort.get_available_providers():
            return None
        try:
            # The CUDA and cuDNN libraries come as pip packages next to onnxruntime-gpu.
            ort.preload_dlls()
        except Exception:
            pass
        return ["CUDAExecutionProvider", "CPUExecutionProvider"]
    except Exception:
        return None


def load_kokoro():
    """Kokoro on the graphics card when it can be used, else ONNX Runtime on the CPU capped to
    the allowed thread count."""
    from kokoro_onnx import Kokoro

    onnx_path, voices_path = kokoro_model_paths()
    gpu = _gpu_providers()
    if gpu:
        try:
            import onnxruntime as ort

            session = ort.InferenceSession(onnx_path, providers=gpu)
            if "CUDAExecutionProvider" in session.get_providers():
                return Kokoro.from_session(session, voices_path)
        except Exception:
            pass
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


def piper_any_ready(lang: str) -> bool:
    """At least one Piper voice of `lang` ("en" or "hi") is downloaded. Each voice is its own pack."""
    voice_set, dest = (PIPER_EN_VOICES, PIPER_EN_DIR) if lang == "en" else (PIPER_VOICES, PIPER_HI_DIR)
    return any(_piper_set_ready({key: files}, dest) for key, files in voice_set.items())


def piper_models_ready() -> bool:
    """At least one Piper voice is downloaded."""
    return piper_any_ready("hi") or piper_any_ready("en")


def piper_hi_ready() -> bool:
    """Every Hindi Piper voice is downloaded."""
    return _piper_set_ready(PIPER_VOICES, PIPER_HI_DIR)


def piper_en_ready() -> bool:
    """Every English Piper voice is downloaded."""
    return _piper_set_ready(PIPER_EN_VOICES, PIPER_EN_DIR)


def _is_english_voice(voice_id: str) -> bool:
    v = (voice_id or "").lower()
    if v.startswith("xtts_en_") or v.startswith("en_piper") or v.startswith("cb_"):
        return True
    return v.startswith(("af_", "am_", "bf_", "bm_")) or v in ("say-default",)


def _is_xtts_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("xtts_")


def _is_f5_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("f5_")


def _is_parler_voice(voice_id: str) -> bool:
    return (voice_id or "").startswith("parler_")


# Studio-grade voices (F5-TTS, Parler-TTS, XTTS v2) need several GB and a strong GPU. Until their
# engine is installed each one is voiced by the closest Kokoro voice, then by a Piper voice of the
# same language and gender (`fallback_candidates`). Mirrored by `stand_in` in mv-studio/src/tts.rs.
STUDIO_KOKORO_STAND_IN: dict[str, str] = {
    "f5_hi_female": "hf_alpha",
    "f5_hi_male": "hm_omega",
    "f5_hi_storyteller": "hm_psi",
    "f5_hi_narrator": "hm_omega",
    "parler_hi_female_clear": "hf_beta",
    "parler_hi_female_warm": "hf_alpha",
    "parler_hi_male_deep": "hm_psi",
    "parler_hi_male_bright": "hm_omega",
    "xtts_hi_female": "hf_alpha",
    "xtts_hi_male": "hm_omega",
    "xtts_hi_storyteller": "hf_beta",
    "xtts_hi_narrator": "hm_psi",
    "f5_en_female": "af_heart",
    "f5_en_male": "am_michael",
    "f5_en_narrator": "bf_emma",
    "f5_en_storyteller": "af_nova",
    "parler_en_female_neutral": "af_nicole",
    "parler_en_female_warm": "af_heart",
    "parler_en_male_narrator": "am_adam",
    "parler_en_male_casual": "am_liam",
    "xtts_en_female": "af_heart",
    "xtts_en_male": "am_echo",
    "xtts_en_narrator": "bf_emma",
    "xtts_en_storyteller": "am_adam",
    "xtts_en_warm": "af_jessica",
    "xtts_en_clear": "bm_george",
    "cb_narrator_f": "af_heart",
    "cb_narrator_m": "am_michael",
    "cb_storyteller_m": "am_adam",
}

# Old names, kept for callers: the first stand-in of each studio-grade voice.
XTTS_FALLBACK_VOICE: dict[str, str] = {k: v for k, v in STUDIO_KOKORO_STAND_IN.items() if k.startswith("xtts_")}
STUDIO_FALLBACK_VOICE: dict[str, str] = {k: v for k, v in STUDIO_KOKORO_STAND_IN.items() if not k.startswith("xtts_")}

# Piper voices by (language, gender), preferred first.
PIPER_BY_LANG_GENDER: dict[tuple[str, str], list[str]] = {
    ("en", "f"): ["en_piper_f", "en_piper_f2"],
    ("en", "m"): ["en_piper_m", "en_piper_m2"],
    ("hi", "f"): ["hi_piper_f"],
    ("hi", "m"): ["hi_piper_m", "hi_piper_rohan"],
}

# Catalog id of each Piper voice file set in PIPER_VOICE_MAP.
_PIPER_VOICE_ID: dict[tuple[str, str], str] = {
    ("en", "amy"): "en_piper_f",
    ("en", "lessac"): "en_piper_m",
    ("en", "kristin"): "en_piper_f2",
    ("en", "ryan"): "en_piper_m2",
    ("hi", "priyamvada"): "hi_piper_f",
    ("hi", "pratham"): "hi_piper_m",
    ("hi", "rohan"): "hi_piper_rohan",
}

# Pack names in Settings → Audio (mv-studio/src/voices.rs PACKS), so messages say what to get.
KOKORO_PACK_NAME = "Kokoro v1.0"
PIPER_PACK_NAMES: dict[str, str] = {
    "en_piper_f": "Piper · Amy (US)",
    "en_piper_m": "Piper · Lessac (US)",
    "en_piper_f2": "Piper · Kristin (US)",
    "en_piper_m2": "Piper · Ryan (US)",
    "hi_piper_f": "Piper · Priyamvada (Hindi)",
    "hi_piper_m": "Piper · Pratham (Hindi)",
    "hi_piper_rohan": "Piper · Rohan (Hindi)",
}
ENGINE_NAMES: dict[str, str] = {
    "kokoro": "Kokoro",
    "piper": "Piper",
    "f5-tts": "F5-TTS",
    "parler-tts": "Parler-TTS",
    "xtts": "XTTS v2",
    "chatterbox": "Chatterbox",
}


def xtts_fallback_voice(voice: str) -> str | None:
    return XTTS_FALLBACK_VOICE.get(voice or "")


def studio_fallback_voice(voice: str) -> str | None:
    return STUDIO_FALLBACK_VOICE.get(voice or "")


def _is_hindi_voice_id(voice_id: str) -> bool:
    v = voice_id or ""
    if v.startswith(("xtts_hi_", "hi_piper", "f5_hi_", "parler_hi_")) or v.startswith(("hf_", "hm_")):
        return True
    return v in ("hi-lekha", "hi-female", "hi-male", "hi-male-aman", "hi-male-deep")


def _voice_engine_hint(voice_id: str) -> str | None:
    if voice_id in VOICE_MAP:
        return VOICE_MAP[voice_id][2]
    if _is_xtts_voice(voice_id):
        return "xtts"
    if _is_f5_voice(voice_id):
        return "f5-tts"
    if _is_parler_voice(voice_id):
        return "parler-tts"
    if (voice_id or "").startswith(("hf_", "hm_")):
        return "kokoro"
    if (voice_id or "").startswith(("hi_piper_", "en_piper_")):
        return "piper"
    if (voice_id or "").startswith("cb_"):
        return "chatterbox"
    return None


def own_engine(voice: str) -> str:
    """The engine a voice is made for (Kokoro for unknown ids, like `resolve_voice`)."""
    return _voice_engine_hint(voice) or "kokoro"


def _voice_gender(voice: str) -> str:
    v = STUDIO_KOKORO_STAND_IN.get(voice) or VOICE_MAP.get(voice, (voice,))[0]
    if v.startswith(("am_", "bm_", "hm_")) or v in ("en_piper_m", "en_piper_m2", "hi_piper_m", "hi_piper_rohan"):
        return "m"
    return "f"


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
    """Download Piper ONNX files for this voice if missing. Explicit use only: synthesis never
    calls this (the app owns downloads and shows their progress)."""
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


def fallback_candidates(voice: str) -> list[str]:
    """Voices that stand in, best first, when `voice`'s own engine cannot run.

    Studio-grade voices try their Kokoro stand-in, then Piper voices of the same language
    (same gender first). Kokoro voices try their matching Piper voice, then the same list.
    A Piper voice has none: it is its own download."""
    own = own_engine(voice)
    if own == "piper":
        return []
    lang = "hi" if _is_hindi_voice_id(voice) else "en"
    kokoro = voice if own == "kokoro" else STUDIO_KOKORO_STAND_IN.get(voice)
    out: list[str] = []
    if kokoro and kokoro != voice:
        out.append(kokoro)
    mapped = PIPER_VOICE_MAP.get(kokoro or "")
    if mapped in _PIPER_VOICE_ID:
        out.append(_PIPER_VOICE_ID[mapped])
    gender = _voice_gender(voice)
    other = "m" if gender == "f" else "f"
    out += PIPER_BY_LANG_GENDER.get((lang, gender), []) + PIPER_BY_LANG_GENDER.get((lang, other), [])
    seen: set[str] = set()
    result = []
    for v in out:
        if v != voice and v not in seen:
            seen.add(v)
            result.append(v)
    return result


# Checked in the voice interpreter without importing the heavy studio-grade engines.
_HEAVY_MODULES = (("f5", "f5_tts"), ("parler", "parler_tts"), ("xtts", "TTS"), ("torch", "torch"))
_DEPS_PROBE = r"""
import json, importlib.util as u
out = {}
try:
    import kokoro_onnx, soundfile
    out["kokoro"] = True
except Exception:
    out["kokoro"] = False
try:
    from piper import PiperVoice
    out["piper"] = True
except Exception:
    out["piper"] = False
for key, mod in %r:
    try:
        out[key] = u.find_spec(mod) is not None
    except Exception:
        out[key] = False
print(json.dumps(out))
""" % (_HEAVY_MODULES,)


def _probe_deps_here() -> dict:
    """The deps probe in this interpreter (when it is the voice interpreter), no new process."""
    from importlib.machinery import PathFinder

    out: dict = {}
    try:
        import kokoro_onnx, soundfile  # noqa: F401
        out["kokoro"] = True
    except Exception:
        out["kokoro"] = False
    try:
        from piper import PiperVoice  # noqa: F401
        out["piper"] = True
    except Exception:
        out["piper"] = False
    # This folder has f5_tts.py / parler_tts.py helpers that are not the engines.
    here = os.path.normcase(os.path.dirname(os.path.abspath(__file__)))
    search = [p for p in sys.path if p and os.path.normcase(os.path.abspath(p)) != here]
    for key, mod in _HEAVY_MODULES:
        try:
            out[key] = PathFinder.find_spec(mod, search) is not None
        except Exception:
            out[key] = False
    return out


def _probe_deps(python: str) -> dict:
    """Which engine packages `python` can use: {kokoro, piper} import, {f5, parler, xtts, torch} exist."""
    if _same_python(python, sys.executable):
        return _probe_deps_here()
    try:
        r = subprocess.run(
            [python, "-c", _DEPS_PROBE], capture_output=True, text=True, timeout=60,
            cwd=BASE_DIR if os.path.isdir(BASE_DIR) else None, creationflags=_NO_WINDOW,
        )
        lines = [l for l in (r.stdout or "").splitlines() if l.strip()]
        return json.loads(lines[-1]) if r.returncode == 0 and lines else {}
    except Exception:
        return {}


def _xtts_ready(python: str) -> bool:
    try:
        from xtts_tts import xtts_ready
    except ImportError:
        return False
    return xtts_ready(python)


def _xtts_ref_ready(voice: str) -> bool:
    try:
        from xtts_tts import xtts_voice_ref_ready
    except ImportError:
        return False
    return xtts_voice_ref_ready(voice)


class EngineProbe:
    """What the voice engines have installed, checked once (each check may start Python)."""

    def __init__(self, python: str | None = None):
        self.python = python or resolve_python()
        self._deps: dict | None = None
        self._flags: dict[str, bool] = {}

    def deps(self) -> dict:
        if self._deps is None:
            self._deps = _probe_deps(self.python)
        return self._deps

    def _once(self, key: str, check) -> bool:
        if key not in self._flags:
            try:
                self._flags[key] = bool(check())
            except Exception:
                self._flags[key] = False
        return self._flags[key]

    def kokoro_models(self) -> bool:
        return self._once("kokoroModels", kokoro_models_ready)

    def kokoro_deps(self) -> bool:
        return bool(self.deps().get("kokoro"))

    def kokoro(self) -> bool:
        return self.kokoro_models() and self.kokoro_deps()

    def piper_deps(self) -> bool:
        return bool(self.deps().get("piper"))

    def f5_installed(self) -> bool:
        return bool(self.deps().get("f5") and self.deps().get("torch"))

    def parler_installed(self) -> bool:
        return bool(self.deps().get("parler") and self.deps().get("torch"))

    def xtts_installed(self) -> bool:
        xtts_venv_python = os.path.join(BASE_DIR, "xtts-venv", "bin", "python3")
        return bool(self.deps().get("xtts") and self.deps().get("torch")) or os.path.isfile(xtts_venv_python)

    # The full checks import torch, so they only run when the engine's model is in place.
    def f5(self) -> bool:
        return self._once("f5", lambda: os.path.isfile(F5_MARKER) and self.f5_installed() and f5_deps_ready(self.python))

    def parler(self) -> bool:
        return self._once("parler", lambda: os.path.isfile(PARLER_MARKER) and self.parler_installed() and parler_deps_ready(self.python))

    def xtts(self) -> bool:
        return self._once("xtts", lambda: os.path.isfile(XTTS_MARKER) and self.xtts_installed() and _xtts_ready(self.python))

    # Chatterbox lives in its own venv; its install writes a marker, so no torch import here.
    def chatterbox_installed(self) -> bool:
        return self._once("chatterboxEngine", lambda: _chatterbox().engine_installed())

    def chatterbox_models(self) -> bool:
        return self._once("chatterboxModels", lambda: _chatterbox().weights_ready())

    def chatterbox(self) -> bool:
        return self.chatterbox_installed() and self.chatterbox_models()

    def active_engine(self) -> str | None:
        """Best ready engine: F5-TTS > Parler-TTS > Kokoro > XTTS > Piper."""
        if self.f5():
            return "f5-tts"
        if self.parler():
            return "parler-tts"
        if self.kokoro():
            return "kokoro"
        if self.xtts():
            return "xtts"
        if piper_models_ready() and self.piper_deps():
            return "piper"
        return None


def _native_status(voice: str, probe: EngineProbe) -> tuple[bool, bool, bool]:
    """(ready, model files present, engine installed) for the voice's own engine."""
    own = own_engine(voice)
    if own == "kokoro":
        models, deps = probe.kokoro_models(), probe.kokoro_deps()
        return models and deps, models, deps
    if own == "piper":
        models, deps = piper_voice_ready(voice), probe.piper_deps()
        return models and deps, models, deps
    if own == "f5-tts":
        return probe.f5(), os.path.isfile(F5_MARKER), probe.f5_installed()
    if own == "parler-tts":
        return probe.parler(), os.path.isfile(PARLER_MARKER), probe.parler_installed()
    if own == "xtts":
        ref = _xtts_ref_ready(voice)
        return probe.xtts() and ref, os.path.isfile(XTTS_MARKER) and ref, probe.xtts_installed()
    if own == "chatterbox":
        model_ok = probe.chatterbox_models() and _chatterbox().reference_ready(voice)
        engine_ok = probe.chatterbox_installed()
        return model_ok and engine_ok, model_ok, engine_ok
    return False, False, False


_CHATTERBOX = None


def _chatterbox():
    """chatterbox_tts (stdlib-only on this side), imported on first use."""
    global _CHATTERBOX
    if _CHATTERBOX is None:
        import chatterbox_tts as _cb
        _CHATTERBOX = _cb
    return _CHATTERBOX


def _voice_name(voice: str) -> str:
    if own_engine(voice) == "piper":
        return PIPER_VOICE_MAP.get(voice, ("", voice))[1].capitalize()
    return voice


def missing_message(voice: str, probe: EngineProbe | None = None) -> str | None:
    """What `voice` lacks and where to get it, or None when it can be voiced as it is."""
    probe = probe or EngineProbe()
    ready, model_ok, engine_ok = _native_status(voice, probe)
    if ready:
        return None
    own = own_engine(voice)
    eng = ENGINE_NAMES.get(own, own)
    if own in ("kokoro", "piper"):
        pack = KOKORO_PACK_NAME if own == "kokoro" else PIPER_PACK_NAMES.get(voice, "its Piper voice pack")
        name = _voice_name(voice)
        if not model_ok:
            also = "" if engine_ok else f" and the {eng} engine is not installed"
            return f'Voice {name} is not downloaded{also}. In Settings → Audio, download "{pack}"; its engine installs with it.'
        return f'Voice {name} is downloaded but the {eng} engine is not installed. In Settings → Audio, press Install engine on "{pack}".'
    if own == "chatterbox":
        pack = _chatterbox().PACK_NAME
        if not engine_ok:
            what = "the Chatterbox engine is not installed"
        elif not probe.chatterbox_models():
            what = "its voice files are not downloaded"
        elif not (probe.kokoro() or (piper_models_ready() and probe.piper_deps())):
            # Setup records the clip with a local Kokoro (or Piper) voice.
            return (f'{voice} clones a reference clip that is recorded with a local voice, and none is installed. '
                    f'In Settings → Audio, download "{KOKORO_PACK_NAME}", then press Set up on "{pack}" again.')
        else:
            what = "its reference clip is missing"
        stand_in = STUDIO_KOKORO_STAND_IN.get(voice)
        tail = f" Until then {stand_in} (Kokoro) can stand in once Kokoro is installed." if stand_in else ""
        return f'{voice} needs Chatterbox and {what}. In Settings → Audio, press Set up on "{pack}" (about 8 GB, NVIDIA GPU recommended).{tail}'
    if not engine_ok:
        why = f"needs {eng}, which is not installed"
    elif not model_ok:
        why = f"needs the {eng} model, which is not downloaded"
    else:
        why = f"needs {eng}, which is installed but does not load"
    if probe.kokoro_models() and not probe.kokoro_deps():
        get = f'install the Kokoro engine on "{KOKORO_PACK_NAME}"'
    else:
        get = f'download "{KOKORO_PACK_NAME}" (its engine installs with it)'
    return f"{voice} {why}, and no stand-in voice is ready. In Settings → Audio, {get} so a Kokoro voice can stand in."


def voice_plan(voice: str, probe: EngineProbe | None = None) -> dict:
    """How `voice` is voiced right now: by its own engine, by a stand-in voice, or not at all.

    ready          it can be synthesized now
    engine         the engine that runs ("none" when not ready)
    synthVoice     the voice actually synthesized (the stand-in when there is one)
    fallbackVoice  the stand-in, when the voice's own engine cannot run
    fallbackEngine the stand-in's engine
    missing        what the voice's own engine lacks: "model", "engine" or None
    message        what to get and where (only when not ready)
    """
    probe = probe or EngineProbe()
    own = own_engine(voice)
    ready, model_ok, engine_ok = _native_status(voice, probe)
    if ready:
        missing = None
    elif own in ("kokoro", "piper"):
        # Downloading the pack also installs its engine, so the model is the first thing to get.
        missing = "model" if not model_ok else "engine"
    else:
        # Studio-grade installers set up the engine and then fetch its model.
        missing = "engine" if not engine_ok else "model" if not model_ok else "engine"
    plan: dict = {
        "ready": ready,
        "engine": own if ready else "none",
        "ownEngine": own,
        "synthVoice": voice,
        "fallbackVoice": None,
        "fallbackEngine": None,
        "missing": missing,
    }
    if not ready:
        for cand in fallback_candidates(voice):
            if _native_status(cand, probe)[0]:
                eng = own_engine(cand)
                plan.update(ready=True, engine=eng, synthVoice=cand, fallbackVoice=cand, fallbackEngine=eng)
                break
    if not plan["ready"]:
        plan["message"] = missing_message(voice, probe)
    return plan


def catalog_voices() -> list[str]:
    return list(VOICE_MAP.keys())


def voices_ready_map(probe: EngineProbe | None = None) -> dict:
    """`voice_plan` for every catalog voice, sharing one probe (one deps check)."""
    probe = probe or EngineProbe()
    return {v: voice_plan(v, probe) for v in catalog_voices()}


def resolve_engine_for_voice(
    voice: str,
    *,
    allow_download: bool = False,
    allow_fallback: bool = False,
) -> str | None:
    """Engine that voices `voice` now; with `allow_fallback` a stand-in's engine counts too.
    Never downloads (`allow_download` is accepted for old callers and ignored)."""
    del allow_download
    plan = voice_plan(voice)
    if not plan["ready"] or (plan["fallbackVoice"] and not allow_fallback):
        return None
    return plan["engine"]


def voice_ready(voice: str) -> dict:
    probe = EngineProbe()
    plan = voice_plan(voice, probe)
    return {
        "voice": voice,
        **plan,
        "previewFallback": plan["fallbackVoice"] is not None,
        "previewVoice": plan["synthVoice"],
        "f5Ready": probe.f5(),
        "parlerReady": probe.parler(),
        "xttsInstalled": probe.xtts(),
        "xttsReady": probe.xtts(),
        "kokoroReady": probe.kokoro(),
        "chatterboxReady": probe.chatterbox(),
        "piperHiReady": piper_any_ready("hi") and probe.piper_deps(),
        "piperEnReady": piper_any_ready("en") and probe.piper_deps(),
    }


def kokoro_deps_ready(python: str | None = None) -> bool:
    py = python or resolve_python()
    try:
        r = subprocess.run([py, "-c", "import kokoro_onnx, soundfile"], capture_output=True, timeout=60, creationflags=_NO_WINDOW)
        return r.returncode == 0
    except Exception:
        return False


def piper_deps_ready(python: str | None = None) -> bool:
    py = python or resolve_python()
    try:
        r = subprocess.run([py, "-c", "from piper import PiperVoice"], capture_output=True, timeout=60, creationflags=_NO_WINDOW)
        return r.returncode == 0
    except Exception:
        return False


def f5_deps_ready(python: str | None = None) -> bool:
    """Check if F5-TTS dependencies are installed."""
    py = python or sys.executable
    try:
        r = subprocess.run(
            [py, "-c", "from f5_tts.api import F5TTS; import torch"],
            capture_output=True, timeout=60, cwd=BASE_DIR, creationflags=_NO_WINDOW,
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
            capture_output=True, timeout=60, cwd=BASE_DIR, creationflags=_NO_WINDOW,
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
    return EngineProbe().active_engine()


def status() -> dict:
    probe = EngineProbe()
    py = probe.python
    engine = probe.active_engine()
    active_py_version = sys.version.split()[0]
    if not _same_python(py, sys.executable):
        try:
            r = subprocess.run([py, "-c", "import sys; print(sys.version.split()[0])"], capture_output=True, text=True, timeout=10, creationflags=_NO_WINDOW)
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

    f5_ready_flag = probe.f5()
    parler_ready_flag = probe.parler()
    xtts_ready_flag = probe.xtts()
    kokoro_deps_flag = probe.kokoro_deps()
    piper_deps_flag = probe.piper_deps()
    kokoro_models = probe.kokoro_models()
    piper_models = piper_models_ready()
    chatterbox_engine = probe.chatterbox_installed()
    chatterbox_ready = probe.chatterbox()
    try:
        chatterbox_info = _chatterbox().status()
    except Exception as e:
        chatterbox_info = {"error": str(e)}

    st = {
        "installed": engine is not None or kokoro_models or piper_models or f5_ready_flag or parler_ready_flag,
        "kokoroDepsReady": kokoro_deps_flag,
        # Settings → Audio offers "Use graphics card" when a card is there and not used yet.
        "nvidiaGpu": nvidia_gpu_present(),
        "kokoroGpu": kokoro_deps_flag and nvidia_gpu_present() and kokoro_gpu_ready(py),
        "piperDepsReady": piper_deps_flag,
        "engine": engine or "none",
        "f5Ready": f5_ready_flag,
        "parlerReady": parler_ready_flag,
        "kokoroReady": kokoro_models,
        "piperReady": piper_models,
        # Any voice of the language downloaded (each Piper voice is its own pack).
        "piperHiReady": piper_any_ready("hi"),
        "piperEnReady": piper_any_ready("en"),
        "xttsReady": xtts_ready_flag,
        # Chatterbox: its own venv (`engines/chatterbox`) and weights (`models/chatterbox`).
        "chatterboxDepsReady": chatterbox_engine,
        "chatterboxReady": chatterbox_ready,
        "chatterbox": chatterbox_info,
        "depsReady": kokoro_deps_flag or piper_deps_flag or f5_ready_flag or parler_ready_flag,
        "modelsReady": kokoro_models or piper_models or f5_ready_flag or parler_ready_flag,
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
            # Chatterbox (expressive)
            "cb_narrator_f", "cb_narrator_m", "cb_storyteller_m",
        ],
        "engines": {
            "f5-tts": {"ready": f5_ready_flag, "description": "State-of-the-art multilingual TTS"},
            "parler-tts": {"ready": parler_ready_flag, "description": "Fast, controllable high-quality TTS"},
            "xtts": {"ready": xtts_ready_flag, "description": "ElevenLabs-class quality with voice cloning"},
            "kokoro": {"ready": kokoro_models and kokoro_deps_flag, "description": "Efficient 82M parameter model"},
            "piper": {"ready": piper_models and piper_deps_flag, "description": "Standard open-source TTS"},
            "chatterbox": {"ready": chatterbox_ready, "description": "Expressive narrator with emotion control (GPU recommended)"},
        },
        # Per-voice truth (same as `voices-ready`): the app reads readiness from here.
        "voices": voices_ready_map(probe),
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
        subprocess.run([cand, "-m", "venv", VENV_DIR], check=True, timeout=120, capture_output=True, creationflags=_NO_WINDOW)
        if os.path.isfile(VENV_PYTHON):
            _forget_resolved_python()
            return VENV_PYTHON
    uv = _uv_binary()
    if uv:
        try:
            emit({"type": "info", "message": "Creating private Python 3.12 runtime for studio voices…"})
            os.makedirs(BASE_DIR, exist_ok=True)
            env = os.environ.copy()
            env["UV_CACHE_DIR"] = UV_CACHE_DIR
            env["UV_PYTHON_INSTALL_DIR"] = UV_PYTHON_INSTALL_DIR
            subprocess.run([uv, "python", "install", "3.12"], check=True, timeout=900, env=env, capture_output=True, creationflags=_NO_WINDOW)
            # --seed puts pip in the venv; _pip_install needs it.
            subprocess.run([uv, "venv", "--seed", "--python", "3.12", VENV_DIR], check=True, timeout=300, env=env, capture_output=True, creationflags=_NO_WINDOW)
            if os.path.isfile(VENV_PYTHON):
                _forget_resolved_python()
                return VENV_PYTHON
        except Exception as e:
            emit({"type": "info", "message": f"Private Python setup skipped: {str(e)[:200]}"})
    return None


def _forget_resolved_python() -> None:
    global _RESOLVED_PYTHON
    _RESOLVED_PYTHON = None


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
    """16-bit mono WAV at the engine's native rate. No per-page peak normalisation: the
    narration is loudness-normalised per chapter (tts_bridge / narration_tts). Only a page
    that would clip is scaled down."""
    import numpy as np
    arr = np.asarray(samples, dtype=np.float64)
    if arr.ndim > 1:
        arr = arr.mean(axis=1)
    peak = float(np.max(np.abs(arr))) if arr.size else 0.0
    if peak > 0.999:
        arr = arr / peak * 0.999
    pcm = (arr * 32767).astype(np.int16)
    os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
    with wave.open(path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm.tobytes())


def synthesize_kokoro(text: str, output_path: str, voice: str, speed: float) -> None:
    kokoro_voice, lang, _ = resolve_voice(voice)
    if not kokoro_models_ready():
        raise RuntimeError(missing_message(voice) or "The Kokoro voice pack is not downloaded")
    try:
        # Loaded once per process (the worker keeps it for a whole chapter).
        engine = get_kokoro_model()
    except ImportError as e:
        raise RuntimeError(
            f'The Kokoro engine is not installed in {sys.executable} ({e}). '
            f'In Settings → Audio, press Install engine on "{KOKORO_PACK_NAME}".'
        ) from e
    samples, sample_rate = engine.create(text, voice=kokoro_voice, speed=float(speed or 1.0), lang=lang)
    _write_wav_pcm16(output_path, samples, sample_rate)


def synthesize_piper(text: str, output_path: str, voice: str, speed: float) -> None:
    # Never downloads here: a job must not fetch 60 MB without the app showing it.
    if not piper_voice_ready(voice):
        raise RuntimeError(missing_message(voice) or f"Piper voice {voice} is not downloaded")
    try:
        from piper import PiperVoice
    except ImportError as e:
        pack = PIPER_PACK_NAMES.get(voice, "its Piper voice pack")
        raise RuntimeError(
            f'The Piper engine is not installed in {sys.executable} ({e}). '
            f'In Settings → Audio, press Install engine on "{pack}".'
        ) from e

    lang, key = PIPER_VOICE_MAP.get(voice, ("hi", "pratham"))
    if lang == "en":
        prefix, dest = "en_US", PIPER_EN_DIR
    else:
        prefix, dest = "hi_IN", PIPER_HI_DIR

    onnx = os.path.join(dest, f"{prefix}-{key}-medium.onnx")
    cfg = os.path.join(dest, f"{prefix}-{key}-medium.onnx.json")
    if not os.path.isfile(onnx):
        raise RuntimeError(missing_message(voice) or f"Piper model missing: {onnx}")

    piper = PiperVoice.load(onnx, config_path=cfg)
    # Speed (the user's setting times the scene's pace) is a shorter or longer phoneme length.
    syn = None
    speed = float(speed or 1.0)
    if abs(speed - 1.0) > 0.005:
        try:
            from piper import SynthesisConfig
            syn = SynthesisConfig(length_scale=float(piper.config.length_scale) / max(0.25, speed))
        except Exception:
            syn = None
    chunks = list(piper.synthesize(text, syn_config=syn) if syn else piper.synthesize(text))
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


def synthesize_to_file(text: str, output_path: str, voice: str = "hf_alpha", speed: float = 1.0) -> str:
    """Synthesizes `text` with `voice`, or with its stand-in when the voice's own engine is not
    installed. Returns the engine used. Never downloads anything."""
    # What is installed does not change while a process runs, so each voice is planned once.
    plan = _PLANS.get(voice)
    if plan is None or not plan["ready"]:
        plan = _PLANS[voice] = voice_plan(voice)
    if not plan["ready"]:
        raise RuntimeError(plan.get("message") or f"No installed engine can voice {voice}")
    engine, synth_voice = plan["engine"], plan["synthVoice"]
    if plan["fallbackVoice"]:
        emit({"type": "info", "message": f"{voice}: {ENGINE_NAMES.get(plan['ownEngine'], plan['ownEngine'])} is not installed; using {synth_voice} ({ENGINE_NAMES.get(engine, engine)})"})

    if engine == "f5-tts":
        synthesize_f5(text, output_path, synth_voice, speed)
    elif engine == "parler-tts":
        synthesize_parler(text, output_path, synth_voice, speed)
    elif engine == "xtts":
        from xtts_tts import synthesize_xtts
        synthesize_xtts(text, output_path, synth_voice, speed, resolve_python())
    elif engine == "kokoro":
        synthesize_kokoro(text, output_path, synth_voice, speed)
    elif engine == "piper":
        synthesize_piper(text, output_path, synth_voice, speed)
    elif engine == "chatterbox":
        _chatterbox().synthesize_one(text, output_path, synth_voice, speed)
    else:
        raise RuntimeError(f"Unknown voice engine {engine} for {voice}")
    return engine


_PLANS: dict = {}

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
        engine = synthesize_to_file(text, args["output_path"], args.get("voice", "hf_alpha"), args.get("speed", 1.0))
        emit({"type": "done", "path": args["output_path"], "engine": engine})
        return 0
    except Exception as e:
        emit({"type": "error", "message": str(e)})
        return 1


def run_worker() -> int:
    """One process for a whole chapter (tts_bridge.py starts it): a JSON request per line on
    stdin, {"text", "voice", "output_path", "speed"}, and one answer line per request on stdout,
    {"ok": true, "path", "engine"} or {"ok": false, "message"}. The model loads once, so each
    request costs only the speech itself. Other stdout lines (info) carry no "ok" key."""
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            engine = synthesize_to_file((req.get("text") or "").strip(), req["output_path"], req.get("voice", "hf_alpha"), req.get("speed", 1.0))
            out = {"ok": True, "path": req["output_path"], "engine": engine}
        except Exception as e:
            out = {"ok": False, "message": str(e)[:500]}
        sys.stdout.write(json.dumps(out) + "\n")
        sys.stdout.flush()
    return 0


def kokoro_gpu_ready(python: str | None = None) -> bool:
    """The voice environment's ONNX Runtime can use an NVIDIA graphics card."""
    py = python or resolve_python()
    try:
        r = subprocess.run([py, "-c", "import onnxruntime as o; import sys; sys.exit(0 if 'CUDAExecutionProvider' in o.get_available_providers() else 1)"],
                           capture_output=True, timeout=60, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        return r.returncode == 0
    except Exception:
        return False


def nvidia_gpu_present() -> bool:
    return shutil.which("nvidia-smi") is not None


def install_kokoro_gpu() -> int:
    """Swaps the voice environment's ONNX Runtime for the CUDA build (about 2 GB with NVIDIA's
    CUDA and cuDNN libraries), so Kokoro speaks on an NVIDIA graphics card. Restores the CPU
    build when the GPU one cannot be used."""
    if not nvidia_gpu_present():
        emit({"type": "error", "message": "No NVIDIA graphics card was found (nvidia-smi is missing). Kokoro keeps using the CPU."})
        return 1
    py = _ensure_venv()
    if not py:
        emit({"type": "error", "message": "The voice environment is not set up yet. Install the Kokoro engine first."})
        return 1
    if kokoro_gpu_ready(py):
        emit({"type": "done", "message": "Kokoro already uses the graphics card", "python": py})
        return 0
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    emit({"type": "progress", "stage": "pip", "current": 1, "total": 3, "message": "Downloading the graphics card runtime (about 2 GB)…"})
    try:
        subprocess.run([py, "-m", "pip", "uninstall", "-y", "onnxruntime"], capture_output=True, timeout=600, creationflags=flags)
        _pip_install(py, ["onnxruntime-gpu[cuda,cudnn]>=1.21"])
    except Exception as e:
        emit({"type": "progress", "stage": "pip", "current": 2, "total": 3, "message": "Putting the CPU runtime back…"})
        try:
            subprocess.run([py, "-m", "pip", "uninstall", "-y", "onnxruntime-gpu"], capture_output=True, timeout=600, creationflags=flags)
            _pip_install(py, ["onnxruntime>=1.19.0,<2"])
        except Exception:
            pass
        emit({"type": "error", "message": f"Could not install the graphics card runtime: {str(e)[-400:]}"})
        return 1
    emit({"type": "progress", "stage": "verify", "current": 3, "total": 3, "message": "Checking the graphics card…"})
    if not kokoro_gpu_ready(py):
        emit({"type": "error", "message": "The graphics card runtime installed but cannot see the card. Update the NVIDIA driver and try again."})
        return 1
    emit({"type": "done", "message": "Kokoro now speaks on the graphics card", "python": py})
    return 0


ENGINE_PACKAGES = {
    "kokoro": ["kokoro-onnx>=0.4.0", "soundfile>=0.12.0", "numpy>=1.26"],
    "piper": ["piper-tts>=1.3.0", "onnxruntime>=1.19.0,<2"],
}


def install_engine(name: str) -> int:
    """Install one engine's Python packages into the managed voice environment.

    Model files are downloaded separately (Settings → Audio), so this only runs pip.
    Emits JSON lines: info / progress / done / error.
    """
    if name == "chatterbox":
        # Its own Python 3.11 venv, PyTorch and weights: see chatterbox_tts.install.
        return _chatterbox().install()
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
    # The graphics card runtime already provides onnxruntime; the CPU build would replace it.
    if kokoro_gpu_ready(py):
        packages = [p for p in packages if not p.startswith("onnxruntime")]
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
        name = sys.argv[2] if len(sys.argv) > 2 else "kokoro"
        return install_kokoro_gpu() if name == "kokoro-gpu" else install_engine(name)
    if cmd == "worker":
        return run_worker()
    if cmd == "serve":
        voice_id = sys.argv[2] if len(sys.argv) > 2 else "hf_alpha"
        run_server(voice_id)
        return 0
    if cmd == "voices-ready":
        emit(voices_ready_map())
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
