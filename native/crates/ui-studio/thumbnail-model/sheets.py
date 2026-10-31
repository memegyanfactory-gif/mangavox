"""Picks thumbnails to label (per channel: the 7 best and 5 worst against expectation, 2 from the
middle, 2 recent outperformers) and draws them 9 to a sheet for labelling (see LABELING.md).
"""
import json, random, os
from PIL import Image, ImageDraw, ImageFont
from pixels import load
random.seed(7)
rows = json.load(open("rows.json", encoding="utf-8"))
by = {}
for v in rows: by.setdefault(v["handle"], []).append(v)
pick = []
for h, vs in by.items():
    old = sorted([v for v in vs if v["age_days"] >= 5], key=lambda v: -v["perf"])
    if len(old) < 6: continue
    sel = old[:7] + old[-5:]
    mid = [v for v in old[7:-5]]
    sel += random.sample(mid, min(2, len(mid)))
    recent = sorted([v for v in vs if 5 <= v["age_days"] <= 60 and v not in sel], key=lambda v: -v["perf"])[:2]
    sel += recent
    pick += sel
random.shuffle(pick)
for i, v in enumerate(pick): v["n"] = i + 1
os.makedirs("sheets", exist_ok=True)
font = ImageFont.truetype("arial.ttf", 22)
for s in range(0, len(pick), 9):
    sheet = Image.new("RGB", (1440, 900), "white"); d = ImageDraw.Draw(sheet)
    for k, v in enumerate(pick[s:s + 9]):
        im = Image.open(f"img/{v['id']}.jpg").convert("RGB"); w, h = im.size; bar = (h - w * 9 // 16) // 2
        im = im.crop((0, bar, w, h - bar)).resize((480, 270))
        x, y = (k % 3) * 480, (k // 3) * 300
        d.rectangle([x, y, x + 479, y + 29], fill="black"); d.text((x + 8, y + 3), f"#{v['n']}", fill="yellow", font=font)
        sheet.paste(im, (x, y + 30))
    sheet.save(f"sheets/sheet_{s // 9 + 1:02d}.png")
json.dump([{"n": v["n"], "id": v["id"]} for v in pick], open("picked.json", "w"), indent=0)
print(len(pick), "picked,", (len(pick) + 8) // 9, "sheets")
