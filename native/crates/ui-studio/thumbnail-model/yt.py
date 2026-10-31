"""YouTube page helpers shared by the scripts (ytInitialData, innertube continuations)."""
import json, re, sys, urllib.request, urllib.parse, time
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36"
def get(url, data=None):
    h = {"User-Agent": UA, "Accept-Language": "en-US,en;q=0.9", "Cookie": "SOCS=CAI; CONSENT=YES+"}
    if data is not None:
        h["Content-Type"] = "application/json"
        data = json.dumps(data).encode()
    req = urllib.request.Request(url, data=data, headers=h)
    for i in range(3):
        try:
            return urllib.request.urlopen(req, timeout=30).read().decode("utf-8", "replace")
        except Exception as e:
            err = e; time.sleep(2)
    raise err
def initial(html):
    m = re.search(r"var ytInitialData = ", html) or re.search(r'window\["ytInitialData"\] = ', html)
    if not m: return None
    return json.JSONDecoder().raw_decode(html[m.end():])[0]
def ytcfg(html):
    k = re.search(r'"INNERTUBE_API_KEY":"([^"]+)"', html).group(1)
    v = re.search(r'"INNERTUBE_CLIENT_VERSION":"([^"]+)"', html).group(1)
    return k, v
def collect(v, key, out):
    if isinstance(v, dict):
        for k, x in v.items():
            if k == key: out.append(x)
            else: collect(x, key, out)
    elif isinstance(v, list):
        for x in v: collect(x, key, out)
    return out
def text(v):
    if not v: return ""
    if "simpleText" in v: return v["simpleText"]
    return "".join(r.get("text", "") for r in v.get("runs", []))
def parse_views(s):
    s = s.lower().replace(",", "")
    m = re.search(r"([\d.]+)\s*([kmb])?\s*(?:thousand|million|billion)?", s)
    if not m: return None
    n = float(m.group(1)); u = m.group(2)
    if "thousand" in s: u = "k"
    if "million" in s: u = "m"
    if "billion" in s: u = "b"
    return int(n * {"k":1e3,"m":1e6,"b":1e9}.get(u,1))
def parse_age(s):
    """Days since upload from "3 weeks ago" or the compact "3w ago", "8mo ago", "7h ago"."""
    m = re.search(r"(\d+)\s*([a-z]+)", s.lower())
    if not m: return None
    n = int(m.group(1)); u = m.group(2)
    if u.startswith("mo"): return n * 30
    if u.startswith(("s", "mi", "h")) or u == "m": return 0.5
    if u.startswith("d"): return n
    if u.startswith("w"): return n * 7
    if u.startswith("y"): return n * 365
    return None
