#!/usr/bin/env python3
"""
MangaVox Auto-Crop Worker

Trims uniform-color borders (white/black/gray scan margins) from manga pages
using a content-bounding-box detection. Originals are preserved by moving the
untouched files into pages_original/ on first run, so the operation is fully
reversible.

Communicates via JSON-lines on stdout (same protocol as vision_extract.py):
  {"type": "progress", "current": N, "total": T, "percent": P}
  {"type": "done", "cropped": K, "skipped": S}
  {"type": "error", "message": "..."}

Usage: python3 autocrop.py <job.json>
job.json schema: { "pages_dir": str, "padding": int (optional, default 6),
                   "threshold": int (optional, default 12) }
"""
import sys
import json
import os
import shutil
from pathlib import Path

try:
    from PIL import Image, ImageChops, ImageOps
except ImportError:
    print(json.dumps({"type": "error", "message": "Pillow not installed. Run: pip install Pillow"}), flush=True)
    sys.exit(1)


def emit(obj):
    print(json.dumps(obj), flush=True)


def detect_bbox(img: "Image.Image", threshold: int) -> tuple[int, int, int, int] | None:
    """
    Find the content bounding box by comparing each edge to its corner color.
    Works for both white-bordered and black-bordered scans. Returns (l, t, r, b)
    or None if nothing meaningful was found.
    """
    rgb = img.convert("RGB")

    # Sample the four corners — pick the most common as the assumed background
    w, h = rgb.size
    corners = [rgb.getpixel((0, 0)), rgb.getpixel((w - 1, 0)),
               rgb.getpixel((0, h - 1)), rgb.getpixel((w - 1, h - 1))]
    # Quantise each corner to nearest 16 to merge near-identical ones
    quantised = [tuple(c // 16 * 16 for c in p) for p in corners]
    bg = max(set(quantised), key=quantised.count)

    bg_img = Image.new("RGB", rgb.size, bg)
    diff = ImageChops.difference(rgb, bg_img)
    # Boost weak signals so very faint edges still register, then threshold
    diff = ImageChops.add(diff, diff, 2.0, -threshold)
    bbox = diff.getbbox()
    return bbox


def crop_image(src: Path, dst: Path, padding: int, threshold: int) -> bool:
    """Returns True if a meaningful crop was applied, False if skipped."""
    with Image.open(src) as img:
        # Keep the original mode for output (don't force RGB — preserves PNG alpha)
        bbox = detect_bbox(img, threshold)
        if bbox is None:
            return False

        l, t, r, b = bbox
        w, h = img.size

        # Add padding within image bounds
        l = max(0, l - padding)
        t = max(0, t - padding)
        r = min(w, r + padding)
        b = min(h, b + padding)

        # Skip if the crop barely changes the image (less than 1% off any side)
        min_trim_px = 4
        if (l < min_trim_px and t < min_trim_px
                and (w - r) < min_trim_px and (h - b) < min_trim_px):
            return False

        cropped = img.crop((l, t, r, b))
        # Strip EXIF/metadata when saving — Pillow does this implicitly when no exif kwarg passed
        save_kwargs = {}
        fmt = (img.format or "").upper()
        if fmt in ("JPEG", "JPG"):
            save_kwargs.update(quality=92, optimize=True)
        cropped.save(dst, **save_kwargs)
        return True


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

    pages_dir = Path(job.get("pages_dir", ""))
    padding = int(job.get("padding", 6))
    threshold = int(job.get("threshold", 12))

    if not pages_dir.is_dir():
        emit({"type": "error", "message": f"pages_dir not found: {pages_dir}"})
        return 1

    # Collect image files
    exts = {".jpg", ".jpeg", ".png", ".webp", ".gif"}
    pages = sorted(p for p in pages_dir.iterdir()
                   if p.is_file() and p.suffix.lower() in exts)
    if not pages:
        emit({"type": "error", "message": "No image files found in pages_dir"})
        return 1

    # Backup originals on first run so the user can restore later
    backup_dir = pages_dir.parent / "pages_original"
    backup_dir.mkdir(exist_ok=True)
    for p in pages:
        backup = backup_dir / p.name
        if not backup.exists():
            shutil.copy2(p, backup)

    total = len(pages)
    cropped = 0
    skipped = 0

    for i, page in enumerate(pages, 1):
        try:
            tmp = page.with_suffix(page.suffix + ".tmp")
            did_crop = crop_image(page, tmp, padding, threshold)
            if did_crop:
                tmp.replace(page)
                cropped += 1
            else:
                tmp.unlink(missing_ok=True)
                skipped += 1
        except Exception as e:
            emit({"type": "warn", "page": page.name, "message": str(e)})
            skipped += 1

        emit({"type": "progress", "current": i, "total": total,
              "percent": round(i / total * 100, 1)})

    emit({"type": "done", "cropped": cropped, "skipped": skipped, "total": total})
    return 0


if __name__ == "__main__":
    sys.exit(main())
