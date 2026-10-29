"""
Video Generator for MangaVox
Combines manga panels/pages + TTS narration into a narrated recap video: every panel sits
centred (with a soft shadow), fully visible, on a still, zoomed-in, blurred copy of itself; only
the panel moves, with a slow eased sub-pixel Ken Burns, and it crossfades into the next clip
exactly when that clip's narration starts.
Narration is assembled sample-accurately, mixed with the score and loudness-normalised
in two passes.
Requires: ffmpeg, Pillow
"""
import os
import sys
import json
import subprocess
import shutil
import wave
import math
import re

FPS = 30
AUDIO_RATE = 48000
# Clips per intermediate xfade pass (keeps the number of open decoders bounded).
CHUNK_CLIPS = 14
# Panel inset from the frame edge (fraction of the frame) when the blurred fill is used.
FG_MARGIN = 0.05
# Ken Burns: push-in / pull-out amount and the fixed zoom used while panning.
FG_PUSH = 0.05
FG_PAN_ZOOM = 1.04
# The still blurred background is the panel zoomed in this much past "cover".
BG_ZOOM = 1.15
# Transition into the next beat of the same page, and the rotation used on a page change.
SAME_PAGE_TRANSITION = ("fade", 0.35)
PAGE_TRANSITIONS = ["smoothleft", "fade", "smoothup", "fadeblack"]
# Loudness target for the final mix (LUFS / dBTP).
LOUD_I = -16.0
LOUD_TP = -1.5

FG_MOTIONS = {
    "tall": ["push", "pan_down", "pull", "pan_up"],
    "wide": ["push", "pan_right", "pull", "pan_left"],
    "even": ["push", "pull", "pan_right", "push", "pull", "pan_down"],
}
# Where the visible window sits in the slack left by the zoom (0..1 per axis; "e" is the
# eased 0..1 progress) for each panel move.
FG_WINDOW = {
    "push": ("0.5", "0.5"), "pull": ("0.5", "0.5"), "still": ("0.5", "0.5"),
    "pan_down": ("0.5", "{e}"), "pan_up": ("0.5", "(1-{e})"),
    "pan_right": ("{e}", "0.5"), "pan_left": ("(1-{e})", "0.5"),
}
# Bumped whenever the look of a rendered clip changes, so cached clips are not reused.
CLIP_VERSION = 3


def check_ffmpeg():
    """Verify ffmpeg is available."""
    if shutil.which("ffmpeg") is None:
        raise RuntimeError(
            "ffmpeg not found. Install it with: brew install ffmpeg"
        )

def get_audio_duration_sec(audio_path):
    """Get audio duration in seconds using ffprobe, falling back to WAV headers."""
    try:
        result = subprocess.run(
            [
                "ffprobe", "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                audio_path,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            duration = float((result.stdout or "").strip())
            if math.isfinite(duration) and duration > 0:
                return duration
    except Exception:
        pass
    try:
        with wave.open(audio_path, 'r') as wf:
            return wf.getnframes() / wf.getframerate()
    except Exception:
        return 5.0

def get_resolution(preset, fmt="landscape"):
    """Return (width, height) for a resolution preset; "reels" is the same size turned upright (9:16)."""
    presets = {
        "720p":  (1280, 720),
        "1080p": (1920, 1080),
        "1440p": (2560, 1440),
        "4k":    (3840, 2160),
        "2160p": (3840, 2160),
    }
    w, h = presets.get(preset, (1920, 1080))
    return (h, w) if fmt == "reels" else (w, h)


REEL_TARGET_SEC = 60.0
# A reel may end this far from its ideal cut point to land on a narration boundary.
REEL_SNAP_SEC = 15.0


def plan_reel_cuts(boundaries, total, target=REEL_TARGET_SEC, snap=REEL_SNAP_SEC):
    """Split `total` seconds into consecutive ~`target`-second reels.

    `boundaries` are the times (seconds) where one narration beat ends and the next
    begins. The reel count is `round(total / target)` so every reel stays close to
    the target, and each cut moves to the nearest boundary within `snap` seconds of
    its ideal spot (only a beat longer than that gets cut mid-sentence).
    Returns [(start, end), ...] covering 0..total.
    """
    if total <= 0:
        return []
    n = max(1, int(round(total / target)))
    cuts = []
    prev = 0.0
    for k in range(1, n):
        ideal = total * k / n
        near = [b for b in boundaries if abs(b - ideal) <= snap and prev + 1.0 < b < total - 1.0]
        cut = min(near, key=lambda b: abs(b - ideal)) if near else ideal
        if cut <= prev + 1.0:
            continue
        cuts.append(cut)
        prev = cut
    edges = [0.0, *cuts, float(total)]
    return [(round(a, 3), round(b, 3)) for a, b in zip(edges, edges[1:])]


def split_reels(final_video, cuts, reels_dir):
    """Cut `final_video` into reels_dir/reel_01.mp4 ... at `cuts` (re-encoded so every
    cut is frame-exact). Writes reels.json and returns the reel paths."""
    if os.path.isdir(reels_dir):
        shutil.rmtree(reels_dir, ignore_errors=True)
    os.makedirs(reels_dir, exist_ok=True)
    out = []
    manifest = []
    for i, (start, end) in enumerate(cuts):
        dur = end - start
        if dur < 0.2:
            continue
        path = os.path.join(reels_dir, f"reel_{i + 1:02d}.mp4")
        fade = min(0.15, dur / 4)
        cmd = [
            "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
            "-ss", f"{start:.3f}", "-i", final_video, "-t", f"{dur:.3f}",
            # Every audio track (with music, "Narration only" rides along).
            "-map", "0:v:0", "-map", "0:a",
            "-af", f"afade=t=in:d=0.03,afade=t=out:st={max(0.0, dur - fade):.3f}:d={fade:.3f}",
            "-c:v", "libx264", "-preset", "fast", "-crf", "20", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "160k",
            "-movflags", "+faststart",
            path,
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        if result.returncode != 0 or not os.path.exists(path):
            raise RuntimeError(f"Failed to cut reel {i + 1}: {result.stderr[:300]}")
        out.append(path)
        manifest.append({"file": os.path.basename(path), "start": start, "end": end})
    with open(os.path.join(reels_dir, "reels.json"), "w", encoding="utf-8") as f:
        json.dump({"reels": manifest}, f, indent=2)
    return out


def combine_audio_files(audio_paths, output_path):
    """Concatenate WAV files into a single WAV using ffmpeg."""
    if os.path.exists(output_path):
        return

    list_file = output_path + ".txt"
    with open(list_file, 'w') as f:
        for p in audio_paths:
            f.write(f"file '{p}'\n")

    subprocess.run([
        "ffmpeg", "-y", "-f", "concat", "-safe", "0",
        "-i", list_file,
        "-c:a", "pcm_s16le", "-ar", "22050", "-ac", "1",
        output_path
    ], capture_output=True, timeout=300)

    os.unlink(list_file)


# ── Timeline planning (pure helpers) ──

def plan_transitions(durations, pages, kind="crossfade", base=0.5):
    """Plan crossfades between clips of `durations` seconds without moving the narration.

    Returns (starts, trans): starts[k] is when clip k's narration begins (running sum)
    and trans[k] = (xfade transition, seconds) for the change into clip k (trans[0] is
    None). Each clip's video runs on for trans[k + 1] seconds past its narration, so the
    fade into clip k begins exactly at starts[k] and the total stays sum(durations).
    Same-page beats dissolve quickly, page changes rotate through PAGE_TRANSITIONS, short
    clips get shorter fades, and kind "none" gives one-frame (hard) cuts.
    """
    starts = []
    t = 0.0
    for d in durations:
        starts.append(t)
        t += d
    frame = 1.0 / FPS
    trans = [None]
    turn = 0
    for k in range(1, len(durations)):
        if kind == "none":
            name, dur = "fade", frame
        elif pages[k] == pages[k - 1]:
            name, dur = SAME_PAGE_TRANSITION[0], min(SAME_PAGE_TRANSITION[1], base)
        else:
            name, dur = PAGE_TRANSITIONS[turn % len(PAGE_TRANSITIONS)], base
            turn += 1
        dur = max(frame, min(dur, 0.45 * durations[k - 1], 0.45 * durations[k]))
        trans.append((name, round(dur, 3)))
    return starts, trans


def plan_chunks(n, size=CHUNK_CLIPS):
    """Split n clips into consecutive [first, end) runs of at most `size` clips."""
    return [(i, min(n, i + size)) for i in range(0, n, size)]


def clip_frames(starts, trans, k, last_duration):
    """Frames clip k must render: up to the next clip's start plus that fade's tail (and a
    spare frame), or its own duration for the last clip."""
    first = round(starts[k] * FPS)
    if k + 1 < len(starts):
        return round(starts[k + 1] * FPS) - first + math.ceil(trans[k + 1][1] * FPS) + 1
    return math.ceil(last_duration * FPS) + 1


def xfade_chain(labels, starts, trans, out):
    """Filtergraph that chains video `labels` with xfade so labels[k] first appears at
    starts[k] (seconds, snapped to the frame grid, relative to starts[0]); trans[k] is
    the (transition, seconds) into labels[k]. The result is labelled `out`."""
    if len(labels) == 1:
        return f"[{labels[0]}]null[{out}]"
    base = round(starts[0] * FPS)
    parts = []
    prev = labels[0]
    for k in range(1, len(labels)):
        name, dur = trans[k]
        # A hair before the frame, so xfade (which starts at the first frame with
        # pts >= offset) never slips a frame through decimal rounding.
        offset = max(0.0, (round(starts[k] * FPS) - base) / FPS - 0.001)
        nxt = out if k == len(labels) - 1 else f"{out}_{k}"
        parts.append(
            f"[{prev}][{labels[k]}]xfade=transition={name}:duration={dur:.3f}:offset={offset:.6f}[{nxt}]"
        )
        prev = nxt
    return ";".join(parts)


def pick_motion(aspect, box_aspect, turn, prev):
    """Ken Burns move for a panel of `aspect` (w/h) in a box of `box_aspect`: tall panels pan
    vertically, wide ones horizontally, all push/pull; never the same move twice running."""
    if aspect < box_aspect * 0.75:
        options = FG_MOTIONS["tall"]
    elif aspect > box_aspect * 1.33:
        options = FG_MOTIONS["wide"]
    else:
        options = FG_MOTIONS["even"]
    motion = options[turn % len(options)]
    if motion == prev:
        motion = options[(turn + 1) % len(options)]
    return motion


def _ease(var, frames):
    """Cosine ease 0..1 over `frames` frames, as an ffmpeg expression of frame counter `var`."""
    return f"(0.5-0.5*cos(PI*{var}/{max(1, frames - 1)}))"


def _motion_zoom(motion, amount, pan_zoom):
    """Zoom expression (of the eased progress {e}) for a Ken Burns move."""
    if motion == "push":
        return f"1+{amount}*{{e}}"
    if motion == "pull":
        return f"{1 + amount:.4f}-{amount}*{{e}}"
    if motion in ("pan_down", "pan_up", "pan_right", "pan_left"):
        return f"{pan_zoom}"
    return "1"


def _warp(window, zoom, frames, interp):
    """One still held for `frames` frames with an eased move: each frame shows the
    sub-rectangle 1/zoom the size of the image, placed by the `window` fractions, stretched
    over the whole frame. perspective resamples with subpixel precision, so slow moves glide
    (zoompan rounds its crop to whole, chroma-aligned pixels and visibly steps)."""
    e = _ease("on", frames)
    z = "(" + zoom.format(e=e) + ")"
    px, py = (v.format(e=e) for v in window)
    cw, ch = f"W/{z}", f"H/{z}"
    x, y = f"(W-{cw})*{px}", f"(H-{ch})*{py}"
    return (
        f"loop=loop={max(0, frames - 1)}:size=1:start=0,setpts=N/{FPS}/TB,"
        f"perspective=x0='{x}':y0='{y}':x1='{x}+{cw}':y1='{y}':"
        f"x2='{x}':y2='{y}+{ch}':x3='{x}+{cw}':y3='{y}+{ch}':"
        f"interpolation={interp}:eval=frame"
    )


# ── Per-clip images ──

def _crop_to_focus(page_image, focus_point, aspect, out_path):
    """Crop `page_image` around a 0..1000 focus box (15% margin, widened to `aspect`)."""
    from PIL import Image
    img = Image.open(page_image)
    img_w, img_h = img.size

    ymin, xmin, ymax, xmax = focus_point
    y_min = int(ymin * img_h / 1000)
    x_min = int(xmin * img_w / 1000)
    y_max = int(ymax * img_h / 1000)
    x_max = int(xmax * img_w / 1000)

    pad_w = int((x_max - x_min) * 0.15)
    pad_h = int((y_max - y_min) * 0.15)
    x_min = max(0, x_min - pad_w)
    y_min = max(0, y_min - pad_h)
    x_max = min(img_w, x_max + pad_w)
    y_max = min(img_h, y_max + pad_h)

    box_w = x_max - x_min
    box_h = y_max - y_min
    if box_w <= 0 or box_h <= 0:
        return page_image
    if box_w / box_h > aspect:
        new_h = int(box_w / aspect)
        center_y = (y_min + y_max) // 2
        y_min = max(0, center_y - new_h // 2)
        y_max = min(img_h, y_min + new_h)
        if y_max - y_min < new_h:
            y_min = max(0, y_max - new_h)
    else:
        new_w = int(box_h * aspect)
        center_x = (x_min + x_max) // 2
        x_min = max(0, center_x - new_w // 2)
        x_max = min(img_w, x_min + new_w)
        if x_max - x_min < new_w:
            x_min = max(0, x_max - new_w)
    img.convert("RGB").crop((x_min, y_min, x_max, y_max)).save(out_path, "JPEG", quality=95)
    return out_path


def _prepare_clip_images(src, idx, w, h, fill, work_dir):
    """Write this clip's stills: the upscaled panel and the frame-sized background it sits on.
    For the blurred fill the background is the panel zoomed in past "cover", blurred, darkened
    and desaturated, with the panel's soft drop shadow baked in; otherwise black. The
    background never moves, so it is composed once here. Returns where everything goes."""
    from PIL import Image, ImageDraw, ImageEnhance, ImageFilter

    img = Image.open(src)
    img.load()
    img = img.convert("RGB")
    iw, ih = img.size
    blur = fill == "blur"
    mx, my = (round(w * FG_MARGIN), round(h * FG_MARGIN)) if blur else (0, 0)
    bw, bh = w - 2 * mx, h - 2 * my
    scale = min(bw / iw, bh / ih)
    fw = max(2, int(iw * scale) // 2 * 2)
    fh = max(2, int(ih * scale) // 2 * 2)
    fx = (w - fw) // 4 * 2
    fy = (h - fh) // 4 * 2

    fg_path = os.path.join(work_dir, f"fg_{idx:04d}.jpg")
    img.resize((fw, fh), Image.LANCZOS).save(fg_path, "JPEG", quality=95)

    bg_path = os.path.join(work_dir, f"bg_{idx:04d}.jpg")
    if blur:
        # Blur at 1/8 size (much cheaper, and smoother once scaled back up).
        sw, sh = max(16, w // 8), max(16, h // 8)
        cover = max(sw / iw, sh / ih) * BG_ZOOM
        small = img.resize((max(sw, round(iw * cover)), max(sh, round(ih * cover))), Image.BOX)
        left = (small.width - sw) // 2
        top = (small.height - sh) // 2
        small = small.crop((left, top, left + sw, top + sh))
        small = small.filter(ImageFilter.GaussianBlur(radius=max(2.0, sw * 0.03)))
        small = ImageEnhance.Color(small).enhance(0.6)
        small = ImageEnhance.Brightness(small).enhance(0.55)
        bg = small.resize((w, h), Image.BICUBIC).filter(ImageFilter.GaussianBlur(radius=max(2, w // 400)))
        # Soft drop shadow where the panel sits.
        pad = max(8, round(min(w, h) * 0.04))
        mask = Image.new("L", (fw + 2 * pad, fh + 2 * pad), 0)
        ImageDraw.Draw(mask).rectangle([pad, pad, pad + fw - 1, pad + fh - 1], fill=210)
        mask = mask.filter(ImageFilter.GaussianBlur(radius=pad / 2.5))
        bg.paste((0, 0, 0), (fx - pad, fy - pad + max(2, round(h * 0.008))), mask)
    else:
        bg = Image.new("RGB", (w, h), (0, 0, 0))
    bg.save(bg_path, "JPEG", quality=92)

    return {
        "fg": fg_path, "fg_pos": (fx, fy), "bg": bg_path,
        "aspect": iw / ih, "box_aspect": bw / bh,
    }


def clip_key(src, focus, motion, frames, w, h, fill):
    """Short hash naming a rendered clip, so a re-run (after Pause / Resume) reuses the clips
    it already made and renders only what changed."""
    import hashlib
    try:
        st = os.stat(src)
        ident = [os.path.abspath(src), st.st_size, int(st.st_mtime)]
    except OSError:
        ident = [src]
    raw = json.dumps([CLIP_VERSION, ident, focus, motion, frames, w, h, fill], default=str)
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:12]


def _render_clip(clip, frames, w, h, out_path):
    """Render one clip's video (no audio): the still background with the Ken Burns panel on it."""
    imgs = clip["images"]
    motion = clip["motion"]
    # Short clips move proportionally less, so a half-second beat never lurches.
    k = min(1.0, frames / (3.0 * FPS))
    fg_zoom = _motion_zoom(motion, round(FG_PUSH * k, 4), round(1 + (FG_PAN_ZOOM - 1) * k, 4))
    fx, fy = imgs["fg_pos"]
    graph = [
        f"[0:v]format=yuv420p,setsar=1,loop=loop={max(0, frames - 1)}:size=1:start=0,setpts=N/{FPS}/TB[bg]",
        f"[1:v]{_warp(FG_WINDOW[motion], fg_zoom, frames, 'cubic')},format=yuv420p,setsar=1[fg]",
        f"[bg][fg]overlay={fx}:{fy},format=yuv420p[v]",
    ]
    tmp = out_path + ".part.mp4"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-i", imgs["bg"], "-i", imgs["fg"],
        "-filter_complex", ";".join(graph),
        "-map", "[v]", "-frames:v", str(frames), "-r", str(FPS),
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
        "-an", tmp,
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    if result.returncode != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"clip render failed: {result.stderr[:300]}")
    os.replace(tmp, out_path)


def run_ffmpeg_progress(cmd, seconds, on_fraction=None, timeout=7200):
    """Run an ffmpeg command, calling on_fraction(0..1) as its output advances through
    `seconds` of media (from ffmpeg's -progress report). Returns (returncode, stderr)."""
    import tempfile
    import threading
    cmd = [cmd[0], "-progress", "pipe:1", "-nostats", *cmd[1:]]
    err = tempfile.TemporaryFile()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=err, text=True)
    timer = threading.Timer(timeout, proc.kill)
    timer.start()
    try:
        for line in proc.stdout:
            key, _, value = line.strip().partition("=")
            if key == "out_time_us" and on_fraction and seconds > 0:
                try:
                    on_fraction(min(1.0, max(0.0, int(value) / 1e6 / seconds)))
                except ValueError:
                    pass
        proc.wait()
    finally:
        timer.cancel()
    err.seek(0)
    stderr = err.read().decode("utf-8", "replace")
    err.close()
    return proc.returncode, stderr


def _render_chunk(paths, starts, trans, out_path, seconds=0.0, on_fraction=None):
    """Crossfade a run of clips into one intermediate video."""
    inputs = []
    graph = []
    labels = []
    for i, p in enumerate(paths):
        inputs += ["-i", p]
        graph.append(f"[{i}:v]settb=AVTB,setpts=PTS-STARTPTS[c{i}]")
        labels.append(f"c{i}")
    graph.append(xfade_chain(labels, starts, trans, "v"))
    tmp = out_path + ".part.mp4"
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        *inputs,
        "-filter_complex", ";".join(graph),
        "-map", "[v]", "-r", str(FPS),
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
        "-an", tmp,
    ]
    code, stderr = run_ffmpeg_progress(cmd, seconds, on_fraction, timeout=3600)
    if code != 0 or not os.path.exists(tmp):
        raise RuntimeError(f"Failed to join clips: {stderr[:300]}")
    os.replace(tmp, out_path)


def _build_narration(clips, work_dir, out_path):
    """Sample-exact narration track (48 kHz mono PCM): each clip's slice of its audio file,
    silence-padded if the file runs short, laid end to end on the rendered timeline."""
    sources = list(dict.fromkeys(c["audio_path"] for c in clips))
    converted = {src: os.path.join(work_dir, f"narr_{i:04d}.wav") for i, src in enumerate(sources)}
    # Convert every source to 48 kHz mono PCM, a batch of files per ffmpeg call.
    for b in range(0, len(sources), 24):
        batch = sources[b:b + 24]
        cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
        for src in batch:
            cmd += ["-i", src]
        for i, src in enumerate(batch):
            cmd += ["-map", f"{i}:a:0", "-ac", "1", "-ar", str(AUDIO_RATE), "-c:a", "pcm_s16le", converted[src]]
        subprocess.run(cmd, check=True, capture_output=True, timeout=600)
    with wave.open(out_path, "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(AUDIO_RATE)
        for c in clips:
            src = c["audio_path"]
            count = round((c["start"] + c["duration"]) * AUDIO_RATE) - round(c["start"] * AUDIO_RATE)
            with wave.open(converted[src], "rb") as wf:
                pos = min(wf.getnframes(), round(c["offset"] * AUDIO_RATE))
                wf.setpos(pos)
                data = wf.readframes(count)
            out.writeframes(data + b"\0\0" * (count - len(data) // 2))


# Share of the video step's progress bar each stage takes (they add up to 1).
PROGRESS_SPLIT = {"prepare": 0.05, "clips": 0.55, "chunks": 0.15, "finalize": 0.25}


class _Progress:
    """Maps stage-local 0..1 fractions onto one 0..1000 bar for progress_cb("video", ...)."""

    def __init__(self, cb, has_chunks):
        self.cb = cb
        split = dict(PROGRESS_SPLIT)
        if not has_chunks:
            split["clips"] += split.pop("chunks")
        self.split = split
        self.last = -1

    def __call__(self, stage, fraction):
        if not self.cb:
            return
        done = 0.0
        for name, share in self.split.items():
            if name == stage:
                done += share * min(1.0, max(0.0, fraction))
                break
            done += share
        value = min(1000, int(done * 1000))
        if value > self.last:
            self.last = value
            self.cb("video", value, 1000)


def generate_video(page_files, sync_map, output_dir, config=None, progress_cb=None):
    """
    Generate a narrated manga video.

    Args:
        page_files: sorted list of page image paths
        sync_map: dict with 'segments' (page→timing) from sync_map.json
        output_dir: pipeline output directory
        config: dict with options:
            - transition: 'fade' | 'crossfade' | 'none' (default: 'fade'). Anything but
              'none'/'cut' crossfades consecutive clips without shifting the narration.
            - resolution: '720p' | '1080p' | '1440p' (default: '1080p')
            - transition_duration: page-change transition seconds (default: 0.5)
            - effect: 'ken_burns' | 'none' (default: 'ken_burns')
            - fill: 'blur' (still blurred copy of the panel behind it) | 'black' (default: 'blur')
            - format: 'landscape' (one 16:9 video) | 'reels' (9:16, also cut into
              ~1 minute reels in output_dir/reels/) (default: 'landscape')
            - reel_seconds: target reel length (default: 60)
            - score / bg_music / bg_music_volume / bg_music_loop / narration_volume
            - music_volume: linear multiplier on the score / music (default: 1.0, 0 mutes)
            - subtitles: caption setting (caption_engine.normalize_settings keys: style,
              size, posX, posY, wordsPerLine, lines, uppercase, color, highlightColor,
              emphasis, enabled); falls back to <output_dir>/subtitles.json, then the
              Studio default. Words are timed against each clip's audio.
        progress_cb: function(stage, current, total)

    With music the video carries two audio tracks: "Narration + music" (the default) and
    "Narration only", so a player or an export can drop the music without a re-render.
    Rendered clips are kept (named by their content) until the video is finished, so a
    re-run after a pause or a crash only renders what is missing. The narration boundaries
    are saved next to the video in video_timeline.json (exports cut there).

    Returns:
        path to final video file
    """
    check_ffmpeg()
    config = config or {}

    transition = config.get("transition", "fade")
    resolution = config.get("resolution", "1080p")
    try:
        trans_dur = min(1.5, max(0.2, float(config.get("transition_duration", 0.5))))
    except (TypeError, ValueError):
        trans_dur = 0.5
    effect = config.get("effect", "ken_burns")
    if transition == "zoom-punch":
        effect = "ken_burns"
    trans_kind = "none" if transition in ("none", "cut") else "crossfade"
    force_rerender = config.get("force_rerender", False)
    fmt = "reels" if config.get("format") == "reels" else "landscape"
    w, h = get_resolution(resolution, fmt)
    fill = "black" if config.get("fill") == "black" else "blur"
    reels_dir = os.path.join(output_dir, "reels")

    video_dir = os.path.join(output_dir, "video_parts")
    os.makedirs(video_dir, exist_ok=True)

    # Prefer panel-level beats (each panel image synced to its slice of the page
    # audio) so the video advances panel-by-panel with the narration. Fall back
    # to page-level segments when no beats were produced.
    beat_segments = sync_map.get("beat_segments") or []
    segments = sync_map.get("segments", [])
    use_beats = bool(beat_segments)
    render_items = beat_segments if use_beats else segments
    if not render_items:
        raise RuntimeError("No segments in sync map — nothing to render")

    audio_dir = os.path.join(output_dir, "audio")
    final_video = os.path.join(output_dir, "video_final.mp4")
    timeline_file = os.path.join(output_dir, "video_timeline.json")

    # A forced re-render replaces the video; clips already rendered for the same content
    # (kept in video_parts by an interrupted run) are reused.
    if force_rerender:
        if os.path.exists(final_video):
            try:
                os.unlink(final_video)
            except OSError:
                pass
    elif os.path.exists(final_video) and os.path.getsize(final_video) > 1000 and (fmt == "reels") == os.path.isdir(reels_dir):
        return final_video

    report = _Progress(progress_cb, False)

    # ── Step 1: Resolve every clip (image, audio slice, timing) and prepare its stills ──
    clips = []
    # End time (seconds) of every rendered clip - where reels may be cut.
    boundaries = []
    # The narration actually on screen, timed on the rendered timeline (captions follow it).
    caption_segments = []
    timeline = 0.0
    total = len(render_items)
    box_aspect = (w - 2 * round(w * FG_MARGIN)) / (h - 2 * round(h * FG_MARGIN)) if fill == "blur" else w / h
    turn = 0
    prev_motion = None

    for idx, seg in enumerate(render_items):
        report("prepare", idx / total)

        page_num = seg["page"]
        audio_file = seg.get("audio_file", f"page_{page_num:04d}.wav")
        audio_path = os.path.join(audio_dir, audio_file)

        if not os.path.exists(audio_path):
            continue

        # Image: a beat carries its exact panel crop; otherwise resolve the page.
        page_image = seg.get("panel_image") if use_beats else None
        if page_image and not os.path.exists(page_image):
            page_image = None
        if not page_image:
            for pf in page_files:
                basename = os.path.basename(pf)
                # Match page number (0001.jpg, page_0001.jpg, etc.)
                if f"{page_num:04d}" in basename or f"{page_num + 1:04d}" in basename:
                    page_image = pf
                    break
        if not page_image and isinstance(page_num, int) and page_num < len(page_files):
            page_image = page_files[page_num]
        if not page_image:
            continue
        source_image = page_image

        # Crop to focus point if present (aspect matched to the panel box)
        focus_point = seg.get("focus_point")
        if focus_point and len(focus_point) == 4:
            try:
                page_image = _crop_to_focus(
                    page_image, focus_point, box_aspect,
                    os.path.join(video_dir, f"crop_{idx:04d}.jpg"),
                )
            except Exception as e:
                print(f"[video] Failed to crop page to focus point: {e}", file=sys.stderr)
        else:
            focus_point = None

        # For beats, play just this panel's slice of the page audio; for pages,
        # use the whole file.
        audio_offset = 0.0
        if use_beats and seg.get("duration_ms"):
            audio_offset = max(0.0, float(seg.get("audio_offset_ms", 0)) / 1000.0)
            duration = float(seg["duration_ms"]) / 1000.0
        else:
            duration = get_audio_duration_sec(audio_path)
        if duration < 0.1:
            continue
        duration = round(duration, 3)

        try:
            images = _prepare_clip_images(page_image, idx, w, h, fill, video_dir)
        except Exception as e:
            print(f"[video] Skipping unreadable image for page {page_num}: {e}", file=sys.stderr)
            continue

        motion = "still"
        if effect != "none":
            motion = pick_motion(images["aspect"], images["box_aspect"], turn, prev_motion)
            turn += 1
            prev_motion = motion

        clips.append({
            "idx": idx, "page": page_num, "images": images, "motion": motion,
            "source": source_image, "focus": focus_point,
            "audio_path": audio_path, "offset": audio_offset,
            "start": timeline, "duration": duration,
        })
        caption_segments.append({
            "text": seg.get("text", ""),
            "start_ms": int(round(timeline * 1000)),
            "end_ms": int(round((timeline + duration) * 1000)),
            "audio_path": audio_path,
            "audio_offset_ms": int(round(audio_offset * 1000)),
        })
        timeline += duration
        boundaries.append(timeline)

    if not clips:
        raise RuntimeError("No video segments were generated")

    chunks = plan_chunks(len(clips), CHUNK_CLIPS)
    report = _Progress(progress_cb, len(chunks) > 1)
    report("prepare", 1.0)

    # ── Step 2: Render each clip's video (with a tail that the next crossfade eats) ──
    durations = [c["duration"] for c in clips]
    starts, trans = plan_transitions(durations, [c["page"] for c in clips], trans_kind, trans_dur)
    frame_counts = [clip_frames(starts, trans, k, durations[-1]) for k in range(len(clips))]
    all_frames = max(1, sum(frame_counts))
    rendered = 0
    clip_paths = []
    for k, c in enumerate(clips):
        frames = frame_counts[k]
        key = clip_key(c["source"], c["focus"], c["motion"], frames, w, h, fill)
        path = os.path.join(video_dir, f"clip_{c['idx']:04d}_{key}.mp4")
        # Reuse a clip an interrupted run already rendered.
        if not (os.path.exists(path) and os.path.getsize(path) > 100):
            try:
                _render_clip(c, frames, w, h, path)
            except Exception as e:
                print(f"[video] {e} (page {c['page']}); retrying as a still", file=sys.stderr)
                c["motion"] = "still"
                _render_clip(c, frames, w, h, path)
        clip_paths.append(path)
        rendered += frames
        report("clips", rendered / all_frames)

    # Narration, sample-accurate against the same timeline.
    narration = os.path.join(video_dir, "narration.wav")
    _build_narration(clips, video_dir, narration)

    # ── Step 3: Crossfade runs of clips into chunks (long chapters only) ──
    if len(chunks) == 1:
        video_inputs = clip_paths
        video_starts, video_trans = starts, trans
    else:
        video_inputs, video_starts, video_trans = [], [], []
        for ci, (a, b) in enumerate(chunks):
            names = [os.path.basename(p) for p in clip_paths[a:b]]
            ckey = clip_key("", None, [names, trans[a + 1:b]], 0, w, h, fill)
            chunk_path = os.path.join(video_dir, f"chunk_{ci:03d}_{ckey}.mp4")
            span = (starts[b] if b < len(starts) else timeline) - starts[a]
            if not (os.path.exists(chunk_path) and os.path.getsize(chunk_path) > 100):
                _render_chunk(
                    clip_paths[a:b], starts[a:b], [None] + trans[a + 1:b], chunk_path, span,
                    lambda f, ci=ci: report("chunks", (ci + f) / len(chunks)),
                )
            video_inputs.append(chunk_path)
            video_starts.append(starts[a])
            video_trans.append(trans[a])
            report("chunks", (ci + 1) / len(chunks))

    # ── Step 4: Burn captions + mix narration and music + move to final location ──
    bg_music = config.get("bg_music")
    bg_music_volume = float(config.get("bg_music_volume") or 0.15)
    narration_volume = float(config.get("narration_volume") or 1.0)
    bg_music_loop = bool(config.get("bg_music_loop", True))
    try:
        music_volume = max(0.0, float(config.get("music_volume", 1.0)))
    except (TypeError, ValueError):
        music_volume = 1.0
    # MangaVox's own score (mv-score): already timed to the narration, ducked and limited.
    score = config.get("score")

    ass_path = None
    try:
        import captions
        ass_path = captions.write_styled_captions(
            caption_segments, output_dir, w, h, config=config,
        )
    except Exception as e:
        print(f"[video] Failed to generate styled captions: {e}", file=sys.stderr)

    video = {"inputs": video_inputs, "starts": video_starts, "trans": video_trans}
    on_final = lambda f: report("finalize", f)
    success = _finalize_video_with_audio_mix_and_captions(
        video, narration, final_video, ass_path, bg_music,
        narration_volume, bg_music_volume, bg_music_loop, w, h, timeline,
        score=score, music_volume=music_volume, on_fraction=on_final,
    )
    if not success:
        print("[video] Finalization failed, retrying without captions and music", file=sys.stderr)
        success = _finalize_video_with_audio_mix_and_captions(
            video, narration, final_video, None, None,
            narration_volume, 0.0, False, w, h, timeline,
            score=None, music_volume=0.0, on_fraction=on_final,
        )
    if not success:
        raise RuntimeError("Failed to finalize the video")

    try:
        with open(timeline_file, "w", encoding="utf-8") as f:
            json.dump({"duration": round(timeline, 3), "boundaries": [round(b, 3) for b in boundaries[:-1]]}, f)
    except OSError as e:
        print(f"[video] Could not save the timeline: {e}", file=sys.stderr)

    # Clean up temp segment files
    try:
        shutil.rmtree(video_dir)
    except Exception:
        pass

    if fmt == "reels":
        total_sec = get_audio_duration_sec(final_video) if os.path.exists(final_video) else timeline
        target = float(config.get("reel_seconds") or REEL_TARGET_SEC)
        split_reels(final_video, plan_reel_cuts(boundaries[:-1], total_sec, target=target), reels_dir)
    elif os.path.isdir(reels_dir):
        # Reels from an earlier upright render no longer match this video.
        shutil.rmtree(reels_dir, ignore_errors=True)

    report("finalize", 1.0)
    return final_video


def _audio_mix_graph(first, has_music, narration_vol, music_vol, solo=False):
    """Narration (input `first`) plus optional music (input `first + 1`) mixed at unity into
    [mix]; the music is ducked under the narration by a gentle sidechain compressor. With
    `solo` (and music) the narration alone is also left in [solo]."""
    fmt = f"aresample={AUDIO_RATE},aformat=sample_fmts=fltp:channel_layouts=stereo"
    nar = f"[{first}:a]{fmt},volume={narration_vol}"
    if not has_music:
        return f"{nar}[mix]"
    if solo:
        split = f"{nar},asplit=3[nar][key][solo];"
    else:
        split = f"{nar},asplit=2[nar][key];"
    return (
        split +
        f"[{first + 1}:a]{fmt},volume={music_vol}[mus];"
        "[mus][key]sidechaincompress=threshold=0.03:ratio=4:attack=20:release=400[duck];"
        "[nar][duck]amix=inputs=2:duration=first:normalize=0[mix]"
    )


def _measure_loudness(audio_inputs, mix_graph):
    """Integrated loudness (LUFS) of the mix, from a fast audio-only EBU R128 analysis pass."""
    cmd = [
        "ffmpeg", "-hide_banner", "-nostats", *audio_inputs,
        "-filter_complex", f"{mix_graph};[mix]ebur128=framelog=quiet[out]",
        "-map", "[out]", "-f", "null", "-",
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
        found = re.findall(r"^\s*I:\s*(-?[\d.]+|-inf)\s*LUFS", result.stderr or "", re.M)
        if result.returncode != 0 or not found:
            return None
        value = float(found[-1])
        # Below ~-70 LUFS is effectively silence: no meaningful gain to derive.
        return value if math.isfinite(value) and value > -70 else None
    except Exception as e:
        print(f"[video] Loudness measurement failed: {e}", file=sys.stderr)
        return None


def _finalize_video_with_audio_mix_and_captions(
    video, narration, output_video, ass_path, bg_music,
    narration_vol, bg_vol, bg_loop, w, h, total, score=None, music_volume=1.0, on_fraction=None,
):
    """
    Joins the clip/chunk videos with their crossfades, burns subtitles, and mixes the
    narration with the score (or looped background music) in a single FFmpeg command.
    With music, a second "Narration only" audio track is written too.

    Loudness is two-pass and linear: the mix is measured first, then one fixed gain brings
    it to LOUD_I and a limiter catches peaks, so music never swells in narration pauses.
    """
    inputs = []
    for p in video["inputs"]:
        inputs += ["-i", p]
    n_video = len(video["inputs"])

    # The chapter's own score replaces any looped background track.
    has_score = bool(score) and os.path.exists(score) and music_volume > 0
    has_bg = bool(bg_music) and os.path.exists(bg_music) and not has_score and music_volume > 0 and bg_vol > 0
    audio_inputs = ["-i", narration]
    music_vol = 1.0
    if has_score:
        audio_inputs += ["-i", score]
        music_vol = music_volume
    elif has_bg:
        if bg_loop:
            audio_inputs += ["-stream_loop", "-1"]
        audio_inputs += ["-i", bg_music]
        music_vol = bg_vol * music_volume

    has_music = has_score or has_bg
    if on_fraction:
        on_fraction(0.0)
    measured = _measure_loudness(audio_inputs, _audio_mix_graph(0, has_music, narration_vol, music_vol))
    gain_db = 0.0 if measured is None else max(-20.0, min(20.0, LOUD_I - measured))
    limit = round(10 ** (LOUD_TP / 20), 4)
    if on_fraction:
        on_fraction(0.05)

    graph = []
    # 1. Video: crossfade the parts, then captions
    labels = []
    for i in range(n_video):
        graph.append(f"[{i}:v]settb=AVTB,setpts=PTS-STARTPTS[p{i}]")
        labels.append(f"p{i}")
    graph.append(xfade_chain(labels, video["starts"], video["trans"], "vx"))
    has_subs = bool(ass_path) and os.path.exists(ass_path)
    if has_subs:
        from caption_engine import subtitles_filter
        graph.append(f"[vx]{subtitles_filter(ass_path)},format=yuv420p[v]")
    else:
        graph.append("[vx]format=yuv420p[v]")

    # 2. Audio: mix, one measured gain, limiter (and the narration alone, when there is music)
    graph.append(_audio_mix_graph(n_video, has_music, narration_vol, music_vol, solo=has_music))
    graph.append(f"[mix]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a]")
    maps = ["-map", "[v]", "-map", "[a]"]
    # MP4 keeps a track's name as its handler name (players list tracks by it).
    first = "Narration + music" if has_music else "Narration"
    tracks = ["-metadata:s:a:0", f"title={first}", "-metadata:s:a:0", f"handler_name={first}", "-disposition:a:0", "default"]
    if has_music:
        graph.append(f"[solo]volume={gain_db:.2f}dB,alimiter=limit={limit}:level=0:latency=1,aresample={AUDIO_RATE}[a2]")
        maps += ["-map", "[a2]"]
        tracks += ["-metadata:s:a:1", "title=Narration only", "-metadata:s:a:1", "handler_name=Narration only", "-disposition:a:1", "0"]

    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        *inputs, *audio_inputs,
        "-filter_complex", ";".join(graph),
        *maps,
        "-c:v", "libx264", "-preset", "fast", "-crf", "21", "-pix_fmt", "yuv420p", "-r", str(FPS),
        "-c:a", "aac", "-b:a", "192k", "-ar", str(AUDIO_RATE),
        *tracks,
        "-t", f"{total:.3f}",
        "-movflags", "+faststart",
        output_video,
    ]

    try:
        code, stderr = run_ffmpeg_progress(
            cmd, total, (lambda f: on_fraction(0.05 + 0.95 * f)) if on_fraction else None,
        )
        if code != 0:
            print(f"[video] Finalization command failed: {stderr[:400]}", file=sys.stderr)
            return False
        return os.path.exists(output_video) and os.path.getsize(output_video) > 1000
    except Exception as e:
        print(f"[video] Finalization command error: {e}", file=sys.stderr)
        return False
