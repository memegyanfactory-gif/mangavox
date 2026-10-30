#!/usr/bin/env python3
"""Voice and render the hook: a short teaser played before chapter 1 of a full recap video.

  python hook_maker.py <args.json>

args: {lines: [{text, image, focus?}], outro: {text, image}, voice, engine, speed, resolution,
       subtitles, score: {wav, start, seconds} | null, out, work_dir}

Each line is voiced (tts_bridge.run_tts, one "page" per line, the outro last), shown over its
image (cropped to `focus` when given), and rendered like a chapter video (video_generator,
dynamic look, the chapter's caption style). With `score`, a slice of that chapter's score.wav
from `start` plays under it, fading out over the last 1.5 s. The video is copied to `out`.

Prints one JSON line: {"ok": true, "path": out, "seconds": ...} or {"ok": false, "message": ...}.
"""
import json
import os
import shutil
import subprocess
import sys
import traceback

FADE_SEC = 1.5


def _score_slice(score, seconds, work_dir):
    """`seconds` of the score from its `start`, fading out at the end; None when unusable."""
    wav = (score or {}).get("wav")
    if not wav or not os.path.isfile(wav) or seconds <= 0:
        return None
    start = max(0.0, float(score.get("start") or 0.0))
    out = os.path.join(work_dir, "hook_score.wav")
    fade_at = max(0.0, seconds - FADE_SEC)
    cmd = [
        "ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
        "-ss", f"{start:.3f}", "-t", f"{seconds:.3f}", "-i", wav,
        "-af", f"afade=t=out:st={fade_at:.3f}:d={min(FADE_SEC, seconds):.3f}",
        out,
    ]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not os.path.isfile(out):
        print(f"[hook] could not cut the score: {(r.stderr or '').strip()[-300:]}", file=sys.stderr)
        return None
    return out


def make_hook(args):
    from tts_bridge import run_tts
    from video_generator import generate_video, get_audio_duration_sec

    work_dir = args["work_dir"]
    out = args["out"]
    os.makedirs(work_dir, exist_ok=True)
    shots = [{"text": (l.get("text") or "").strip(), "image": l.get("image"), "focus": l.get("focus")} for l in args.get("lines") or []]
    outro = args.get("outro") or {}
    if (outro.get("text") or "").strip():
        # The closing line shows the cover whole, centred (never half of a side-by-side pair).
        shots.append({"text": outro["text"].strip(), "image": outro.get("image"), "focus": None, "layout": "single"})
    shots = [s for s in shots if s["text"] and s["image"] and os.path.isfile(s["image"])]
    if not shots:
        raise RuntimeError("The hook has no lines with a picture")

    segments = [{"page": i, "text": s["text"]} for i, s in enumerate(shots)]
    tts = {"engine": args.get("engine") or "kokoro", "voice": args.get("voice") or "af_heart", "speed": args.get("speed") or 1.0, "force_regen": True}
    voiced = {a.get("page"): a for a in (run_tts(segments, work_dir, tts) or [])}

    beats = []
    total = 0.0
    for i, s in enumerate(shots):
        a = voiced.get(i) or {}
        path = a.get("path")
        if not path or not os.path.isfile(path) or a.get("error"):
            print(f"[hook] line {i + 1} was not voiced: {a.get('error') or 'no clip'}", file=sys.stderr)
            continue
        secs = get_audio_duration_sec(path)
        if secs < 0.1:
            continue
        beat = {
            "page": i,
            "panel_image": s["image"],
            "text": s["text"],
            "audio_file": os.path.basename(path),
            "audio_offset_ms": 0,
            "duration_ms": int(round(secs * 1000)),
            "start_ms": int(round(total * 1000)),
        }
        if s.get("layout"):
            beat["layout"] = s["layout"]
        if s["focus"] and len(s["focus"]) == 4:
            beat["focus_point"] = s["focus"]
        beats.append(beat)
        total += secs
    if not beats:
        raise RuntimeError("No line of the hook could be voiced")
    sync_map = {"beat_segments": beats, "total_duration_ms": int(round(total * 1000))}
    with open(os.path.join(work_dir, "sync_map.json"), "w", encoding="utf-8") as f:
        json.dump(sync_map, f, indent=1)

    config = {
        "resolution": args.get("resolution") or "1080p",
        "look": "dynamic",
        "subtitles": args.get("subtitles"),
        "force_rerender": True,
    }
    score = _score_slice(args.get("score"), total, work_dir)
    if score:
        config["score"] = score
    pages = [b["panel_image"] for b in beats]
    video = generate_video(pages, sync_map, work_dir, config)
    if not video or not os.path.isfile(video):
        raise RuntimeError("The hook video was not made")
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    shutil.copyfile(video, out)
    return {"ok": True, "path": out, "seconds": round(total, 3), "lines": len(beats)}


def main() -> int:
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "Usage: hook_maker.py <args.json>"}), flush=True)
        return 2
    real_stdout = sys.stdout
    try:
        with open(sys.argv[1], "r", encoding="utf-8") as f:
            args = json.load(f)
        if not args.get("out") or not args.get("work_dir"):
            raise RuntimeError("out and work_dir required")
        # The voice and video modules print progress lines; only our result goes to stdout.
        sys.stdout = sys.stderr
        result = make_hook(args)
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
