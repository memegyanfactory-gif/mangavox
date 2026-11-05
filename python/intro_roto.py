#!/usr/bin/env python3
"""Cut characters out of manga pages for the intro's cards (run in the layered-motion pack's venv).

  python intro_roto.py <jobs.json>

jobs: [{candidates: [{image, box: [x0, y0, x1, y1] (pixels, the character's body), point: [x, y]
        (the face), face_h (pixels)}, ...] (best first), out: png}]

Each character is matted with SAM2 (the pack's sam2.1-hiera-small, prompted with the body box and
a point on the face), cleaned (holes filled, only the piece holding the face kept, edges feathered
a pixel) and saved as an RGBA PNG cropped to the character. A matte must look like a standing
figure (taller than wide, filling its box sensibly, the face near its top); else the next
candidate is tried. Prints one JSON line:
{"ok": true, "done": [out, ...]} (a job whose matte is poor is left out) or {"ok": false, "message"}.
"""
import json
import os
import sys
import traceback

import numpy as np
from PIL import Image, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)


def pack_dir():
    import layered_motion
    return layered_motion.PACK_DIR


def fill_holes(m):
    """Fills enclosed holes of a boolean mask (flood fill from the border)."""
    h, w = m.shape
    pad = np.pad(~m, 1, constant_values=True)
    seen = np.zeros_like(pad, bool)
    stack = [(0, 0)]
    seen[0, 0] = True
    while stack:
        y, x = stack.pop()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h + 2 and 0 <= nx < w + 2 and pad[ny, nx] and not seen[ny, nx]:
                seen[ny, nx] = True
                stack.append((ny, nx))
    return m | ~seen[1:-1, 1:-1]


def component_at(m, pt):
    """The connected piece of `m` holding `pt` (or the largest piece)."""
    h, w = m.shape
    lab = np.zeros((h, w), np.int32)
    best, best_n, at = 0, 0, 0
    cur = 0
    px, py = int(min(w - 1, max(0, pt[0]))), int(min(h - 1, max(0, pt[1])))
    for sy in range(0, h, 2):
        for sx in range(0, w, 2):
            if m[sy, sx] and not lab[sy, sx]:
                cur += 1
                stack, n = [(sy, sx)], 0
                lab[sy, sx] = cur
                while stack:
                    y, x = stack.pop()
                    n += 1
                    for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                        ny, nx = y + dy, x + dx
                        if 0 <= ny < h and 0 <= nx < w and m[ny, nx] and not lab[ny, nx]:
                            lab[ny, nx] = cur
                            stack.append((ny, nx))
                if n > best_n:
                    best, best_n = cur, n
    keep = lab[py, px] if lab[py, px] else best
    return lab == keep


def lettering(crop, faces):
    """Speech bubbles and lettering in `crop` (layered_plan's detector), grown a little, as a mask
    the size of the crop; None when the detector is not available."""
    try:
        import layered_plan as lp
    except ImportError:
        return None
    side = 480
    k = side / max(crop.width, crop.height)
    small = crop.convert("L").resize((max(8, int(crop.width * k)), max(8, int(crop.height * k))), Image.BILINEAR)
    gray = np.asarray(small).astype(np.float32) / 255
    f = [(x * k - 20, y * k - 20, x * k + 20, y * k + 20) for x, y in faces]
    try:
        mask = lp.detect_lettering(gray, f)
    except Exception:
        return None
    im = Image.fromarray(np.asarray(mask).astype(np.uint8) * 255).filter(ImageFilter.MaxFilter(5))
    return np.asarray(im.resize(crop.size, Image.NEAREST)) > 127


def drawn(rgb, m):
    """Whether the cut is line art (dense edges) rather than a flat block of colour."""
    g = rgb[..., :3].astype(np.float32).mean(axis=2)
    mag = np.maximum(np.abs(np.diff(g, axis=1))[:-1, :], np.abs(np.diff(g, axis=0))[:, :-1])
    mm = m[:-1, :-1]
    if mm.sum() < 50:
        return False
    edge = (mag > 40)[mm].mean()
    px = rgb[..., :3][m].astype(np.float32)
    mx, mn = px.max(axis=1), px.min(axis=1)
    sat = ((mx - mn) / np.maximum(mx, 1) > 0.45).mean()
    return edge >= 0.03 and not (sat > 0.6 and edge < 0.06)


def matte(proc, sam, j):
    """The character of one candidate as a cropped RGBA image, or None when the matte is poor."""
    import torch
    page = Image.open(j["image"]).convert("RGB")
    x0, y0, x1, y1 = [float(v) for v in j["box"]]
    pad = 0.08 * max(x1 - x0, y1 - y0)
    cx0, cy0 = max(0, int(x0 - pad)), max(0, int(y0 - pad))
    cx1, cy1 = min(page.width, int(x1 + pad)), min(page.height, int(y1 + pad))
    crop = page.crop((cx0, cy0, cx1, cy1))
    box = [x0 - cx0, y0 - cy0, x1 - cx0, y1 - cy0]
    pt = [j["point"][0] - cx0, j["point"][1] - cy0]
    # The face and the chest say "this one"; the other characters' faces in the panel say "not them".
    pts, labels = [pt], [1]
    if j.get("chest"):
        c = [j["chest"][0] - cx0, j["chest"][1] - cy0]
        if 0 <= c[0] < crop.width and 0 <= c[1] < crop.height:
            pts.append(c)
            labels.append(1)
    for q in j.get("not") or []:
        q = [q[0] - cx0, q[1] - cy0]
        if 0 <= q[0] < crop.width and 0 <= q[1] < crop.height:
            pts.append(q)
            labels.append(0)
    inp = proc(images=crop, input_boxes=[[box]], input_points=[[pts]], input_labels=[[labels]], return_tensors="pt").to("cuda")
    with torch.no_grad():
        out = sam(**inp, multimask_output=True)
    masks = proc.post_process_masks(out.pred_masks.cpu(), inp["original_sizes"])[0].numpy().astype(bool)[0]
    scores = out.iou_scores[0].float().cpu().numpy()[0]
    area = (box[2] - box[0]) * (box[3] - box[1])
    for k in np.argsort(-scores):
        share = masks[k].sum() / max(1.0, area)
        if not (0.12 < share < 0.95 and scores[k] > 0.6):
            continue
        m = component_at(fill_holes(masks[k]), pt)
        ys, xs = np.nonzero(m)
        if len(ys) == 0:
            continue
        bw, bh = xs.max() - xs.min() + 1, ys.max() - ys.min() + 1
        fill = m.sum() / float(bw * bh)
        face_at = (pt[1] - ys.min()) / bh
        if bh / bw < 1.15 or not 0.22 < fill < 0.9 or not 0.0 <= face_at < 0.35:
            continue
        # Sized like a figure around that face: some 3.5 to 9.5 heads tall, a few heads wide.
        fh = float(j.get("face_h") or 0)
        if fh > 0 and (not 3.5 <= bh / fh <= 9.5 or bw / fh > 4.8):
            continue
        # Nobody else: another character's face must not be inside the cut.
        if any(0 <= int(q[1] - cy0) < m.shape[0] and 0 <= int(q[0] - cx0) < m.shape[1] and m[int(q[1] - cy0), int(q[0] - cx0)] for q in (j.get("not") or [])):
            continue
        # No lettering: speech bubbles, captions and sound effects are cut away; a figure that
        # loses too much with them was mostly lettering.
        letters = lettering(crop, [pt])
        if letters is not None and (m & letters).sum() > 0.015 * m.sum():
            before = m.sum()
            m = component_at(m & ~letters, pt)
            if m.sum() < 0.8 * before:
                continue
        # Drawn, not a flat blob: a character is dense with line art; a block of colour (an aura,
        # a flat backdrop) is not.
        if not drawn(np.asarray(crop), m):
            continue
        alpha = Image.fromarray(m.astype(np.uint8) * 255).filter(ImageFilter.MinFilter(3)).filter(ImageFilter.GaussianBlur(1.0))
        rgba = crop.convert("RGBA")
        rgba.putalpha(alpha)
        bb = alpha.getbbox()
        return rgba.crop(bb) if bb else None
    return None


def main():
    jobs = json.load(open(sys.argv[1], encoding="utf-8"))
    import torch
    from transformers import Sam2Model, Sam2Processor
    path = os.path.join(pack_dir(), "sam2.1-hiera-small")
    proc = Sam2Processor.from_pretrained(path)
    sam = Sam2Model.from_pretrained(path).to("cuda").eval()
    done = []
    for j in jobs:
        for c in j.get("candidates") or [j]:
            cut = matte(proc, sam, c)
            if cut is not None:
                cut.save(j["out"])
                done.append(j["out"])
                break
    print(json.dumps({"ok": True, "done": done}))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # noqa: BLE001 - reported to the app
        traceback.print_exc(file=sys.stderr)
        print(json.dumps({"ok": False, "message": str(e)}))
