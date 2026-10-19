#!/usr/bin/env python3
"""Indic Parler-TTS — natural open-source Hindi + Indian English voices."""
from __future__ import annotations

import os
import subprocess
import sys
import json

# The app passes its data folder (Settings → Storage can move it).
BASE_DIR = os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")
PARLER_DIR = os.path.join(BASE_DIR, "models", "indic-parler-tts")
MODEL_MARKER = os.path.join(PARLER_DIR, ".model_ready")
SAFE_CWD = BASE_DIR
PARLER_MODEL = os.environ.get("MANGAVOX_PARLER_MODEL", "parler-tts/parler_tts_mini_v0.1")

# Voice ID -> description_text. Use named speakers where the model supports
# them so presets produce genuinely different voices, not only label changes.
VOICE_MAP: dict[str, tuple[str, str]] = {
    "parler_hi_female_clear": (
        "hi",
        "Divya speaks in Hindi with a clear, close-sounding voice, natural pitch, moderate pace, and very clear studio-quality audio with almost no background noise."
    ),
    "parler_hi_female_warm": (
        "hi",
        "Rani speaks in Hindi with a warm, expressive storyteller voice, gentle emotion, balanced pitch, moderate pace, and very clear studio-quality audio."
    ),
    "parler_hi_male_deep": (
        "hi",
        "Rohit speaks in Hindi with a deep, calm narrator voice, close microphone sound, moderate pitch, natural pace, and very clear studio-quality audio."
    ),
    "parler_hi_male_bright": (
        "hi",
        "Aman speaks in Hindi with a youthful, bright male voice, slightly expressive delivery, moderate pace, and very clear studio-quality audio."
    ),
    "parler_en_female_neutral": (
        "en",
        "Mary speaks in Indian English with clear articulation, balanced pitch, moderate pace, and very clear studio-quality audio."
    ),
    "parler_en_female_warm": (
        "en",
        "Meera speaks in Indian English with a warm, gentle storyteller voice, close microphone sound, natural emotion, and very clear studio-quality audio."
    ),
    "parler_en_male_narrator": (
        "en",
        "Thoma speaks in Indian English with a deep professional narrator voice, moderate pace, confident tone, and very clear studio-quality audio."
    ),
    "parler_en_male_casual": (
        "en",
        "Kabir speaks in Indian English with a friendly conversational male voice, natural pace, close recording, and very clear studio-quality audio."
    ),
}

def _emit(obj: dict) -> None:
    import json
    print(json.dumps(obj), flush=True)


def parler_deps_ready(python: str | None = None) -> bool:
    """Check if Parler-TTS and dependencies are installed."""
    py = python or sys.executable
    try:
        r = subprocess.run(
            [py, "-c", "from parler_tts import ParlerTTSForConditionalGeneration; from transformers import AutoTokenizer; import soundfile; import torch"],
            capture_output=True, text=True, timeout=60, cwd=SAFE_CWD,
        )
        return r.returncode == 0
    except Exception:
        return False


def parler_ready(python: str | None = None) -> bool:
    """Check if Parler-TTS model is ready for inference."""
    return parler_deps_ready(python) and os.path.isfile(MODEL_MARKER)


def install_parler(python: str, progress_cb=None) -> bool:
    """Install Parler-TTS and download model."""
    def info(msg: str) -> None:
        if progress_cb:
            progress_cb({"type": "info", "message": msg})
        else:
            _emit({"type": "info", "message": msg})

    os.makedirs(PARLER_DIR, exist_ok=True)
    info("Installing Parler-TTS (Fast controllable TTS, ~1 GB)…")

    # Upgrade pip
    subprocess.run(
        [python, "-m", "ensurepip", "--upgrade"],
        capture_output=True, text=True, timeout=180,
    )
    subprocess.run(
        [python, "-m", "pip", "install", "--upgrade", "pip"],
        check=True, capture_output=True, text=True, timeout=180, cwd=SAFE_CWD,
    )

    # Install Parler-TTS and dependencies. If the runtime is already usable,
    # skip this so repeated installs do not disturb the shared voice venv.
    if not parler_deps_ready(python):
        try:
            subprocess.run(
                [python, "-m", "pip", "install",
                 "git+https://github.com/huggingface/parler-tts.git", "torch", "torchaudio", "transformers==4.46.1", "fsspec<=2026.2.0", "soundfile", "accelerate"],
                check=True, capture_output=True, text=True, timeout=1200, cwd=SAFE_CWD,
            )
        except subprocess.CalledProcessError as e:
            details = (e.stderr or e.stdout or str(e))[-600:]
            info(f"Parler-TTS installation error: {details}")
            return False

    # Warm up model
    warm_script = f"""
import torch
import os
os.environ['HF_HOME'] = {PARLER_DIR!r}

try:
    from parler_tts import ParlerTTSForConditionalGeneration
    from transformers import AutoTokenizer
    
    device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
    if device == "cpu" and torch.cuda.is_available():
        device = "cuda"
    
    model = ParlerTTSForConditionalGeneration.from_pretrained({PARLER_MODEL!r}).to(device)
    tokenizer = AutoTokenizer.from_pretrained({PARLER_MODEL!r})
    AutoTokenizer.from_pretrained(model.config.text_encoder._name_or_path)
    
    open({MODEL_MARKER!r}, "w").close()
    print('{{"type": "success", "message": "Parler-TTS model ready"}}')
except Exception as e:
    print('{{"type": "error", "message": "Parler-TTS warm-up failed: " + str(e)}}')
"""

    try:
        result = subprocess.run(
            [python, "-c", warm_script],
            capture_output=True, text=True, timeout=600, cwd=SAFE_CWD,
        )
        if result.returncode != 0:
            info(f"Model warm-up failed: {result.stderr[:200]}")
            return False
        return parler_ready(python)
    except subprocess.TimeoutExpired:
        info("Model download timeout")
        return False


def synthesize_parler(
    text: str,
    output_path: str,
    voice: str,
    speed: float = 1.0,
    python: str | None = None,
) -> None:
    """Synthesize speech using Parler-TTS."""
    py = python or sys.executable
    
    if voice not in VOICE_MAP:
        raise ValueError(f"Unknown Parler-TTS voice: {voice}")
    
    lang, description = VOICE_MAP[voice]
    
    # Adjust description for speed
    if speed > 1.1:
        description += " Speaking quickly, with an increased pace."
    elif speed < 0.9:
        description += " Speaking slowly and deliberately."
    
    synth_script = f"""
import torch
import os
import sys
import soundfile as sf
os.environ['HF_HOME'] = {PARLER_DIR!r}

from parler_tts import ParlerTTSForConditionalGeneration
from transformers import AutoTokenizer

device = "mps" if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available() else "cpu"
if device == "cpu" and torch.cuda.is_available():
    device = "cuda"

try:
    model = ParlerTTSForConditionalGeneration.from_pretrained({PARLER_MODEL!r}).to(device)
    tokenizer = AutoTokenizer.from_pretrained({PARLER_MODEL!r})
    description_tokenizer = AutoTokenizer.from_pretrained(model.config.text_encoder._name_or_path)
    
    description = {description!r}
    description_inputs = description_tokenizer(description, return_tensors="pt").to(device)
    
    prompt_inputs = tokenizer({text!r}, return_tensors="pt").to(device)
    generation = model.generate(
        input_ids=description_inputs.input_ids,
        attention_mask=description_inputs.attention_mask,
        prompt_input_ids=prompt_inputs.input_ids,
        prompt_attention_mask=prompt_inputs.attention_mask,
    )
    audio_arr = generation.cpu().numpy().squeeze()
    
    sf.write({output_path!r}, audio_arr, model.config.sampling_rate)
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
        raise RuntimeError(f"Parler-TTS synthesis failed: {result.stderr}")


def get_status(python: str | None = None) -> dict:
    """Get Parler-TTS installation status."""
    py = python or sys.executable
    return {
        "engine": "parler-tts",
        "installed": parler_deps_ready(py),
        "ready": parler_ready(py),
        "voices": list(VOICE_MAP.keys()),
        "description": "Fast, controllable open-source TTS (Parler-TTS)",
    }
