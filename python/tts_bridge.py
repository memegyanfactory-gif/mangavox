"""
TTS Bridge — generates per-segment audio for the pipeline.

Narration follows the story: each page's scene (kind, mood, intensity) drives the delivery
(see narration_tts.py). Chatterbox runs as one long-lived worker per job; the chapter is
loudness-normalised to about -16 LUFS and `audio/narration_params.json` records what each
segment was given.

Engine selection cascade (first available wins):
  1. kokoro_tts.py (bundled): the voice's own engine (F5-TTS, Parler-TTS, XTTS, Kokoro,
     Piper) or its stand-in voice (`voice-ready`). Models come from Settings → Audio;
     nothing is downloaded during a job.
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
import time

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

    # kokoro_tts.py knows exactly what this voice lacks; its message beats a generic one.
    detail = None
    if os.path.isfile(KOKORO_SCRIPT):
        try:
            ready = _run_kokoro_json(["voice-ready", voice or "hf_alpha"])
            eng = ready.get("engine") if ready.get("ready") else None
            if eng and eng != "none":
                return eng
            detail = ready.get("message")
        except Exception as exc:
            print(f"[tts] voice-ready failed: {exc}", file=sys.stderr)
            detail = f"The voice check failed: {exc}"

    if requested_l in ("say", "macos") and _say_available():
        return "say"
    if requested_l in ("kokoro", "piper", "aura", "xtts") and _aura_available():
        return "aura"
    if _say_available():
        return "say"
    if _aura_available():
        return "aura"
    raise RuntimeError(detail or _NO_ENGINE_MESSAGE)


_NO_ENGINE_MESSAGE = (
    'No voice engine is installed. In Settings → Audio, download "Kokoro v1.0" '
    "(recommended); its engine installs with it."
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


def _num(v, default):
    try:
        f = float(v)
        return f if f == f else default
    except (TypeError, ValueError):
        return default


def run_tts(text_segments, output_dir, config=None, progress_cb=None):
    """
    Generate audio for each text segment. Returns list of dicts:
      [{ "page": int, "path": str|None, "text": str, "error": str? }]

    Each page is voiced with the emotion of its scene (narration_tts.build_plan: read.json
    scenes, per-beat styles, smoothed intensity). Chatterbox gets exaggeration / cfg_weight /
    temperature per chunk from one long-lived worker; Kokoro and Piper get a speed. The whole
    chapter is then loudness-normalised to about -16 LUFS, and the chosen parameters are
    written to `audio/narration_params.json`.

    config: engine, voice, speed, force_regen, emotion_strength (0-2, default 1),
            voice_reference (a WAV a Chatterbox voice clones instead of its own clip),
            regen_pages (page indices voiced again even when their clip exists; Studio's
            per-page Regenerate / New take, the other pages are reused).

    A page's take (`audio/takes.json`) is mixed into its chunk seeds, so a new take is a new
    performance. A page voiced again is written next to its clip and only replaces it once it
    came out usable, so a failed take never loses the clip that was there.
    """
    config = config or {}
    requested_engine = config.get("engine", "kokoro")
    voice = config.get("voice", "af_heart")
    speed = _num(config.get("speed", 1.0), 1.0) or 1.0
    force_regen = bool(config.get("force_regen", False))
    regen = set()
    for p in config.get("regen_pages") or []:
        try:
            regen.add(int(p))
        except (TypeError, ValueError):
            pass
    # Studio's "New take": the regen pages move to their next take. It is only written to
    # audio/takes.json once the new clip is in place, so a failed take leaves the page's clip
    # and its take number as they were.
    new_take = bool(config.get("new_take")) and bool(regen)
    # Only these pages are voiced again; the rest of the chapter is reused as it is.
    pages_only = bool(regen) and not force_regen
    strength = config.get("emotion_strength")
    if strength is None:
        # The chapter's own setting (mv_studio::voices::set_emotion_strength).
        try:
            with open(os.path.join(output_dir, "narration_style.json"), "r", encoding="utf-8") as f:
                strength = json.load(f).get("emotion_strength")
        except Exception:
            strength = None
    strength = max(0.0, min(2.0, _num(strength, 1.0)))

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
    synth_voice = ready_info.get("synthVoice") or ready_info.get("previewVoice") or voice
    if ready_info.get("previewFallback") and synth_voice != voice:
        print(f"[tts] Using engine: {engine} ({voice} → installed stand-in {synth_voice})", file=sys.stderr)
    else:
        print(f"[tts] Using engine: {engine} (requested: {requested_engine}, voice: {voice})", file=sys.stderr)

    audio_dir = os.path.join(output_dir, "audio")
    os.makedirs(audio_dir, exist_ok=True)
    _discard_fresh(audio_dir)

    import narration_tts as nt
    takes = nt.load_takes(output_dir)
    if new_take:
        takes = {**takes, **{p: takes.get(p, 0) + 1 for p in regen}}
    try:
        plan = nt.build_plan(text_segments, output_dir, strength)
    except Exception as exc:
        print(f"[tts] emotion plan unavailable ({exc}); narrating neutrally", file=sys.stderr)
        plan = []
        for s in text_segments:
            t = (s.get("text") or "").strip()
            units = [{"text": t, "kind": "other", "mood": "", "intensity": 3.0, "raw_intensity": None,
                      "line_kind": "narration", "source": "default"}] if t else []
            plan.append({"page": int(s.get("page", 0) or 0), "text": t, "units": units})

    log = {"engine": engine, "voice": voice, "synthVoice": synth_voice, "speed": speed,
           "emotionStrength": strength, "takes": {str(k): v for k, v in takes.items()}, "pages": []}
    if regen:
        log["regenPages"] = sorted(regen)
    audio_files = None
    if engine == "chatterbox":
        _announce_engine(engine, voice, synth_voice)
        try:
            audio_files = _run_chatterbox(plan, audio_dir, synth_voice, speed, strength, force_regen, progress_cb, log,
                                          config.get("voice_reference"), regen, takes)
        except RuntimeError as exc:
            if pages_only:
                # Only a page or two were to be voiced again: a stand-in voice there would not
                # match the Chatterbox pages around it, so those pages keep their clips.
                raise RuntimeError(f"Chatterbox could not run ({str(exc)[:200]}). "
                                   "Pages that had a clip keep it.") from exc
            # The engine would not start: the chapter is voiced by the stand-in instead.
            stand_in = _stand_in_for(synth_voice)
            print(f"[tts] Chatterbox unavailable ({exc}); using {stand_in}", file=sys.stderr)
            # A protocol line for the job's status (pipeline_worker's stdout), so the swap is seen.
            _announce_engine("kokoro", voice, stand_in, reason=f"Chatterbox could not run ({str(exc)[:200]})")
            log["chatterboxError"] = str(exc)
            engine, synth_voice = "kokoro", stand_in
            log.update(engine=engine, synthVoice=synth_voice, pages=[])
    else:
        _announce_engine(engine, voice, synth_voice)
    if audio_files is None:
        audio_files = _run_basic(plan, audio_dir, engine, requested_engine, synth_voice, speed, strength,
                                 force_regen, progress_cb, log, regen)

    # Empty pages: a short silence at the chapter's sample rate (so concatenation works and
    # the video's per-page timing does not drift).
    rate = _chapter_rate(audio_files, engine)
    for a in audio_files:
        if a.pop("_empty", False):
            try:
                _write_silence(a["path"], seconds=0.6, framerate=rate)
            except Exception as e:
                a.update(path=None, error=str(e))

    spoken = [a["path"] for a in audio_files if a.get("path") and a.get("text") and os.path.isfile(a["path"])]
    if spoken:
        log["loudness"] = _normalize_chapter_pages(audio_dir, spoken, engine, synth_voice)
    if new_take and engine == "chatterbox":
        done = {a["page"] for a in audio_files if a.get("page") in regen and a.get("path") and not a.get("error")}
        if done:
            _save_takes(output_dir, {p: takes[p] for p in done})
    # The takes the clips on disk were made with (a failed new take keeps its old number).
    log["takes"] = {str(k): v for k, v in nt.load_takes(output_dir).items()}
    _write_log(audio_dir, log)
    if pages_only:
        failed = [a for a in audio_files if a.get("page") in regen and a.get("error")]
        if failed and len(failed) == len([a for a in audio_files if a.get("page") in regen]):
            # Say so (the job fails) instead of reporting "audio ready" with the old clip.
            first = failed[0]
            kept = " It keeps its previous clip." if first.get("path") else ""
            raise RuntimeError(f"Page {int(first['page']) + 1} could not be voiced ({first['error']}).{kept}")
    return audio_files


def _save_takes(output_dir, pages):
    """Records the take each of `pages` ({page: take}) is now narrated with, in
    `audio/takes.json` (the format mv_studio::voices::page_take reads)."""
    import narration_tts as nt
    path = os.path.join(output_dir, "audio", "takes.json")
    merged = {str(k): v for k, v in nt.load_takes(output_dir).items()}
    merged.update({str(k): int(v) for k, v in pages.items()})
    try:
        with open(path + ".tmp", "w", encoding="utf-8") as f:
            json.dump(merged, f, indent=1)
        os.replace(path + ".tmp", path)
    except OSError as exc:
        print(f"[tts] could not save the takes: {exc}", file=sys.stderr)


_LOUDNESS_LEDGER = ".loudness.json"


def _file_sig(path):
    try:
        st = os.stat(path)
        return f"{st.st_size}:{st.st_mtime_ns}"
    except OSError:
        return None


def _normalize_chapter_pages(audio_dir, spoken, engine, voice):
    """Chapter loudness without normalising a page twice. `audio/.loudness.json` remembers
    the pages this step already normalised (size and mtime) and the gain it used. Reused pages
    stay as they are; new pages (a regenerated page, an edited one) get the chapter's earlier
    gain when the engine and voice are the same, so they match the pages around them; a fresh
    chapter gets one gain for all its pages."""
    path = os.path.join(audio_dir, _LOUDNESS_LEDGER)
    try:
        with open(path, "r", encoding="utf-8") as f:
            ledger = json.load(f)
        if not isinstance(ledger, dict):
            ledger = {}
    except Exception:
        ledger = {}
    known = ledger.get("files") if isinstance(ledger.get("files"), dict) else {}
    done = [p for p in spoken if known.get(os.path.basename(p)) == _file_sig(p)]
    fresh = [p for p in spoken if p not in done]
    if not fresh:
        return {"ok": True, "unchanged_pages": len(done), "gain_db": ledger.get("gain_db")}
    same_voice = ledger.get("engine") == engine and ledger.get("voice") == voice
    gain = ledger.get("gain_db") if done and same_voice and isinstance(ledger.get("gain_db"), (int, float)) else None
    rate = None
    if done:
        import wave
        try:
            with wave.open(done[0], "rb") as wf:
                rate = wf.getframerate()
        except Exception:
            rate = None
    res = _normalize_files(fresh, gain_db=gain, rate=rate)
    if res.get("ok"):
        files = {os.path.basename(p): _file_sig(p) for p in done + fresh}
        entry = {"engine": engine, "voice": voice, "target": res.get("target"),
                 "gain_db": gain if gain is not None else res.get("gain_db"), "files": files}
        try:
            with open(path + ".tmp", "w", encoding="utf-8") as f:
                json.dump(entry, f, indent=1)
            os.replace(path + ".tmp", path)
        except Exception as exc:
            print(f"[tts] could not write {_LOUDNESS_LEDGER}: {exc}", file=sys.stderr)
    res["unchanged_pages"] = len(done)
    return res


_ENGINE_NAMES = {"chatterbox": "Chatterbox", "kokoro": "Kokoro", "piper": "Piper", "xtts": "XTTS",
                 "aura": "AURA", "say": "macOS voices"}


def _announce_engine(engine, voice, synth_voice, reason=None):
    """One protocol line (pipeline_worker's stdout) saying who narrates this chapter, so the
    job's status can show it: the voice's own engine, or a stand-in voice and why."""
    name = _ENGINE_NAMES.get(engine, engine)
    stand_in = bool(synth_voice and synth_voice != voice)
    if reason:
        message = f"{reason}; narrating with {name} ({synth_voice}) instead."
    elif stand_in:
        message = f"Narrating with {name}: {synth_voice} stands in for {voice}."
    else:
        message = f"Narrating with {name}."
    print(json.dumps({"type": "info", "stage": "tts", "message": message, "engine": engine,
                      "voice": voice, "synth_voice": synth_voice, "stand_in": stand_in or bool(reason)}), flush=True)


def _fresh_path(output_path):
    """Where a page is voiced again before it replaces its clip (`page_0004.new.wav`)."""
    return output_path[:-4] + ".new.wav" if output_path.endswith(".wav") else output_path + ".new"


def _discard_fresh(audio_dir):
    """Drops half-made takes a crashed or cancelled run left behind."""
    try:
        for name in os.listdir(audio_dir):
            if name.endswith(".new.wav"):
                os.remove(os.path.join(audio_dir, name))
    except OSError:
        pass


def _voice_target(output_path):
    """The file to voice into: the clip itself, or a side file when a clip is already there
    (it replaces the clip only once it came out usable, see `_commit`)."""
    return _fresh_path(output_path) if os.path.isfile(output_path) else output_path


def _commit(target, output_path):
    """Puts a page voiced into `target` in place. False (and the old clip kept) when the new
    file is not usable."""
    if target == output_path:
        return _wav_is_usable(output_path)
    if not _wav_is_usable(target):
        try:
            os.remove(target)
        except OSError:
            pass
        return False
    os.replace(target, output_path)
    return True


def _stand_in_for(voice):
    try:
        from kokoro_tts import STUDIO_KOKORO_STAND_IN
        return STUDIO_KOKORO_STAND_IN.get(voice, "af_heart")
    except Exception:
        return "af_heart"


def _chapter_rate(audio_files, engine):
    import wave
    for a in audio_files:
        if a.get("path") and a.get("text") and os.path.isfile(a["path"]):
            try:
                with wave.open(a["path"], "rb") as wf:
                    return wf.getframerate()
            except Exception:
                pass
    return 24000 if engine in ("kokoro", "chatterbox") else 22050


def _normalize_files(paths, gain_db=None, rate=None):
    """Loudness to about -16 LUFS with one gain for all `paths` (a chapter), or with
    `gain_db` when given. Runs in this interpreter when it has numpy, else in the voice
    environment's Python."""
    import narration_tts as nt
    if nt.np is not None:
        try:
            return nt.normalize_chapter(paths, gain_db=gain_db, rate=rate)
        except Exception as exc:
            return {"ok": False, "message": str(exc)}
    script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "narration_tts.py")
    args = tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, encoding="utf-8")
    json.dump({"files": paths, "gain_db": gain_db, "rate": rate}, args)
    args.close()
    try:
        r = subprocess.run([_tts_subprocess_python(), script, "loudnorm", args.name], capture_output=True, text=True,
                           timeout=900, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        lines = [l for l in (r.stdout or "").splitlines() if l.strip()]
        return json.loads(lines[-1]) if lines else {"ok": False, "message": (r.stderr or "")[-300:]}
    except Exception as exc:
        return {"ok": False, "message": str(exc)}
    finally:
        os.unlink(args.name)


def _write_log(audio_dir, log):
    """Per-segment parameters next to the audio, for debugging the delivery."""
    try:
        path = os.path.join(audio_dir, "narration_params.json")
        with open(path + ".tmp", "w", encoding="utf-8") as f:
            json.dump(log, f, indent=1, ensure_ascii=False)
        os.replace(path + ".tmp", path)
    except Exception as exc:
        print(f"[tts] could not write narration_params.json: {exc}", file=sys.stderr)


def _unit_log(u):
    return {"kind": u.get("kind"), "mood": u.get("mood"), "intensity": u.get("intensity"),
            "rawIntensity": u.get("raw_intensity"), "lineKind": u.get("line_kind"), "source": u.get("source")}


def _run_basic(plan, audio_dir, engine, requested_engine, voice, speed, strength, force_regen, progress_cb, log, regen=()):
    """Kokoro / Piper / XTTS / system voices: one call per page, emotion as speed only. These
    engines say a text the same way every time, so a page's take does not change them."""
    import narration_tts as nt
    audio_files = []
    for i, p in enumerate(plan):
        if progress_cb:
            progress_cb("tts", i + 1, len(plan))
        text, page = p["text"], p["page"]
        output_path = os.path.join(audio_dir, f"page_{page:04d}.wav")
        if not text:
            audio_files.append({"page": page, "path": output_path, "text": "", "engine": engine, "_empty": True})
            continue
        style = nt.page_style(p["units"])
        page_speed = speed
        if engine in ("kokoro", "piper", "xtts"):
            page_speed = round(speed * nt.kokoro_speed(style.get("kind"), style.get("intensity"), strength), 3)
        entry = {"page": page, "engine": engine, "speed": page_speed, "units": [_unit_log(u) for u in p["units"]]}
        if not force_regen and page not in regen and _wav_is_usable(output_path):
            audio_files.append({"page": page, "path": output_path, "text": text, "engine": engine})
            entry["reused"] = True
            log["pages"].append(entry)
            continue
        target = _voice_target(output_path)
        try:
            if engine in ("piper", "kokoro", "xtts"):
                _synthesize_local(text, target, voice, page_speed)
            elif engine == "aura":
                _generate_audio_aura(text, target, requested_engine, voice, page_speed)
            else:  # say
                _generate_audio_say(text, target, voice, page_speed)
            if not _commit(target, output_path):
                raise RuntimeError("the new clip came out empty")
            audio_files.append({"page": page, "path": output_path, "text": text, "engine": engine})
        except Exception as e:
            print(f"[tts] Failed for page {page}: {e}", file=sys.stderr)
            entry["error"] = str(e)
            if target != output_path:
                # The new take failed: the page keeps the clip it had.
                try:
                    os.remove(target)
                except OSError:
                    pass
                entry["keptPrevious"] = True
                audio_files.append({"page": page, "path": output_path, "text": text, "engine": engine, "error": str(e)})
            else:
                audio_files.append({"page": page, "path": None, "text": text, "error": str(e)})
        log["pages"].append(entry)
    return audio_files


def _reference_signature(path):
    try:
        st = os.stat(path)
        return f"{os.path.abspath(path)}:{st.st_size}:{int(st.st_mtime)}"
    except OSError:
        return ""


def _chatterbox_chunks(nt, cb, page, units, voice, strength, ref_sig, take=0):
    out = []
    for ci, c in enumerate(nt.plan_chunks(units)):
        params = nt.chatterbox_params(c["kind"], c["intensity"], c["line_kind"], strength)
        seed = nt.segment_seed(page, ci, c["text"], take)
        key = nt.cache_key(f"chatterbox-{cb.CHATTERBOX_VERSION}", voice, c["synth_text"], params, seed, ref_sig)
        out.append({"text": c["synth_text"], "params": params, "seed": seed, "key": key,
                    "pause_before_ms": c["pause_before_ms"], "kind": c["kind"], "intensity": c["intensity"],
                    "shout": c["shout"], "boundary": c["boundary"]})
    return out


def _run_chatterbox(plan, audio_dir, voice, speed, strength, force_regen, progress_cb, log, reference=None, regen=(), takes=None):
    """Every page through ONE Chatterbox worker (the 0.5B model loads once per job), chunked,
    seeded per chunk (and per take), cached by hash(text, style, engine, seed) in
    audio/.tts-cache. Pages whose chunks are all cached are put together here, and the worker
    only starts when a page needs new speech (so re-rendering an edited chapter only voices
    what changed, and a new take only voices its page).
    Raises RuntimeError when Chatterbox cannot start: the caller then uses the stand-in."""
    takes = takes or {}
    import narration_tts as nt
    import chatterbox_tts as cb
    reference = reference if reference and os.path.isfile(reference) else None
    ref_path = reference or cb.reference_path(voice)
    ref_sig = _reference_signature(ref_path) if ref_path else "builtin"
    cache_dir = os.path.join(audio_dir, ".tts-cache", "chatterbox")
    state = {"worker": None, "restarts": 0}

    def worker():
        w = state["worker"]
        if w is None or not w.alive():
            if w is not None:
                if state["restarts"] >= 2:
                    raise RuntimeError("Chatterbox keeps stopping")
                state["restarts"] += 1
                w.close()
            w = cb.Worker()
            state["worker"] = w
            w.start()
            log["worker"] = {k: w.ready_info.get(k) for k in ("device", "gpu", "sr", "load_s", "torch")}
        return w

    audio_files = []
    t_job = time.time()
    try:
        for i, p in enumerate(plan):
            if progress_cb:
                progress_cb("tts", i + 1, len(plan))
            text, page = p["text"], p["page"]
            output_path = os.path.join(audio_dir, f"page_{page:04d}.wav")
            if not text:
                audio_files.append({"page": page, "path": output_path, "text": "", "engine": "chatterbox", "_empty": True})
                continue
            style = nt.page_style(p["units"])
            tempo = round(speed * (nt.chatterbox_tempo(style.get("kind")) if strength > 0 else 1.0), 3)
            take = takes.get(page, 0)
            chunks = _chatterbox_chunks(nt, cb, page, p["units"], voice, strength, ref_sig, take)
            entry = {"page": page, "engine": "chatterbox", "voice": voice, "reference": ref_path, "tempo": tempo,
                     "take": take, "units": [_unit_log(u) for u in p["units"]],
                     "chunks": [{k: c[k] for k in ("text", "params", "seed", "key", "pause_before_ms", "kind", "intensity", "shout")}
                                for c in chunks]}
            if not force_regen and page not in regen and _wav_is_usable(output_path):
                audio_files.append({"page": page, "path": output_path, "text": text, "engine": "chatterbox"})
                entry["reused"] = True
                log["pages"].append(entry)
                continue
            request = [{k: c[k] for k in ("text", "params", "seed", "key", "pause_before_ms")} for c in chunks]
            target = _voice_target(output_path)
            res = None
            try:
                res = cb.assemble_cached(target, request, cache_dir, nt.PAGE_LEAD_MS, nt.PAGE_TAIL_MS, tempo)
            except Exception as e:
                print(f"[tts] cached page {page} could not be assembled ({e}); voicing it again", file=sys.stderr)
            for _attempt in range(2):
                if res:
                    break
                w = worker()
                try:
                    res = w.page(target, voice, request, reference=ref_path, cache_dir=cache_dir,
                                 lead_ms=nt.PAGE_LEAD_MS, tail_ms=nt.PAGE_TAIL_MS, tempo=tempo)
                except RuntimeError as e:
                    entry["error"] = str(e)
                    if w.alive():
                        break  # this page failed; the worker is fine
                    print(f"[tts] Chatterbox stopped on page {page} ({e}); restarting", file=sys.stderr)
            if res and not _commit(target, output_path):
                res, entry["error"] = None, "the new clip came out empty"
            if res:
                entry.pop("error", None)
                entry["result"] = {k: res.get(k) for k in ("duration", "elapsed", "tempo", "device")}
                for c, r in zip(entry["chunks"], res.get("chunks") or []):
                    c.update(cached=r.get("cached"), retried=r.get("retried"), seconds=r.get("raw_seconds"), elapsed=r.get("elapsed"))
                audio_files.append({"page": page, "path": output_path, "text": text, "engine": "chatterbox"})
            elif target != output_path:
                # The new take failed: the page keeps the clip it had.
                print(f"[tts] New take failed for page {page} ({entry.get('error')}); keeping the previous clip", file=sys.stderr)
                try:
                    os.remove(target)
                except OSError:
                    pass
                entry["keptPrevious"] = True
                audio_files.append({"page": page, "path": output_path, "text": text, "engine": "chatterbox",
                                    "error": entry.get("error") or "Chatterbox failed"})
            else:
                print(f"[tts] Failed for page {page}: {entry.get('error')}", file=sys.stderr)
                audio_files.append({"page": page, "path": None, "text": text, "error": entry.get("error") or "Chatterbox failed"})
            log["pages"].append(entry)
    finally:
        if state["worker"] is not None:
            state["worker"].close()
    spoken = sum((e.get("result") or {}).get("duration") or 0 for e in log["pages"])
    elapsed = time.time() - t_job
    log["timing"] = {"seconds": round(elapsed, 1), "audioSeconds": round(spoken, 1),
                     "rtf": round(elapsed / spoken, 3) if spoken else None}
    return audio_files


def _write_silence(output_path, seconds=0.5, framerate=22050):
    """Write a tiny silent WAV — pure stdlib so we don't pull deps."""
    import wave
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


_WORKER = None


def _kokoro_worker():
    """The `kokoro_tts.py worker` process for this run, started on first use. It keeps the voice
    model loaded, so a chapter loads it once instead of once per chunk (about 11 s each)."""
    global _WORKER
    if _WORKER is None or _WORKER.poll() is not None:
        _WORKER = subprocess.Popen(
            [_tts_subprocess_python(), KOKORO_SCRIPT, "worker"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, encoding="utf-8", bufsize=1,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    return _WORKER


def _worker_synthesize(text, output_path, voice, speed):
    """Speaks one chunk through the worker. True when it is written; raises the voice's own
    error; None when the worker itself is unusable (the caller then runs the old way)."""
    try:
        w = _kokoro_worker()
        w.stdin.write(json.dumps({"text": text, "voice": voice, "output_path": output_path, "speed": speed}) + "\n")
        w.stdin.flush()
        while True:
            line = w.stdout.readline()
            if not line:
                return None
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(msg, dict) and "ok" in msg:
                break
    except Exception:
        return None
    if msg.get("ok") and os.path.exists(output_path):
        return True
    raise RuntimeError((msg.get("message") or "Kokoro synthesis failed")[:500])


def _generate_audio_kokoro(text, output_path, voice, speed):
    """Bundled Kokoro-82M — local ONNX, no API, Hindi + English."""
    if os.path.isfile(KOKORO_SCRIPT) and _worker_synthesize(text, output_path, voice, speed):
        return
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
    "cb_narrator_f": "Long ago, in a city of bells, a girl made a promise she could never take back.",
    "cb_narrator_m": "He had lost everything that night... but he was not finished. Not yet.",
    "cb_storyteller_m": "Some say the old sword still hangs in that hall, waiting for a hand brave enough to lift it.",
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

    local_error = None
    if os.path.isfile(KOKORO_SCRIPT):
        try:
            ready = _run_kokoro_json(["voice-ready", voice])
            engine = ready.get("engine") if ready.get("ready") else None
            if not engine or engine == "none":
                raise RuntimeError(ready.get("message") or _NO_ENGINE_MESSAGE)
            synth_voice = ready.get("previewVoice") or voice
            preview_fallback = bool(ready.get("previewFallback") and synth_voice != voice)
            if engine == "chatterbox":
                import chatterbox_tts as cb
                cb.synthesize_one(text, output_path, synth_voice, speed, kind="other", intensity=4)
            else:
                _generate_audio_kokoro(text, output_path, synth_voice, speed)
            if not os.path.isfile(output_path) or os.path.getsize(output_path) < 100:
                raise RuntimeError("Preview synthesis produced no audio")
            # Same loudness as a narrated chapter (engines no longer peak-normalise).
            _normalize_files([output_path])
            return {
                "path": output_path,
                "engine": engine,
                "text": text,
                "previewFallback": preview_fallback,
                "synthVoice": synth_voice,
            }
        except Exception as exc:
            local_error = exc

    # Only macOS `say` or an external AURA worker are left; without them the local error is
    # the one that says what to install.
    if local_error is not None and not (_say_available() or _aura_available()):
        raise local_error

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


# ─────────────────────────────────────────────────────────────────────────────
# Voice lab: one sentence at several intensities, to hear and tune the emotion curve
# ─────────────────────────────────────────────────────────────────────────────

LAB_TEXT = {
    "fight": "He hit the wall hard, but before the dust could settle he was already charging back in!",
    "action": "The bridge was collapsing behind them, and there were still three hundred meters to go.",
    "emotional": "She held the letter for a long time... and then, very quietly, she began to cry.",
    "comedy": "And that is how the strongest swordsman in the kingdom lost a duel to a goose.",
    "reveal": "The mask fell away, and the face beneath it belonged to the man they had buried ten years ago.",
    "horror": "Something was breathing in the dark, slow and patient, just behind the door.",
    "romance": "For a moment neither of them spoke, and the whole city seemed to hold its breath with them.",
    "flashback": "Long ago, before the war, the two of them used to race each other along this very river.",
    "calm": "Morning came slowly over the village, and for once, nobody was in a hurry.",
    "dialogue": "You came back, she said. I told you I would, he answered.",
    "other": "The next day, the team set out for the capital, unsure of what was waiting for them there.",
}


def voice_lab(voice, kind, out_dir, text=None, intensities=(2, 5, 8), mood="", strength=1.0, reference=None):
    """Renders the same sentence for scene `kind` at each intensity with `voice`'s engine
    (Chatterbox: exaggeration/cfg/temperature; Kokoro/Piper: speed), each sample normalised
    to the chapter loudness. Returns {ok, voice, engine, kind, text, samples: [{intensity,
    path, params, duration, elapsed, rtf}]}."""
    import narration_tts as nt
    kind = nt.normalize_kind(kind)
    text = (text or LAB_TEXT.get(kind) or LAB_TEXT["other"]).strip()
    os.makedirs(out_dir, exist_ok=True)
    ready = _run_kokoro_json(["voice-ready", voice])
    engine = ready.get("engine") if ready.get("ready") else None
    if not engine or engine == "none":
        raise RuntimeError(ready.get("message") or _NO_ENGINE_MESSAGE)
    synth_voice = ready.get("synthVoice") or ready.get("previewVoice") or voice
    safe = "".join(c if c.isalnum() or c in "_-" else "_" for c in voice)
    samples = []
    if engine == "chatterbox":
        import chatterbox_tts as cb
        ref = reference if reference and os.path.isfile(reference) else None
        with cb.Worker() as w:
            for level in intensities:
                units = [{"text": text, "kind": kind, "mood": mood, "intensity": float(level), "line_kind": "narration"}]
                chunks = _chatterbox_chunks(nt, cb, 0, units, synth_voice, strength, "")
                out = os.path.join(out_dir, f"lab_{safe}_{kind}_{int(level)}.wav")
                res = w.page(out, synth_voice, [{k: c[k] for k in ("text", "params", "seed", "pause_before_ms")} for c in chunks],
                             reference=ref, lead_ms=nt.PAGE_LEAD_MS, tail_ms=nt.PAGE_TAIL_MS,
                             tempo=nt.chatterbox_tempo(kind) if strength > 0 else 1.0)
                loud = _normalize_files([out])
                dur = float(res.get("duration") or 0)
                samples.append({"intensity": level, "path": out, "params": chunks[0]["params"] if chunks else {},
                                "chunks": [{"text": c["text"], "params": c["params"], "pause_before_ms": c["pause_before_ms"]} for c in chunks],
                                "tempo": res.get("tempo"), "duration": dur, "elapsed": res.get("elapsed"),
                                "rtf": round(float(res.get("elapsed") or 0) / dur, 3) if dur else None,
                                "device": res.get("device"), "loudness": loud})
    else:
        for level in intensities:
            sp = nt.kokoro_speed(kind, level, strength)
            out = os.path.join(out_dir, f"lab_{safe}_{kind}_{int(level)}.wav")
            t0 = time.time()
            _generate_audio_kokoro(text, out, synth_voice, sp)
            elapsed = time.time() - t0
            loud = _normalize_files([out])
            dur = nt.wav_duration(out)
            samples.append({"intensity": level, "path": out, "params": {"speed": sp}, "duration": round(dur, 3),
                            "elapsed": round(elapsed, 2), "rtf": round(elapsed / dur, 3) if dur else None, "loudness": loud})
    return {"ok": True, "voice": voice, "synthVoice": synth_voice, "engine": engine, "kind": kind, "text": text, "samples": samples}
