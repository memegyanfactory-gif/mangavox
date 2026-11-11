#!/usr/bin/env python3
"""
Panel upscaler for the recap video: Real-ESRGAN's anime model (RealESRGAN_x4plus_anime_6B,
BSD-3-Clause) redraws a panel that the video shows larger than it was scanned, instead of the
plain resize that leaves line art soft. A panel is upscaled once and kept (see `cached`), at
the size the video needs, never per frame.

The weights (18 MB) live in the layered motion pack's folder, `<data>/models/vfx/realesrgan-anime`,
and are fetched on first use (pinned by SHA-256). Without the pack, or when anything here
fails, the render resizes as it always did.

This file runs in two interpreters:
  * any Python 3.9+ (the renderer): `wanted`, `cached`, `plan`. Stdlib only.
  * the pack's venv (PyTorch with CUDA): `run`, `upscale_image`.

Usage:
  python upscale.py run jobs.json    (pack venv) JSON lines: progress / done / error
"""
from __future__ import annotations

import hashlib
import json
import os
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import layered_plan as lp  # noqa: E402
try:
    import gpu_power  # noqa: E402
except ImportError:  # an older install without it
    gpu_power = None

# Changing the model or how its picture is saved makes every kept panel again.
VERSION = 1
MODEL_FILE = "RealESRGAN_x4plus_anime_6B.pth"
MODEL_URL = "https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.2.4/" + MODEL_FILE
MODEL_BYTES = 17938799
MODEL_SHA256 = "f872d837d3c90ed2e05227bed711af5671a6fd1c9f7d7e91c911a61f155e99da"
MODEL_SCALE = 4
# A panel shown less than this much larger than its scan is resized as before: the model has
# nothing to add and would only cost time.
MIN_UP = 1.15
# The model draws this much of a panel at a time (pixels of the scan), with an overlap that
# hides the seams: the whole of a tall webtoon panel at once does not fit the card's memory.
TILE = 384
TILE_PAD = 16
# The kept picture is this much larger than the video needs it, for the camera's push-in.
HEADROOM = 1.1


def model_dir(root=None):
    return os.path.join(lp.pack_dir(root), "realesrgan-anime")


def model_path(root=None):
    return os.path.join(model_dir(root), MODEL_FILE)


def wanted(config=None):
    """The upscaler is on: `upscale` in the video settings (default on), MANGAVOX_UPSCALE=0 turns it off."""
    env = os.environ.get("MANGAVOX_UPSCALE")
    if env is not None:
        return env.strip().lower() not in ("0", "off", "false", "no", "")
    config = config if isinstance(config, dict) else {}
    return config.get("upscale", True) is not False


def needs(src_size, shown_size):
    """How much larger than its scan a panel is shown (`shown_size` on screen), when that is
    enough for the model to be worth it; else None."""
    (iw, ih), (fw, fh) = src_size, shown_size
    if iw < 8 or ih < 8:
        return None
    up = max(fw / iw, fh / ih)
    return up if up >= MIN_UP else None


def out_size(src_size, up):
    """The size the upscaled panel is kept at: what the video shows plus headroom, never more
    than the model makes."""
    s = min(MODEL_SCALE, up * HEADROOM)
    return max(2, round(src_size[0] * s)), max(2, round(src_size[1] * s))


def cached(src, size, folder):
    """Where the upscaled copy of `src` at `size` is kept in `folder` (named by the file's
    identity, so an edited or re-read panel is made again)."""
    try:
        st = os.stat(src)
        ident = [os.path.abspath(src), st.st_size, int(st.st_mtime)]
    except OSError:
        ident = [src]
    key = hashlib.sha1(json.dumps([VERSION, ident, list(size)]).encode("utf-8")).hexdigest()[:16]
    return os.path.join(folder, f"{key}.jpg")


def plan(sources, w, h, folder):
    """The jobs for `run` and where each panel's upscaled copy goes: ([{src, dst, size}],
    {src: dst}) for the `sources` a `w`x`h` video shows larger than their scan (a panel fills
    the frame at most, so that is the most it is ever enlarged)."""
    from PIL import Image
    jobs, where = [], {}
    for src in dict.fromkeys(sources):
        try:
            with Image.open(src) as im:
                size = im.size
        except Exception:
            continue
        up = needs(size, (min(w, size[0] * h / size[1]), min(h, size[1] * w / size[0])))
        if up is None:
            continue
        to = out_size(size, up)
        dst = cached(src, to, folder)
        jobs.append({"src": src, "dst": dst, "size": list(to)})
        where[src] = dst
    return jobs, where


def emit(obj):
    print(json.dumps(obj, ensure_ascii=False), flush=True)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def ensure_model(root=None):
    """The weights' path, fetched first when missing. Raises when they cannot be had or are
    not the pinned file."""
    path = model_path(root)
    if os.path.isfile(path) and os.path.getsize(path) == MODEL_BYTES:
        return path
    os.makedirs(os.path.dirname(path), exist_ok=True)
    part = path + ".part"
    req = urllib.request.Request(MODEL_URL, headers={"User-Agent": "mangavox"})
    with urllib.request.urlopen(req, timeout=60) as r, open(part, "wb") as f:
        while True:
            block = r.read(1 << 20)
            if not block:
                break
            f.write(block)
    if os.path.getsize(part) != MODEL_BYTES or sha256(part) != MODEL_SHA256:
        os.remove(part)
        raise RuntimeError("the upscaler's weights did not download whole")
    os.replace(part, path)
    return path


# ------------------------------------------------------------------------------- the model (venv)
def _net():
    """RRDBNet as Real-ESRGAN defines it (6 blocks, 64 features, 32 growth, x4)."""
    import torch
    from torch import nn
    F = nn.functional

    class RDB(nn.Module):
        def __init__(self, nf=64, gc=32):
            super().__init__()
            self.conv1 = nn.Conv2d(nf, gc, 3, 1, 1)
            self.conv2 = nn.Conv2d(nf + gc, gc, 3, 1, 1)
            self.conv3 = nn.Conv2d(nf + 2 * gc, gc, 3, 1, 1)
            self.conv4 = nn.Conv2d(nf + 3 * gc, gc, 3, 1, 1)
            self.conv5 = nn.Conv2d(nf + 4 * gc, nf, 3, 1, 1)

        def forward(self, x):
            a = F.leaky_relu(self.conv1(x), 0.2)
            b = F.leaky_relu(self.conv2(torch.cat((x, a), 1)), 0.2)
            c = F.leaky_relu(self.conv3(torch.cat((x, a, b), 1)), 0.2)
            d = F.leaky_relu(self.conv4(torch.cat((x, a, b, c), 1)), 0.2)
            return self.conv5(torch.cat((x, a, b, c, d), 1)) * 0.2 + x

    class RRDB(nn.Module):
        def __init__(self):
            super().__init__()
            self.rdb1, self.rdb2, self.rdb3 = RDB(), RDB(), RDB()

        def forward(self, x):
            return self.rdb3(self.rdb2(self.rdb1(x))) * 0.2 + x

    class Net(nn.Module):
        def __init__(self):
            super().__init__()
            self.conv_first = nn.Conv2d(3, 64, 3, 1, 1)
            self.body = nn.Sequential(*[RRDB() for _ in range(6)])
            self.conv_body = nn.Conv2d(64, 64, 3, 1, 1)
            self.conv_up1 = nn.Conv2d(64, 64, 3, 1, 1)
            self.conv_up2 = nn.Conv2d(64, 64, 3, 1, 1)
            self.conv_hr = nn.Conv2d(64, 64, 3, 1, 1)
            self.conv_last = nn.Conv2d(64, 3, 3, 1, 1)

        def forward(self, x):
            f = self.conv_first(x)
            f = f + self.conv_body(self.body(f))
            f = F.leaky_relu(self.conv_up1(F.interpolate(f, scale_factor=2, mode="nearest")), 0.2)
            f = F.leaky_relu(self.conv_up2(F.interpolate(f, scale_factor=2, mode="nearest")), 0.2)
            return self.conv_last(F.leaky_relu(self.conv_hr(f), 0.2))

    return Net()


_MODEL = None


def load(root=None):
    """The model on the GPU in half precision (loaded once per process)."""
    global _MODEL
    if _MODEL is None:
        import torch
        state = torch.load(ensure_model(root), map_location="cpu", weights_only=True)
        state = state.get("params_ema") or state.get("params") or state
        net = _net()
        net.load_state_dict(state, strict=True)
        _MODEL = net.eval().half().cuda()
    return _MODEL


def upscale_image(img, size, pace=None):
    """`img` (a PIL RGB image) redrawn by the model and brought to `size` (w, h). `pace` is
    called after every tile (the GPU power governor)."""
    import numpy as np
    import torch
    from PIL import Image
    net = load()
    x = torch.from_numpy(np.array(img.convert("RGB"))).cuda().permute(2, 0, 1)[None].half() / 255
    _, _, h, w = x.shape
    s = MODEL_SCALE
    out = torch.empty((3, h * s, w * s), dtype=torch.float16, device="cuda")
    with torch.inference_mode():
        for y in range(0, h, TILE):
            for x0 in range(0, w, TILE):
                y1, x1 = min(h, y + TILE), min(w, x0 + TILE)
                py0, px0 = max(0, y - TILE_PAD), max(0, x0 - TILE_PAD)
                py1, px1 = min(h, y1 + TILE_PAD), min(w, x1 + TILE_PAD)
                t = net(x[:, :, py0:py1, px0:px1])[0]
                out[:, y * s:y1 * s, x0 * s:x1 * s] = t[:, (y - py0) * s:(y1 - py0) * s, (x0 - px0) * s:(x1 - px0) * s]
                if pace:
                    torch.cuda.synchronize()
                    pace()
        if (size[0], size[1]) != (w * s, h * s):
            out = torch.nn.functional.interpolate(out[None].float(), size=(size[1], size[0]), mode="bicubic", antialias=True, align_corners=False)[0]
        arr = (out.float().clamp(0, 1) * 255 + 0.5).to(torch.uint8).permute(1, 2, 0).cpu().numpy()
    return Image.fromarray(arr)


def run(jobs_path):
    """Upscales every job {src, dst, size} of the JSON file (a list) that is not kept yet."""
    from PIL import Image
    import torch
    with open(jobs_path, encoding="utf-8") as f:
        jobs = json.load(f)
    t0 = time.monotonic()
    gov = gpu_power.governor("layers") if gpu_power else None
    pace = gov.pace if gov is not None else None
    torch.cuda.reset_peak_memory_stats()
    made = 0
    failed = {}
    for n, j in enumerate(jobs):
        dst = j["dst"]
        if not os.path.isfile(dst):
            try:
                img = Image.open(j["src"]).convert("RGB")
                big = upscale_image(img, tuple(j["size"]), pace)
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                part = dst + ".part"
                big.save(part, "JPEG", quality=93)
                os.replace(part, dst)
                made += 1
            except Exception as e:  # one bad panel is resized as before; the rest go on
                failed[j["src"]] = str(e)[:200]
                if "out of memory" in str(e).lower():
                    torch.cuda.empty_cache()
        emit({"type": "progress", "done": n + 1, "total": len(jobs)})
    emit({"type": "done", "made": made, "kept": len(jobs) - made - len(failed), "failed": failed,
          "seconds": round(time.monotonic() - t0, 1), "vram_peak_mb": round(torch.cuda.max_memory_allocated() / 2**20),
          "power": gov.summary() if gov is not None else None})


def main(argv):
    if len(argv) >= 3 and argv[1] == "run":
        try:
            run(argv[2])
        except Exception as e:
            emit({"type": "error", "message": str(e)[:400]})
            return 1
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
