#!/usr/bin/env python3
"""
XTTS v2 — ultra-natural multilingual TTS (closest open-source quality to ElevenLabs).
Hindi + English via voice-cloning reference clips; Apple Silicon MPS when available.
"""
from __future__ import annotations

import os
import subprocess
import sys

BASE_DIR = os.path.join(os.path.expanduser("~"), ".mangavox")
XTTS_DIR = os.path.join(BASE_DIR, "models", "xtts")
XTTS_VENV_DIR = os.path.join(BASE_DIR, "xtts-venv")
XTTS_PYTHON = os.path.join(XTTS_VENV_DIR, "bin", "python3")
SAFE_CWD = BASE_DIR
REF_DIR = os.path.join(XTTS_DIR, "refs")
REF_HI_FEMALE = os.path.join(REF_DIR, "hi_female.wav")
REF_HI_MALE = os.path.join(REF_DIR, "hi_male.wav")
REF_EN_FEMALE = os.path.join(REF_DIR, "en_female.wav")
REF_EN_MALE = os.path.join(REF_DIR, "en_male.wav")
MODEL_MARKER = os.path.join(XTTS_DIR, ".model_ready")

XTTS_MODEL = "tts_models/multilingual/multi-dataset/xtts_v2"

TRANSFORMERS_MPS_SHIM = """
import torch
try:
    import transformers.pytorch_utils as _ptu
    if not hasattr(_ptu, "isin_mps_friendly"):
        def isin_mps_friendly(elements, test_elements):
            return torch.isin(elements, test_elements)
        _ptu.isin_mps_friendly = isin_mps_friendly
except Exception:
    pass
"""

# voice_id -> (language code, reference wav path)
VOICE_META: dict[str, tuple[str, str]] = {
    "xtts_hi_female": ("hi", REF_HI_FEMALE),
    "xtts_hi_male": ("hi", REF_HI_MALE),
    "xtts_hi_storyteller": ("hi", REF_HI_FEMALE),
    "xtts_hi_narrator": ("hi", REF_HI_MALE),
    "xtts_en_female": ("en", REF_EN_FEMALE),
    "xtts_en_male": ("en", REF_EN_MALE),
    "xtts_en_narrator": ("en", REF_EN_FEMALE),
    "xtts_en_storyteller": ("en", REF_EN_MALE),
    "xtts_en_warm": ("en", REF_EN_FEMALE),
    "xtts_en_clear": ("en", REF_EN_MALE),
}

_HINDI_BOOTSTRAP_FEMALE = "नमस्ते, मैं आपकी कहानी सुनाने के लिए तैयार हूँ। यह स्वर बहुत स्वाभाविक और स्पष्ट है।"
_HINDI_BOOTSTRAP_MALE = "नमस्ते, मैं आपकी कहानी का वर्णन करूँगा। यह आवाज़ गर्म और प्राकृतिक है।"
_EN_BOOTSTRAP_FEMALE = (
    "Hello, I'm ready to narrate your story. This voice is warm, clear, and natural — "
    "just like a real human storyteller."
)
_EN_BOOTSTRAP_MALE = (
    "Hello, I'll be your narrator for this chapter. This voice is calm, confident, "
    "and easy to listen to for long sessions."
)


def _emit(obj: dict) -> None:
    import json
    print(json.dumps(obj), flush=True)


def xtts_deps_ready(python: str | None = None) -> bool:
    py = XTTS_PYTHON if os.path.isfile(XTTS_PYTHON) else (python or sys.executable)
    try:
        r = subprocess.run(
            [py, "-c", TRANSFORMERS_MPS_SHIM + "\nfrom TTS.api import TTS; import torch"],
            capture_output=True, text=True, timeout=60,
        )
        return r.returncode == 0
    except Exception:
        return False


def _ref_ok(path: str) -> bool:
    return os.path.isfile(path) and os.path.getsize(path) > 1000


def xtts_voice_ref_ready(voice: str) -> bool:
    meta = VOICE_META.get(voice)
    if not meta:
        return False
    return _ref_ok(meta[1])


def xtts_refs_ready() -> bool:
    return all(_ref_ok(p) for p in (REF_HI_FEMALE, REF_HI_MALE, REF_EN_FEMALE, REF_EN_MALE))


def xtts_ready(python: str | None = None) -> bool:
    return xtts_deps_ready(python) and xtts_refs_ready() and os.path.isfile(MODEL_MARKER)


def bootstrap_refs_with_piper() -> bool:
    """Build Hindi + English reference clips using Piper (for XTTS cloning)."""
    try:
        from kokoro_tts import (
            synthesize_piper,
            piper_hi_ready,
            piper_en_ready,
            piper_deps_ready,
        )
    except ImportError:
        return False
    if not piper_deps_ready():
        return False
    os.makedirs(REF_DIR, exist_ok=True)
    try:
        if piper_hi_ready():
            if not _ref_ok(REF_HI_FEMALE):
                synthesize_piper(_HINDI_BOOTSTRAP_FEMALE, REF_HI_FEMALE, "hi_piper_f", 1.0)
            if not _ref_ok(REF_HI_MALE):
                synthesize_piper(_HINDI_BOOTSTRAP_MALE, REF_HI_MALE, "hi_piper_m", 1.0)
        if piper_en_ready():
            if not _ref_ok(REF_EN_FEMALE):
                synthesize_piper(_EN_BOOTSTRAP_FEMALE, REF_EN_FEMALE, "en_piper_f", 1.0)
            if not _ref_ok(REF_EN_MALE):
                synthesize_piper(_EN_BOOTSTRAP_MALE, REF_EN_MALE, "en_piper_m", 1.0)
        return xtts_refs_ready()
    except Exception:
        return False


def install_xtts(python: str, progress_cb=None) -> bool:
    def info(msg: str) -> None:
        if progress_cb:
            progress_cb({"type": "info", "message": msg})
        else:
            _emit({"type": "info", "message": msg})

    os.makedirs(XTTS_DIR, exist_ok=True)
    info("Installing Ultra Natural TTS (XTTS v2) — ~2 GB, one-time download…")

    if not os.path.isfile(XTTS_PYTHON):
        info("Creating isolated XTTS environment (keeps XTTS deps from breaking Parler)…")
        os.makedirs(BASE_DIR, exist_ok=True)
        subprocess.run([python, "-m", "venv", XTTS_VENV_DIR], check=True, timeout=180)

    xtts_python = XTTS_PYTHON if os.path.isfile(XTTS_PYTHON) else python

    subprocess.run(
        [xtts_python, "-m", "ensurepip", "--upgrade"],
        capture_output=True, text=True, timeout=180,
    )
    subprocess.run(
        [xtts_python, "-m", "pip", "install", "--upgrade", "pip"],
        check=True, capture_output=True, text=True, timeout=180, cwd=SAFE_CWD,
    )
    subprocess.run(
        [xtts_python, "-m", "pip", "install", "coqui-tts[codec]>=0.24.2", "torch", "torchaudio", "transformers>=4.57", "torchcodec"],
        check=True, capture_output=True, text=True, timeout=1200, cwd=SAFE_CWD,
    )

    if not bootstrap_refs_with_piper():
        info("Building voice references — ensure Hindi + English Piper packs downloaded…")
        bootstrap_refs_with_piper()

    warm_script = f"""
import os
import torch
{TRANSFORMERS_MPS_SHIM}
from TTS.api import TTS

os.environ["COQUI_TOS_AGREED"] = "1"
os.environ["HF_HOME"] = {os.path.join(XTTS_DIR, "hf-cache")!r}
os.environ["TTS_HOME"] = {os.path.join(XTTS_DIR, "tts-cache")!r}

device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
if device == "cpu" and torch.cuda.is_available():
    device = "cuda"
tts = TTS("{XTTS_MODEL}")
try:
    tts.to(device)
except Exception:
    pass
open({MODEL_MARKER!r}, "w").close()
print("ok")
"""
    r = subprocess.run([xtts_python, "-c", warm_script], capture_output=True, text=True, timeout=1800, cwd=SAFE_CWD)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout or "XTTS model download failed")[-1200:])
    return xtts_ready(xtts_python)


def synthesize_xtts(text: str, output_path: str, voice: str, speed: float = 1.0, python: str | None = None) -> None:
    py = XTTS_PYTHON if os.path.isfile(XTTS_PYTHON) else (python or sys.executable)
    meta = VOICE_META.get(voice)
    if not meta:
        raise RuntimeError(f"Unknown XTTS voice: {voice}")

    lang, ref = meta
    if not _ref_ok(ref) and not bootstrap_refs_with_piper():
        raise RuntimeError(f"XTTS reference missing for {voice} — re-download AI Voice Models")
    if not _ref_ok(ref):
        raise RuntimeError(f"XTTS reference missing: {ref}")

    os.makedirs(os.path.dirname(os.path.abspath(output_path)) or ".", exist_ok=True)

    script = f"""
import os
import torch
{TRANSFORMERS_MPS_SHIM}
from TTS.api import TTS

text = {text!r}
output_path = {output_path!r}
ref = {ref!r}
lang = {lang!r}
speed = {float(speed or 1.0)}
os.environ["COQUI_TOS_AGREED"] = "1"
os.environ["HF_HOME"] = {os.path.join(XTTS_DIR, "hf-cache")!r}
os.environ["TTS_HOME"] = {os.path.join(XTTS_DIR, "tts-cache")!r}

device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
if device == "cpu" and torch.cuda.is_available():
    device = "cuda"

tts = TTS("{XTTS_MODEL}")
try:
    tts.to(device)
except Exception:
    pass

kwargs = dict(text=text, file_path=output_path, speaker_wav=ref, language=lang)
try:
    tts.tts_to_file(**kwargs, speed=speed)
except TypeError:
    tts.tts_to_file(**kwargs)

if not os.path.isfile(output_path) or os.path.getsize(output_path) < 100:
    raise SystemExit("XTTS produced no audio")
"""
    r = subprocess.run([py, "-c", script], capture_output=True, text=True, timeout=600, cwd=SAFE_CWD)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout or "XTTS synthesis failed")[:500])
