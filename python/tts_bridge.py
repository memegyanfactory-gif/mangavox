"""
TTS Bridge — generates per-segment audio for the pipeline.

Engine selection cascade (first available wins):
  1. Kokoro-82M (bundled) — open-source, local, no API. Best for Hindi on Apple
     Silicon (hf_alpha / hm_omega). Model auto-downloads to ~/.mangavox/models/kokoro
  2. AURA VOICE tts_worker.py (kokoro / piper) if installed externally
  3. macOS `say` command — built-in fallback
  4. Fail with a clear message — no silent failures.
"""
import os
import sys
import json
import shutil
import subprocess
import tempfile

# Path to AURA VOICE TTS worker
TTS_WORKER = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "..", "TTS software", "tts_worker.py"
)

# Fallback: try common locations
if not os.path.exists(TTS_WORKER):
    alt = os.path.expanduser("~/Documents/VS CODE/TTS software/tts_worker.py")
    if os.path.exists(alt):
        TTS_WORKER = alt

def _venv_python(venv_dir):
    """Interpreter inside a venv (Scripts/python.exe on Windows, bin/python3 elsewhere)."""
    if os.name == "nt":
        return os.path.join(venv_dir, "Scripts", "python.exe")
    return os.path.join(venv_dir, "bin", "python3")


# Find the correct Python with TTS dependencies (kokoro, soundfile, numpy)
# The AURA VOICE venv has these installed — system Python does NOT
def _find_tts_python():
    tts_dir = os.path.dirname(TTS_WORKER) if os.path.exists(TTS_WORKER) else os.path.expanduser("~/Documents/VS CODE/TTS software")
    # Check for venv inside aura_voice/
    venv_python = _venv_python(os.path.join(tts_dir, "aura_voice", "venv"))
    if os.path.exists(venv_python):
        return venv_python
    # Check for venv at TTS software root
    venv_python2 = _venv_python(os.path.join(tts_dir, "venv"))
    if os.path.exists(venv_python2):
        return venv_python2
    # Last resort: current Python (likely won't have deps)
    print("[tts] WARNING: No TTS venv found, falling back to sys.executable — TTS will likely fail", file=sys.stderr)
    return sys.executable

TTS_PYTHON = _find_tts_python()

KOKORO_SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "kokoro_tts.py")

_HINDI_VOICE_IDS = frozenset({
    "f5_hi_female", "f5_hi_male", "f5_hi_storyteller", "f5_hi_narrator",
    "parler_hi_female_clear", "parler_hi_female_warm", "parler_hi_male_deep", "parler_hi_male_bright",
    "hf_alpha", "hf_beta", "hm_omega", "hm_psi",
    "xtts_hi_female", "xtts_hi_male", "xtts_hi_storyteller", "xtts_hi_narrator",
    "hi_piper_f", "hi_piper_m", "hi_piper_rohan",
    "hi-lekha", "hi-female", "hi-male", "hi-male-aman", "hi-male-deep",
})

_TTS_VENV_PYTHON = _venv_python(os.path.join(os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox"), "tts-venv"))


def _tts_subprocess_python() -> str:
    if os.path.isfile(_TTS_VENV_PYTHON):
        return _TTS_VENV_PYTHON
    return sys.executable


def _run_kokoro_json(args):
    """Run kokoro_tts.py using the managed TTS venv when present."""
    if not os.path.isfile(KOKORO_SCRIPT):
        raise FileNotFoundError(f"kokoro_tts.py not found at {KOKORO_SCRIPT}")
    result = subprocess.run(
        [_tts_subprocess_python(), KOKORO_SCRIPT, *args],
        capture_output=True, text=True, timeout=60,
        cwd=os.path.dirname(KOKORO_SCRIPT),
    )
    last_line = result.stdout.strip().splitlines()[-1] if result.stdout.strip() else ""
    try:
        data = json.loads(last_line) if last_line else {}
    except Exception:
        data = {}
    if result.returncode != 0:
        msg = data.get("message") if isinstance(data, dict) else None
        raise RuntimeError(msg or result.stderr or result.stdout or f"kokoro_tts exited {result.returncode}")
    return data


def _kokoro_local_available():
    """Bundled local TTS (Kokoro or Piper) with downloaded model files."""
    if not os.path.isfile(KOKORO_SCRIPT):
        return False
    try:
        data = _run_kokoro_json(["status"])
        return bool(data.get("installed"))
    except Exception:
        return False


def _local_tts_supports_voice(voice):
    """True when bundled kokoro_tts.py can synthesize this voice id."""
    if not voice or not os.path.isfile(KOKORO_SCRIPT):
        return False
    try:
        data = _run_kokoro_json(["voice-ready", voice])
        return bool(data.get("ready"))
    except Exception:
        return False


def _is_hindi_voice(voice):
    v = (voice or "").lower()
    return (
        v in _HINDI_VOICE_IDS
        or v.startswith(("hf_", "hm_", "xtts_hi_", "hi_piper", "f5_hi_", "parler_hi_"))
    )


# ─────────────────────────────────────────────────────────────────────────────
# Engine cascade — AURA → macOS say → fail
# ─────────────────────────────────────────────────────────────────────────────

def _aura_available():
    return os.path.exists(TTS_WORKER)


def _say_available():
    """macOS built-in `say` command. Always present on macOS, never elsewhere."""
    return sys.platform == "darwin" and shutil.which("say") is not None


def resolve_tts_engine(requested, voice=None):
    """
    Pick the engine that will actually run. Returns: piper | kokoro | xtts | say | aura.
  """
    voice = voice or ""
    requested_l = (requested or "").lower()

    if os.path.isfile(KOKORO_SCRIPT):
        try:
            ready = _run_kokoro_json(["voice-ready", voice or "hf_alpha"])
            eng = ready.get("engine") if ready.get("ready") else None
            if eng:
                return eng
        except Exception as exc:
            print(f"[tts] voice-ready failed: {exc}", file=sys.stderr)

    if requested_l in ("say", "macos") and _say_available():
        return "say"
    if requested_l in ("kokoro", "piper", "aura", "xtts") and _aura_available():
        return "aura"
    if _say_available():
        return "say"
    if _aura_available():
        return "aura"
    raise RuntimeError(
        "No TTS engine available. Open Audio → Download all voice packs, "
        "or use macOS system voices."
    )


def _resolved_engine(requested, voice=None):
    """Backward-compatible alias — prefer resolve_tts_engine."""
    return resolve_tts_engine(requested, voice)


def _synthesize_local(text, output_path, voice, speed):
    """Bundled Piper/Kokoro/XTTS via kokoro_tts.py (with XTTS→Piper fallback)."""
    _generate_audio_kokoro(text, output_path, voice, speed)


def _wav_is_usable(path, min_bytes=3500, min_sec=0.2):
    """True when an existing WAV looks like real speech, not silence or a stub."""
    try:
        if not os.path.isfile(path) or os.path.getsize(path) < min_bytes:
            return False
        import wave
        with wave.open(path, "rb") as wf:
            return (wf.getnframes() / max(wf.getframerate(), 1)) >= min_sec
    except Exception:
        return False


def run_tts(text_segments, output_dir, config=None, progress_cb=None):
    """
    Generate audio for each text segment. Returns list of dicts:
      [{ "page": int, "path": str|None, "text": str, "error": str? }]
    """
    config = config or {}
    requested_engine = config.get("engine", "kokoro")
    voice = config.get("voice", "af_heart")
    speed = config.get("speed", 1.0)
    force_regen = bool(config.get("force_regen", False))

    try:
        engine = resolve_tts_engine(requested_engine, voice)
    except RuntimeError as exc:
        print(f"[tts] FATAL: {exc}", file=sys.stderr)
        return []

    ready_info = {}
    try:
        ready_info = _run_kokoro_json(["voice-ready", voice])
    except Exception:
        ready_info = {}
    preview_voice = ready_info.get("previewVoice")
    if ready_info.get("previewFallback") and preview_voice and preview_voice != voice:
        print(f"[tts] Using engine: {engine} ({voice} → installed stand-in {preview_voice})", file=sys.stderr)
    else:
        print(f"[tts] Using engine: {engine} (requested: {requested_engine}, voice: {voice})", file=sys.stderr)

    audio_dir = os.path.join(output_dir, "audio")
    os.makedirs(audio_dir, exist_ok=True)

    audio_files = []
    for i, segment in enumerate(text_segments):
        if progress_cb:
            progress_cb("tts", i + 1, len(text_segments))

        text = (segment.get("text") or "").strip()
        page = segment["page"]
        output_path = os.path.join(audio_dir, f"page_{page:04d}.wav")

        # Empty pages — emit a tiny silence file rather than skipping (so the
        # video stage's per-page timing doesn't drift)
        if not text:
            try:
                _write_silence(output_path, seconds=0.6)
                audio_files.append({"page": page, "path": output_path, "text": "", "engine": engine})
            except Exception as e:
                audio_files.append({"page": page, "path": None, "text": "", "error": str(e)})
            continue

        if not force_regen and _wav_is_usable(output_path):
            audio_files.append({"page": page, "path": output_path, "text": text, "engine": engine})
            continue

        try:
            if engine in ("piper", "kokoro", "xtts"):
                _synthesize_local(text, output_path, voice, speed)
            elif engine == "aura":
                _generate_audio_aura(text, output_path, requested_engine, voice, speed)
            else:  # say
                _generate_audio_say(text, output_path, voice, speed)
            audio_files.append({"page": page, "path": output_path, "text": text, "engine": engine})
        except Exception as e:
            print(f"[tts] Failed for page {page}: {e}", file=sys.stderr)
            audio_files.append({"page": page, "path": None, "text": text, "error": str(e)})

    return audio_files


def _write_silence(output_path, seconds=0.5):
    """Write a tiny silent WAV — pure stdlib so we don't pull deps."""
    import wave
    import struct
    framerate = 22050
    nframes = int(framerate * seconds)
    with wave.open(output_path, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(framerate)
        f.writeframes(b"\x00\x00" * nframes)


def _say_installed_voices():
    """Return the set of voice names `say` actually has installed."""
    try:
        out = subprocess.run(["say", "-v", "?"], capture_output=True, text=True, timeout=15)
        names = set()
        for line in out.stdout.splitlines():
            # Format: "Lekha               hi_IN    # ..."
            parts = line.split()
            if parts:
                names.add(parts[0])
        return names
    except Exception:
        return set()


# Logical voice id → (macOS `say` voice, pitch-baseline embedded command).
# macOS only ships ONE Hindi voice (Lekha, female). For a male Hindi voice we
# reuse Lekha with a lowered pitch baseline ([[pbas N]]), which `say` honours —
# it deepens the timbre so it reads as masculine. Kokoro voice ids that reach
# the `say` fallback are mapped to a sensible same-gender system voice.
_SAY_VOICE_MAP = {
    # Narration is English, so a genuine MALE Indian-English voice (Rishi/Aman)
    # reads far better than a pitch-shifted Hindi female. Fall back to a
    # deepened Lekha only if those aren't installed (handled by validation).
    "hi-male":      ("Rishi", None),    # Indian-English male
    "hi-male-aman": ("Aman", None),
    "hi-male-deep": ("Lekha", 20),      # deepened Hindi female (for Devanagari)
    "hi-lekha":     ("Lekha", None),    # Hindi female
    "hi-female":    ("Lekha", None),
    # Kokoro fallbacks (when AURA isn't installed) — keep gender right.
    "af_heart": ("Samantha", None), "af_bella": ("Samantha", None),
    "af_nova": ("Samantha", None), "bf_emma": ("Daniel", None),
    "am_michael": ("Daniel", None), "am_adam": ("Fred", None),
    "say-default": ("Samantha", None),
}
# Preference order for the male-Hindi fallback if the mapped voice is missing.
_HI_MALE_FALLBACKS = ["Rishi", "Aman", "Daniel", "Fred"]


def _generate_audio_say(text, output_path, voice, speed):
    """
    Use macOS `say` to synth → AIFF, then `afconvert` to WAV. Both ship with
    macOS, no install required.
    """
    pbas = None
    mapped = _SAY_VOICE_MAP.get(voice)
    if mapped:
        say_voice, pbas = mapped
    elif voice and not voice.startswith(("af_", "am_", "bf_")):
        # A literal system voice name (e.g. "Lekha", "Daniel") passed straight through.
        say_voice = voice
    else:
        say_voice = "Samantha"

    # Validate the chosen voice is actually installed — otherwise `say -v X`
    # exits non-zero and we'd surface a confusing "no output file" error. Fall
    # back to a guaranteed-present default. (This is the common crash cause.)
    installed = _say_installed_voices()
    if installed and say_voice not in installed:
        # For a male voice request, prefer another installed MALE voice before
        # giving up to Samantha (female) — otherwise a "male" pick sounds female.
        wants_male = voice in ("hi-male", "hi-male-aman", "am_michael", "am_adam") or say_voice in _HI_MALE_FALLBACKS
        fallback = None
        if wants_male:
            for cand in _HI_MALE_FALLBACKS:
                if cand in installed:
                    fallback = cand; break
            # Last resort for male: deepen Lekha so it at least isn't female.
            if not fallback and "Lekha" in installed:
                fallback, pbas = "Lekha", 18
        if not fallback:
            fallback = "Samantha" if "Samantha" in installed else (next(iter(installed)) if installed else None)
            pbas = None
        print(f"[tts] voice '{say_voice}' not installed; falling back to '{fallback}'", file=sys.stderr)
        say_voice = fallback or say_voice

    rate = max(80, min(400, int(175 * (float(speed) or 1.0))))
    # Prepend a pitch-baseline command for the deepened (male) Hindi variants.
    spoken = f"[[pbas {pbas}]] {text}" if pbas is not None else text

    aiff = output_path + ".aiff"
    try:
        subprocess.run(
            ["say", "-v", say_voice, "-r", str(rate), "-o", aiff, "--", spoken],
            check=True, timeout=180, capture_output=True, text=True,
        )
        subprocess.run(
            ["afconvert", "-f", "WAVE", "-d", "LEI16@22050", "-c", "1", aiff, output_path],
            check=True, timeout=60, capture_output=True, text=True,
        )
    except subprocess.CalledProcessError as e:
        raise RuntimeError(f"`say` failed (voice={say_voice}): {e.stderr or e}")
    finally:
        if os.path.exists(aiff):
            try: os.unlink(aiff)
            except OSError: pass

    if not os.path.exists(output_path):
        raise RuntimeError("`say` produced no output file")


def _generate_audio_kokoro(text, output_path, voice, speed):
    """Bundled Kokoro-82M — local ONNX, no API, Hindi + English."""
    # Attempt to hit the memory caching HTTP daemon on port 5566 first
    import urllib.request
    import urllib.error
    
    server_url = "http://127.0.0.1:5566/synthesize"
    post_data = json.dumps({
        "text": text,
        "voice": voice,
        "output_path": output_path,
        "speed": speed
    }).encode("utf-8")
    
    req = urllib.request.Request(
        server_url,
        data=post_data,
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    
    try:
        with urllib.request.urlopen(req, timeout=90) as resp:
            if resp.status == 200:
                res_body = json.loads(resp.read().decode("utf-8"))
                if res_body.get("ok") and os.path.exists(output_path):
                    # Instant synthesis succeeded!
                    return
    except Exception as e:
        # Fall back to standard subprocess if offline or server fails
        pass

    if not os.path.isfile(KOKORO_SCRIPT):
        raise FileNotFoundError(f"kokoro_tts.py not found at {KOKORO_SCRIPT}")

    args = {"text": text, "voice": voice, "output_path": output_path, "speed": speed}
    args_file = tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False)
    json.dump(args, args_file)
    args_file.close()

    # The managed voice venv (Scripts\python.exe on Windows, bin/python3 elsewhere).
    kokoro_py = _tts_subprocess_python()

    try:
        result = subprocess.run(
            [kokoro_py, KOKORO_SCRIPT, "synthesize", args_file.name],
            capture_output=True, text=True, timeout=600,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        if result.returncode != 0:
            err = result.stderr or result.stdout or "Kokoro synthesis failed"
            for line in reversed((result.stdout or "").splitlines()):
                try:
                    msg = json.loads(line)
                    if msg.get("type") == "error":
                        err = msg.get("message", err)
                        break
                except json.JSONDecodeError:
                    continue
            raise RuntimeError(err[:500])
        if not os.path.exists(output_path):
            raise RuntimeError("Kokoro did not produce output file")
    finally:
        os.unlink(args_file.name)


def _generate_audio_aura(text, output_path, engine, voice, speed):
    """Call AURA VOICE tts_worker.py to generate audio."""
    if not os.path.exists(TTS_WORKER):
        raise FileNotFoundError(
            f"AURA VOICE tts_worker.py not found at {TTS_WORKER}. "
            "Please ensure the TTS software is installed."
        )

    # Create args file for tts_worker
    args = {
        "engine": engine,
        "text": text,
        "voice": voice,
        "output_path": output_path,
        "speed": speed,
    }

    args_file = tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False)
    json.dump(args, args_file)
    args_file.close()

    try:
        result = subprocess.run(
            [TTS_PYTHON, TTS_WORKER, args_file.name],
            capture_output=True,
            text=True,
            timeout=120,
            cwd=os.path.dirname(TTS_WORKER),
        )

        if result.returncode != 0:
            raise RuntimeError(f"TTS worker failed: {result.stderr[:500]}")

        # Check output
        stdout = result.stdout.strip()
        if stdout.startswith("done:"):
            actual_path = stdout.split("done:", 1)[1].strip()
            if actual_path != output_path and os.path.exists(actual_path):
                os.rename(actual_path, output_path)

        if not os.path.exists(output_path):
            raise RuntimeError("TTS worker did not produce output file")

    finally:
        os.unlink(args_file.name)


# Short phrases for voice preview in the Audio tab UI
VOICE_SAMPLE_TEXT = {
    "f5_hi_female": "नमस्ते, यह आवाज़ गर्म, स्वाभाविक और बिल्कुल मानवीय कहानीकार जैसी है।",
    "f5_hi_male": "नमस्ते, मैं आपकी कहानी शांत, स्पष्ट और प्राकृतिक स्वर में सुनाऊँगा।",
    "f5_hi_storyteller": "नमस्ते, यह सिनेमैटिक कहानी सुनाने के लिए एक बेहद प्राकृतिक हिंदी आवाज़ है।",
    "f5_hi_narrator": "नमस्ते, मैं लंबे अध्याय को साफ और मानवीय अंदाज़ में सुनाऊँगा।",
    "f5_en_female": "Hello, this is a warm, natural studio voice for expressive storytelling.",
    "f5_en_male": "Hello, I'll narrate your chapter with a calm, realistic human tone.",
    "f5_en_narrator": "Hello, this is a polished audiobook narrator for long listening sessions.",
    "f5_en_storyteller": "Hello, this voice brings dramatic scenes to life with natural pacing.",
    "parler_hi_female_clear": "नमस्ते, यह साफ, सहज और प्राकृतिक हिंदी महिला आवाज़ है।",
    "parler_hi_female_warm": "नमस्ते, यह गर्म और भावनात्मक कहानी सुनाने वाली हिंदी आवाज़ है।",
    "parler_hi_male_deep": "नमस्ते, यह गहरी और पेशेवर हिंदी पुरुष आवाज़ है।",
    "parler_hi_male_bright": "नमस्ते, यह स्पष्ट, ऊर्जावान और प्राकृतिक हिंदी पुरुष आवाज़ है।",
    "parler_en_female_neutral": "Hello, this is a clean and natural English voice with steady pacing.",
    "parler_en_female_warm": "Hello, this warm English voice is made for comfortable narration.",
    "parler_en_male_narrator": "Hello, this deep narrator voice gives your chapter a premium feel.",
    "parler_en_male_casual": "Hello, this casual voice sounds friendly, human, and easy to follow.",
    "xtts_hi_female": "नमस्ते, यह अति स्वाभाविक आवाज़ है — जैसे कोई वास्तविक कहानीकार बोल रहा हो।",
    "xtts_hi_male": "नमस्ते, मैं आपकी कहानी सुनाऊँगा। यह स्वर गर्म और बिल्कुल प्राकृतिक है।",
    "xtts_hi_storyteller": "नमस्ते, यह आपकी कहानी का सबसे प्राकृतिक स्वर है।",
    "xtts_hi_narrator": "नमस्ते, मैं आपके अध्याय का वर्णन करूँगा — बहुत ही मानवीय आवाज़ में।",
    "xtts_en_female": "Hello, this is ultra-natural speech — warm, human, and perfect for storytelling.",
    "xtts_en_male": "Hello, I'll narrate your chapter with a calm, natural human voice.",
    "xtts_en_narrator": "Hello, this voice sounds like a real audiobook narrator.",
    "xtts_en_storyteller": "Hi there, this is how your manga story will sound with cinematic narration.",
    "xtts_en_warm": "Hello, this warm voice brings your pages to life naturally.",
    "xtts_en_clear": "Hello, this clear male voice is easy to listen to for long chapters.",
    "hf_alpha": "नमस्ते, यह आपकी कहानी का स्वर है।",
    "hf_beta": "नमस्ते, यह कोकोरो की दूसरी महिला आवाज़ है — कोमल और स्पष्ट।",
    "hm_omega": "नमस्ते, मैं आपकी कहानी सुनाऊँगा।",
    "hm_psi": "नमस्ते, यह गहरी पुरुष आवाज़ में आपकी कहानी है।",
    "hi_piper_f": "नमस्ते, यह प्रियंवादा की आवाज़ है।",
    "hi_piper_m": "नमस्ते, यह प्रथम की पुरुष आवाज़ है।",
    "hi_piper_rohan": "नमस्ते, यह रोहन की आवाज़ में आपकी कहानी है।",
    "af_heart": "Hello, this is how your manga story will sound — warm and natural.",
    "af_bella": "Hi there, ready to bring your pages to life with a bright voice.",
    "af_nova": "Hello, this energetic voice keeps your narration engaging.",
    "af_sky": "Hello, this soft voice is perfect for emotional scenes.",
    "af_nicole": "Hi, this smooth American voice sounds clear and human.",
    "af_jessica": "Hello, this friendly voice makes every line easy to follow.",
    "am_michael": "Hello, I'm your confident narrator for this chapter.",
    "am_adam": "Hello, this deep male voice adds weight to your story.",
    "am_echo": "Hello, this calm voice is ideal for long listening sessions.",
    "am_eric": "Hello, I'll guide you through the story with a steady tone.",
    "am_liam": "Hello, this young male voice feels fresh and natural.",
    "bf_emma": "Hello, this British voice will guide you through the story.",
    "bf_isabella": "Hello, this elegant British tone suits dramatic narration.",
    "bm_george": "Hello, this British male voice sounds refined and clear.",
    "en_piper_f": "Hello, this is Amy — a clear, natural English female voice.",
    "en_piper_m": "Hello, this is Lessac — a warm English male narrator.",
    "en_piper_f2": "Hello, this is Kristin — smooth and easy to listen to.",
    "en_piper_m2": "Hello, this is Ryan — a natural American male voice.",
    "hi-lekha": "नमस्ते, यह लेखा की आवाज़ है।",
    "hi-male": "Hello, this is the Rishi voice for your narration.",
    "say-default": "Hello, this is the default macOS system voice.",
}


def preview_voice(output_path, voice, speed=1.0, requested_engine=None):
    """Synthesize a short sample clip for voice selection UI."""
    text = VOICE_SAMPLE_TEXT.get(voice)
    if not text:
        text = VOICE_SAMPLE_TEXT["hf_alpha" if _is_hindi_voice(voice) else "af_heart"]

    parent = os.path.dirname(os.path.abspath(output_path)) or "."
    os.makedirs(parent, exist_ok=True)

    voice_l = (voice or "").lower()
    is_system = voice_l in ("hi-lekha", "hi-male", "say-default") or requested_engine == "say"

    if is_system and _say_available():
        _generate_audio_say(text, output_path, voice, speed)
        return {"path": output_path, "engine": "say", "text": text}

    if os.path.isfile(KOKORO_SCRIPT):
        try:
            ready = _run_kokoro_json(["voice-ready", voice])
            engine = ready.get("engine") if ready.get("ready") else None
            if not engine:
                raise RuntimeError(
                    "No AI voice model installed — open Audio tab and click Download AI Voice Models"
                )
            synth_voice = ready.get("previewVoice") or voice
            preview_fallback = bool(ready.get("previewFallback") and synth_voice != voice)
            _generate_audio_kokoro(text, output_path, synth_voice, speed)
            if not os.path.isfile(output_path) or os.path.getsize(output_path) < 100:
                raise RuntimeError("Preview synthesis produced no audio")
            return {
                "path": output_path,
                "engine": engine,
                "text": text,
                "previewFallback": preview_fallback,
                "synthVoice": synth_voice,
            }
        except Exception:
            pass

    engine = _resolved_engine(requested_engine, voice)
    if engine == "kokoro":
        _generate_audio_kokoro(text, output_path, voice, speed)
    elif engine == "aura":
        _generate_audio_aura(text, output_path, requested_engine or "kokoro", voice, speed)
    else:
        _generate_audio_say(text, output_path, voice, speed)

    if not os.path.isfile(output_path) or os.path.getsize(output_path) < 100:
        raise RuntimeError("Preview synthesis produced no audio")

    return {"path": output_path, "engine": engine, "text": text}
