"""Scrapes recap channels' public video lists (latest 120 + most popular 60) the way a browser
reads them: no API key. Writes raw.json in the current folder.

    py collect.py [@extraHandle ...]
"""
import sys, os
from yt import *
CH = ["@mangorotv","@MrManhwas01","@Webtoon_Library","@Manhwa_Fresh","@manhwadealer","@FrierenManhwa1","@AniTiger","@Tyler_Manhwa","@lobsmanhwa","@Magical_ManhwaRecaps","@hacsrecap","@TobsManhwa","@TheLibrarianRecap","@ManhwaSenpai99","@MamoruManhwa","@Definemanhwa","@GrimmyManhwas","@OPDigestHB","@SystemTales","@Shura_recaps","@Yogirirecap","@ManhwaMob","@manhwarecount01"] + sys.argv[1:]
def lockup(l):
    if l.get("contentType") != "LOCKUP_CONTENT_TYPE_VIDEO": return None
    meta = l.get("metadata", {}).get("lockupMetadataViewModel", {})
    parts = []
    for r in meta.get("metadata", {}).get("contentMetadataViewModel", {}).get("metadataRows", []):
        parts += r.get("metadataParts", [])
    views = None; pub = ""
    for p in parts:
        t = p.get("text", {}).get("content", ""); lab = p.get("accessibilityLabel", "")
        for s in (lab, t):
            if "view" in s and views is None: views = parse_views(s)
        if "ago" in t: pub = t
    badges = collect(l, "thumbnailBadgeViewModel", [])
    length = badges[0].get("text", "") if badges else ""
    if views is None: return None
    return {"id": l["contentId"], "title": meta.get("title", {}).get("content", ""), "views": views, "published": pub, "age_days": parse_age(pub), "length": length}
def page_items(d):
    vids = [v for v in (lockup(l) for l in collect(d, "lockupViewModel", [])) if v]
    toks = []
    for cir in collect(d, "continuationItemRenderer", []):
        t = cir.get("continuationEndpoint", {}).get("continuationCommand", {}).get("token")
        if t and cir.get("trigger") == "CONTINUATION_TRIGGER_ON_ITEM_SHOWN": toks.append(t)
    return vids, toks
def browse(key, ver, token):
    body = {"context": {"client": {"clientName": "WEB", "clientVersion": ver, "hl": "en", "gl": "US"}}, "continuation": token}
    return json.loads(get(f"https://www.youtube.com/youtubei/v1/browse?key={key}&prettyPrint=false", body))
out = json.load(open("raw.json", encoding="utf-8")) if os.path.exists("raw.json") else {}
for h in CH:
    if h in out: continue
    try:
        html = get(f"https://www.youtube.com/{h}/videos?hl=en&gl=US")
        d = initial(html); key, ver = ytcfg(html)
        hdr = collect(d, "pageHeaderViewModel", [])
        name = hdr[0]["title"]["dynamicTextViewModel"]["text"]["content"] if hdr else h
        subs = next((p for p in collect(hdr, "content", []) if isinstance(p, str) and "subscriber" in p), "")
        latest, toks = page_items(d)
        for _ in range(3):
            if not toks: break
            r = browse(key, ver, toks[0]); v, toks = page_items(r); latest += v
        chips = {}
        def walk(v):
            if isinstance(v, dict):
                lab = v.get("text") if isinstance(v.get("text"), str) else (v.get("title", {}).get("content") if isinstance(v.get("title"), dict) else None)
                if lab == "Popular" and "Popular" not in chips:
                    cc = collect(v, "continuationCommand", [])
                    if cc: chips["Popular"] = cc[0]["token"]
                for x in v.values(): walk(x)
            elif isinstance(v, list):
                for x in v: walk(x)
        walk(d)
        popular = []
        if "Popular" in chips:
            r = browse(key, ver, chips["Popular"]); popular, toks = page_items(r)
            if toks:
                r = browse(key, ver, toks[0]); v, _ = page_items(r); popular += v
        out[h] = {"name": name, "subs": subs, "latest": latest, "popular": popular}
        print(h, name, subs, len(latest), len(popular), flush=True)
    except Exception as e:
        print("FAIL", h, e, flush=True)
    json.dump(out, open("raw.json", "w", encoding="utf-8"), ensure_ascii=False)
