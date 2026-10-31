"""Pixel features of a thumbnail on a 160x90 copy; ported 1:1 in ui-studio/src/ytpack/click.rs.
Run to add `px` to rows.json.
"""
import json, numpy as np
from PIL import Image
def load(path):
    im = Image.open(path).convert("RGB")
    w, h = im.size
    if abs(w / h - 4 / 3) < 0.02:  # hqdefault: 16:9 inside 4:3 with bars
        bar = (h - w * 9 // 16) // 2
        im = im.crop((0, bar, w, h - bar))
    return np.asarray(im.resize((160, 90), Image.BILINEAR), dtype=np.float64) / 255.0
def features(a):
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    mx = a.max(-1); mn = a.min(-1)
    lum = 0.299 * r + 0.587 * g + 0.114 * b
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1e-9), 0)
    d = mx - mn
    hue = np.zeros_like(mx)
    m = d > 1e-9
    hr = m & (mx == r); hg = m & (mx == g) & ~hr; hb = m & ~hr & ~hg
    hue[hr] = ((g - b)[hr] / d[hr]) % 6; hue[hg] = (b - r)[hg] / d[hg] + 2; hue[hb] = (r - g)[hb] / d[hb] + 4
    hue *= 60
    vivid = (sat > 0.35) & (mx > 0.35)
    rg = r - g; yb = 0.5 * (r + g) - b
    gx = np.abs(np.diff(lum, axis=1))[:-1, :]; gy = np.abs(np.diff(lum, axis=0))[:, :-1]
    edges = (gx + gy) > 0.12
    H, W = edges.shape
    center = edges[H // 4: 3 * H // 4, W // 4: 3 * W // 4].mean()
    return {
        "brightness": lum.mean(),
        "contrast": lum.std(),
        "saturation": sat.mean(),
        "colorfulness": np.sqrt(rg.std() ** 2 + yb.std() ** 2) + 0.3 * np.sqrt(rg.mean() ** 2 + yb.mean() ** 2),
        "red_share": (vivid & ((hue < 15) | (hue >= 340))).mean(),
        "warm_share": (vivid & ((hue < 60) | (hue >= 330))).mean(),
        "cool_share": (vivid & (hue >= 180) & (hue < 300)).mean(),
        "bright_share": (lum > 0.85).mean(),
        "dark_share": (lum < 0.15).mean(),
        "edge_density": edges.mean(),
        "center_focus": center / max(edges.mean(), 1e-6),
    }
if __name__ == "__main__":
    rows = json.load(open("rows.json", encoding="utf-8"))
    for v in rows:
        v["px"] = {k: round(float(x), 4) for k, x in features(load(f"img/{v['id']}.jpg")).items()}
    json.dump(rows, open("rows.json", "w", encoding="utf-8"), ensure_ascii=False, indent=0)
    import numpy as np
    perf = np.array([v["perf"] for v in rows])
    for k in rows[0]["px"]:
        x = np.array([v["px"][k] for v in rows])
        print(f"{k:14s} mean {x.mean():.3f}  corr with perf {np.corrcoef(x, perf)[0,1]:+.3f}")
