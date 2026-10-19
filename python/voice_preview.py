#!/usr/bin/env python3
"""Generate a short voice preview clip for the Audio tab UI.

  python voice_preview.py <args.json>        {voice, speed?, engine?, output_path}
  python voice_preview.py lab <args.json>    voice lab: {voice, kind, out_dir, text?, intensities?
                                             (default [2, 5, 8]), mood?, strength?, reference?}

Both print one JSON line: {"ok": true, ...} or {"ok": false, "message": ...}.
"""
import json
import sys
import traceback


def lab(args_path: str) -> int:
    with open(args_path, "r", encoding="utf-8") as f:
        args = json.load(f)
    if not args.get("out_dir"):
        print(json.dumps({"ok": False, "message": "out_dir required"}), flush=True)
        return 1
    try:
        from tts_bridge import voice_lab

        result = voice_lab(
            args.get("voice", "cb_narrator_f"),
            args.get("kind", "other"),
            args["out_dir"],
            text=args.get("text"),
            intensities=tuple(args.get("intensities") or (2, 5, 8)),
            mood=args.get("mood", ""),
            strength=float(args.get("strength", 1.0)),
            reference=args.get("reference"),
        )
        print(json.dumps(result), flush=True)
        return 0
    except Exception as exc:
        traceback.print_exc(file=sys.stderr)
        print(json.dumps({"ok": False, "message": str(exc)}), flush=True)
        return 1


def main() -> int:
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "Usage: voice_preview.py <args.json> | lab <args.json>"}), flush=True)
        return 2
    if sys.argv[1] == "lab":
        if len(sys.argv) < 3:
            print(json.dumps({"ok": False, "message": "Usage: voice_preview.py lab <args.json>"}), flush=True)
            return 2
        return lab(sys.argv[2])

    with open(sys.argv[1], "r", encoding="utf-8") as f:
        args = json.load(f)

    voice = args.get("voice", "hf_alpha")
    speed = float(args.get("speed", 1.0))
    engine = args.get("engine")
    output_path = args.get("output_path")
    if not output_path:
        print(json.dumps({"ok": False, "message": "output_path required"}), flush=True)
        return 1

    try:
        from tts_bridge import preview_voice

        result = preview_voice(output_path, voice, speed, engine)
        print(json.dumps({"ok": True, **result}), flush=True)
        return 0
    except Exception as exc:
        traceback.print_exc(file=sys.stderr)
        print(json.dumps({"ok": False, "message": str(exc)}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
