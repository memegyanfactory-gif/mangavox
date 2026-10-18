#!/usr/bin/env python3
"""Generate a short voice preview clip for the Audio tab UI."""
import json
import sys
import traceback


def main() -> int:
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "Usage: voice_preview.py <args.json>"}), flush=True)
        return 2

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
