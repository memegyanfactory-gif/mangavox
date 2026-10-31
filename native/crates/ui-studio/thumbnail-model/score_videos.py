"""Scores each video against its own channel: log10(views) minus the channel's fit of log views on
log age (`perf`; 10**perf = views ÷ expected). Downloads the thumbnails to img/ (not committed)
and writes rows.json.
"""
import json, math, os, urllib.request, io, random
import numpy as np
from PIL import Image, ImageDraw, ImageFont
from yt import UA, parse_age
raw = json.load(open("raw.json", encoding="utf-8"))
rows = []
for h, c in raw.items():
    seen = {}
    for src in ("latest", "popular"):
        for v in c[src]:
            if v["id"] in seen: seen[v["id"]]["src"].add(src); continue
            v = dict(v); v["age_days"] = parse_age(v["published"]); v["src"] = {src}; v["channel"] = c["name"]; v["handle"] = h; v["subs"] = c["subs"]; seen[v["id"]] = v
    vids = [v for v in seen.values() if v["age_days"] is not None and v["views"] > 0]
    # Per channel: log views against log age (videos older than 3 days), robustly.
    fit = [v for v in vids if v["age_days"] >= 3]
    if len(fit) < 8: continue
    x = np.array([math.log10(v["age_days"] + 1) for v in fit]); y = np.array([math.log10(v["views"]) for v in fit])
    b, a = np.polyfit(x, y, 1)
    b = min(max(b, 0.0), 1.0); a = float(np.median(y - b * x))
    med = float(np.median([v["views"] for v in vids]))
    for v in vids:
        exp = a + b * math.log10(v["age_days"] + 1)
        v["perf"] = round(math.log10(v["views"]) - exp, 4)          # log10 of views ÷ expected for its age on its channel
        v["x_expected"] = round(10 ** v["perf"], 3)
        v["x_channel_median"] = round(v["views"] / med, 3)
        v["views_per_day"] = round(v["views"] / max(v["age_days"], 1), 1)
        v["thumbnail"] = f"https://i.ytimg.com/vi/{v['id']}/hqdefault.jpg"
        v["src"] = sorted(v["src"])
        rows.append(v)
print(len(rows), "videos")
os.makedirs("img", exist_ok=True)
def fetch(v):
    p = f"img/{v['id']}.jpg"
    if not os.path.exists(p):
        req = urllib.request.Request(v["thumbnail"], headers={"User-Agent": UA})
        open(p, "wb").write(urllib.request.urlopen(req, timeout=30).read())
    return p
from concurrent.futures import ThreadPoolExecutor
with ThreadPoolExecutor(16) as ex:
    list(ex.map(fetch, rows))
json.dump(rows, open("rows.json", "w", encoding="utf-8"), ensure_ascii=False, indent=0)
