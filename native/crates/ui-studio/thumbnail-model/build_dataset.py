"""Merges the scraped videos (rows.json from collect.py + dataset step), the pixel features and the
thumbnail labels (labels_*.jsonl) into dataset.json, the training data kept in the repo.

    py build_dataset.py <workdir>

Only public data: video ids, titles, view counts, upload ages and channel names of public YouTube
videos, plus labels describing their thumbnails. No images are stored; `thumbnail` is the URL.
"""
import glob
import json
import os
import sys

work = sys.argv[1]
here = os.path.dirname(os.path.abspath(__file__))
rows = json.load(open(os.path.join(work, "rows.json"), encoding="utf-8"))
picked = {}
for f in sorted(glob.glob(os.path.join(work, "picked*.json"))):
    picked.update({p["n"]: p["id"] for p in json.load(open(f, encoding="utf-8"))})
labels = {}
for f in sorted(glob.glob(os.path.join(work, "labels_*.jsonl"))):
    for line in open(f, encoding="utf-8"):
        line = line.strip()
        if not line:
            continue
        o = json.loads(line)
        vid = picked.get(o.pop("n", None))
        if vid:
            labels[vid] = o

keep = ["id", "title", "channel", "handle", "subs", "views", "published", "age_days", "length", "src",
        "views_per_day", "x_channel_median", "x_expected", "perf", "thumbnail", "px"]
out = []
for v in rows:
    o = {k: v[k] for k in keep}
    if v["id"] in labels:
        o["label"] = labels[v["id"]]
    out.append(o)
out.sort(key=lambda o: (o["handle"].lower(), -o["perf"]))
with open(os.path.join(here, "dataset.json"), "w", encoding="utf-8") as f:
    f.write("[\n" + ",\n".join(json.dumps(o, ensure_ascii=False) for o in out) + "\n]\n")
print(f"{len(out)} videos, {sum('label' in o for o in out)} labelled -> dataset.json")
