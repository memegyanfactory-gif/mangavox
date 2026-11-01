#!/usr/bin/env python3
"""One frame of a chapter's video with a given video style, made by the renderer itself.

  python style_preview.py <args.json>

args: {output_dir, pages_dir, config, work_dir, out, at?, beats?, max_seconds?}

`config` is the chapter's `video_config` as a render gets it (look, subtitles, edit, format, ...),
built by Studio's dashboard "Video style" panel from the choices on screen. A couple of the
chapter's beats around `at` (a share of the chapter, 0.15 by default) are rendered with
video_generator.generate_video into `work_dir` - the same clip camera, framing QA, look filters,
mood grade, captions and finish as the full render - and the frame halfway into the last of them
is saved to `out` (JPEG). So the preview shows what the render will, with these differences: the
clips are capped to `max_seconds` each (the camera eases over the shorter clip the same way), the
Video editor's clip list and overlays are left out (they are timed to the whole chapter), there is
no sound, and layered motion is off (the pack's animated layers are made by the full render).

Prints one JSON line: {"ok": true, "path": out, "beats": [...], "at": seconds} or
{"ok": false, "message": ...}.
"""
import json
import os
import shutil
import subprocess
import sys
import traceback

# Beats rendered (two show a side-by-side pair with its partner) and the longest each may be.
BEATS = 2
MAX_SECONDS = 6.0
# Where in the chapter the frame is taken (past the opening, where the picture is typical), and
# how far into the last clip (its transition is over, its caption is on screen).
AT = 0.15
INTO_CLIP = 0.45


def pick_beats(items, at=AT, count=BEATS):
    """Indexes of `count` consecutive render items: from the first one with words at or after
    `at` of the chapter (else from there), kept inside the list."""
    n = len(items)
    if n == 0:
        return []
    count = max(1, min(count, n))
    start = min(n - 1, max(0, int(n * at)))
    for i in range(start, n):
        if str(items[i].get("text") or "").strip():
            start = i
            break
    lo = max(0, min(start, n - count))
    return list(range(lo, lo + count))


def preview_beats(sync_map, output_dir, picks, max_seconds=MAX_SECONDS):
    """The picked items as beats of their own: audio by absolute path (so they render from any
    folder), each at most `max_seconds`, laid end to end from 0."""
    items = sync_map.get("beat_segments") or sync_map.get("segments") or []
    out = []
    t = 0
    for i in picks:
        seg = dict(items[i])
        page = seg.get("page", i)
        audio = seg.get("audio_file") or f"page_{page:04d}.wav"
        audio = audio if os.path.isabs(audio) else os.path.join(output_dir, "audio", audio)
        if seg.get("duration_ms"):
            dur = float(seg["duration_ms"])
        else:
            from video_generator import get_audio_duration_sec
            dur = get_audio_duration_sec(audio) * 1000.0
            seg["audio_offset_ms"] = 0
        dur = int(round(min(dur, max_seconds * 1000.0)))
        if dur < 100:
            continue
        seg.update({"audio_file": audio, "duration_ms": dur, "start_ms": t})
        # The editor's per-clip choices belong to its own clip list, which is left out.
        for k in ("cut", "transition", "motion", "amount", "focus_from", "focus_to", "speed"):
            seg.pop(k, None)
        out.append(seg)
        t += dur
    return out, t


def preview_config(config):
    """`config` for a silent preview render: no music or sound effects, no layered motion, the
    editor's clip list and overlays left out (they are timed to the whole chapter)."""
    cfg = dict(config or {})
    cfg["force_rerender"] = True
    cfg["sfx"] = "off"
    for k in ("score", "bg_music", "vfx_python", "scenes"):
        cfg.pop(k, None)
    cfg["music_volume"] = 0.0
    if isinstance(cfg.get("look"), dict):
        cfg["look"] = dict(cfg["look"], layered="off")
    if isinstance(cfg.get("edit"), dict):
        edit = dict(cfg["edit"])
        for k in ("clips", "overlays"):
            edit.pop(k, None)
        cfg["edit"] = edit
    return cfg


def list_pages(pages_dir):
    try:
        names = sorted(os.listdir(pages_dir))
    except OSError:
        return []
    return [os.path.join(pages_dir, n) for n in names if n.lower().endswith((".jpg", ".jpeg", ".png", ".webp"))]


def make_preview(args):
    from video_generator import generate_video

    output_dir = args["output_dir"]
    work_dir = args["work_dir"]
    out = args["out"]
    with open(os.path.join(output_dir, "sync_map.json"), "r", encoding="utf-8") as f:
        sync_map = json.load(f)
    items = sync_map.get("beat_segments") or sync_map.get("segments") or []
    picks = pick_beats(items, float(args.get("at") or AT), int(args.get("beats") or BEATS))
    beats, total_ms = preview_beats(sync_map, output_dir, picks, float(args.get("max_seconds") or MAX_SECONDS))
    if not beats:
        raise RuntimeError("The chapter has no narrated beat to preview")

    shutil.rmtree(work_dir, ignore_errors=True)
    os.makedirs(work_dir, exist_ok=True)
    # The chapter's reading (moods, panel boxes and faces) decides the grade and the framing.
    read = os.path.join(output_dir, "studio", "read.json")
    if os.path.isfile(read):
        os.makedirs(os.path.join(work_dir, "studio"), exist_ok=True)
        shutil.copyfile(read, os.path.join(work_dir, "studio", "read.json"))
    small = {"beat_segments": beats, "total_duration_ms": total_ms}
    video = generate_video(list_pages(args.get("pages_dir") or ""), small, work_dir, preview_config(args.get("config")))
    if not video or not os.path.isfile(video):
        raise RuntimeError("The preview was not rendered")

    durs = [b["duration_ms"] / 1000.0 for b in beats]
    at = sum(durs[:-1]) + durs[-1] * INTO_CLIP
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    tmp = out + ".tmp.jpg"
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", video, "-ss", f"{at:.3f}", "-frames:v", "1", "-q:v", "3", tmp]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not os.path.isfile(tmp):
        raise RuntimeError(f"Could not take the frame: {(r.stderr or '').strip()[-300:]}")
    os.replace(tmp, out)
    shutil.rmtree(work_dir, ignore_errors=True)
    return {"ok": True, "path": out, "beats": picks, "at": round(at, 3)}


def main() -> int:
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "Usage: style_preview.py <args.json>"}), flush=True)
        return 2
    real_stdout = sys.stdout
    try:
        with open(sys.argv[1], "r", encoding="utf-8") as f:
            args = json.load(f)
        if not args.get("output_dir") or not args.get("work_dir") or not args.get("out"):
            raise RuntimeError("output_dir, work_dir and out required")
        # The video module prints progress lines; only our result goes to stdout.
        sys.stdout = sys.stderr
        result = make_preview(args)
        sys.stdout = real_stdout
        print(json.dumps(result), flush=True)
        return 0
    except Exception as exc:
        sys.stdout = real_stdout
        traceback.print_exc(file=sys.stderr)
        print(json.dumps({"ok": False, "message": str(exc)}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
