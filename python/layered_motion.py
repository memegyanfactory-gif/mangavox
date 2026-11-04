#!/usr/bin/env python3
"""
Layered motion pack: 2.5D animated panels for the recap video (the "layered" look option).

A panel is cut into layers and animated the way a motion designer would: characters on their
own layers (SAM2 mattes prompted from read.json), lettering kept rigid on top (an automatic
bubble / lettering detector, layered_plan.detect_lettering), a clean background plate behind
every cut-out (LaMa), a depth map (Depth Anything V2) for a 2.5D parallax, and masked local
motion chosen from the scene (layered_plan.motion_plan): breathing, whip / chain sway, blood
and debris drift, a shockwave with rubble and floor dust on a big hit, streaks along speed
lines. Faces are never warped and nothing narrated leaves the panel: the panel itself stays
the same rectangle; only what is inside it moves, by a few percent at most.

The pack lives in `<data>/models/vfx/`: its own Python (`venv`, PyTorch with CUDA), the model
weights (pinned Hugging Face revisions, checked by size and SHA-256) and `mangavox-vfx.json`,
written last by `install` once everything loaded on the GPU. Delete models removes it all.

This file runs in two interpreters:
  * any Python 3.9+ (the app, the renderer): install, status, remove. Stdlib only.
  * the pack's venv: run (prep + animate a batch of clips), probe.

Usage:
  python layered_motion.py install        JSON lines: progress / info / done / error
  python layered_motion.py status         one JSON object
  python layered_motion.py remove         deletes <data>/models/vfx
  python layered_motion.py run jobs.json  (pack venv) JSON lines; see run()
  python layered_motion.py probe          (pack venv) imports + CUDA + models load
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import layered_plan as lp  # noqa: E402

BASE_DIR = lp.data_dir()
PACK_DIR = lp.pack_dir(BASE_DIR)
VENV_DIR = os.path.join(PACK_DIR, "venv")
VENV_PY = lp.venv_python(PACK_DIR)
MARKER = os.path.join(PACK_DIR, "mangavox-vfx.json")
UV_CACHE_DIR = os.path.join(BASE_DIR, "uv-cache")
UV_PYTHON_INSTALL_DIR = os.path.join(BASE_DIR, "uv-python")
UV_BIN = os.path.join(BASE_DIR, "bin", "uv.exe" if os.name == "nt" else "uv")
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
_BELOW_NORMAL = getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)

PYTHON_VERSION = "3.12"
TORCH_PINS = ["torch==2.11.0", "torchvision==0.26.0"]
CUDA_INDEX = "https://download.pytorch.org/whl/cu128"
DEPS = ["transformers==5.22.1", "safetensors>=0.5", "numpy>=1.26", "pillow>=10"]
# About 3 GB downloaded, 4.6 GB on disk (PyTorch with CUDA is most of it).
VENV_BYTES = 4_600_000_000
TORCH_DOWNLOAD = 3_000_000_000
# What the PyTorch step's progress counts: uv unpacks the wheels into its cache as they arrive,
# so the cache grows to about this (not the download size).
TORCH_UNPACKED = 4_400_000_000

# (folder, repo, revision, file, bytes, sha256 or None for small config files). All three are
# Apache-2.0. LaMa is the TorchScript export of the official big-lama weights.
WEIGHTS = [
    ("sam2.1-hiera-small", "facebook/sam2.1-hiera-small", "ee5bba1d82bb8749febdf90f45e84b687142ba03", "config.json", 5698, None),
    ("sam2.1-hiera-small", "facebook/sam2.1-hiera-small", "ee5bba1d82bb8749febdf90f45e84b687142ba03", "preprocessor_config.json", 683, None),
    ("sam2.1-hiera-small", "facebook/sam2.1-hiera-small", "ee5bba1d82bb8749febdf90f45e84b687142ba03", "processor_config.json", 95, None),
    ("sam2.1-hiera-small", "facebook/sam2.1-hiera-small", "ee5bba1d82bb8749febdf90f45e84b687142ba03", "model.safetensors", 184305280,
     "0a4067b11ce1e23d5229203f11c718a823060d15a4b23fa2372a7d4b77cbbc60"),
    ("depth-anything-v2-small", "depth-anything/Depth-Anything-V2-Small-hf", "5426e4f0f36572d16453bbda7a8389317b1bef99", "config.json", 950, None),
    ("depth-anything-v2-small", "depth-anything/Depth-Anything-V2-Small-hf", "5426e4f0f36572d16453bbda7a8389317b1bef99", "preprocessor_config.json", 775, None),
    ("depth-anything-v2-small", "depth-anything/Depth-Anything-V2-Small-hf", "5426e4f0f36572d16453bbda7a8389317b1bef99", "model.safetensors", 99173660,
     "3152477ce0d8d6978d76b995120de97cb5b928701fd0f817769f59e249a16b70"),
    ("big-lama", "fashn-ai/LaMa", "da55aaf971ce271b42eddca79831e0f646c7711e", "big-lama.pt", 205803670,
     "7ba7aa7ac37a4d41fdbbeba3a2af7ead18058552997e3a3cd1a3b2210c9e6b4c"),
]
WEIGHT_BYTES = sum(w[4] for w in WEIGHTS)
FPS = 30


def emit(obj):
    print(json.dumps(obj), flush=True)


def weight_path(folder, name):
    return os.path.join(PACK_DIR, folder, name)


def weight_url(repo, rev, name):
    return f"https://huggingface.co/{repo}/resolve/{rev}/{name}"


# ----------------------------------------------------------------------------- readiness

def _dir_size(path):
    total = 0
    for dirpath, _, files in os.walk(path):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(dirpath, f))
            except OSError:
                pass
    return total


def missing_weights():
    out = []
    for folder, _, _, name, size, _ in WEIGHTS:
        p = weight_path(folder, name)
        try:
            have = os.path.getsize(p)
        except OSError:
            have = -1
        if have != size:
            out.append(f"{folder}/{name}")
    return out


def status():
    info = lp.pack_marker(PACK_DIR)
    py, why = lp.pack_python(root=BASE_DIR)
    return {
        "installed": bool(py) and not missing_weights(),
        "reason": why,
        "python": VENV_PY,
        "dir": PACK_DIR,
        "sizeBytes": _dir_size(PACK_DIR) if os.path.isdir(PACK_DIR) else 0,
        "downloadBytes": TORCH_DOWNLOAD + WEIGHT_BYTES,
        "diskBytes": VENV_BYTES + WEIGHT_BYTES,
        "missingWeights": missing_weights(),
        "gpu": info.get("gpu"),
        "torch": info.get("torch"),
        "installedAt": info.get("installed_at"),
    }


# ----------------------------------------------------------------------------- install

def _uv():
    return UV_BIN if os.path.isfile(UV_BIN) else shutil.which("uv")


def _env():
    env = os.environ.copy()
    env.update({"UV_CACHE_DIR": UV_CACHE_DIR, "UV_PYTHON_INSTALL_DIR": UV_PYTHON_INSTALL_DIR, "UV_NO_PROGRESS": "1",
                "PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8", "HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1"})
    env.pop("VIRTUAL_ENV", None)
    return env


def _nvidia_gpu():
    smi = shutil.which("nvidia-smi")
    if not smi and os.name == "nt":
        cand = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "nvidia-smi.exe")
        smi = cand if os.path.isfile(cand) else None
    if not smi:
        return None
    try:
        r = subprocess.run([smi, "--query-gpu=name", "--format=csv,noheader"], capture_output=True, text=True, timeout=20, creationflags=_NO_WINDOW)
        names = (r.stdout or "").strip().splitlines()
        return names[0].strip() if r.returncode == 0 and names else None
    except Exception:
        return None


def _run(cmd, label, n, total, expect_bytes=0, timeout=7200):
    """Runs an installer command, its lines as progress (and download growth every 10 s)."""
    import queue
    import threading
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
                            env=_env(), creationflags=_NO_WINDOW | _BELOW_NORMAL)
    lines = queue.Queue()

    def pump():
        for line in proc.stdout:
            lines.put(line.rstrip())
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()
    tail, started, beat = [], time.time(), time.time()
    base = _dir_size(UV_CACHE_DIR) if expect_bytes and os.path.isdir(UV_CACHE_DIR) else 0
    while True:
        try:
            line = lines.get(timeout=10)
        except queue.Empty:
            line = ""
        if line is None:
            break
        now = time.time()
        if now - started > timeout:
            proc.kill()
            raise RuntimeError(f"{label} took too long")
        if line:
            tail = (tail + [line])[-20:]
            emit({"type": "progress", "stage": "install", "current": n, "total": total, "message": f"{label}: {line.strip()[:160]}"})
            beat = now
        elif now - beat >= 10:
            grown = (_dir_size(UV_CACHE_DIR) - base) if expect_bytes and os.path.isdir(UV_CACHE_DIR) else 0
            msg = {"type": "progress", "stage": "install", "current": n, "total": total,
                   "message": f"{label}: {min(grown, expect_bytes) / 1e9:.2f} of about {expect_bytes / 1e9:.1f} GB unpacked" if grown > 0 else f"{label}: working"}
            if grown > 0 and expect_bytes:
                msg["percent"] = round(min(99.0, grown / expect_bytes * 100), 1)
            emit(msg)
            beat = now
    code = proc.wait()
    if code != 0:
        raise RuntimeError(f"{label} failed (exit {code}): " + " | ".join(tail[-6:])[-700:])


def _make_venv(n, total):
    os.makedirs(PACK_DIR, exist_ok=True)
    uv = _uv()
    if uv:
        _run([uv, "venv", "--seed", "--clear", "--python", PYTHON_VERSION, VENV_DIR], f"Python {PYTHON_VERSION}", n, total)
        return
    py = shutil.which("py")
    for v in ("3.12", "3.13", "3.11", "3.10"):
        if py:
            r = subprocess.run([py, f"-{v}", "-m", "venv", "--clear", VENV_DIR], capture_output=True, text=True, timeout=600, creationflags=_NO_WINDOW)
            if r.returncode == 0 and os.path.isfile(VENV_PY):
                return
    if (3, 10) <= sys.version_info[:2] <= (3, 13):
        r = subprocess.run([sys.executable, "-m", "venv", "--clear", VENV_DIR], capture_output=True, text=True, timeout=600, creationflags=_NO_WINDOW)
        if r.returncode == 0 and os.path.isfile(VENV_PY):
            return
    raise RuntimeError("The layered motion pack needs Python 3.10-3.13. Install uv (winget install -e --id astral-sh.uv) and try again.")


def _pip(packages, label, n, total, index=None, expect_bytes=0):
    uv = _uv()
    cmd = [uv, "pip", "install", "--python", VENV_PY, *packages] if uv else [VENV_PY, "-m", "pip", "install", "--disable-pip-version-check", *packages]
    if index:
        cmd += ["--index-url", index]
    _run(cmd, label, n, total, expect_bytes)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(folder, repo, rev, name, size, digest, n, total, before=0, all_bytes=0):
    """Resumable download (`.part` + HTTP Range) into the pack, checked by size and SHA-256.
    The progress percent is over all the step's downloads (`before` bytes of `all_bytes` came
    before this file), so the bar does not start over with every file."""
    dest = weight_path(folder, name)
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    if os.path.isfile(dest) and os.path.getsize(dest) == size and (not digest or sha256(dest) == digest):
        return
    part = dest + ".part"
    last = ""
    for attempt in range(6):
        if attempt:
            time.sleep(min(30, 2 ** attempt))
        have = os.path.getsize(part) if os.path.isfile(part) else 0
        if have > size:
            os.remove(part)
            have = 0
        headers = {"User-Agent": "MangaVox/1.0"}
        if have:
            headers["Range"] = f"bytes={have}-"
        try:
            with urllib.request.urlopen(urllib.request.Request(weight_url(repo, rev, name), headers=headers), timeout=60) as resp:
                resumed = resp.status == 206
                done = have if resumed else 0
                t0 = tick = time.time()
                with open(part, "ab" if resumed else "wb") as out:
                    while True:
                        chunk = resp.read(1 << 20)
                        if not chunk:
                            break
                        out.write(chunk)
                        done += len(chunk)
                        if time.time() - tick >= 1.0:
                            tick = time.time()
                            rate = (done - (have if resumed else 0)) / max(0.001, tick - t0)
                            emit({"type": "progress", "stage": "download", "current": n, "total": total, "file": name,
                                  "percent": round((before + done) / max(all_bytes, before + size) * 100, 1), "completed": done, "totalBytes": size,
                                  "message": f"Downloading {folder}/{name}: {done / 1e6:.0f} of {size / 1e6:.0f} MB ({rate / 1e6:.0f} MB/s)"})
            if os.path.getsize(part) != size:
                last = f"{name} ended early"
                continue
            if digest and sha256(part) != digest:
                os.remove(part)
                last = f"{name} failed its checksum"
                continue
            os.replace(part, dest)
            return
        except urllib.error.HTTPError as e:
            last = f"HTTP {e.code}"
            if e.code == 416 and os.path.isfile(part):
                os.remove(part)
            elif 400 <= e.code < 500 and e.code not in (408, 429):
                break
        except Exception as e:
            last = str(e)[:200]
    raise RuntimeError(f"{folder}/{name} could not be downloaded: {last}. Run the install again to resume.")


def _venv_json(args, timeout=900):
    try:
        r = subprocess.run([VENV_PY, os.path.abspath(__file__), *args], capture_output=True, text=True, timeout=timeout,
                           env=_env(), creationflags=_NO_WINDOW | _BELOW_NORMAL, cwd=HERE)
        lines = [l for l in (r.stdout or "").splitlines() if l.strip().startswith("{")]
        if lines:
            return json.loads(lines[-1])
        return {"ok": False, "error": ((r.stderr or "") + (r.stdout or "")).strip()[-800:]}
    except Exception as e:
        return {"ok": False, "error": str(e)}


def install():
    """Sets up the pack end to end. Emits JSON lines; returns the exit code."""
    total = 5
    t_start = time.time()

    def step(n, msg):
        emit({"type": "progress", "stage": "install", "current": n, "total": total, "message": msg})

    try:
        gpu = _nvidia_gpu()
        if not gpu:
            raise RuntimeError("Layered motion runs on an NVIDIA graphics card (CUDA), and none was found.")
        step(1, f"Preparing Python {PYTHON_VERSION} for the layered motion pack…")
        if not os.path.isfile(VENV_PY):
            _make_venv(1, total)
        step(2, f"Installing PyTorch with CUDA for {gpu} (about 3 GB download)…")
        _pip(TORCH_PINS, "PyTorch", 2, total, index=CUDA_INDEX, expect_bytes=TORCH_UNPACKED)
        step(3, "Installing the model libraries…")
        _pip([*TORCH_PINS, *DEPS], "Libraries", 3, total, expect_bytes=200_000_000)
        uv = _uv()
        if uv:
            # uv hard-links the venv to its cache: dropping the big wheels there leaves one copy.
            subprocess.run([uv, "cache", "clean", "torch", "torchvision", "nvidia-cudnn-cu12", "nvidia-cublas-cu12", "triton"],
                           capture_output=True, timeout=600, env=_env(), creationflags=_NO_WINDOW)
        step(4, f"Downloading the models ({WEIGHT_BYTES / 1e6:.0f} MB)…")
        before = 0
        for folder, repo, rev, name, size, digest in WEIGHTS:
            download(folder, repo, rev, name, size, digest, 4, total, before, WEIGHT_BYTES)
            before += size
        step(5, "Checking that the models load on the graphics card…")
        probe_out = _venv_json(["probe"], 900)
        if not probe_out.get("ok"):
            raise RuntimeError(f"The pack installed but does not load: {probe_out.get('error', 'unknown error')}")
        info = {"ok": True, "gpu": probe_out.get("gpu"), "torch": probe_out.get("torch"), "python": probe_out.get("python"),
                "installed_at": time.strftime("%Y-%m-%dT%H:%M:%S"), "install_seconds": round(time.time() - t_start)}
        with open(MARKER, "w", encoding="utf-8") as f:
            json.dump(info, f, indent=1)
        emit({"type": "done", "message": f"Layered motion pack installed (on {info['gpu']}).", **info})
        return 0
    except Exception as e:
        emit({"type": "error", "message": str(e)[-900:]})
        return 1


def remove():
    try:
        if os.path.isdir(PACK_DIR):
            shutil.rmtree(PACK_DIR)
        emit({"type": "done", "message": "Layered motion pack removed."})
        return 0
    except Exception as e:
        emit({"type": "error", "message": f"Could not remove {PACK_DIR}: {e}"})
        return 1


# ----------------------------------------------------------------------------- models (venv)

class Models:
    """Loads one model at a time on the GPU (and frees it), so the pack fits beside the app."""

    def __init__(self):
        import torch
        self.torch = torch
        self.dev = "cuda"

    def free(self):
        import gc
        gc.collect()
        self.torch.cuda.empty_cache()

    def sam(self):
        from transformers import Sam2Model, Sam2Processor
        path = os.path.join(PACK_DIR, "sam2.1-hiera-small")
        return Sam2Processor.from_pretrained(path), Sam2Model.from_pretrained(path).to(self.dev).eval()

    def lama(self):
        return self.torch.jit.load(weight_path("big-lama", "big-lama.pt"), map_location=self.dev).eval()

    def depth(self):
        from transformers import AutoImageProcessor, AutoModelForDepthEstimation
        path = os.path.join(PACK_DIR, "depth-anything-v2-small")
        return AutoImageProcessor.from_pretrained(path), AutoModelForDepthEstimation.from_pretrained(path).to(self.dev).eval()


def probe():
    out = {"python": sys.version.split()[0]}
    try:
        import numpy as np
        import torch
        out["torch"] = torch.__version__
        out["cuda"] = bool(torch.cuda.is_available())
        if not out["cuda"]:
            raise RuntimeError("PyTorch does not see the graphics card")
        out["gpu"] = torch.cuda.get_device_name(0)
        from PIL import Image
        m = Models()
        img = Image.fromarray((np.random.default_rng(0).random((96, 128, 3)) * 255).astype(np.uint8))
        proc, sam = m.sam()
        with torch.no_grad():
            inp = proc(images=img, input_boxes=[[[10, 10, 90, 80]]], return_tensors="pt").to("cuda")
            sam(**inp, multimask_output=False)
        del sam
        m.free()
        lama = m.lama()
        with torch.no_grad():
            lama(torch.zeros(1, 3, 64, 64, device="cuda"), torch.zeros(1, 1, 64, 64, device="cuda"))
        del lama
        m.free()
        proc, dep = m.depth()
        with torch.no_grad():
            dep(**proc(images=img, return_tensors="pt").to("cuda"))
        out["ok"] = True
    except Exception as e:
        out["ok"] = False
        out["error"] = f"{type(e).__name__}: {e}"
    print(json.dumps(out), flush=True)
    return 0 if out.get("ok") else 1


# ----------------------------------------------------------------------------- prep (venv)

ANALYSIS_SIDE = 520
PREP_SIDE = 1600


def _dilate(m, r):
    from PIL import Image, ImageFilter
    import numpy as np
    if r <= 0:
        return m
    return np.asarray(Image.fromarray(m.astype(np.uint8) * 255).filter(ImageFilter.MaxFilter(2 * r + 1))) > 127


def _close(m, r):
    from PIL import Image, ImageFilter
    import numpy as np
    im = Image.fromarray(m.astype(np.uint8) * 255).filter(ImageFilter.MaxFilter(2 * r + 1)).filter(ImageFilter.MinFilter(2 * r + 1))
    return np.asarray(im) > 127


def _resize_mask(m, size):
    from PIL import Image
    import numpy as np
    return np.asarray(Image.fromarray(m.astype(np.uint8) * 255).resize(size, Image.BILINEAR)) > 127


def _sam_masks(models, prepped):
    """SAM2 pass for every panel to prep: character mattes (read.json prompts, gated), lettering
    (automatic detector, minus runs lying on a character), and a long thin object when the
    scene can have one."""
    import numpy as np
    import torch
    from PIL import Image
    proc, sam = models.sam()

    emb = None

    def run(img, box=None, points=None, labels=None, multi=True, objects=None):
        """One prompt (a box and / or points), or `objects` single-point prompts at once, on
        the panel's image embedding (encoded once per panel)."""
        kw = {}
        if box is not None:
            kw["input_boxes"] = [[list(map(float, box))]]
        if points:
            kw["input_points"] = [[[list(map(float, p)) for p in points]]]
            kw["input_labels"] = [[labels]]
        if objects:
            kw["input_points"] = [[[list(map(float, p))] for p in objects]]
            kw["input_labels"] = [[[1] for _ in objects]]
        inp = proc(images=img, return_tensors="pt", **kw).to("cuda")
        inp.pop("pixel_values", None)
        with torch.no_grad():
            out = sam(**inp, image_embeddings=emb, multimask_output=multi)
        masks = proc.post_process_masks(out.pred_masks.cpu(), inp["original_sizes"])[0].numpy().astype(bool)
        scores = out.iou_scores[0].float().cpu().numpy()
        if objects:
            return masks[:, 0], scores[:, 0]
        return masks[0], scores[0]

    for p in prepped:
        t0 = time.time()
        img = p["img"]
        w, h = img.size
        with torch.no_grad():
            emb = sam.get_image_embeddings(proc(images=img, return_tensors="pt").to("cuda")["pixel_values"])
        small = img.convert("L").resize(p["asize"], Image.BILINEAR)
        gray = np.asarray(small).astype(np.float32) / 255
        sx, sy = p["asize"][0] / w, p["asize"][1] / h
        prompts = lp.char_prompts(p["panel"], w, h)
        faces_small = [(f[0] * sx, f[1] * sy, f[2] * sx, f[3] * sy) for f in (q["face"] for q in prompts)]
        text_small = lp.detect_lettering(gray, faces_small)
        text = _resize_mask(text_small, (w, h))
        layers, dropped, union = [], [], np.zeros((h, w), bool)
        for q in prompts:
            masks, scores = run(img, q["box"], [q["point"]], [1])
            best = None
            for k in np.argsort(-scores):
                m = lp.fill_holes(_close(masks[k], max(1, round(min(w, h) / 300)))) & ~text
                st = lp.mask_stats(m, q["face"], float(scores[k]))
                ok, why = lp.mask_gate(st)
                if ok:
                    best = (m, st)
                    break
            if best is None:
                dropped.append({"name": q["name"], "reason": why})
                continue
            m, st = best
            m &= ~union
            layers.append({"name": q["name"], "mask": m, "face": list(q["face"]), "stats": st})
            union |= m
        # Lettering runs that lie on a character are art, not text (a bubble stays: it covers).
        if union.any() and text.any():
            tl, n = lp.label(text)
            for i in range(1, n + 1):
                comp = tl == i
                on = (comp & union).sum() / max(1, comp.sum())
                light = float(np.asarray(img.convert("L"))[comp].mean()) / 255 if comp.any() else 0
                if on > 0.5 and light < 0.7:
                    text &= ~comp
        thin = None
        words = lp._words(p["scene"]) + " " + " ".join(str(c.get("description") or "") for c in [p["panel"]] if isinstance(c, dict))
        if lp.THIN_TAGS.search(words):
            pts = [((i + 0.5) / 6 * w, (j + 0.5) / 6 * h) for j in range(6) for i in range(6)]
            pts = [pt for pt in pts if not union[int(pt[1]), int(pt[0])] and not text[int(pt[1]), int(pt[0])]]
            best = None
            if pts:
                masks, scores = run(img, objects=pts, multi=False)
                text_small_m = _resize_mask(text, p["asize"])
                for k in range(len(scores)):
                    if scores[k] < 0.6:
                        continue
                    # Shape checks at analysis size (fast); the winner is measured at full size.
                    m_small = _resize_mask(masks[k], p["asize"]) & ~text_small_m
                    st = lp.thin_stats(m_small)
                    if not lp.thin_object_ok(st):
                        continue
                    if any(masks[k][int(f[1]):int(f[3]), int(f[0]):int(f[2])].mean() > 0.05 for f in (q["face"] for q in prompts)):
                        continue
                    if best is None or st["length"] > best[1]["length"]:
                        best = (k, st)
            if best is not None:
                m = masks[best[0]] & ~text
                st = lp.thin_stats(m)
                ends = st["ends"]
                # The root is the end held by a character (nearest a matte), else the lower end.
                if union.any():
                    ys, xs = np.nonzero(_dilate(union, 4))
                    d = [float(np.min((xs - e[0]) ** 2 + (ys - e[1]) ** 2)) for e in ends]
                    root = ends[int(np.argmin(d))]
                else:
                    root = max(ends, key=lambda e: e[1])
                tip = ends[1] if root is ends[0] else ends[0]
                thin = {"mask": m, "root": [root[0] / w, root[1] / h], "tip": [tip[0] / w, tip[1] / h], "stats": st}
        p.update(text=text, layers=layers, dropped=dropped, union=union, thin=thin, gray=gray)
        p["times"]["sam"] = round(time.time() - t0, 2)
        emb = None
    del sam
    models.free()


def _lama_fill(lama, img, mask):
    import numpy as np
    import torch
    h, w = mask.shape
    ph, pw = (h + 7) // 8 * 8, (w + 7) // 8 * 8
    x = torch.from_numpy(np.pad(img, ((0, ph - h), (0, pw - w), (0, 0)), mode="reflect")).float().permute(2, 0, 1)[None] / 255
    m = torch.from_numpy(np.pad(mask, ((0, ph - h), (0, pw - w)))).float()[None, None]
    with torch.no_grad():
        y = lama(x.cuda(), m.cuda())
    y = (y[0].permute(1, 2, 0).clamp(0, 1).cpu().numpy() * 255 + 0.5).astype(np.uint8)[:h, :w]
    # LaMa can tint its fill (green / magenta specks); a black-and-white panel stays grey.
    i16 = img.astype(np.int16)
    if np.abs(i16[..., 0] - i16[..., 1]).mean() < 4 and np.abs(i16[..., 1] - i16[..., 2]).mean() < 4:
        y = np.repeat((y.astype(np.float32).mean(axis=2, keepdims=True) + 0.5).astype(np.uint8), 3, axis=2)
    out = img.copy()
    out[mask] = y[mask]
    return out


def _lama(models, prepped):
    import numpy as np
    lama = models.lama()
    for p in prepped:
        t0 = time.time()
        arr = np.asarray(p["img"])
        w, h = p["img"].size
        r = max(2, round(min(w, h) / 300))
        clean = _lama_fill(lama, arr, _dilate(p["text"], r + 1)) if p["text"].any() else arr.copy()
        plate = _lama_fill(lama, clean, _dilate(p["union"], 2 * r + 2)) if p["union"].any() else clean.copy()
        p.update(clean=clean, plate=plate)
        p["times"]["lama"] = round(time.time() - t0, 2)
    del lama
    models.free()


def _depth(models, prepped):
    import numpy as np
    import torch
    from PIL import Image, ImageFilter
    proc, model = models.depth()
    for p in prepped:
        t0 = time.time()
        im = Image.fromarray(p["clean"])
        with torch.no_grad():
            out = model(**proc(images=im, return_tensors="pt").to("cuda"))
        z = out.predicted_depth.squeeze().float().cpu().numpy()
        z = np.asarray(Image.fromarray(z).resize(im.size, Image.BILINEAR))
        lo, hi = np.percentile(z, 2), np.percentile(z, 98)
        z = np.clip((z - lo) / max(1e-6, hi - lo), 0, 1)
        z8 = Image.fromarray((z * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(max(im.size) / 60))
        p["depth"] = np.asarray(z8).astype(np.float32) / 255
        p["times"]["depth"] = round(time.time() - t0, 2)
    del model
    models.free()


def _save_prep(p):
    import numpy as np
    from PIL import Image
    d = p["cache"]
    os.makedirs(d, exist_ok=True)
    w, h = p["img"].size
    Image.fromarray(p["text"].astype(np.uint8) * 255).save(os.path.join(d, "text.png"))
    Image.fromarray(p["clean"]).save(os.path.join(d, "clean.png"))
    Image.fromarray(p["plate"]).save(os.path.join(d, "plate.png"))
    Image.fromarray((p["depth"] * 255).astype(np.uint8)).save(os.path.join(d, "depth.png"))
    bg = ~p["union"] & ~p["text"]
    zbg = float(np.median(p["depth"][bg])) if bg.any() else 0.3
    layers = []
    for k, L in enumerate(p["layers"]):
        f = f"char_{k}.png"
        Image.fromarray(L["mask"].astype(np.uint8) * 255).save(os.path.join(d, f))
        z = float(np.median(p["depth"][L["mask"]]))
        layers.append({"name": L["name"], "file": f, "face": [round(v, 1) for v in L["face"]], "area": round(L["stats"]["area"], 4),
                       "depth": round(min(1.0, max(0.3, 0.45 + (z - zbg) * 1.5)), 3),
                       "stats": {k2: (round(v, 4) if isinstance(v, float) else v) for k2, v in L["stats"].items()}})
    asize = p["asize"]
    bg_small = _resize_mask(bg, asize)
    from PIL import Image as _I
    plate_gray = np.asarray(_I.fromarray(p["plate"]).convert("L").resize(asize, _I.BILINEAR)).astype(np.float32) / 255
    analysis = {"speed": lp.speed_lines(plate_gray, bg_small), "impact": lp.impact_centre(plate_gray, bg_small), "thin": None}
    if p["thin"]:
        Image.fromarray(p["thin"]["mask"].astype(np.uint8) * 255).save(os.path.join(d, "thin.png"))
        analysis["thin"] = {"file": "thin.png", "root": [round(v, 4) for v in p["thin"]["root"]], "tip": [round(v, 4) for v in p["thin"]["tip"]]}
    man = {"src": p["src"], "size": [w, h], "text": "text.png", "plate": "plate.png", "clean": "clean.png", "depth": "depth.png",
           "text_share": round(float(p["text"].mean()), 4), "layers": layers, "dropped": p["dropped"], "analysis": analysis,
           "times": p["times"]}
    lp.write_manifest(d, man)
    return man


def prep_panels(models, todo):
    """Prepares every panel in `todo` ({src, panel, scene, cache}): models one after another."""
    from PIL import Image
    prepped = []
    for t in todo:
        img = Image.open(t["src"]).convert("RGB")
        if max(img.size) > PREP_SIDE:
            s = PREP_SIDE / max(img.size)
            img = img.resize((round(img.width * s), round(img.height * s)), Image.LANCZOS)
        s = ANALYSIS_SIDE / max(img.size)
        prepped.append(dict(t, img=img, asize=(max(8, round(img.width * s)), max(8, round(img.height * s))), times={}))
    if not prepped:
        return {}
    _sam_masks(models, prepped)
    _lama(models, prepped)
    _depth(models, prepped)
    out = {}
    for p in prepped:
        p["times"]["total"] = round(sum(p["times"].values()), 2)
        out[p["cache"]] = _save_prep(p)
        emit({"type": "info", "kind": "prep", "panel": os.path.basename(p["src"]), "layers": len(p["layers"]),
              "dropped": p["dropped"], "times": p["times"], "message": f"Layers for {os.path.basename(p['src'])}: "
              f"{len(p['layers'])} character layer(s), {len(p['dropped'])} dropped, {p['times']['total']:.1f} s"})
    return out


# ----------------------------------------------------------------------------- animate (venv)

class Animator:
    """Rebuilds a prepared panel every frame (see the module docstring), on the GPU."""

    def __init__(self, folder, man, size, plan, faces, subject, drift_dir):
        import numpy as np
        import torch
        from PIL import Image, ImageChops, ImageFilter
        self.torch, self.F = torch, torch.nn.functional
        W, H = size
        self.W, self.H, self.plan = W, H, plan
        sw, sh = man["size"]
        up = H / sh

        def rgb(name, sharpen=True):
            im = Image.open(os.path.join(folder, name)).convert("RGB").resize((W, H), Image.LANCZOS)
            if sharpen and up > 1.3:
                im = im.filter(ImageFilter.UnsharpMask(radius=1.6, percent=70, threshold=2))
            return torch.from_numpy(np.asarray(im).astype(np.float32) / 255).permute(2, 0, 1).cuda()

        def matte(name, blur, grow=0, grow_src=0, close=0.0):
            im = Image.open(os.path.join(folder, name)).convert("L")
            if close:
                # Gaps between detected letters closed (made small, they are wide): a letter half
                # in the rigid text and half in the (zoomed) plate shows doubled.
                small = im.resize((max(8, im.width // 4), max(8, im.height // 4)), Image.BILINEAR)
                k = 2 * max(1, round(close * min(small.size))) + 1
                small = small.filter(ImageFilter.MaxFilter(k)).filter(ImageFilter.MinFilter(k))
                small = Image.fromarray(lp.fill_holes(np.asarray(small) > 127).astype(np.uint8) * 255).filter(ImageFilter.MaxFilter(k // 2 * 2 + 1))
                im = ImageChops.lighter(im, small.resize(im.size, Image.BILINEAR).point(lambda v: 255 if v > 127 else 0))
            if grow_src:
                im = im.filter(ImageFilter.MaxFilter(2 * grow_src + 1))
            im = im.resize((W, H), Image.BILINEAR)
            if grow:
                im = im.filter(ImageFilter.MaxFilter(2 * grow + 1))
            im = im.filter(ImageFilter.GaussianBlur(blur))
            return torch.from_numpy(np.asarray(im).astype(np.float32) / 255)[None].cuda()

        def halo(name, frac):
            """The mask grown by `frac` of the panel's short side (made small, it is wide)."""
            im = Image.open(os.path.join(folder, name)).convert("L")
            small = im.resize((max(8, im.width // 4), max(8, im.height // 4)), Image.BILINEAR)
            k = max(1, round(frac * min(small.size)))
            small = small.filter(ImageFilter.MaxFilter(2 * k + 1))
            big = small.resize((W, H), Image.BILINEAR).filter(ImageFilter.GaussianBlur(max(2.0, 2 * k * up)))
            return torch.from_numpy(np.asarray(big).astype(np.float32) / 255).cuda()

        self.orig = self._orig(man["src"], W, H, up)
        self.plate = rgb(man["plate"])
        clean = rgb(man["clean"])
        # The original goes back over everything the clean plate inpainted around the lettering
        # (_lama grows the text mask by r + 1 prep pixels) and over the gaps between letters, else
        # letters next to the detected ones vanish or show doubled against the zoomed plate.
        self.text_a = matte(man["text"], max(1.0, up * 0.8), grow=max(1, round(up)), grow_src=max(2, round(min(sw, sh) / 300)) + 1, close=0.012)
        # Streaks keep clear of the lettering and the bubble around it.
        self.text_halo = halo(man["text"], 0.025)
        # A pixel entrance of the characters (set by animate from the job), else None.
        self.entrance = None
        self.layers = []
        for L in man["layers"]:
            a = matte(L["file"], max(0.9, up * 0.6))
            br = plan.get("breathe") if plan.get("breathe") and plan["breathe"]["layer"] == L["name"] else None
            self.layers.append({"rgb": clean * a, "a": a, "depth": float(L["depth"]), "breathe": br})
        self.layers.sort(key=lambda l: l["depth"])
        z = Image.open(os.path.join(folder, man["depth"])).convert("L").resize((W, H), Image.BILINEAR)
        self.depth = torch.from_numpy(np.asarray(z).astype(np.float32) / 255).cuda()
        thin = (man.get("analysis") or {}).get("thin")
        self.thin_m = matte(thin["file"], 4 * up + 2, grow=max(2, round(3 * up)))[0] if thin and plan.get("sway") else None
        fm = Image.new("L", (W, H), 0)
        from PIL import ImageDraw
        dr = ImageDraw.Draw(fm)
        for f in faces:
            fw, fh = f[2] - f[0], f[3] - f[1]
            dr.rectangle([(f[0] - 0.15 * fw) * W, (f[1] - 0.15 * fh) * H, (f[2] + 0.15 * fw) * W, (f[3] + 0.15 * fh) * H], fill=255)
        self.face_m = torch.from_numpy(np.asarray(fm.filter(ImageFilter.GaussianBlur(6))).astype(np.float32) / 255).cuda()
        ys, xs = torch.meshgrid(torch.arange(H, device="cuda", dtype=torch.float32) + 0.5, torch.arange(W, device="cuda", dtype=torch.float32) + 0.5, indexing="ij")
        self.X, self.Y = xs, ys
        lum = self.plate[0] * 0.3 + self.plate[1] * 0.59 + self.plate[2] * 0.11
        self.light = ((lum - 0.45) / 0.4).clamp(0, 1)
        self.subj = (subject[0] * W, subject[1] * H)
        v = np.array(drift_dir if drift_dir else (0.6, 0.0), float)
        self.dv = v / max(1e-6, np.linalg.norm(v))
        self.px = W / 1000.0
        if plan.get("shock"):
            rng = np.random.default_rng(11)
            n = 30
            side = np.where(rng.random(n) < 0.5, 1.0, -1.0)
            self.puffs = dict(side=side, sp=rng.uniform(0.07, 0.2, n), sz=rng.uniform(0.025, 0.055, n), y0=rng.uniform(0.0, 0.06, n),
                              br=rng.uniform(0.25, 0.5, n), delay=rng.uniform(0, 0.25, n), lift=rng.uniform(-0.05, 0.02, n))

    def _pixel_in(self, s, t):
        """A character layer `s` (premultiplied RGB and alpha) on its entrance (self.entrance:
        {"at", "dur" seconds, "block" px at 1080 rows}): not there before "at" (the clean plate shows
        where it stands), then assembling from pixel blocks that light up one by one and shrink to
        sharp, a little bright as they land. None while it is not there yet."""
        en = self.entrance
        p = (t - en["at"]) / max(1e-3, en["dur"])
        if p >= 1:
            return s
        if p <= 0:
            return None
        torch, F = self.torch, self.F
        b = max(1, round(en["block"] * self.H / 1080 * (1 - p) ** 2))
        if b > 1:
            small = F.avg_pool2d(s[None], b, stride=b, ceil_mode=True)
            gh, gw = small.shape[2], small.shape[3]
            gy, gx = torch.meshgrid(torch.arange(gh, device="cuda", dtype=torch.float32), torch.arange(gw, device="cuda", dtype=torch.float32), indexing="ij")
            # Each block lights at its own moment (a fresh pattern at every block size).
            rnd = torch.remainder(torch.sin(gx * 12.9898 + gy * 78.233 + b * 3.17) * 43758.5453, 1.0)
            small = small * (rnd < p * 1.7).float()
            s = F.interpolate(small, scale_factor=b, mode="nearest")[0, :, :self.H, :self.W]
        glow = 0.35 * (1 - p) ** 2
        return torch.cat([s[:3] + glow * s[3:4], s[3:4]], 0) * min(1.0, p / 0.12)

    def _orig(self, src, W, H, up):
        import numpy as np
        from PIL import Image, ImageFilter
        im = Image.open(src).convert("RGB")
        if max(im.size) > PREP_SIDE:
            s = PREP_SIDE / max(im.size)
            im = im.resize((round(im.width * s), round(im.height * s)), Image.LANCZOS)
        im = im.resize((W, H), Image.LANCZOS)
        if up > 1.3:
            im = im.filter(ImageFilter.UnsharpMask(radius=1.6, percent=70, threshold=2))
        return self.torch.from_numpy(np.asarray(im).astype(np.float32) / 255).permute(2, 0, 1).cuda()

    def _sample(self, img, qx, qy, pad="border"):
        grid = self.torch.stack([qx / self.W * 2 - 1, qy / self.H * 2 - 1], -1)[None]
        return self.F.grid_sample(img[None], grid, mode="bilinear", padding_mode=pad, align_corners=False)[0]

    def frame(self, e, t):
        """The panel at eased progress e (0..1, the camera's) and t seconds into the clip."""
        torch = self.torch
        plan, W, H, X, Y = self.plan, self.W, self.H, self.X, self.Y
        amp = plan["parallax"] * W
        pvx, pvy = (e - 0.5) * 2 * amp * self.dv[0], (e - 0.5) * 2 * amp * self.dv[1]
        cx, cy = self.subj
        over = 1.025 * (1 - 0.004 * e)
        dz = self.depth - 0.5
        qx = cx + (X - cx) / over + 0.35 * pvx - pvx * dz * 0.9
        qy = cy + (Y - cy) / over + 0.35 * pvy - pvy * dz * 0.9
        keep = (1 - self.face_m) * (1 - self.text_a[0])
        if plan.get("drift"):
            c = plan["drift"]["centre"]
            R = 0.45 * math.hypot(W, H)
            rx, ry = X - c[0] * W, Y - c[1] * H
            fall = torch.exp(-(rx * rx + ry * ry) / (R * R)) * keep
            k = plan["drift"]["k"] * e
            qx, qy = qx - rx * k * fall, qy - ry * k * fall
        if plan.get("shock"):
            c = plan["shock"]["centre"]
            k = 0.016 * (1 - math.exp(-t / 0.9))
            # Rubble on the floor only: from a little above the impact down.
            wgt = ((Y - (c[1] - 0.12) * H) / (0.12 * H)).clamp(0, 1) * keep
            qx, qy = qx - (X - c[0] * W) * k * wgt, qy - (Y - c[1] * H) * k * 0.4 * wgt
        # The background holds still around the lettering: the detector can miss a letter next to
        # the ones it found, and a missed letter moving with the plate while its neighbours stay
        # rigid reads as doubled or torn text.
        hold = 1 - self.text_halo
        qx, qy = X + (qx - X) * hold, Y + (qy - Y) * hold
        out = self._sample(self.plate, qx, qy)
        cover = torch.zeros(1, H, W, device="cuda")
        for L in self.layers:
            d = L["depth"]
            sz = 1 + plan["zoom"] * e * d
            px2, py2 = X - pvx * d, Y - pvy * d
            px1, py1 = cx + (px2 - cx) / sz, cy + (py2 - cy) / sz
            if L["breathe"]:
                b = L["breathe"]
                sb = 1 + b["amp"] * math.sin(2 * math.pi * t / b["period"])
                ax, ay = cx, H
                px1, py1 = ax + (px1 - ax) / sb, ay + (py1 - ay) / sb
            s = self._sample(torch.cat([L["rgb"], L["a"]], 0), px1, py1, pad="zeros")
            if self.entrance:
                s = self._pixel_in(s, t)
                if s is None:
                    continue
            out = out * (1 - s[3:4]) + s[:3]
            cover = cover + s[3:4] * (1 - cover)
        if plan.get("streaks"):
            st = plan["streaks"]
            vx, vy = st["vec"]
            a_ = (X * vx + Y * vy) / self.px
            b_ = (-X * vy + Y * vx) / self.px
            lane = torch.floor(b_ / 9.0)

            def hsh(k):
                return torch.frac(torch.sin(lane * 12.9898 + k * 78.233) * 43758.5453)
            act = (hsh(1) < 0.24).float()
            dist = (torch.frac(b_ / 9.0) - 0.5).abs() * 9.0
            prof = (1 - dist / (0.9 + 1.1 * hsh(2))).clamp(0, 1)
            P = 520 + 520 * hsh(3)
            Ld = P * (0.22 + 0.25 * hsh(4))
            pos = torch.remainder(a_ - (900 + 900 * hsh(5)) * t + hsh(6) * P, P)
            dash = (pos / 50).clamp(0, 1) * (1 - ((pos - Ld + 70) / 70).clamp(0, 1))
            s = act * prof * dash * (0.55 + 0.45 * hsh(7))
            wgt = (1 - cover[0]) * (1 - self.text_a[0]) * (1 - self.text_halo) * self.light * min(1.0, t / 0.4)
            out = out * (1 - st["strength"] * s * wgt)[None]
        dx = torch.zeros(H, W, device="cuda")
        dy = torch.zeros(H, W, device="cuda")
        local = False
        if self.thin_m is not None:
            sw = plan["sway"]
            r0 = (sw["root"][0] * W, sw["root"][1] * H)
            r1 = (sw["tip"][0] * W, sw["tip"][1] * H)
            ln = max(1.0, math.hypot(r1[0] - r0[0], r1[1] - r0[1]))
            ux, uy = (r1[0] - r0[0]) / ln, (r1[1] - r0[1]) / ln
            s = (((X - r0[0]) * ux + (Y - r0[1]) * uy) / ln).clamp(0, 1.2)
            disp = sw["amp"] * W * s ** 1.6 * torch.sin(2 * math.pi * (0.9 * t - s * 1.1))
            m = self.thin_m * keep
            dx, dy = dx - uy * disp * m, dy + ux * disp * m
            local = True
        ring = None
        if plan.get("shock") and t < 1.1:
            c = plan["shock"]["centre"]
            R = 0.85 * W * (1 - (1 - min(t / 0.85, 1)) ** 2.4)
            rx, ry = X - c[0] * W, (Y - c[1] * H) / 0.55
            r = torch.sqrt(rx * rx + ry * ry) + 1e-3
            ring = torch.exp(-((r - R) / (0.035 * W)) ** 2) * max(0.0, 1 - t / 1.1) ** 1.3 * (1 - self.face_m)
            dx, dy = dx + rx / r * 0.009 * W * ring, dy + ry / r * 0.009 * W * ring * 0.55
            local = True
        if local:
            out = self._sample(out, X - dx, Y - dy)
        if ring is not None:
            out = out + (1 - out) * (0.28 * ring)[None]
        if plan.get("shock"):
            pf, c = self.puffs, plan["shock"]["centre"]
            dust = torch.zeros(H, W, device="cuda")
            for i in range(len(pf["side"])):
                tt = t - pf["delay"][i]
                if tt <= 0:
                    continue
                prog = 1 - math.exp(-tt / 0.7)
                px_ = (c[0] + pf["side"][i] * (0.08 + pf["sp"][i] * prog * 2.2)) * W
                py_ = (min(0.96, c[1] + 0.12) + pf["y0"][i] + pf["lift"][i] * prog) * H
                rad = pf["sz"][i] * (0.6 + 1.4 * prog) * W
                a = pf["br"][i] * min(1.0, tt / 0.12) * max(0.0, 1 - tt / 2.6)
                dust = dust + a * torch.exp(-((X - px_) ** 2 + (Y - py_) ** 2) / (2 * rad * rad))
            dust = (dust * (1 - self.face_m)).clamp(0, 0.6)
            out = out * (1 - dust) + 0.86 * dust
        out = out * (1 - self.text_a) + self.orig * self.text_a
        return out.clamp(0, 1)


def _grade_fn(grade, glow, W, H):
    """The renderer's still treatment per frame (video_generator.apply_grade and _bloom)."""
    import torch
    import torch.nn.functional as F
    lut = None
    if grade:
        sat, con, bri, sh, hi = grade
        x = torch.linspace(0, 1, 256, device="cuda")
        lut = torch.stack([((x - 0.5) * con + 0.5 + bri + sh[c] * (1 - x) ** 2 + hi[c] * x * x).clamp(0, 1) for c in range(3)])

    def apply(img):
        if grade:
            if abs(grade[0] - 1.0) > 1e-3:
                g = (img[0] * 0.299 + img[1] * 0.587 + img[2] * 0.114)[None]
                img = (g + (img - g) * grade[0]).clamp(0, 1)
            idx = (img * 255 + 0.5).long().clamp(0, 255)
            img = torch.stack([lut[c][idx[c]] for c in range(3)])
        if glow:
            small = F.avg_pool2d(img[None], 4)
            r = max(2.0, small.shape[-1] / 60)
            k = int(r * 3) * 2 + 1
            ax = torch.arange(k, device="cuda", dtype=torch.float32) - k // 2
            gk = torch.exp(-ax ** 2 / (2 * r * r))
            gk = gk / gk.sum()
            pad = k // 2
            sm = F.conv2d(F.pad(small, (pad, pad, 0, 0), mode="replicate"), gk.view(1, 1, 1, k).repeat(3, 1, 1, 1), groups=3)
            sm = F.conv2d(F.pad(sm, (0, 0, pad, pad), mode="replicate"), gk.view(1, 1, k, 1).repeat(3, 1, 1, 1), groups=3)
            glow_img = F.interpolate(sm, size=(H, W), mode="bicubic", align_corners=False)[0].clamp(0, 1) * 0.14
            img = 1 - (1 - img) * (1 - glow_img)
        return img
    return apply


def _encoder(out, W, H, threads):
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS),
           "-i", "-", "-vf", "scale=out_range=pc:out_color_matrix=bt601", "-c:v", "libx264", "-preset", "veryfast", "-crf", "10",
           "-pix_fmt", "yuvj420p", "-color_range", "pc", "-colorspace", "bt470bg", "-threads", str(threads), out]
    return subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=_NO_WINDOW | _BELOW_NORMAL)


def animator(job, man):
    """The clip's Animator, its per-frame grade and its motion plan: (anim, grade, plan). Also used
    by the GPU engine (gpu_render.py), which draws the frames straight into the video."""
    W, H = job["size"]
    faces = [l["face"] for l in man["layers"]]
    sw, sh = man["size"]
    faces = [[f[0] / sw, f[1] / sh, f[2] / sw, f[3] / sh] for f in faces] + [list(f) for f in job.get("faces") or []]
    plan = lp.motion_plan(job.get("scene"), {"layers": man["layers"], **(man.get("analysis") or {})}, job.get("intensity"))
    subject = (0.5, 0.5)
    if man["layers"]:
        main = max(man["layers"], key=lambda l: l["area"])
        f = main["face"]
        subject = ((f[0] + f[2]) / 2 / sw, (f[1] + f[3]) / 2 / sh)
    # The panel as it is now (the cached manifest's path is stale after the data folder moved).
    anim = Animator(job["cache"], dict(man, src=job["src"]), (W, H), plan, faces, subject, job.get("drift"))
    if isinstance(job.get("entrance"), dict) and man["layers"]:
        anim.entrance = job["entrance"]
    return anim, _grade_fn(job.get("grade"), job.get("glow"), W, H), plan


def frame_ease(job, k):
    """The camera's eased progress at frame k of the clip (the job's, else a cosine ease)."""
    ease = job.get("ease") or []
    n = int(job["frames"])
    return float(ease[k]) if k < len(ease) else (0.5 - 0.5 * math.cos(math.pi * k / max(1, n - 1)))


def animate(job, man, threads):
    import torch
    W, H = job["size"]
    anim, grade, plan = animator(job, man)
    tmp = job["out"] + ".part.mp4"
    enc = _encoder(tmp, W, H, threads)
    n = int(job["frames"])
    try:
        for k in range(n):
            img = grade(anim.frame(frame_ease(job, k), k / FPS))
            enc.stdin.write((img.permute(1, 2, 0) * 255 + 0.5).to(torch.uint8).cpu().numpy().tobytes())
        enc.stdin.close()
        err = enc.stderr.read().decode("utf-8", "replace")
        if enc.wait() != 0 or not os.path.isfile(tmp):
            raise RuntimeError(f"encode failed: {err[-300:]}")
        os.replace(tmp, job["out"])
    except Exception:
        enc.kill()
        raise
    return plan


def run(jobs_path):
    """Prepares (cached) and animates a batch of clips. jobs.json:
    {"threads": n, "jobs": [{"id", "src", "panel" (read.json), "scene", "intensity", "cache",
    "size": [w, h], "frames", "ease": [per frame 0..1], "drift": [x, y], "grade", "glow",
    "entrance"? ({"at", "dur", "block"}: the characters assemble from pixel blocks, Animator._pixel_in),
    "faces": [[x0, y0, x1, y1] crop fractions], "out", "prep_only"? (the layers only: the GPU engine
    animates the clip itself, see animator)}]}.
    Emits progress / info lines and a final {"type": "done", "results": {id: {...}}}."""
    import torch
    with open(jobs_path, "r", encoding="utf-8") as f:
        spec = json.load(f)
    threads = int(spec.get("threads") or max(2, (os.cpu_count() or 4) // 2))
    torch.set_num_threads(threads)
    if not torch.cuda.is_available():
        emit({"type": "error", "message": "PyTorch does not see the graphics card"})
        return 1
    torch.cuda.reset_peak_memory_stats()
    jobs = spec.get("jobs") or []
    results = {}
    t0 = time.time()
    models = Models()
    todo, seen = [], set()
    for j in jobs:
        if j["cache"] in seen or lp.read_manifest(j["cache"]):
            continue
        seen.add(j["cache"])
        todo.append({"src": j["src"], "panel": j.get("panel") or {}, "scene": j.get("scene") or {}, "cache": j["cache"]})
    if todo:
        emit({"type": "progress", "stage": "layers", "current": 0, "total": len(jobs), "message": f"Cutting {len(todo)} panel(s) into layers…"})
        try:
            prep_panels(models, todo)
        except Exception as e:
            emit({"type": "info", "kind": "prep_failed", "message": f"Layer prep failed: {type(e).__name__}: {e}"})
    prep_s = time.time() - t0
    for i, j in enumerate(jobs):
        man = lp.read_manifest(j["cache"])
        if not man:
            results[j["id"]] = {"ok": False, "error": "no layers"}
            continue
        if j.get("prep_only"):
            results[j["id"]] = {"ok": True, "prep_only": True, "layers": len(man["layers"]), "prep": man.get("times")}
            continue
        t1 = time.time()
        try:
            plan = animate(j, man, threads)
            results[j["id"]] = {"ok": True, "effects": lp.plan_effects(plan), "layers": len(man["layers"]), "seconds": round(time.time() - t1, 2),
                                "frames": j["frames"], "prep": man.get("times")}
        except Exception as e:
            results[j["id"]] = {"ok": False, "error": f"{type(e).__name__}: {e}"}
        emit({"type": "progress", "stage": "layers", "current": i + 1, "total": len(jobs), "message": f"Animated {i + 1} of {len(jobs)} layered clips"})
    peak = round(torch.cuda.max_memory_allocated() / 2 ** 20)
    emit({"type": "done", "results": results, "prep_seconds": round(prep_s, 2), "seconds": round(time.time() - t0, 2), "vram_peak_mb": peak})
    return 0


def main(argv):
    cmd = argv[1] if len(argv) > 1 else "status"
    if cmd == "install":
        return install()
    if cmd == "status":
        print(json.dumps(status()), flush=True)
        return 0
    if cmd == "remove":
        return remove()
    if cmd == "probe":
        return probe()
    if cmd == "run" and len(argv) > 2:
        return run(argv[2])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
