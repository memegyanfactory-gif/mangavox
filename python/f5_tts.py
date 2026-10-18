#!/usr/bin/env python3
"""
F5-TTS — State-of-the-art open-source TTS (ElevenLabs-quality).
Multilingual support for Hindi, English, and more with natural prosody.
Faster inference than XTTS v2, no reference audio needed.
"""
from __future__ import annotations

import os
import subprocess
import sys
import json

BASE_DIR = os.path.join(os.path.expanduser("~"), ".mangavox")
F5_DIR = os.path.join(BASE_DIR, "models", "f5-tts")
MODEL_MARKER = os.path.join(F5_DIR, ".model_ready")
SAFE_CWD = BASE_DIR
REF_DIR = os.path.join(BASE_DIR, "models", "xtts", "refs")
REF_HI_FEMALE = os.path.join(REF_DIR, "hi_female.wav")
REF_HI_MALE = os.path.join(REF_DIR, "hi_male.wav")
REF_EN_FEMALE = os.path.join(REF_DIR, "en_female.wav")
REF_EN_MALE = os.path.join(REF_DIR, "en_male.wav")

# Voice ID -> (language, voice_name, description)
VOICE_MAP: dict[str, tuple[str, str, str]] = {
    # F5-TTS Hindi voices (natural, multilingual model)
    "f5_hi_female": ("hi", "default_female", "Natural Hindi female voice"),
    "f5_hi_male": ("hi", "default_male", "Natural Hindi male voice"),
    "f5_hi_storyteller": ("hi", "storyteller", "Professional Hindi narrator"),
    "f5_hi_narrator": ("hi", "narrator", "Clear Hindi voice for long-form content"),
    # F5-TTS English voices
    "f5_en_female": ("en", "default_female", "Natural English female voice"),
    "f5_en_male": ("en", "default_male", "Natural English male voice"),
    "f5_en_narrator": ("en", "narrator", "Professional English narrator"),
    "f5_en_storyteller": ("en", "storyteller", "Expressive English storyteller"),
}

def _emit(obj: dict) -> None:
    import json
    print(json.dumps(obj), flush=True)


def f5_deps_ready(python: str | None = None) -> bool:
    """Check if F5-TTS and dependencies are installed."""
    py = python or sys.executable
    try:
        r = subprocess.run(
            [py, "-c", "from f5_tts.api import F5TTS; import torch"],
            capture_output=True, text=True, timeout=60, cwd=SAFE_CWD,
        )
        return r.returncode == 0
    except Exception:
        return False


def f5_ready(python: str | None = None) -> bool:
    """Check if F5-TTS model is ready for inference."""
    return f5_deps_ready(python) and os.path.isfile(MODEL_MARKER)


def install_f5(python: str, progress_cb=None) -> bool:
    """Install F5-TTS and download model."""
    def info(msg: str) -> None:
        if progress_cb:
            progress_cb({"type": "info", "message": msg})
        else:
            _emit({"type": "info", "message": msg})

    os.makedirs(F5_DIR, exist_ok=True)
    info("Installing F5-TTS (State-of-the-art multilingual TTS, ~1.5 GB)…")

    # Upgrade pip and install dependencies
    subprocess.run(
        [python, "-m", "ensurepip", "--upgrade"],
        capture_output=True, text=True, timeout=180,
    )
    subprocess.run(
        [python, "-m", "pip", "install", "--upgrade", "pip"],
        check=True, capture_output=True, text=True, timeout=180, cwd=SAFE_CWD,
    )

    # Install F5-TTS and core dependencies
    try:
        subprocess.run(
            [python, "-m", "pip", "install", 
             "f5-tts>=0.2.0", "torch", "torchaudio", "einops>=0.8.0", "librosa"],
            check=True, capture_output=True, text=True, timeout=1200, cwd=SAFE_CWD,
        )
    except subprocess.CalledProcessError as e:
        info(f"F5-TTS installation error: {str(e)[:200]}")
        return False

    # Download and warm up model
    warm_script = f"""
import torch
import os
os.environ['F5_HOME'] = {F5_DIR!r}

try:
    from f5_tts.api import F5TTS
    device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
    if device == "cpu" and torch.cuda.is_available():
        device = "cuda"
    
    # Load model (downloads on first run)
    tts = F5TTS(device=device)
    open({MODEL_MARKER!r}, "w").close()
    print('{{"type": "success", "message": "F5-TTS model ready"}}')
except Exception as e:
    print('{{"type": "error", "message": "F5-TTS warm-up failed: " + str(e)}}')
"""

    try:
        result = subprocess.run(
            [python, "-c", warm_script],
            capture_output=True, text=True, timeout=600, cwd=SAFE_CWD,
        )
        if result.returncode != 0:
            info(f"Model warm-up failed: {result.stderr[:200]}")
            return False
        return f5_ready(python)
    except subprocess.TimeoutExpired:
        info("Model download timeout (very large model, try again)")
        return False


def synthesize_f5(
    text: str,
    output_path: str,
    voice: str,
    speed: float = 1.0,
    python: str | None = None,
) -> None:
    """Synthesize speech using F5-TTS."""
    py = python or sys.executable
    
    if voice not in VOICE_MAP:
        raise ValueError(f"Unknown F5-TTS voice: {voice}")
    
    lang, voice_name, _ = VOICE_MAP[voice]
    ref_file = REF_EN_FEMALE
    ref_text = "Hello, I'm ready to narrate your story. This voice is warm, clear, and natural."
    if lang == "hi":
        if "male" in voice_name or "narrator" in voice_name:
            ref_file = REF_HI_MALE
            ref_text = "नमस्ते, मैं आपकी कहानी का वर्णन करूँगा। यह आवाज़ गर्म और प्राकृतिक है।"
        else:
            ref_file = REF_HI_FEMALE
            ref_text = "नमस्ते, मैं आपकी कहानी सुनाने के लिए तैयार हूँ। यह स्वर बहुत स्वाभाविक और स्पष्ट है।"
    elif "male" in voice_name or "storyteller" in voice_name:
        ref_file = REF_EN_MALE
        ref_text = "Hello, I'll be your narrator for this chapter. This voice is calm and easy to listen to."

    try:
        if not os.path.isfile(ref_file):
            from xtts_tts import bootstrap_refs_with_piper
            bootstrap_refs_with_piper()
    except Exception:
        pass
    if not os.path.isfile(ref_file):
        raise RuntimeError(f"F5 reference voice missing: {ref_file}. Install Piper voice packs first.")
    
    synth_script = f"""
import torch
import os
import sys
os.environ['F5_HOME'] = {F5_DIR!r}

from f5_tts.api import F5TTS

device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
if device == "cpu" and torch.cuda.is_available():
    device = "cuda"

try:
    tts = F5TTS(device=device)
    tts.infer(
        ref_file={ref_file!r},
        ref_text={ref_text!r},
        gen_text={text!r},
        file_wave={output_path!r},
        speed={float(speed)},
    )
    print("Synthesis complete")
except Exception as e:
    print(f"Error: {{str(e)}}", file=sys.stderr)
    sys.exit(1)
"""

    result = subprocess.run(
        [py, "-c", synth_script],
        capture_output=True, text=True, timeout=300, cwd=SAFE_CWD,
    )
    
    if result.returncode != 0:
        raise RuntimeError(f"F5-TTS synthesis failed: {result.stderr}")


def get_status(python: str | None = None) -> dict:
    """Get F5-TTS installation status."""
    py = python or sys.executable
    return {
        "engine": "f5-tts",
        "installed": f5_deps_ready(py),
        "ready": f5_ready(py),
        "voices": list(VOICE_MAP.keys()),
        "description": "State-of-the-art multilingual TTS (F5-TTS)",
    }
