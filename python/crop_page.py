#!/usr/bin/env python3
"""
MangaVox Single-Page Manual Crop Worker

Crops one specific page image to a user-drawn bounding box and writes the
result in place. Backs up the original to <pages_original>/<filename> on
the first run so the existing Restore flow continues to work.

Communicates via JSON-lines on stdout:
  {"type": "done", "image_path": "..."}
  {"type": "error", "message": "..."}

Usage: python3 crop_page.py <job.json>

job.json schema:
  {
    "image_path": "/abs/path/to/page.jpg",
    "backup_dir": "/abs/path/to/pages_original",
    "bbox": [x, y, w, h]   // in natural-image pixel coordinates
  }
"""
import sys
import json
import shutil
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

    image_path = Path(job.get("image_path", ""))
    backup_dir = Path(job.get("backup_dir", ""))
    bbox = job.get("bbox")

    if not image_path.is_file():
        emit({"type": "error", "message": f"image_path not found: {image_path}"})
        return 1
    if not isinstance(bbox, list) or len(bbox) != 4:
        emit({"type": "error", "message": "bbox must be [x, y, w, h]"})
        return 1

    x, y, w, h = (int(round(v)) for v in bbox)
    if w <= 0 or h <= 0:
        emit({"type": "error", "message": "bbox width/height must be positive"})
        return 1

    # Backup the original on first manual crop so Restore still works
    backup_dir.mkdir(parents=True, exist_ok=True)
    backup_path = backup_dir / image_path.name
    if not backup_path.exists():
        try:
            shutil.copy2(image_path, backup_path)
        except Exception as e:
            emit({"type": "error", "message": f"Could not back up original: {e}"})
            return 1

    try:
        with Image.open(image_path) as img:
            iw, ih = img.size
            # Clamp bbox into the image
            l = max(0, min(x, iw))
            t = max(0, min(y, ih))
            r = max(0, min(x + w, iw))
            b = max(0, min(y + h, ih))
            if r - l <= 0 or b - t <= 0:
                emit({"type": "error", "message": "bbox lies outside the image"})
                return 1

            cropped = img.crop((l, t, r, b))
            save_kwargs = {}
            fmt = (img.format or "").upper()
            if fmt in ("JPEG", "JPG"):
                save_kwargs.update(quality=92, optimize=True)

            tmp = image_path.with_suffix(image_path.suffix + ".tmp")
            cropped.save(tmp, **save_kwargs)
            tmp.replace(image_path)
    except Exception as e:
        emit({"type": "error", "message": f"Crop failed: {e}"})
        return 1

    emit({"type": "done", "image_path": str(image_path)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
