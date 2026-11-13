"""Creative's scene voice with Qwen3-TTS VoiceDesign: every line of a scene is spoken by a
voice described in words (who the character is and how they say this line), then the lines are
joined into one WAV.

Runs in the Qwen3-TTS engine's own Python (`<data>/engines/qwen3tts/.venv`), never in the
Kokoro/Piper `tts-venv`:

    python qwen_scene_tts.py <args.json>

args: {"model_dir": "...", "out": "scene.wav", "language": "English", "gap_ms": 300,
       "units": [{"text": "...", "instruct": "A man in his thirties ... He is furious.", "seed": 123}]}

Prints one JSON line: {"ok": true, "path", "duration", "lines": [seconds...]} or
{"ok": false, "message"}. A unit's `seed` is its character's own, so the same description gives
the same kind of voice from scene to scene.
"""
import json
import os
import sys
import traceback


def join(parts, sr, gap_ms):
    """The lines one after another with `gap_ms` of silence between them, peak at 0.9."""
    import numpy as np

    gap = np.zeros(int(sr * max(0, gap_ms) / 1000.0), dtype=np.float32)
    out = []
    for i, p in enumerate(parts):
        if i:
            out.append(gap)
        out.append(np.asarray(p, dtype=np.float32).reshape(-1))
    audio = np.concatenate(out) if out else np.zeros(1, dtype=np.float32)
    peak = float(np.max(np.abs(audio))) if audio.size else 0.0
    if peak > 1e-4:
        audio = audio * (0.9 / peak)
    return audio


def run(args):
    import torch
    import soundfile as sf
    from qwen_tts import Qwen3TTSModel

    units = [u for u in args.get("units") or [] if str(u.get("text") or "").strip()]
    if not units:
        raise ValueError("no lines to speak")
    cuda = torch.cuda.is_available()
    model = Qwen3TTSModel.from_pretrained(
        args["model_dir"],
        device_map="cuda:0" if cuda else "cpu",
        dtype=torch.bfloat16 if cuda else torch.float32,
    )
    parts, lengths, sr = [], [], 24000
    for u in units:
        seed = int(u.get("seed") or 0) & 0x7FFFFFFF
        torch.manual_seed(seed)
        if cuda:
            torch.cuda.manual_seed_all(seed)
        wavs, sr = model.generate_voice_design(
            text=str(u["text"]).strip(),
            language=args.get("language") or "English",
            instruct=str(u.get("instruct") or "Speaking naturally."),
        )
        parts.append(wavs[0])
        lengths.append(round(len(wavs[0]) / sr, 2))
    audio = join(parts, sr, int(args.get("gap_ms", 300)))
    out = args["out"]
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    sf.write(out, audio, sr)
    return {"ok": True, "path": out, "duration": round(len(audio) / sr, 3), "lines": lengths}


def main():
    if len(sys.argv) < 2:
        print(json.dumps({"ok": False, "message": "Usage: qwen_scene_tts.py <args.json>"}), flush=True)
        return 2
    try:
        with open(sys.argv[1], "r", encoding="utf-8") as f:
            args = json.load(f)
        print(json.dumps(run(args)), flush=True)
        return 0
    except Exception as exc:  # the caller shows the reason and falls back to the other voice
        traceback.print_exc(file=sys.stderr)
        print(json.dumps({"ok": False, "message": f"{type(exc).__name__}: {exc}"}), flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())
