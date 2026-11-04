"""
The free audio pack: music beds per mood and recorded sound effects per cue, all CC0 / public
domain (or a licence that allows monetised videos with no strings), fetched from their sources into
the user's data folder. Nothing here is bundled in the repo; `audio_pack.json` only lists where each
file comes from, under which licence, and how it is tagged.

* Music beds go to `<data>/music/library/beds/` and are listed in `<data>/music/library/manifest.json`
  (`{"music": [...]}`), which mv-score's studio library reads (`library.rs`, picked by the scene's
  mood).
* Sound effects go to `<data>/studio/sfx/<cue>_<id>.<ext>`, where sfx.py finds them by cue name
  (`sfx.user_sounds`), and are listed in `<data>/studio/sfx/manifest.json` (`{"sfx": [...]}`).

An entry is fetched from `url` (a direct file) or from `zip` + `member` (one file out of a zip, which
is downloaded once). Entries whose licence is not clearly free (see `usable`) are never fetched.

    python audio_pack.py [--data DIR] [--only music|sfx] [--dry-run]
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
PACK = os.path.join(HERE, "audio_pack.json")
UA = "MangaVox audio pack"
EXTS = (".wav", ".mp3", ".ogg", ".flac")


def data_dir():
    return os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")


def usable(entry):
    """CC0, public domain or the Pixabay licence; CC BY only with its credit line. Never NC, ND
    or SA (share-alike would bind the video itself), never an unknown licence."""
    lic = str(entry.get("license") or "").lower().replace("-", " ").replace("_", " ")
    if any(x in lic for x in ("cc0", "public domain", "pixabay")):
        return True
    words = lic.split()
    return lic.startswith("cc by") and not {"nc", "nd", "sa"} & set(words) and bool(str(entry.get("attribution") or "").strip())


def _get(url, dest):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    tmp = dest + ".part"
    with urllib.request.urlopen(req, timeout=120) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f)
    os.replace(tmp, dest)
    return dest


def _duration(path):
    try:
        out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path],
                             capture_output=True, text=True, timeout=60).stdout.strip()
        return float(out)
    except Exception:
        return 0.0


def fetch(entry, dest, zips):
    """Writes the entry's audio to `dest`. `zips` caches downloaded zips by URL."""
    if entry.get("zip"):
        url = entry["zip"]
        if url not in zips:
            zips[url] = _get(url, os.path.join(tempfile.mkdtemp(prefix="mv-pack-"), hashlib.md5(url.encode()).hexdigest() + ".zip"))
        with zipfile.ZipFile(zips[url]) as z, z.open(entry["member"]) as src, open(dest + ".part", "wb") as out:
            shutil.copyfileobj(src, out)
        os.replace(dest + ".part", dest)
    else:
        _get(entry["url"], dest)
    if _duration(dest) <= 0.05:
        os.remove(dest)
        raise ValueError("not audio")


def _ext(entry):
    src = entry.get("member") or entry.get("url") or ""
    ext = os.path.splitext(src.split("?")[0])[1].lower()
    return ext if ext in EXTS else ".mp3"


def _record(entry, file):
    keep = ("title", "artist", "license", "source", "attribution", "moods", "energy", "cue")
    return dict({k: entry[k] for k in keep if k in entry}, file=file)


def _write_manifest(path, key, records):
    """Merges `records` into the manifest at `path` (by file), keeping entries made elsewhere."""
    old = []
    try:
        with open(path, encoding="utf-8") as f:
            old = json.load(f).get(key) or []
    except (OSError, ValueError):
        pass
    files = {r["file"] for r in records}
    merged = [r for r in old if r.get("file") not in files] + records
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump({key: merged, "about": "Free audio fetched by MangaVox (python/audio_pack.py): each file's source and licence."}, f, indent=2)
    os.replace(tmp, path)


def build(data, only=None, dry_run=False):
    with open(PACK, encoding="utf-8") as f:
        pack = json.load(f)
    music_dir = os.path.join(data, "music", "library")
    sfx_dir = os.path.join(data, "studio", "sfx")
    zips, done, failed = {}, {"music": [], "sfx": []}, []
    for kind in ("music", "sfx"):
        if only and only != kind:
            continue
        for e in pack.get(kind, []):
            if not usable(e):
                print(f"[pack] skipped {e.get('id')}: licence {e.get('license')!r} is not clearly free", file=sys.stderr)
                continue
            if kind == "music":
                rel = f"beds/{e['id']}{_ext(e)}"
                dest = os.path.join(music_dir, *rel.split("/"))
            else:
                rel = f"{e['cue']}_{e['id']}{_ext(e)}"
                dest = os.path.join(sfx_dir, rel)
            if dry_run:
                print(f"[pack] would fetch {rel}")
                continue
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            if not os.path.isfile(dest):
                try:
                    fetch(e, dest, zips)
                    print(f"[pack] {rel}")
                except Exception as ex:
                    failed.append(e.get("id"))
                    print(f"[pack] could not fetch {e.get('id')}: {ex}", file=sys.stderr)
                    continue
            done[kind].append(_record(e, rel))
    for p in zips.values():
        shutil.rmtree(os.path.dirname(p), ignore_errors=True)
    if not dry_run:
        if done["music"]:
            _write_manifest(os.path.join(music_dir, "manifest.json"), "music", done["music"])
        if done["sfx"]:
            _write_manifest(os.path.join(sfx_dir, "manifest.json"), "sfx", done["sfx"])
    return done, failed


def main():
    ap = argparse.ArgumentParser(description=__doc__.strip().splitlines()[0])
    ap.add_argument("--data", default=data_dir())
    ap.add_argument("--only", choices=("music", "sfx"))
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    done, failed = build(a.data, a.only, a.dry_run)
    print(json.dumps({"music": len(done["music"]), "sfx": len(done["sfx"]), "failed": failed}))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
