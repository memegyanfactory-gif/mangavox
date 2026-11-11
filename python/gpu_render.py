"""The GPU engine (the default whenever the layered pack is installed, see
video_generator.render_engine): draws a video planned by gpu_timeline on the
graphics card (PyTorch CUDA, the layered pack's Python) in one pass, frame by frame, and streams
the frames to one ffmpeg that burns the captions and encodes (NVENC). No clip files, no second
encode, no frame through the CPU's filters.

    python gpu_render.py run plan.json

Pictures are worked on as YUV 4:4:4 floats in video range (BT.601, as the stills become in the
CPU path), so blends, fades and transitions give what ffmpeg's yuv420p overlays and xfade give;
the lookup table, grain and the noise act on Y, U and V as lutyuv and noise do. Emits JSON lines:
{"type": "progress", "done", "total"} and a last {"type": "done", "seconds", "fps"} (or "error")."""
import bisect
import json
import math
import os
import queue
import subprocess
import sys
import threading
import time

import numpy as np

_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
_BELOW_NORMAL = getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
BLACK = (16.0, 128.0, 128.0)


def emit(obj):
    print(json.dumps(obj), flush=True)


HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    # Run isolated (-I) by the renderer: the pack's Animator (layered_motion) sits beside this file.
    sys.path.insert(0, HERE)


try:
    import gpu_power
except ImportError:
    gpu_power = None


def layered_motion_ease(job, k):
    import layered_motion
    return layered_motion.frame_ease(job, k)


def at(v, i):
    """Frame i of a plan series (one number, or a list held at its last value)."""
    if isinstance(v, list):
        return v[min(max(0, i), len(v) - 1)] if v else 0
    return v


class Engine:
    def __init__(self, plan):
        import torch
        import torch.nn.functional as F
        self.torch, self.F = torch, F
        self.dev = torch.device("cuda")
        self.plan = plan
        self.w, self.h, self.fps = plan["w"], plan["h"], plan["fps"]
        m = torch.tensor([[65.481, 128.553, 24.966], [-37.797, -74.203, 112.0], [112.0, -93.786, -18.214]], dtype=torch.float64) / 255
        self.rgb2yuv = m.float().to(self.dev)
        self.yuv2rgb = torch.linalg.inv(m).float().to(self.dev)
        self.offset = torch.tensor(BLACK, device=self.dev).view(3, 1, 1)
        self.images = {}
        self.image_bytes = 0
        self.loops = {}
        self.decoding = {}
        self.sequences = {}
        self.clips = {}
        self.vignette = None
        self.dissolve = None

    # ── Colour ──

    def to_yuv(self, rgb):
        """(3, H, W) RGB 0..255 -> video-range YUV."""
        return (self.rgb2yuv @ rgb.reshape(3, -1)).reshape(rgb.shape) + self.offset

    def to_rgb(self, yuv):
        return (self.yuv2rgb @ (yuv - self.offset).reshape(3, -1)).reshape(yuv.shape)

    # ── Sources ──

    def still(self, path):
        """A still (JPEG / PNG) as one tensor: YUV, plus alpha 0..1 as a 4th channel when it has
        one; kept (least recently used out) while memory allows."""
        hit = self.images.pop(path, None)
        if hit is not None:
            self.images[path] = hit
            return hit
        from PIL import Image
        img = Image.open(path)
        img.load()
        has_alpha = img.mode in ("RGBA", "LA", "P") and ("A" in img.getbands() or "transparency" in img.info)
        a = np.asarray(img.convert("RGBA" if has_alpha else "RGB"))
        t = self.torch.from_numpy(np.ascontiguousarray(a)).to(self.dev).permute(2, 0, 1).float()
        hit = self.torch.cat([self.to_yuv(t[:3]), t[3:4] / 255]) if has_alpha else self.to_yuv(t)
        self.images[path] = hit
        self.image_bytes += hit.numel() * 4
        while self.image_bytes > 2.5e9 and len(self.images) > 4:
            old = self.images.pop(next(iter(self.images)))
            self.image_bytes -= old.numel() * 4
        return hit

    def image(self, path):
        """A still as (YUV (3, H, W), alpha (1, H, W) 0..1 or None): views of still(path)."""
        t = self.still(path)
        return (t[:3], t[3:4]) if t.shape[0] == 4 else (t, None)

    def loop(self, path):
        """An alpha loop (FFV1 yuva420p: particles, a mood's atmosphere) decoded whole, at half
        size (its sprites are soft: drawn at half size and scaled up already), on the GPU. All a
        chapter uses are kept (least recently used out past about 2.5 GB), so a mood coming back
        is never decoded twice."""
        hit = self.loops.pop(path, None)
        if hit is not None:
            self.loops[path] = hit
            return hit
        while self.loops and sum(f[0].numel() for f in self.loops.values()) > 2.5e9:
            self.loops.pop(next(iter(self.loops)))
            self.torch.cuda.empty_cache()
        job = self.decoding.pop(path, None)
        raw, lw, lh = job.result() if job is not None else self._decode_loop(path)
        size = lw * lh * 5 // 2
        n = len(raw) // size
        if n < 1:
            raise RuntimeError(f"could not read the loop {os.path.basename(path)}")
        frames = self.torch.from_numpy(np.frombuffer(raw, dtype=np.uint8, count=n * size).reshape(n, size)).to(self.dev)
        del raw
        hit = self.loops[path] = (frames, lw, lh)
        return hit

    def _decode_loop(self, path, count=None):
        """The loop's frames as raw yuva420p bytes at the loop size: in four pieces side by side
        when its length is known (FFV1: every frame a keyframe, so each piece seeks straight in;
        one ffmpeg alone takes several seconds)."""
        from concurrent.futures import ThreadPoolExecutor
        lw, lh = max(2, self.w // 4 * 2), max(2, self.h // 4 * 2)
        if self.w * self.h > 2560 * 1440:
            lw, lh = max(2, self.w // 8 * 2), max(2, self.h // 8 * 2)
        # 4:2:0 as it is stored (the chroma is scaled on the GPU): ffmpeg's scaler is the slow part.
        vf = ["-vf", f"scale={lw}:{lh}:flags=fast_bilinear,format=yuva420p", "-f", "rawvideo", "-"]

        def piece(a, n):
            seek = ["-ss", f"{(a - 0.5) / self.fps:.6f}"] if a else []
            frames = ["-frames:v", str(n)] if n else []
            cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", *seek, "-i", path, *frames, *vf]
            return subprocess.run(cmd, capture_output=True, timeout=600, creationflags=_NO_WINDOW).stdout

        if not count or count < 16:
            return piece(0, None), lw, lh
        cuts = [round(count * k / 4) for k in range(5)]
        with ThreadPoolExecutor(max_workers=4) as pool:
            parts = list(pool.map(lambda k: piece(cuts[k], cuts[k + 1] - cuts[k]), range(4)))
        size = lw * lh * 5 // 2
        if any(len(p) != (cuts[k + 1] - cuts[k]) * size for k, p in enumerate(parts)):
            # A piece came out short (seeking): the whole loop in one go instead.
            return piece(0, None), lw, lh
        return b"".join(parts), lw, lh

    def prefetch_loops(self, loops):
        """Decodes the loops the video uses ({path: frame count}) in the background from the
        start, so a clip never waits for its atmosphere."""
        from concurrent.futures import ThreadPoolExecutor
        loops = {p: n for p, n in loops.items() if p and p not in self.loops and p not in self.decoding}
        if not loops:
            return
        pool = ThreadPoolExecutor(max_workers=1)
        for p, n in loops.items():
            self.decoding[p] = pool.submit(self._decode_loop, p, n)
        pool.shutdown(wait=False)

    def loop_frame(self, path, i):
        frames, lw, lh = self.loop(path)
        f = frames[i % frames.shape[0]].float()
        a, c = lw * lh, (lw // 2) * (lh // 2)
        ya = self.torch.stack([f[:a].view(lh, lw), f[a + 2 * c:].view(lh, lw)])[None]
        uv = self.torch.stack([f[a:a + c].view(lh // 2, lw // 2), f[a + c:a + 2 * c].view(lh // 2, lw // 2)])[None]
        ya = self.F.interpolate(ya, size=(self.h, self.w), mode="bilinear", align_corners=False)[0]
        uv = self.F.interpolate(uv, size=(self.h, self.w), mode="bilinear", align_corners=False)[0]
        return self.torch.stack([ya[0], uv[0], uv[1]]), (ya[1:2] / 255).clamp(0, 1)

    def small_frame(self, pattern, j):
        """Frame j of a PNG sequence as it is drawn: YUV plus alpha 0..255 as a 4th channel, kept
        (the last few sequences) so a frame is read once."""
        seq = self.sequences.get(pattern)
        if seq is None:
            seq = self.sequences[pattern] = {}
            if len(self.sequences) > 6:
                self.sequences.pop(next(iter(self.sequences)))
        small = seq.get(j)
        if small is None:
            from PIL import Image
            a = np.asarray(Image.open(pattern % j).convert("RGBA"))
            t = self.torch.from_numpy(np.ascontiguousarray(a)).to(self.dev).permute(2, 0, 1).float()
            small = seq[j] = self.torch.cat([self.to_yuv(t[:3]), t[3:4]])
        return small

    def sequence_frame(self, pattern, j, flip=False):
        """Frame j of a PNG sequence (flares, bursts: drawn small), scaled to the frame (bicubic)."""
        # Kept small (a few MB a frame; scaled up they would be 60), scaled each time they show.
        small = self.small_frame(pattern, j)
        f = self.F.interpolate(small[None], size=(self.h, self.w), mode="bicubic", align_corners=False)[0]
        yuv, alpha = f[:3].clamp(0, 255), (f[3:4] / 255).clamp(0, 1)
        if flip:
            return yuv.flip(-1), alpha.flip(-1)
        return yuv, alpha

    # ── Drawing ──

    def warp(self, img, x, y, cw, ch, ow, oh, mode="bicubic"):
        """The window (x, y, cw, ch) of img (sub-pixel) stretched over ow x oh."""
        torch = self.torch
        H, W = img.shape[-2:]
        gx = ((torch.arange(ow, device=self.dev, dtype=torch.float32) + 0.5) * (cw / ow) + x) / W * 2 - 1
        gy = ((torch.arange(oh, device=self.dev, dtype=torch.float32) + 0.5) * (ch / oh) + y) / H * 2 - 1
        grid = torch.stack(torch.broadcast_tensors(gx[None, :], gy[:, None]), -1)[None]
        return self.F.grid_sample(img[None], grid, mode=mode, padding_mode="border", align_corners=False)[0]

    @staticmethod
    def crop(img, x, y, cw, ch):
        H, W = img.shape[-2:]
        x, y = max(0, min(int(x), W - cw)), max(0, min(int(y), H - ch))
        return img[:, y:y + ch, x:x + cw]

    def blend(self, dst, src, alpha, x, y):
        """src (3, h, w) over dst at (x, y) with alpha (a number, or (1, h, w)), clipped to dst."""
        H, W = dst.shape[-2:]
        h, w = src.shape[-2:]
        x0, y0 = max(0, x), max(0, y)
        x1, y1 = min(W, x + w), min(H, y + h)
        if x1 <= x0 or y1 <= y0:
            return dst
        s = src[:, y0 - y:y1 - y, x0 - x:x1 - x]
        d = dst[:, y0:y1, x0:x1]
        if isinstance(alpha, (int, float)):
            if alpha >= 1:
                d.copy_(s)
                return dst
            if alpha <= 0:
                return dst
            d.mul_(1 - alpha).add_(s * alpha)
            return dst
        a = alpha[:, y0 - y:y1 - y, x0 - x:x1 - x]
        d.mul_(1 - a).add_(s * a)
        return dst

    def blank(self):
        return self.offset.expand(3, self.h, self.w).clone()

    def planes(self, img, f):
        """f applied as ffmpeg applies a filter to a yuv420p picture: to Y at full size and to U, V
        at half size with the same settings (a blur reaches twice as far in chroma, noise is one
        sample per 2x2 block), the chroma then laid back at full size."""
        y = f(img[:1])
        H, W = img.shape[-2:]
        uv = f(self.F.avg_pool2d(img[1:][None], 2)[0])
        uv = uv.repeat_interleave(2, dim=-1).repeat_interleave(2, dim=-2)[:, :H, :W]
        return self.torch.cat([y, uv])

    def gauss(self, img, sigma):
        if sigma <= 0:
            return img
        torch = self.torch
        r = max(1, int(math.ceil(sigma * 3)))
        ax = torch.arange(-r, r + 1, device=self.dev, dtype=torch.float32)
        k = torch.exp(-ax ** 2 / (2 * sigma * sigma))
        k = k / k.sum()

        def blur(p):
            c = p.shape[0]
            x = self.F.conv2d(self.F.pad(p[None], (r, r, 0, 0), mode="replicate"), k.view(1, 1, 1, -1).repeat(c, 1, 1, 1), groups=c)
            x = self.F.conv2d(self.F.pad(x, (0, 0, r, r), mode="replicate"), k.view(1, 1, -1, 1).repeat(c, 1, 1, 1), groups=c)
            return x[0]
        return self.planes(img, blur)

    def box(self, img, rx, ry):
        def blur(p):
            x = self.F.pad(p[None], (rx, rx, ry, ry), mode="replicate")
            return self.F.avg_pool2d(x, (2 * ry + 1, 2 * rx + 1), stride=1)[0]
        return self.planes(img, blur)

    def shift(self, plane, dx, dy):
        """plane moved (dx, dy) pixels, its edge smeared (rgbashift's default)."""
        if not dx and not dy:
            return plane
        torch = self.torch
        H, W = plane.shape
        xs = (torch.arange(W, device=self.dev) - dx).clamp(0, W - 1)
        ys = (torch.arange(H, device=self.dev) - dy).clamp(0, H - 1)
        return plane[ys][:, xs]

    def noise(self, img, amp, uniform):
        """vf_noise's noise on every plane (uniform, or its Gaussian of sd amp/sqrt(3)), fresh each frame."""
        torch = self.torch

        def draw(p):
            if uniform:
                n = torch.floor(torch.rand(p.shape, device=self.dev) * amp) - amp // 2
            else:
                n = torch.trunc(torch.randn(p.shape, device=self.dev) * (amp / math.sqrt(3)))
            return (p + n).clamp(0, 255)
        return self.planes(img, draw)

    # ── Transitions (vf_xfade.c's formulas; P falls from 1, all `a`, to 0, all `b`) ──

    def transition(self, name, a, b, P):
        torch = self.torch
        H, W = a.shape[-2:]

        def smooth(e0, e1, v):
            t = ((v - e0) / (e1 - e0)).clamp(0, 1) if torch.is_tensor(v) else min(1.0, max(0.0, (v - e0) / (e1 - e0)))
            return t * t * (3 - 2 * t)

        if name == "fade":
            return a * P + b * (1 - P)
        if name in ("fadeblack", "fadewhite"):
            bg = torch.tensor([0.0, 127.0, 127.0] if name == "fadeblack" else [255.0, 127.0, 127.0], device=self.dev).view(3, 1, 1)
            ph = 0.2
            s0, s1 = smooth(1 - ph, 1.0, P), smooth(ph, 1.0, P)
            return (a * s0 + bg * (1 - s0)) * P + (bg * s1 + b * (1 - s1)) * (1 - P)
        if name == "dissolve":
            if self.dissolve is None:
                yy, xx = torch.meshgrid(torch.arange(H, device=self.dev, dtype=torch.float32), torch.arange(W, device=self.dev, dtype=torch.float32), indexing="ij")
                r = torch.sin(xx * 12.9898 + yy * 78.233) * 43758.545
                self.dissolve = r - torch.floor(r)
            return torch.where((self.dissolve * 2 + P * 2 - 1.5) >= 0.5, a, b)
        if name.startswith("smooth"):
            x = torch.arange(W, device=self.dev, dtype=torch.float32)[None, None, :]
            y = torch.arange(H, device=self.dev, dtype=torch.float32)[None, :, None]
            v = {"smoothleft": 1 + x / W, "smoothright": 1 + (W - 1 - x) / W,
                 "smoothup": 1 + y / H, "smoothdown": 1 + (H - 1 - y) / H}[name] - P * 2
            ss = smooth(0.0, 1.0, v)
            return b * ss + a * (1 - ss)
        if name.startswith("slide"):
            horiz = name in ("slideleft", "slideright")
            n = W if horiz else H
            z = int((-P if name in ("slideleft", "slideup") else P) * n)
            idx = torch.arange(n, device=self.dev) + z
            inside = (idx >= 0) & (idx < n)
            zz = torch.remainder(idx, n)
            if horiz:
                return torch.where(inside[None, None, :], b[:, :, zz], a[:, :, zz])
            return torch.where(inside[None, :, None], b[:, zz, :], a[:, zz, :])
        if name.startswith("wipe"):
            if name in ("wipeleft", "wiperight"):
                x = torch.arange(W, device=self.dev)[None, None, :]
                return torch.where(x > int(W * P), b, a) if name == "wipeleft" else torch.where(x > int(W * (1 - P)), a, b)
            y = torch.arange(H, device=self.dev)[None, :, None]
            return torch.where(y > int(H * P), b, a) if name == "wipeup" else torch.where(y > int(H * (1 - P)), a, b)
        if name in ("circleopen", "circleclose"):
            yy = torch.arange(H, device=self.dev, dtype=torch.float32)[:, None] - H / 2
            xx = torch.arange(W, device=self.dev, dtype=torch.float32)[None, :] - W / 2
            z = math.hypot(W / 2, H / 2)
            p = ((P if name == "circleopen" else 1 - P) - 0.5) * 3
            ss = smooth(0.0, 1.0, torch.sqrt(xx * xx + yy * yy) / z + p)[None]
            return a * ss + b * (1 - ss) if name == "circleopen" else b * ss + a * (1 - ss)
        if name == "pixelize":
            d = min(P, 1 - P)
            dist = math.ceil(d * 50) / 50
            if dist <= 0:
                return a * P + b * (1 - P)
            sq = 2 * dist * min(W, H) / 20
            sx = torch.clamp(((torch.floor(torch.arange(W, device=self.dev) / sq) + 0.5) * sq).long(), max=W - 1)
            sy = torch.clamp(((torch.floor(torch.arange(H, device=self.dev) / sq) + 0.5) * sq).long(), max=H - 1)
            return a[:, sy][:, :, sx] * P + b[:, sy][:, :, sx] * (1 - P)
        raise ValueError(f"unknown transition {name}")


class Reader:
    """A video read frame by frame (in order) by an ffmpeg of its own, decoded ahead in a thread."""

    def __init__(self, engine, path, w, h, full_range=False):
        self.e, self.path, self.w, self.h = engine, path, w, h
        vf = ("scale=out_range=tv," if full_range else "") + f"scale={w}:{h},format=yuv444p"
        self.cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", path, "-vf", vf, "-f", "rawvideo", "-"]
        self.size = 3 * w * h
        self.pos = 0
        self.last = None
        self.q = queue.Queue(maxsize=4)
        self.proc = subprocess.Popen(self.cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, creationflags=_NO_WINDOW | _BELOW_NORMAL)
        self.thread = threading.Thread(target=self._pump, daemon=True)
        self.thread.start()

    def _pump(self):
        try:
            while True:
                buf = self.proc.stdout.read(self.size)
                if not buf or len(buf) < self.size:
                    break
                self.q.put(buf)
        finally:
            self.q.put(None)

    def frame(self, i):
        while self.pos <= i:
            buf = self.q.get()
            if buf is None:
                self.q.put(None)
                break
            self.pos += 1
            if self.pos > i:
                t = self.e.torch.frombuffer(bytearray(buf), dtype=self.e.torch.uint8).view(3, self.h, self.w)
                self.last = t.to(self.e.dev).float()
        if self.last is None:
            raise RuntimeError(f"could not read {os.path.basename(self.path)}")
        return self.last

    def close(self):
        try:
            self.proc.kill()
        except Exception:
            pass
        # Unblock the pump (it may be waiting on a full queue), then reap the process.
        try:
            while True:
                self.q.get_nowait()
        except queue.Empty:
            pass
        self.thread.join(timeout=5)
        try:
            self.proc.wait(timeout=5)
        except Exception:
            pass


class Clip:
    def __init__(self, engine, plan):
        self.e, self.p = engine, plan
        self.readers = {}
        self.animators = {}

    def reader(self, key, path, w, h, full_range=False):
        r = self.readers.get(key)
        if r is None:
            r = self.readers[key] = Reader(self.e, path, w, h, full_range)
        return r

    def close(self):
        for r in self.readers.values():
            r.close()
        self.readers = {}
        if self.animators:
            self.animators = {}
            self.e.torch.cuda.empty_cache()

    def animated(self, key, job, i):
        """Frame i of a layered panel, drawn by the pack's Animator from its cached layers (what
        the pack would have encoded to a video, here never leaving the GPU), as YUV."""
        hit = self.animators.get(key)
        if hit is None:
            import layered_motion
            import layered_plan
            man = layered_plan.read_manifest(job["cache"])
            if not man:
                raise RuntimeError("a layered panel's layers are missing")
            anim, grade, _ = layered_motion.animator(job, man)
            hit = self.animators[key] = (anim, grade)
        anim, grade = hit
        img = grade(anim.frame(layered_motion_ease(job, i), i / self.e.fps))
        return self.e.to_yuv(img * 255)

    def frame(self, i):
        e, p = self.e, self.p
        i = min(i, p["frames"] - 1)
        if p["kind"] == "video":
            return self.reader("clip", p["path"], e.w, e.h).frame(i).clone()
        img = e.blank()
        for n, layer in enumerate(p["layers"]):
            on = layer.get("on")
            if on and not (on[0] <= i < on[1]):
                continue
            ow, oh = layer["size"]
            if layer.get("animate"):
                src = self.animated(n, layer["animate"], i)
                if "warp" in layer:
                    x, y, cw, ch = (at(v, i) for v in layer["warp"])
                    if not (cw == ow and ch == oh and x == 0 and y == 0):
                        src = e.warp(src, x, y, cw, ch, ow, oh, layer.get("interp", "bicubic")).clamp(0, 255)
                alpha = None
            elif layer.get("video"):
                src, alpha = self.reader(n, layer["src"], ow, oh, layer.get("full_range")), None
                src = src.frame(i)
                if "warp" in layer:
                    x, y, cw, ch = (at(v, i) for v in layer["warp"])
                    if not (cw == ow and ch == oh and x == 0 and y == 0):
                        src = e.warp(src, x, y, cw, ch, ow, oh, layer.get("interp", "bicubic")).clamp(0, 255)
            else:
                yuv, al = e.image(layer["src"])
                if "warp" in layer:
                    x, y, cw, ch = (at(v, i) for v in layer["warp"])
                    # The stored still (alpha as its 4th channel) warped whole; bicubic can overshoot.
                    out = e.warp(e.still(layer["src"]), x, y, cw, ch, ow, oh, layer.get("interp", "bicubic"))
                    src, alpha = out[:3].clamp(0, 255), (out[3:4].clamp(0, 1) if al is not None else None)
                else:
                    cx, cy = (at(v, i) for v in layer.get("crop", [0, 0]))
                    src = e.crop(yuv, cx, cy, ow, oh)
                    alpha = e.crop(al, cx, cy, ow, oh) if al is not None else None
            a = at(layer.get("alpha", 1.0), i - (on[0] if on else 0))
            if alpha is not None:
                a = alpha * a
            px, py = (int(at(v, i)) for v in layer.get("pos", [0, 0]))
            e.blend(img, src, a, px, py)
        flash = at(p.get("flash"), i) if p.get("flash") is not None else 0
        if flash:
            img[0].add_(255 * flash).clamp_(0, 255)
        for fx in p.get("fx") or []:
            a, b = fx["on"]
            if a <= i < b:
                img = self.effect(fx, img, i - a, i)
        return img

    def effect(self, fx, img, j, i):
        e = self.e
        op = fx["op"]
        if op == "zoomblur":
            z = at(fx["zoom"], j)
            if z != 1:
                img = e.warp(img, (e.w - e.w / z) / 2, (e.h - e.h / z) / 2, e.w / z, e.h / z, e.w, e.h, "bilinear")
            return e.gauss(img, at(fx["sigma"], j))
        if op == "rgbshift":
            rh, bh, rv, bv = (int(at(fx[k], j)) for k in ("rh", "bh", "rv", "bv"))
            if not (rh or bh or rv or bv):
                return img
            rgb = e.to_rgb(img).clamp(0, 255)
            rgb = e.torch.stack([e.shift(rgb[0], rh, rv), rgb[1], e.shift(rgb[2], bh, bv)])
            return e.to_yuv(rgb).clamp(0, 255)
        if op == "noise":
            return e.noise(img, fx["amp"], fx.get("uniform", False))
        if op == "boxblur":
            rx, ry = fx["radius"]
            return e.box(img, rx, ry)
        if op == "loop":
            src, alpha = e.loop_frame(fx["src"], fx["at"] + j)
            return e.blend(img, src, alpha, 0, 0)
        if op == "sequence":
            src, alpha = e.sequence_frame(fx["src"], fx["first"] + j, fx.get("flip", False))
            return e.blend(img, src, alpha, 0, 0)
        raise ValueError(f"unknown effect {op}")


def render(plan):
    import torch
    if not torch.cuda.is_available():
        raise RuntimeError("PyTorch does not see the graphics card")
    torch.set_num_threads(2)
    torch.backends.cudnn.benchmark = True
    e = Engine(plan)
    w, h, fps, total = plan["w"], plan["h"], plan["fps"], plan["frames"]
    first, joins, clips = plan["first"], plan["joins"], plan["clips"]
    look = plan.get("look") or {}
    loops = {f["src"]: f.get("count") for c in clips for f in c.get("fx") or [] if f.get("op") == "loop"}
    if look.get("particles"):
        loops[look["particles"]["src"]] = look["particles"].get("count")
    e.prefetch_loops(loops)
    lut = torch.tensor(look["lut"], device=e.dev, dtype=torch.float32) if look.get("lut") else None
    if look.get("vignette"):
        yy = torch.trunc(torch.arange(h, device=e.dev, dtype=torch.float32) - h / 2)[:, None]
        xx = torch.trunc(torch.arange(w, device=e.dev, dtype=torch.float32) - w / 2)[None, :]
        f = torch.cos(look["vignette"] * torch.clamp(torch.sqrt(xx * xx + yy * yy) / math.hypot(w / 2, h / 2), max=1)) ** 4
        e.vignette = torch.trunc(255 * (1 - f)) / 255
    edge = look.get("edge")
    bar = look.get("bars", 0)
    overlays = []
    for o in plan.get("overlays") or []:
        yuv, al = e.image(o["src"])
        if "width" in o:
            ow = o["width"]
            oh = max(2, round(yuv.shape[1] * ow / yuv.shape[2] / 2) * 2)
            whole = torch.cat([yuv, al if al is not None else torch.ones_like(yuv[:1])])
            whole = e.F.interpolate(whole[None], size=(oh, ow), mode="bicubic", align_corners=False, antialias=True)[0]
            yuv, al = whole[:3].clamp(0, 255), whole[3:4].clamp(0, 1)
            pos = (int(o["anchor"][0] * w - ow / 2) & ~1, int(o["anchor"][1] * h - oh / 2) & ~1)
        else:
            pos = tuple(o["pos"])
        overlays.append((yuv, (al if al is not None else torch.ones_like(yuv[:1])) * o.get("alpha", 1.0), pos, o["on"]))
    card = plan.get("card")

    def clip(k):
        c = e.clips.get(k)
        if c is None:
            c = e.clips[k] = Clip(e, clips[k])
        return c

    def chain(k, g):
        img = clip(k).frame(g - first[k])
        j = joins[k] if k else None
        if j and j["dur"] > 0:
            tg = g / fps - j["offset"]
            if 0 <= tg <= j["dur"]:
                P = min(1.0, max(0.0, 1 - tg / j["dur"]))
                img = e.transition(j["name"], chain(k - 1, g), img, P)
        return img

    def finish(img, g):
        for bst in plan.get("bursts") or []:
            if bst["at"] <= g < bst["at"] + bst["count"]:
                src, alpha = e.sequence_frame(bst["src"], g - bst["at"])
                e.blend(img, src, alpha, 0, 0)
        if look:
            if lut is not None:
                idx = img.round().clamp(0, 255).long()
                img = torch.stack([lut[c][idx[c]] for c in range(3)])
            if look.get("particles"):
                p = look["particles"]
                src, alpha = e.loop_frame(p["src"], g % p["count"])
                e.blend(img, src, alpha, 0, 0)
            if e.vignette is not None:
                a = e.vignette
                img[0].mul_(1 - a)
                img[1:].mul_(1 - a).add_(128 * a)
            if look.get("grain"):
                img = e.noise(img, look["grain"], False)
            if edge:
                seg = int(at(edge["segment"], g))
                atlas, aal = e.image(edge["atlases"][seg])
                # In edge_moves' order (the CPU path's overlay order: the lights over the strips).
                for name, xy in edge["pos"].items():
                    ax, ay, pw, ph = edge["places"][name]
                    px, py = (int(at(v, g)) for v in xy)
                    e.blend(img, atlas[:, ay:ay + ph, ax:ax + pw], aal[:, ay:ay + ph, ax:ax + pw], px, py)
            if bar:
                img[:, :bar] = e.offset
                img[:, h - bar:] = e.offset
        for yuv, al, (px, py), (a, b) in overlays:
            if a <= g < b:
                e.blend(img, yuv, al, px, py)
        if card and card["on"][0] <= g < card["on"][1]:
            # The badge's own frame i (small, drawn as it is: its fade, pop and ring are in it).
            small = e.small_frame(card["src"], g - card["on"][0])
            e.blend(img, small[:3], small[3:4] / 255, int(card["pos"][0]), int(card["pos"][1]))
        return img

    # Frames come back to the CPU through pinned buffers, copied without waiting: the next frame is
    # queued on the GPU while this one is still being drawn and copied. A buffer is used again only
    # after RING frames, more than the encoder's queue, the frame it is writing and those in flight.
    RING = 12
    ring = [torch.empty(w * h * 3 // 2, dtype=torch.uint8, pin_memory=True) for _ in range(RING)]
    flight = []

    def to_420(img, g):
        img = img.clamp(0, 255)
        y = img[0].round().to(torch.uint8)
        uv = e.F.avg_pool2d(img[1:][None], 2)[0].round().to(torch.uint8)
        buf = ring[g % RING]
        buf.copy_(torch.cat([y.reshape(-1), uv.reshape(-1)]), non_blocking=True)
        ev = torch.cuda.Event()
        ev.record()
        flight.append((ev, buf))


    # The encoder: the captions burned in and the picture encoded, as the CPU path's finish does.
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-s", f"{w}x{h}",
           "-r", str(fps), "-color_range", "tv", "-colorspace", "bt470bg", "-i", "-"]
    if plan.get("vf"):
        # Captions are drawn for half a millisecond before each frame's time: a caption line ends
        # exactly where the next begins (whose words fade in from transparent), and a frame on
        # that instant would otherwise show neither for a frame. (The CPU path does the same by
        # luck of rounding in some parts; here it is every boundary.) Timestamps go back after.
        cmd += ["-vf", f"settb=AVTB,setpts=N/({fps}*TB)-0.0005/TB,{plan['vf']},setpts=N/({fps}*TB),format=yuv420p"]
    cmd += ["-frames:v", str(total), "-r", str(fps), *plan["codec"], "-color_range", "tv", "-colorspace", "bt470bg", "-an", plan["out"]]
    enc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=_NO_WINDOW | _BELOW_NORMAL)
    errs = []
    threading.Thread(target=lambda: errs.append(enc.stderr.read()), daemon=True).start()
    out = queue.Queue(maxsize=6)
    failed = []

    def write():
        try:
            while True:
                buf = out.get()
                if buf is None:
                    break
                enc.stdin.write(memoryview(buf))
        except Exception as ex:
            failed.append(ex)
            # Keep draining so the renderer never blocks on a full queue.
            while out.get() is not None:
                pass
        finally:
            try:
                enc.stdin.close()
            except Exception:
                pass

    writer = threading.Thread(target=write, daemon=True)
    writer.start()
    # Paced to the GPU power target (gpu_power: Settings -> System -> GPU power limit), under a
    # frame-rate ceiling from it (MANGAVOX_GPU_MAX_FPS overrides).
    gov = gpu_power.governor("engine") if gpu_power else None
    cap = float(os.environ.get("MANGAVOX_GPU_MAX_FPS") or (gpu_power.fps_cap() if gpu_power else 45))
    t0 = time.monotonic()
    # Seconds spent queueing the drawing, waiting for it and the copy back (to_420), and waiting
    # for the encoder to take a frame: which of the three holds the engine back.
    spent = [0.0, 0.0, 0.0]
    # Seconds slept for the frame-rate ceiling since the governor last paced.
    idle = 0.0
    try:
        with torch.inference_mode():
            for g in range(total):
                if failed:
                    break
                k = bisect.bisect_right(first, g) - 1
                t1 = time.monotonic()
                img = finish(chain(max(0, k), g), g)
                to_420(img, g)
                t2 = time.monotonic()
                spent[0] += t2 - t1
                while len(flight) > 1:
                    ev, buf = flight.pop(0)
                    ev.synchronize()
                    t3 = time.monotonic()
                    out.put(buf.numpy())
                    t4 = time.monotonic()
                    spent[1] += t3 - t2
                    spent[2] += t4 - t3
                    t2 = t4
                # Clips past their last frame let go of their readers.
                for n in [n for n in e.clips if first[n] + clips[n]["frames"] <= g]:
                    e.clips.pop(n).close()
                if g % 15 == 0:
                    emit({"type": "progress", "done": g, "total": total})
                if gov is not None:
                    # The sleep for the frame-rate ceiling is rest already: counted as work, the
                    # governor rested for it a second time.
                    gov.pace(busy_s=max(0.0, time.monotonic() - gov.end - idle))
                    idle = 0.0
                if cap > 0:
                    ahead = (g + 1) / cap - (time.monotonic() - t0)
                    if ahead > 0:
                        time.sleep(ahead)
                        idle = ahead
        while flight and not failed:
            ev, buf = flight.pop(0)
            ev.synchronize()
            out.put(buf.numpy())
    except BaseException:
        # A drawing failure: the half-written file is no use.
        enc.kill()
        raise
    finally:
        out.put(None)
        writer.join()
        for c in e.clips.values():
            c.close()
    code = enc.wait()
    if failed or code != 0 or not os.path.isfile(plan["out"]):
        msg = b"".join(errs).decode("utf-8", "replace").strip()
        # Marked, so the renderer can try the next encoder (NVENC busy or missing -> x264).
        raise RuntimeError(f"ENCODER: the encoder stopped: {msg[-300:] or failed or code}")
    secs = time.monotonic() - t0
    return {"seconds": round(secs, 2), "fps": round(total / max(secs, 1e-6), 1), "vram_peak_mb": round(torch.cuda.max_memory_allocated() / 2 ** 20),
            "draw_s": round(spent[0], 2), "gpu_wait_s": round(spent[1], 2), "encoder_wait_s": round(spent[2], 2),
            "power": gov.summary() if gov is not None else "no GPU power governor", "fps_cap": cap}


def main(argv):
    if len(argv) >= 3 and argv[1] == "run":
        with open(argv[2], "r", encoding="utf-8") as f:
            plan = json.load(f)
        try:
            emit({"type": "done", **render(plan)})
            return 0
        except Exception as ex:
            emit({"type": "error", "message": f"{type(ex).__name__}: {ex}"})
            return 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
