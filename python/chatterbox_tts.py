#!/usr/bin/env python3
"""
Chatterbox (Resemble AI, MIT): the expressive narrator. Graded emotion through
exaggeration / cfg_weight / temperature, driven by each page's scene (see narration_tts.py).

Chatterbox pins torch 2.6, transformers 5.2 and numpy<2, so it lives in its OWN environment,
`<data>/engines/chatterbox` (Python 3.11 made with uv), never in the Kokoro/Piper `tts-venv`.
Its weights live in `<data>/models/chatterbox` (the same files and URLs as the
"chatterbox" pack in mv-studio/src/voices.rs, so either side can download or resume them),
reference clips in `<data>/models/chatterbox/voices`, and any other Hugging Face download in
`<data>/models/chatterbox/hf` (HF_HOME), so Delete models removes everything.

This file runs in two interpreters:
  * any Python 3.9+ (the voice environment, the pipeline): install, status, clients. Stdlib only.
  * the engine's venv: `serve` (the long-lived worker) and `probe`.

Usage:
  python chatterbox_tts.py install                 JSON lines: progress / info / done / error
  python chatterbox_tts.py status                  one JSON object
  python chatterbox_tts.py make-refs               reference clips for voices that need one
  python chatterbox_tts.py synth <args.json>       {text, output_path, voice, kind?, intensity?, mood?}
  python chatterbox_tts.py serve                   (engine venv) JSON-lines worker on stdin/stdout
  python chatterbox_tts.py probe                   (engine venv) import + CUDA check

Worker protocol (one JSON object per line; replies carry the request's `id`):
  -> {"op": "page", "id": 1, "out": "page.wav", "voice": "cb_narrator_m", "reference": "x.wav"|null,
      "cache_dir": "...", "lead_ms": 120, "tail_ms": 300, "tempo": 1.0,
      "chunks": [{"text": "...", "params": {...}, "seed": 123, "key": "...", "pause_before_ms": 0}]}
  <- {"id": 1, "ok": true, "path": "page.wav", "duration": 7.4, "elapsed": 3.1, "chunks": [...]}
  -> {"op": "shutdown"}
"""
from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
BASE_DIR = os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")
ENGINE_DIR = os.path.join(BASE_DIR, "engines", "chatterbox")
ENGINE_PY = os.path.join(ENGINE_DIR, "Scripts", "python.exe") if os.name == "nt" else os.path.join(ENGINE_DIR, "bin", "python3")
MODEL_DIR = os.path.join(BASE_DIR, "models", "chatterbox")
VOICES_DIR = os.path.join(MODEL_DIR, "voices")
HF_HOME = os.path.join(MODEL_DIR, "hf")
# Written last by `install`, once the engine imported and spoke a test sentence.
MARKER = os.path.join(ENGINE_DIR, "mangavox-engine.json")
UV_CACHE_DIR = os.path.join(BASE_DIR, "uv-cache")
UV_PYTHON_INSTALL_DIR = os.path.join(BASE_DIR, "uv-python")
UV_BIN = os.path.join(BASE_DIR, "bin", "uv.exe" if os.name == "nt" else "uv")
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)

ENGINE_ID = "chatterbox"
ENGINE_NAME = "Chatterbox"
PACK_NAME = "Chatterbox · Expressive narrator"
CHATTERBOX_VERSION = "0.1.7"
PYTHON_VERSION = "3.11"
TORCH_PINS = ["torch==2.6.0", "torchaudio==2.6.0"]
CUDA_INDEX = "https://download.pytorch.org/whl/cu124"
CPU_INDEX = "https://download.pytorch.org/whl/cpu"
# chatterbox-tts 0.1.7's requirements without gradio (only its demo apps use it).
DEPS = [
    "numpy>=1.24.0,<2.0.0", "librosa==0.11.0", "s3tokenizer", "transformers==5.2.0", "diffusers==0.29.0",
    "resemble-perth>=1.0.0", "conformer==0.3.2", "safetensors==0.5.3", "spacy-pkuseg", "pykakasi==2.3.0",
    "pyloudnorm", "omegaconf", "soundfile",
    # resemble-perth (the watermarker generate() always applies) imports pkg_resources,
    # which setuptools 81+ no longer ships.
    "setuptools<81",
]

HF_REPO = "ResembleAI/chatterbox"
HF_REVISION = "5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18"
# (file, bytes). English model: voice encoder, T3 (0.5B Llama backbone), S3Gen decoder,
# text tokenizer, and the built-in voice's conditionals.
WEIGHTS = [
    ("ve.safetensors", 5_695_784),
    ("t3_cfg.safetensors", 2_129_653_744),
    ("s3gen.safetensors", 1_056_484_620),
    ("tokenizer.json", 25_470),
    ("conds.pt", 107_374),
]

# Catalog voices of this engine. `reference: None` is Chatterbox's built-in voice (conds.pt,
# a male narrator, median pitch about 130 Hz); the others clone a ~10 s reference clip in
# VOICES_DIR, made once by `make-refs` with a local Kokoro voice. A user clip can be passed
# per request as `reference`.
VOICES = {
    "cb_narrator_f": {"reference": "cb_narrator_f.wav", "make_with": "af_heart"},
    "cb_narrator_m": {"reference": None},
    "cb_storyteller_m": {"reference": "cb_storyteller_m.wav", "make_with": "am_michael"},
}
REFERENCE_TEXT = (
    "The city had been quiet for three long days. Then, just before dawn, the bells began to ring, "
    "and everyone who heard them knew that nothing would ever be the same again."
)


def weight_url(name: str) -> str:
    return f"https://huggingface.co/{HF_REPO}/resolve/{HF_REVISION}/{name}"


def emit(obj: dict) -> None:
    print(json.dumps(obj), flush=True)


# ----------------------------------------------------------------------------- readiness (cheap)

def engine_python_present() -> bool:
    return os.path.isfile(ENGINE_PY)


def engine_info() -> dict:
    try:
        with open(MARKER, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}


def engine_installed() -> bool:
    """The engine venv works: install verified it (imports and a test sentence)."""
    return engine_python_present() and bool(engine_info().get("ok"))


def missing_weights() -> list:
    return [n for n, size in WEIGHTS if not _file_ok(os.path.join(MODEL_DIR, n), size)]


def _file_ok(path: str, size: int) -> bool:
    try:
        have = os.path.getsize(path)
    except OSError:
        return False
    # Big files must be whole; small config files only need to exist.
    return have == size if size > 1_000_000 else have > 0


def weights_ready() -> bool:
    return not missing_weights()


def is_voice(voice: str) -> bool:
    return (voice or "").startswith("cb_")


def reference_path(voice: str) -> str | None:
    """The reference clip `voice` clones (None for the built-in voice)."""
    ref = (VOICES.get(voice) or {}).get("reference")
    return os.path.join(VOICES_DIR, ref) if ref else None


def reference_ready(voice: str) -> bool:
    ref = reference_path(voice)
    return ref is None or (os.path.isfile(ref) and os.path.getsize(ref) > 10_000)


def voice_status(voice: str) -> tuple[bool, bool, bool]:
    """(ready, model files present, engine installed) for a Chatterbox voice."""
    model_ok = weights_ready() and reference_ready(voice)
    engine_ok = engine_installed()
    return model_ok and engine_ok, model_ok, engine_ok


def _dir_size(path: str) -> int:
    total = 0
    for dirpath, _, files in os.walk(path):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(dirpath, f))
            except OSError:
                pass
    return total


def status() -> dict:
    info = engine_info()
    return {
        "engine": ENGINE_ID,
        "installed": engine_installed(),
        "pythonPresent": engine_python_present(),
        "python": ENGINE_PY,
        "engineDir": ENGINE_DIR,
        "modelDir": MODEL_DIR,
        "weightsReady": weights_ready(),
        "missingWeights": missing_weights(),
        "references": {v: reference_ready(v) for v in VOICES},
        "device": info.get("device"),
        "gpu": info.get("gpu"),
        "torch": info.get("torch"),
        "rtf": info.get("rtf"),
        "version": info.get("chatterbox"),
    }


# ----------------------------------------------------------------------------- install

def _uv() -> str | None:
    return UV_BIN if os.path.isfile(UV_BIN) else shutil.which("uv")


def _uv_env() -> dict:
    env = os.environ.copy()
    env["UV_CACHE_DIR"] = UV_CACHE_DIR
    env["UV_PYTHON_INSTALL_DIR"] = UV_PYTHON_INSTALL_DIR
    env["UV_NO_PROGRESS"] = "1"
    env.pop("VIRTUAL_ENV", None)
    return env


def _nvidia_gpu() -> str | None:
    """Name of the first NVIDIA GPU, when the driver is installed."""
    smi = shutil.which("nvidia-smi")
    if not smi and os.name == "nt":
        cand = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "nvidia-smi.exe")
        smi = cand if os.path.isfile(cand) else None
    if not smi:
        return None
    try:
        r = subprocess.run([smi, "--query-gpu=name", "--format=csv,noheader"], capture_output=True, text=True, timeout=20, creationflags=_NO_WINDOW)
        name = (r.stdout or "").strip().splitlines()
        return name[0].strip() if r.returncode == 0 and name else None
    except Exception:
        return None


def _py_json(code: str, timeout: int = 300) -> dict | None:
    """Runs `code` in the engine's Python; its last stdout line is JSON."""
    if not engine_python_present():
        return None
    try:
        r = subprocess.run([ENGINE_PY, "-c", code], capture_output=True, text=True, timeout=timeout,
                           cwd=ENGINE_DIR, env=_worker_env(), creationflags=_NO_WINDOW)
        lines = [l for l in (r.stdout or "").splitlines() if l.strip()]
        if r.returncode == 0 and lines:
            return json.loads(lines[-1])
        return {"error": ((r.stderr or "") + (r.stdout or "")).strip()[-800:]}
    except Exception as e:
        return {"error": str(e)}


def _engine_python_version() -> str | None:
    out = _py_json("import json,sys; print(json.dumps({'v': '%d.%d' % sys.version_info[:2]}))", 60)
    return (out or {}).get("v")


def _run_streaming(cmd: list, label: str, stage: str, n: int, total: int, watch_dir: str | None = None,
                   expect_bytes: int = 0, env: dict | None = None) -> tuple[int, str]:
    """Runs an installer command, turning its output into progress messages. Every 10 s
    without output it reports how much has been downloaded (growth of `watch_dir`)."""
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                            errors="replace", env=env or _uv_env(), creationflags=_NO_WINDOW)
    lines: queue.Queue = queue.Queue()

    def pump():
        for line in proc.stdout:  # type: ignore[union-attr]
            lines.put(line.rstrip())
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()
    tail: list[str] = []
    started = time.time()
    base = _dir_size(watch_dir) if watch_dir and os.path.isdir(watch_dir) else 0
    last_beat = started
    while True:
        try:
            line = lines.get(timeout=10)
        except queue.Empty:
            line = ""
        if line is None:
            break
        now = time.time()
        if line:
            tail = (tail + [line])[-25:]
            emit({"type": "progress", "stage": stage, "current": n, "total": total, "message": f"{label}: {line.strip()[:160]}"})
            last_beat = now
        elif now - last_beat >= 10:
            mins = int((now - started) // 60)
            grown = (_dir_size(watch_dir) - base) if watch_dir and os.path.isdir(watch_dir) else 0
            if grown > 0 and expect_bytes:
                what = f"{grown / 1e9:.2f} of about {expect_bytes / 1e9:.1f} GB"
                pct = min(99.0, grown / expect_bytes * 100)
            elif grown > 0:
                what, pct = f"{grown / 1e6:.0f} MB so far", None
            else:
                what, pct = "working", None
            msg = {"type": "progress", "stage": stage, "current": n, "total": total, "message": f"{label}: {what} ({mins} min)"}
            if pct is not None:
                msg["percent"] = round(pct, 1)
            emit(msg)
            last_beat = now
    code = proc.wait()
    return code, "\n".join(tail)


def _pip(packages: list, label: str, n: int, total: int, index: str | None = None, expect_bytes: int = 0, extra: list | None = None) -> None:
    uv = _uv()
    if uv:
        cmd = [uv, "pip", "install", "--python", ENGINE_PY, *packages]
        if index:
            cmd += ["--index-url", index]
    else:
        cmd = [ENGINE_PY, "-m", "pip", "install", "--disable-pip-version-check", *packages]
        if index:
            cmd += ["--index-url", index]
    cmd += extra or []
    code, tail = _run_streaming(cmd, label, "pip", n, total, watch_dir=UV_CACHE_DIR if uv else None, expect_bytes=expect_bytes)
    if code != 0:
        raise RuntimeError(f"{label} failed (exit {code}): {tail[-700:]}")


def _forget_cached_wheels() -> None:
    """uv hard-links the venv's files to its cache, so PyTorch's ~4.5 GB would stay in
    `uv-cache` after Delete models removes `engines/`. Dropping the big packages from the
    cache leaves the venv's files as their only copy."""
    uv = _uv()
    if not uv:
        return
    try:
        subprocess.run([uv, "cache", "clean", "torch", "torchaudio", "nvidia-cudnn-cu12", "nvidia-cublas-cu12", "triton"],
                       capture_output=True, timeout=600, env=_uv_env(), creationflags=_NO_WINDOW)
    except Exception:
        pass


def _make_venv(n: int, total: int) -> None:
    uv = _uv()
    os.makedirs(os.path.dirname(ENGINE_DIR), exist_ok=True)
    if uv:
        cmd = [uv, "venv", "--seed", "--clear", "--python", PYTHON_VERSION, ENGINE_DIR]
        code, tail = _run_streaming(cmd, f"Python {PYTHON_VERSION}", "venv", n, total)
        if code != 0:
            raise RuntimeError(f"Could not create the Chatterbox environment with uv: {tail[-500:]}")
        return
    py = shutil.which("py")
    if py:
        r = subprocess.run([py, f"-{PYTHON_VERSION}", "-m", "venv", "--clear", ENGINE_DIR], capture_output=True, text=True, timeout=600, creationflags=_NO_WINDOW)
        if r.returncode == 0 and engine_python_present():
            return
    raise RuntimeError(
        f"Chatterbox needs Python {PYTHON_VERSION}. Install uv (winget install -e --id astral-sh.uv) "
        f"or Python {PYTHON_VERSION} (winget install -e --id Python.Python.{PYTHON_VERSION}) and try again."
    )


def _download(name: str, size: int, n: int, total: int) -> None:
    """Resumable download into MODEL_DIR (`<file>.part` + HTTP Range, like the app's downloader)."""
    os.makedirs(MODEL_DIR, exist_ok=True)
    dest = os.path.join(MODEL_DIR, name)
    part = dest + ".part"
    last_err = ""
    for attempt in range(6):
        if attempt:
            time.sleep(min(30, 2 ** attempt))
        have = os.path.getsize(part) if os.path.isfile(part) else 0
        if size and have > size:
            os.remove(part)
            have = 0
        headers = {"User-Agent": "MangaVox/1.0"}
        if have:
            headers["Range"] = f"bytes={have}-"
        try:
            req = urllib.request.Request(weight_url(name), headers=headers)
            with urllib.request.urlopen(req, timeout=60) as resp:
                resumed = resp.status == 206
                mode = "ab" if resumed else "wb"
                done = have if resumed else 0
                t0, last = time.time(), 0.0
                with open(part, mode) as out:
                    while True:
                        chunk = resp.read(1 << 20)
                        if not chunk:
                            break
                        out.write(chunk)
                        done += len(chunk)
                        now = time.time()
                        if now - last >= 1.0:
                            last = now
                            rate = (done - (have if resumed else 0)) / max(0.001, now - t0)
                            pct = done / size * 100 if size else 0
                            emit({"type": "progress", "stage": "download", "current": n, "total": total, "file": name,
                                  "percent": round(pct, 1), "completed": done, "totalBytes": size,
                                  "message": f"Downloading {name}: {pct:.0f}% ({done / 1e9:.2f} of {size / 1e9:.2f} GB, {rate / 1e6:.0f} MB/s)"})
            if size and os.path.getsize(part) != size:
                last_err = f"{name} ended early ({os.path.getsize(part)} of {size} bytes)"
                continue
            os.replace(part, dest)
            return
        except urllib.error.HTTPError as e:
            last_err = f"HTTP {e.code}"
            if e.code == 416:
                try:
                    os.remove(part)
                except OSError:
                    pass
            elif 400 <= e.code < 500 and e.code not in (408, 429):
                break
        except Exception as e:
            last_err = str(e)[:200]
    raise RuntimeError(f"{name} could not be downloaded: {last_err}. Run the setup again to resume.")


def make_references(emit_progress=None) -> dict:
    """Makes the reference clips of voices that clone one, with a local Kokoro voice
    (`kokoro_tts.py synthesize`, which picks a Piper stand-in when Kokoro is missing)."""
    out = {}
    for voice, spec in VOICES.items():
        ref = reference_path(voice)
        if not ref:
            continue
        if reference_ready(voice):
            out[voice] = "present"
            continue
        os.makedirs(VOICES_DIR, exist_ok=True)
        args_path = os.path.join(VOICES_DIR, f".{voice}-args.json")
        with open(args_path, "w", encoding="utf-8") as f:
            json.dump({"text": REFERENCE_TEXT, "output_path": ref, "voice": spec.get("make_with", "af_heart"), "speed": 0.95}, f)
        kokoro_script = os.path.join(HERE, "kokoro_tts.py")
        venv_py = os.path.join(BASE_DIR, "tts-venv", "Scripts", "python.exe") if os.name == "nt" else os.path.join(BASE_DIR, "tts-venv", "bin", "python3")
        py = venv_py if os.path.isfile(venv_py) else sys.executable
        try:
            r = subprocess.run([py, kokoro_script, "synthesize", args_path], capture_output=True, text=True, timeout=600,
                               cwd=HERE, creationflags=_NO_WINDOW)
            ok = r.returncode == 0 and reference_ready(voice)
            out[voice] = "made" if ok else f"failed: {(r.stdout or r.stderr or '').strip()[-300:]}"
        except Exception as e:
            out[voice] = f"failed: {e}"
        finally:
            try:
                os.remove(args_path)
            except OSError:
                pass
        if emit_progress:
            emit_progress(voice, out[voice])
    return out


_PROBE = r"""
import json, sys, importlib.metadata as md
out = {"python": sys.version.split()[0]}
try:
    import torch
    out["torch"] = torch.__version__
    out["cuda"] = bool(torch.cuda.is_available())
    out["gpu"] = torch.cuda.get_device_name(0) if out["cuda"] else None
    from chatterbox.tts import ChatterboxTTS  # noqa: F401
    import perth
    if getattr(perth, "PerthImplicitWatermarker", None) is None:
        raise RuntimeError("resemble-perth's watermarker did not load (it needs setuptools<81 for pkg_resources)")
    out["chatterbox"] = md.version("chatterbox-tts")
    out["ok"] = True
except Exception as e:
    out["ok"] = False
    out["error"] = "%s: %s" % (type(e).__name__, e)
print(json.dumps(out))
"""


def install(force: bool = False) -> int:
    """Sets up Chatterbox end to end. Emits JSON lines; returns the exit code."""
    total = 7
    t_start = time.time()

    def step(n, stage, msg):
        emit({"type": "progress", "stage": stage, "current": n, "total": total, "message": msg})

    try:
        # 1. Its own Python 3.11.
        step(1, "venv", f"Preparing Python {PYTHON_VERSION} for Chatterbox…")
        if force or not engine_python_present() or _engine_python_version() != PYTHON_VERSION:
            _make_venv(1, total)
        if not engine_python_present():
            raise RuntimeError("The Chatterbox environment was not created.")

        # 2. PyTorch, with CUDA when there is an NVIDIA GPU.
        gpu = _nvidia_gpu()
        step(2, "torch", f"Checking PyTorch ({'CUDA 12.4 for ' + gpu if gpu else 'CPU'})…")
        have = _py_json("import json, torch; print(json.dumps({'torch': torch.__version__, 'cuda': torch.cuda.is_available()}))", 300) or {}
        version = str(have.get("torch") or "")
        good = version.startswith("2.6.0") and (("+cu" in version) if gpu else True)
        if force or not good:
            label = "PyTorch 2.6 with CUDA" if gpu else "PyTorch 2.6 (CPU)"
            step(2, "torch", f"Installing {label} (about {'2.5 GB download, 4.5' if gpu else '0.3'} GB on disk)…")
            _pip(TORCH_PINS, label, 2, total, index=CUDA_INDEX if gpu else CPU_INDEX,
                 expect_bytes=4_300_000_000 if gpu else 700_000_000, extra=["--reinstall"] if version and not good else None)

        # 3. Chatterbox and its pinned dependencies (torch stays as installed).
        step(3, "chatterbox", f"Installing Chatterbox {CHATTERBOX_VERSION}…")
        installed = (_py_json("import json, importlib.metadata as m; print(json.dumps({'v': m.version('chatterbox-tts')}))", 60) or {}).get("v")
        if force or installed != CHATTERBOX_VERSION:
            _pip([*TORCH_PINS, *DEPS], "Chatterbox dependencies", 3, total, expect_bytes=400_000_000)
            _pip([f"chatterbox-tts=={CHATTERBOX_VERSION}", "--no-deps"], "Chatterbox", 3, total)

        _forget_cached_wheels()

        # 4. It imports, and sees the GPU.
        step(4, "verify", "Checking that Chatterbox loads…")
        probe = _py_json(_PROBE, 600) or {}
        if not probe.get("ok"):
            # A half-finished or older environment: reinstall the pinned dependencies once.
            step(4, "verify", f"Repairing the Chatterbox environment ({str(probe.get('error'))[:160]})…")
            _pip([*TORCH_PINS, *DEPS], "Chatterbox dependencies", 4, total)
            probe = _py_json(_PROBE, 600) or {}
        if not probe.get("ok") and "ModuleNotFoundError" in str(probe.get("error", "")):
            # A dependency the trimmed list missed: take the package's full requirements.
            step(4, "verify", f"Adding missing dependencies ({probe.get('error')})…")
            _pip([*TORCH_PINS, f"chatterbox-tts=={CHATTERBOX_VERSION}"], "Chatterbox (full requirements)", 4, total)
            probe = _py_json(_PROBE, 600) or {}
        if not probe.get("ok"):
            raise RuntimeError(f"Chatterbox installed but does not load: {probe.get('error', 'unknown error')}")
        if gpu and not probe.get("cuda"):
            emit({"type": "info", "message": f"PyTorch does not see {gpu}; Chatterbox will run on the CPU (much slower). Updating the NVIDIA driver usually fixes this."})

        # 5. The weights (about 3.2 GB), resumable.
        todo = missing_weights()
        for name, size in WEIGHTS:
            if name in todo:
                step(5, "download", f"Downloading {name} ({size / 1e9:.2f} GB)…" if size > 1e8 else f"Downloading {name}…")
                _download(name, size, 5, total)
        step(5, "download", "Voice model files are in place.")

        # 6. Reference clips for the cloned narrators.
        step(6, "references", "Making the narrator reference clips…")
        refs = make_references(lambda v, s: step(6, "references", f"{v}: {s}"))

        # 7. A test sentence through the real worker.
        step(7, "test", "Loading Chatterbox and speaking a test sentence (first load takes a while)…")
        test_out = os.path.join(ENGINE_DIR, "selftest.wav")
        t0 = time.time()
        params = {"exaggeration": 0.5, "cfg_weight": 0.5, "temperature": 0.8}
        with Worker(require_installed=False) as w:
            load_s = time.time() - t0
            # The first generation warms CUDA up; the second one is the speed users get.
            w.page(test_out, "cb_narrator_m", [{"text": "Warming up.", "params": params, "seed": 1, "pause_before_ms": 0}])
            res = w.page(test_out, "cb_narrator_m", [{"text": "Chatterbox is ready to narrate your chapters, scene by scene.",
                                                       "params": params, "seed": 1, "pause_before_ms": 0}])
            ready = dict(w.ready_info)
        chunk = (res.get("chunks") or [{}])[0]
        speech = float(chunk.get("raw_seconds") or 0)
        rtf = round(float(chunk.get("elapsed") or 0) / speech, 3) if speech else None
        info = {
            "ok": True,
            "chatterbox": probe.get("chatterbox"),
            "torch": probe.get("torch"),
            "python": probe.get("python"),
            "device": ready.get("device"),
            "gpu": ready.get("gpu"),
            "sample_rate": ready.get("sr"),
            "load_seconds": round(load_s, 1),
            "rtf": rtf,
            "references": refs,
            "installed_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "install_seconds": round(time.time() - t_start),
        }
        with open(MARKER, "w", encoding="utf-8") as f:
            json.dump(info, f, indent=1)
        where = f"on {ready.get('gpu')}" if ready.get("device") == "cuda" else "on the CPU"
        emit({"type": "done", "message": f"Chatterbox installed and running {where} (real-time factor {rtf}).", **info})
        return 0
    except Exception as e:
        emit({"type": "error", "message": str(e)[-900:]})
        return 1


# ----------------------------------------------------------------------------- worker (engine venv)

def _worker_env() -> dict:
    env = os.environ.copy()
    env.update({
        "HF_HOME": HF_HOME,
        "HF_HUB_DISABLE_TELEMETRY": "1",
        "TOKENIZERS_PARALLELISM": "false",
        "PYTHONUNBUFFERED": "1",
        "PYTHONUTF8": "1",
        "PYTHONIOENCODING": "utf-8",
        "MANGAVOX_DATA": BASE_DIR,
    })
    env.pop("VIRTUAL_ENV", None)
    return env


def _load_narration():
    """narration_tts from this folder, without putting the folder (and its f5_tts.py etc.)
    on sys.path while the engine's packages import."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("narration_tts", os.path.join(HERE, "narration_tts.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


def serve() -> int:
    """The long-lived worker: loads the model once and answers requests on stdin."""
    proto = sys.stdout
    sys.stdout = sys.stderr  # library prints must not break the protocol

    def send(obj):
        proto.write(json.dumps(obj) + "\n")
        proto.flush()

    here = os.path.normcase(HERE)
    sys.path[:] = [p for p in sys.path if os.path.normcase(os.path.abspath(p or ".")) != here]
    os.environ.setdefault("HF_HOME", HF_HOME)
    try:
        import inspect
        import random
        import traceback

        import numpy as np
        import torch
        from chatterbox.tts import ChatterboxTTS
        nt = _load_narration()
        device = "cuda" if torch.cuda.is_available() else "cpu"
        t0 = time.time()
        model = ChatterboxTTS.from_local(MODEL_DIR, device)
        sr = int(getattr(model, "sr", 24000))
        builtin = model.conds
        accepted = set(inspect.signature(model.generate).parameters)
        gpu = torch.cuda.get_device_name(0) if device == "cuda" else None
        send({"type": "ready", "device": device, "gpu": gpu, "sr": sr, "load_s": round(time.time() - t0, 2),
              "torch": torch.__version__, "accepts": sorted(accepted)})
    except Exception as e:
        send({"type": "error", "message": f"Chatterbox could not start: {type(e).__name__}: {e}"})
        return 1

    conds: dict = {}

    def use_voice(voice, reference):
        key = reference or voice or "builtin"
        if key not in conds:
            if reference:
                if not os.path.isfile(reference):
                    raise RuntimeError(f"reference clip missing: {reference}")
                model.prepare_conditionals(reference, exaggeration=0.5)
                conds[key] = model.conds
            else:
                conds[key] = builtin
        model.conds = conds[key]

    def generate(text, params, seed):
        kwargs = {k: v for k, v in (params or {}).items() if k in accepted}
        torch.manual_seed(seed)
        random.seed(seed)
        np.random.seed(seed % (2 ** 32))
        if device == "cuda":
            torch.cuda.manual_seed_all(seed)
        with torch.inference_mode():
            wav = model.generate(text, **kwargs)
        return wav.squeeze().detach().float().cpu().numpy().astype(np.float32)

    def synth_chunk(text, params, seed):
        """One chunk, retried once when it runs away (hallucinated tail) or comes out empty."""
        x = generate(text, params, seed)
        dur = len(x) / sr
        limit = 2.0 + len(text) / 8.0
        retried = False
        if dur > limit or (dur < 0.25 and len(text) > 8):
            p2 = dict(params or {})
            p2["temperature"] = max(0.5, float(p2.get("temperature", 0.8)) - 0.1)
            y = generate(text, p2, seed + 7919)
            if abs(len(y) / sr - len(text) / 14.0) < abs(dur - len(text) / 14.0):
                x = y
            retried = True
        return x, retried

    def do_page(req):
        t_page = time.time()
        use_voice(req.get("voice"), req.get("reference"))
        cache_dir = req.get("cache_dir")
        if cache_dir:
            os.makedirs(cache_dir, exist_ok=True)
        pieces, info = [], []
        for c in req.get("chunks") or []:
            text = (c.get("text") or "").strip()
            if not text:
                continue
            key = c.get("key")
            path = os.path.join(cache_dir, f"{key}.wav") if cache_dir and key else None
            t0 = time.time()
            cached, retried = False, False
            if path and os.path.isfile(path):
                x, _ = nt.read_wav(path)
                cached = True
            else:
                x, retried = synth_chunk(text, c.get("params"), int(c.get("seed", 0)))
                if path:
                    nt.write_wav(path, x, sr)
            pieces.append((x, c.get("pause_before_ms", 0)))
            info.append({"key": key, "cached": cached, "retried": retried, "raw_seconds": round(len(x) / sr, 2),
                         "elapsed": round(time.time() - t0, 2)})
        out = req["out"]
        nt.write_wav(out, nt.assemble_page(pieces, sr, req.get("lead_ms", 0), req.get("tail_ms", 0)), sr)
        tempo = nt.apply_tempo(out, float(req.get("tempo") or 1.0), sr)
        return {"path": out, "duration": round(nt.wav_duration(out), 3), "elapsed": round(time.time() - t_page, 2),
                "sr": sr, "chunks": info, "tempo": tempo, "device": device}

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception:
            send({"ok": False, "message": "bad request"})
            continue
        rid = req.get("id")
        op = req.get("op")
        if op == "shutdown":
            send({"id": rid, "ok": True})
            break
        try:
            if op == "page":
                res = do_page(req)
            elif op == "ping":
                res = {"device": device}
            else:
                raise RuntimeError(f"unknown op {op}")
            send({"id": rid, "ok": True, **res})
        except Exception as e:
            send({"id": rid, "ok": False, "message": f"{type(e).__name__}: {e}", "trace": traceback.format_exc()[-1500:]})
            if device == "cuda":
                try:
                    torch.cuda.empty_cache()
                except Exception:
                    pass
    return 0


def probe() -> int:
    exec(compile(_PROBE, "probe", "exec"), {})
    return 0


# ----------------------------------------------------------------------------- client

class Worker:
    """One long-lived Chatterbox process for a whole narration job (the model loads once)."""

    def __init__(self, require_installed: bool = True, log=None):
        self.require_installed = require_installed
        self.log = log or (lambda s: print(f"[chatterbox] {s}", file=sys.stderr, flush=True))
        self.proc = None
        self.ready_info: dict = {}
        self._lines: queue.Queue = queue.Queue()
        self._stderr: list = []
        self._n = 0

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    def start(self, timeout: float = 900.0) -> dict:
        if self.require_installed and not engine_installed():
            raise RuntimeError(f'The Chatterbox engine is not installed. In Settings → Audio, set up "{PACK_NAME}".')
        if not engine_python_present():
            raise RuntimeError("The Chatterbox environment is missing; run its setup again.")
        missing = missing_weights()
        if missing:
            raise RuntimeError(f'Chatterbox voice files are missing ({", ".join(missing)}). In Settings → Audio, download "{PACK_NAME}".')
        self.proc = subprocess.Popen(
            [ENGINE_PY, os.path.join(HERE, "chatterbox_tts.py"), "serve"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            cwd=ENGINE_DIR, env=_worker_env(), text=True, encoding="utf-8", errors="replace",
            bufsize=1, creationflags=_NO_WINDOW,
        )

        def pump_out():
            for line in self.proc.stdout:  # type: ignore[union-attr]
                line = line.strip()
                if not line:
                    continue
                try:
                    self._lines.put(json.loads(line))
                except Exception:
                    self.log(line)
            self._lines.put(None)

        def pump_err():
            for line in self.proc.stderr:  # type: ignore[union-attr]
                line = line.rstrip()
                if line:
                    self._stderr = (self._stderr + [line])[-40:]

        threading.Thread(target=pump_out, daemon=True).start()
        threading.Thread(target=pump_err, daemon=True).start()
        msg = self._next(timeout)
        if not msg or msg.get("type") != "ready":
            why = (msg or {}).get("message") or self._why()
            self.close()
            raise RuntimeError(why)
        self.ready_info = msg
        self.log(f"ready on {msg.get('device')} {msg.get('gpu') or ''} in {msg.get('load_s')} s")
        return msg

    def _why(self) -> str:
        tail = [l for l in self._stderr if l.strip()]
        return "Chatterbox stopped: " + (" | ".join(tail[-4:]) if tail else "no output")

    def _next(self, timeout: float):
        try:
            return self._lines.get(timeout=timeout)
        except queue.Empty:
            return None

    def request(self, obj: dict, timeout: float = 1800.0) -> dict:
        if not self.alive():
            raise RuntimeError(self._why())
        self._n += 1
        obj = {**obj, "id": self._n}
        self.proc.stdin.write(json.dumps(obj) + "\n")  # type: ignore[union-attr]
        self.proc.stdin.flush()  # type: ignore[union-attr]
        deadline = time.time() + timeout
        while True:
            msg = self._next(max(1.0, deadline - time.time()))
            if msg is None:
                if not self.alive() or time.time() >= deadline:
                    raise RuntimeError(self._why() if not self.alive() else "Chatterbox timed out")
                continue
            if msg.get("id") == self._n:
                if not msg.get("ok"):
                    raise RuntimeError(msg.get("message") or "Chatterbox failed")
                return msg

    def page(self, out: str, voice: str, chunks: list, reference: str | None = None, cache_dir: str | None = None,
             lead_ms: int = 0, tail_ms: int = 0, tempo: float = 1.0) -> dict:
        ref = reference if reference is not None else reference_path(voice)
        # The worker runs in the engine folder: send absolute paths.
        absolute = lambda p: os.path.abspath(p) if p else p  # noqa: E731
        return self.request({"op": "page", "out": absolute(out), "voice": voice, "reference": absolute(ref), "chunks": chunks,
                             "cache_dir": absolute(cache_dir), "lead_ms": lead_ms, "tail_ms": tail_ms, "tempo": tempo})

    def close(self) -> None:
        if not self.proc:
            return
        try:
            if self.alive():
                self.proc.stdin.write(json.dumps({"op": "shutdown"}) + "\n")  # type: ignore[union-attr]
                self.proc.stdin.flush()  # type: ignore[union-attr]
                self.proc.wait(timeout=15)
        except Exception:
            pass
        if self.alive():
            try:
                self.proc.kill()
            except Exception:
                pass
        self.proc = None


def assemble_cached(out: str, chunks: list, cache_dir: str | None, lead_ms: int = 0, tail_ms: int = 0,
                    tempo: float = 1.0) -> dict | None:
    """Builds a page from cached chunks only, in this process (no model, no GPU). Returns
    None when a chunk is not cached (or numpy is missing): the worker has to make it."""
    nt = _load_narration()
    if nt.np is None or not cache_dir:
        return None
    paths = [os.path.join(cache_dir, f"{c.get('key')}.wav") for c in chunks if (c.get("text") or "").strip()]
    if not paths or not all(c.get("key") for c in chunks) or not all(os.path.isfile(p) for p in paths):
        return None
    t0 = time.time()
    pieces, info, sr = [], [], None
    for c, path in zip([c for c in chunks if (c.get("text") or "").strip()], paths):
        x, rate = nt.read_wav(path)
        if sr is None:
            sr = rate
        elif rate != sr:
            return None
        pieces.append((x, c.get("pause_before_ms", 0)))
        info.append({"key": c.get("key"), "cached": True, "retried": False, "raw_seconds": round(len(x) / rate, 2), "elapsed": 0.0})
    nt.write_wav(out, nt.assemble_page(pieces, sr, lead_ms, tail_ms), sr)
    applied = nt.apply_tempo(out, tempo, sr)
    return {"path": out, "duration": round(nt.wav_duration(out), 3), "elapsed": round(time.time() - t0, 2), "sr": sr,
            "chunks": info, "tempo": applied, "device": "cache"}


def synthesize_one(text: str, output_path: str, voice: str, speed: float = 1.0, kind: str = "other",
                   intensity: float = 3.0, mood: str = "", reference: str | None = None) -> dict:
    """One clip (previews): chunked, styled and loudness-normalised like a chapter page."""
    nt = _load_narration()
    units = [{"text": text, "kind": kind, "mood": mood, "intensity": intensity, "line_kind": "narration"}]
    chunks = []
    for i, c in enumerate(nt.plan_chunks(units)):
        params = nt.chatterbox_params(c["kind"], c["intensity"], c["line_kind"])
        chunks.append({"text": c["synth_text"], "params": params, "seed": nt.segment_seed(0, i, c["text"]),
                       "pause_before_ms": c["pause_before_ms"]})
    with Worker() as w:
        res = w.page(output_path, voice, chunks, reference=reference, lead_ms=nt.PAGE_LEAD_MS, tail_ms=nt.PAGE_TAIL_MS,
                     tempo=float(speed or 1.0) * nt.chatterbox_tempo(kind))
    res["loudness"] = nt.normalize_chapter([output_path])
    return res


def main() -> int:
    cmd = sys.argv[1].lower() if len(sys.argv) > 1 else ""
    if cmd == "install":
        return install(force="--force" in sys.argv)
    if cmd == "status":
        emit(status())
        return 0
    if cmd == "make-refs":
        emit({"type": "done", "references": make_references()})
        return 0
    if cmd == "serve":
        return serve()
    if cmd == "probe":
        return probe()
    if cmd == "synth" and len(sys.argv) > 2:
        with open(sys.argv[2], "r", encoding="utf-8") as f:
            a = json.load(f)
        try:
            res = synthesize_one(a["text"], a["output_path"], a.get("voice", "cb_narrator_f"), float(a.get("speed", 1.0)),
                                 a.get("kind", "other"), float(a.get("intensity", 3)), a.get("mood", ""), a.get("reference"))
            emit({"type": "done", **res})
            return 0
        except Exception as e:
            emit({"type": "error", "message": str(e)})
            return 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
