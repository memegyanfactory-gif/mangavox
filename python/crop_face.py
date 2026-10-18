#!/usr/bin/env python3
"""
MangaVox Face-Crop Worker — saves a sub-region of a manga page as a separate
image file (used by the Cast Manager to store character face references).

Unlike `crop_page.py`, this DOES NOT modify the source image. It only writes
a new file at `output_path`.

Usage: python3 crop_face.py <job.json>
job.json:
  {
    "image_path":   "/abs/path/to/page.jpg",
    "output_path":  "/abs/path/to/face.png",
    "bbox":         [x, y, w, h]   // natural-image pixel coordinates
  }
Output (one line, JSON):
  {"type": "done", "output_path": "..."}    or    {"type": "error", "message": "..."}
"""
import sys
import json
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print(json.dumps({"type": "error", "message": "Pillow not installed. Run: pip install Pillow"}), flush=True)
    sys.exit(1)


def emit(obj):
    print(json.dumps(obj), flush=True)


def main() -> int:
    if len(sys.argv) < 2:
        emit({"type": "error", "message": "Missing job.json argument"})
        return 1
    try:
        with open(sys.argv[1], "r", encoding="utf-8") as f:
            job = json.load(f)
    except Exception as e:
        emit({"type": "error", "message": f"Cannot read job.json: {e}"})
        return 1

    src = Path(job.get("image_path", ""))
    dst = Path(job.get("output_path", ""))
    bbox = job.get("bbox")

    if not src.is_file():
        emit({"type": "error", "message": f"image_path not found: {src}"})
        return 1
    if not isinstance(bbox, list) or len(bbox) != 4:
        emit({"type": "error", "message": "bbox must be [x, y, w, h]"})
        return 1

    x, y, w, h = (int(round(v)) for v in bbox)
    if w <= 0 or h <= 0:
        emit({"type": "error", "message": "bbox width/height must be positive"})
        return 1

    dst.parent.mkdir(parents=True, exist_ok=True)

    try:
        with Image.open(src) as img:
            iw, ih = img.size
            l = max(0, min(x, iw))
            t = max(0, min(y, ih))
            r = max(0, min(x + w, iw))
            b = max(0, min(y + h, ih))
            if r - l <= 0 or b - t <= 0:
                emit({"type": "error", "message": "bbox lies outside the image"})
                return 1
            cropped = img.crop((l, t, r, b))
            # Always save face crops as PNG so transparent borders render cleanly
            # in the cast UI when the bounding box doesn't match a rectangle.
            cropped.save(dst, format="PNG", optimize=True)
    except Exception as e:
        emit({"type": "error", "message": f"Crop failed: {e}"})
        return 1

    emit({"type": "done", "output_path": str(dst)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
