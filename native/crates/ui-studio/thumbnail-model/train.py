"""Builds the thumbnail checklist model from dataset.json and writes model.json, which MangaVox
loads (Studio › Thumbnails scores ideas and images with it and shows its rules).

    py train.py

Performance of a video = perf = log10(views) minus its channel's fit of log views on log age, so a
video is compared only with what its own channel usually gets at that age.

What the data showed (and why this is a checklist, not a classifier): a logistic model on the
labelled thumbnail features, tested on held-out channels, ranked videos no better than chance.
Within a channel, which video wins is driven far more by the story, title and length than by the
thumbnail's look. So each feature gets a weight = its effect × how reliably it holds, from two views:
  - within channels: the mean log10 lift where channels have it both ways, × how consistent its
    sign is across those channels (held in N of M);
  - between channels: how much more big channels use it than small ones, × the correlation.
A feature whose two views disagree gets 0. The score is the weighted sum, shown as a percentile of
the studied thumbnails. Validation numbers (held-out channels vs shuffled labels) go in model.json.
Titles + length are scored on held-out channels too; they do carry signal.
"""
import json
import math
import os
import re
from datetime import date

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
data = json.load(open(os.path.join(HERE, "dataset.json"), encoding="utf-8"))
data = [v for v in data if v["age_days"] >= 5]

PIXEL = ["saturation", "contrast", "brightness", "colorfulness", "red_share", "warm_share", "cool_share",
         "bright_share", "dark_share", "edge_density", "center_focus"]


def concept(l):
    """The concept features of a labelled thumbnail; the app reads the same keys from a prompt."""
    t = l.get("text") or ""
    w = int(l.get("text_words") or 0)
    kind = l.get("text_kind") or "none"
    expr = l.get("expression") or "none"
    chars = int(l.get("chars") or 0)
    return {
        "face_closeup": l.get("face_size") == "large",
        "no_face": l.get("face_size") == "none",
        "expr_intense": expr in ("angry", "shocked", "scared"),
        "expr_smirk": expr == "smirk",
        "expr_happy": expr in ("happy", "blush"),
        "solo": chars == 1,
        "crowd": chars >= 4,
        "onlookers": bool(l.get("onlookers")),
        "women": bool(l.get("women")),
        "text": w > 0,
        "text_short": 1 <= w <= 3,
        "text_long": w >= 6,
        "text_yellow": l.get("text_color") == "yellow",
        "text_label": kind == "label",
        "text_exclaim": kind == "exclamation",
        "text_dialogue": kind == "dialogue",
        "text_stat": kind == "stat" or bool(re.search(r"\d", t)),
        "text_headline": kind == "headline",
        "text_question": "?" in t,
        "arrow_circle": bool(l.get("arrow")) or bool(l.get("circle")),
        "glow_aura": bool(l.get("glow_aura")),
        "glowing_eyes": bool(l.get("glowing_eyes")),
        "weapon": bool(l.get("weapon")),
        "monster": bool(l.get("monster")),
        "scale_contrast": bool(l.get("scale_contrast")),
        "split": int(l.get("panels") or 1) >= 2,
        "before_after": bool(l.get("before_after")),
        "system_ui": bool(l.get("system_ui")),
        "kneeling": bool(l.get("kneeling")),
        "blood": bool(l.get("blood")),
        "humor": bool(l.get("humor")),
        "romance": bool(l.get("romance")),
        "wealth": bool(l.get("wealth")),
        "dark_setting": l.get("setting") == "dark",
        "busy_background": bool(l.get("busy_background")),
        "off_center": l.get("subject_pos") in ("left", "right"),
        "dark_mood": l.get("mood") == "dark",
        "raw_panel": l.get("style") == "manhwa_panel",
    }


# Shown in the app and in the prompts. (do, avoid, why)
WORDS = {
    "face_closeup": ("Make the hero's face big: a close-up that fills a third of the frame or more", "Faces too small to read on a phone", "Viewers read emotion from faces; a big face survives the tiny mobile thumbnail."),
    "no_face": ("Thumbnails with no visible face", "Leaving out faces: always show at least one character's face", "A thumbnail without a face has no emotion to read."),
    "expr_intense": ("Give the focal face a strong emotion: shocked, furious or scared", "Strong shocked or angry faces on the focal character", "A loud emotion is the quickest hook in the feed."),
    "expr_smirk": ("A calm, cocky smirk on the overpowered hero", "The cocky smirk", "The smirk sells 'he is stronger than they think'."),
    "expr_happy": ("Laughing or blushing faces", "Laughing or blushing faces as the hook", "Warm faces read as slice-of-life, not as a power fantasy."),
    "solo": ("One character alone, with nothing to react to them", "A lone hero with no one reacting", "A reaction from someone else tells the viewer how impressive the hero is."),
    "crowd": ("A crowd: 4+ characters in the scene", "Crowded scenes of 4+ characters", "A crowd reads as status and spectacle, if the focal face stays clear."),
    "onlookers": ("Add onlookers reacting to the hero: classmates, rivals or girls staring in awe or shock", "Onlookers staring at the hero", "Their reaction is the viewer's cue that something huge happened."),
    "women": ("Put the story's heroines in the shot, reacting to the hero", "Prominent female characters", "Heroines and reactions from women are a recurring hook in this niche."),
    "text": ("Print a few words of text on the thumbnail", "Leaving the thumbnail without any text", "Text gives the twist a voice the image alone cannot."),
    "text_short": ("Keep the text to 1-3 big words", "Text of only 1-3 words", "Short text stays readable at phone size."),
    "text_long": ("Six or more words of text", "Six or more words of text: it turns to noise at phone size", "Long text cannot be read in the half second a thumbnail gets."),
    "text_yellow": ("Yellow text with a thick black outline", "Yellow outlined text", "Yellow on black is the niche's most readable, most recognisable caption style."),
    "text_label": ("A label naming something: 'MC', 'NEWBIE', 'S-RANK', with an arrow at it", "Labels like 'MC' with an arrow", "A label points the eye straight to the story's point."),
    "text_exclaim": ("A short exclamation: 'WHAT??', 'IMPOSSIBLE!'", "Exclamations like 'WHAT??'", "The exclamation voices the onlookers' shock."),
    "text_dialogue": ("A character's line as the text: a taunt or a reaction", "A character's spoken line as the text", "Dialogue teases the scene and the hero's attitude."),
    "text_stat": ("A number or rank in the text: 'LV 999', 'SSS', '1000 YEARS'", "Numbers or ranks in the text", "An absurd number is proof of power in one glance."),
    "text_headline": ("A headline that sums up the story: 'Betrayed / Revenge'", "Headline text that retells the title", "Headlines repeat the title instead of adding a new hook."),
    "text_question": ("A question in the text: 'Waifu?', 'Electricity?'", "Question-mark captions", "A question creates a gap the viewer wants closed."),
    "arrow_circle": ("Red/yellow arrows or circles pointing at the key detail", "Arrows and circles", "Arrows tell the eye where to look first."),
    "glow_aura": ("Visible power: aura, lightning, fire magic, a glowing weapon", "Aura and energy effects", "Glow is shorthand for power and pops against a dark frame."),
    "glowing_eyes": ("Glowing eyes on the hero", "Glowing eyes", "A glowing eye is the classic 'awakening' signal."),
    "weapon": ("Show the signature weapon big and clear", "Weapons as the focus", "The weapon is the hero's power made visible."),
    "monster": ("A monster, beast or dragon in the frame", "Monsters and beasts", "A monster gives the hero something to be bigger than."),
    "scale_contrast": ("A size gap: a small hero facing something gigantic", "Tiny-hero-versus-giant framing", "The size gap is the story in one image."),
    "split": ("A split frame or panels side by side", "Split frames and multi-panel layouts: one scene reads faster", "Each panel shrinks; at phone size a split frame becomes three tiny pictures."),
    "before_after": ("Before / after: weak or poor on one side, overpowered or rich on the other", "Before / after splits", "The transformation is the promise of the whole video."),
    "system_ui": ("A glowing system window or stat card", "System windows and stat cards", "A status window says 'game system' instantly to fans of the genre."),
    "kneeling": ("People kneeling or bowing to the hero", "Kneeling and bowing scenes", "Kneeling is status shown, not told."),
    "blood": ("Blood and injuries", "Blood and gore", "Blood signals stakes, but may limit how widely YouTube recommends it."),
    "humor": ("A comedic, meme-like framing when the story has a funny beat: a deadpan hero, a ridiculous reaction", "Comedic meme framing", "Humour stands out among a feed of serious power shots."),
    "romance": ("A romance beat when the story has one: a heroine blushing at the hero, a couple", "Romance as the main hook", "Romance is a different audience from the power-fantasy core of recaps."),
    "wealth": ("Money, luxury or abundance on show", "Money and luxury imagery", "Wealth is a power fantasy of its own."),
    "dark_setting": ("A dark setting: night, cave, void", "Dark settings", "A dark backdrop makes glow, faces and text pop."),
    "busy_background": ("A detailed, busy background", "Busy, cluttered backgrounds: keep it simple or blurred so the subject pops", "Clutter fights the subject for attention."),
    "off_center": ("The subject on a third (left or right), leaving room for text", "Off-centre subjects", "Rule of thirds leaves room for the text."),
    "dark_mood": ("An overall dark mood", "An overall dark mood", "Dark frames can sink into YouTube's dark mode."),
    "raw_panel": ("A raw comic panel as the thumbnail", "Using a raw manhwa panel as is: redraw it as one polished, composed scene", "Raw panels are small, flat and cramped next to designed thumbnails."),
}

LABELS = {k: v[0] for k, v in WORDS.items()}
LABELS.update({
    "saturation": "Saturated colour", "contrast": "Light/dark contrast", "brightness": "Overall brightness",
    "colorfulness": "Colourfulness", "red_share": "Share of red", "warm_share": "Warm colours (red, orange, yellow)",
    "cool_share": "Cool colours (blue, purple)", "bright_share": "Bright highlights", "dark_share": "Dark areas",
    "edge_density": "Clutter (edges)", "center_focus": "Detail in the centre",
})
SHORT = {
    "face_closeup": "Big face close-up", "no_face": "No face", "expr_intense": "Strong emotion", "expr_smirk": "Cocky smirk",
    "expr_happy": "Laughing / blushing", "solo": "Lone character", "crowd": "Crowd (4+)", "onlookers": "Onlookers reacting",
    "women": "Female characters", "text": "Has text", "text_short": "1-3 words of text", "text_long": "6+ words of text",
    "text_yellow": "Yellow outlined text", "text_label": "Label text ('MC')", "text_exclaim": "Exclamation text",
    "text_dialogue": "Dialogue text", "text_stat": "Number / rank in text", "text_headline": "Headline text",
    "text_question": "Question text", "arrow_circle": "Arrow / circle", "glow_aura": "Aura / energy", "glowing_eyes": "Glowing eyes",
    "weapon": "Weapon", "monster": "Monster", "scale_contrast": "Size gap", "split": "Split frame", "before_after": "Before / after",
    "system_ui": "System window", "kneeling": "Kneeling", "blood": "Blood", "humor": "Comedy", "romance": "Romance", "wealth": "Wealth",
    "dark_setting": "Dark setting", "busy_background": "Busy background", "off_center": "Subject on a third", "dark_mood": "Dark mood",
    "raw_panel": "Raw comic panel",
    "saturation": "Saturation", "contrast": "Contrast", "brightness": "Brightness", "colorfulness": "Colourfulness",
    "red_share": "Red", "warm_share": "Warm colours", "cool_share": "Cool colours", "bright_share": "Highlights",
    "dark_share": "Dark areas", "edge_density": "Clutter", "center_focus": "Centre detail",
}
TEXT_FEATURES = {"text", "text_short", "text_long", "text_yellow", "text_label", "text_exclaim", "text_dialogue", "text_stat", "text_headline", "text_question"}

channels = sorted({v["handle"] for v in data})
lab = [v for v in data if "label" in v]
CONCEPT = list(concept(lab[0]["label"]).keys())
Xc = np.array([[float(concept(v["label"])[k]) for k in CONCEPT] for v in lab])
perf = np.array([v["perf"] for v in lab])
hs = np.array([v["handle"] for v in lab])
Xp_all = np.array([[v["px"][k] for k in PIXEL] for v in data])
perf_all = np.array([v["perf"] for v in data])
hs_all = np.array([v["handle"] for v in data])
mu_px, sd_px = Xp_all.mean(0), Xp_all.std(0)
sd_px[sd_px < 1e-9] = 1
Zp_all = (Xp_all - mu_px) / sd_px
Zp_lab = np.array([[(v["px"][k] - mu_px[i]) / sd_px[i] for i, k in enumerate(PIXEL)] for v in lab])
# Each channel's usual daily views (median), the "how big is it" measure for the between-channel view.
med_vpd = {c: float(np.median([v["views_per_day"] for v in data if v["handle"] == c])) for c in channels}


def within(X, y, groups, binary):
    """Within-channel effect of each feature: the mean log10 difference (with − without, or per
    std for pixels) over the channels that have both, how many channels, and in how many it was
    positive."""
    out = []
    for f in range(X.shape[1]):
        ds = []
        for c in sorted(set(groups)):
            m = groups == c
            x, yy = X[m, f], y[m]
            if binary:
                on = x == 1
                if on.sum() >= 2 and (~on).sum() >= 2:
                    ds.append(yy[on].mean() - yy[~on].mean())
            elif m.sum() >= 8 and x.std() > 1e-9:
                ds.append(np.cov(x, yy)[0, 1] / x.var())
        ds = np.array(ds)
        out.append((float(ds.mean()) if len(ds) else 0.0, len(ds), int((ds > 0).sum())))
    return out


def between(X, groups):
    """Between-channel effect: across channels, log10 usual daily views against how often (or how
    strongly) the channel's thumbnails show the feature. Returns (slope, correlation)."""
    chs = sorted(set(groups))
    p = np.array([X[groups == c].mean(0) for c in chs])
    y = np.array([math.log10(med_vpd[c]) for c in chs])
    out = []
    for f in range(X.shape[1]):
        if p[:, f].std() < 1e-9:
            out.append((0.0, 0.0))
            continue
        out.append((float(np.cov(p[:, f], y)[0, 1] / p[:, f].var()), float(np.corrcoef(p[:, f], y)[0, 1])))
    return out


def weights(X, y, groups, binary):
    """Effect × how reliably it holds, from both views; 0 when the views disagree."""
    w_in = within(X, y, groups, binary)
    w_bt = between(X, groups)
    ws = []
    for (d, m, pos), (b, r) in zip(w_in, w_bt):
        rel_in = abs(2 * pos / m - 1) if m >= 5 else 0.0
        rel_bt = abs(r)
        a = d * rel_in
        # The between-channel slope spans a whole channel switching style: scale it to the same
        # per-thumbnail range and cap it, it is the noisier of the two.
        bb = float(np.clip(b * 0.5, -0.4, 0.4)) * rel_bt
        if a * bb < 0 and min(rel_in, rel_bt) > 0.2:
            ws.append(0.0)
        else:
            ws.append((a + bb) / 2)
    return np.array(ws), w_in, w_bt


w_c, ev_in, ev_bt = weights(Xc, perf, hs, True)
w_p, evp_in, evp_bt = weights(Zp_all, perf_all, hs_all, False)


def score_of(Xc_, Zp_, wc, wp):
    return Xc_ @ wc + Zp_ @ wp


# ---------------------------------------------------------------- validation (held-out channels vs shuffled)
rng = np.random.default_rng(0)


def rank_corr(a, b):
    a = np.argsort(np.argsort(a))
    b = np.argsort(np.argsort(b))
    return float(np.corrcoef(a, b)[0, 1])


def loco_within(y):
    """Held-out channels: does the checklist rank a channel's own videos?"""
    s = np.zeros(len(y))
    for c in channels:
        tr, te = hs != c, hs == c
        if te.sum() == 0:
            continue
        wc, _, _ = weights(Xc[tr], y[tr], hs[tr], True)
        trp = hs_all != c
        wp, _, _ = weights(Zp_all[trp], perf_all[trp], hs_all[trp], False)
        s[te] = score_of(Xc[te], Zp_lab[te], wc, wp)
    rs = [rank_corr(s[hs == c], y[hs == c]) for c in channels if (hs == c).sum() >= 5]
    return float(np.nanmean(rs))


def shuffled_within():
    yp = perf.copy()
    for c in channels:
        m = np.where(hs == c)[0]
        yp[m] = perf[rng.permutation(m)]
    return yp


real_in = loco_within(perf)
perm_in = np.array([loco_within(shuffled_within()) for _ in range(12)])
print(f"within channel, held out: rank corr {real_in:+.3f} vs shuffled {perm_in.mean():+.3f} ± {perm_in.std():.3f}")

# Between channels: score each held-out channel's thumbnails, compare the channel's mean score with
# its usual daily views.
ch_score = {}
for c in channels:
    tr = hs != c
    wc, _, _ = weights(Xc[tr], perf[tr], hs[tr], True)
    trp = hs_all != c
    wp, _, _ = weights(Zp_all[trp], perf_all[trp], hs_all[trp], False)
    m = hs == c
    if m.sum():
        ch_score[c] = float(score_of(Xc[m], Zp_lab[m], wc, wp).mean())
cs = sorted(ch_score)
real_bt = rank_corr([ch_score[c] for c in cs], [med_vpd[c] for c in cs])
print(f"between channels, held out: rank corr {real_bt:+.3f} over {len(cs)} channels")

# Titles + length on every video (the part with real held-out signal).


def minutes(l):
    if not l or not l.replace(":", "").isdigit():
        return 0
    s = 0
    for p in l.split(":"):
        s = s * 60 + int(p)
    return s / 60


TITLE = [
    ("full_completed", r"\b(full|completed?|entire)\b", "Say the recap is complete: 'FULL', 'Completed', 'The Entire Story'", "Saying the recap is complete", "Viewers want the whole story in one sitting."),
    ("first_person", r"^(i|my|i'm|i’m)\b", "Write the title in first person, as the hero: 'I Was Reborn…'", "First-person titles", "The first person puts the viewer in the hero's shoes."),
    ("ellipsis", r"\.\.\.|…", "Use an ellipsis for the cliffhanger: '…Then He Awakened'", "Ellipsis cliffhangers", "The ellipsis leaves the twist unfinished."),
    ("reborn", r"reincarnat|reborn|rebirth|regress|return(ed|s)? to the past|transmigrat", "Lead with the reincarnation / regression premise", "Reincarnation premise", "The genre's audience searches and clicks for the premise."),
    ("villain", r"villain", "Mention the villain angle when the story has one", "Villain angle", "Reincarnated-as-villain is a proven premise."),
    ("big_number", r"\b\d{3,}|\d+,\d{3}|\b\d+\s*(million|thousand|years)\b|\b(sss|lv\.?\s*\d+|level\s*\d+)\b", "Put an absurd number in: '1000 Years', 'Level 999', '$8M'", "Big numbers", "Numbers are concrete proof of power."),
    ("chapter_range", r"[\(\[]\s*\d+\s*[-–~]\s*\d+", "Chapter ranges like '(1-40)'", "Chapter ranges like '(1-40)': they say 'only part of the story'", "A range tells the viewer it is unfinished."),
    ("op_words", r"\b(op|strongest|overpowered|invincible|unbeatable|most powerful)\b", "OP / strongest wording", "Generic 'OP / strongest' wording: every recap says it; name the specific power instead", "The words are everywhere in the niche and no longer stand out."),
    ("weak_words", r"\b(weak|weakest|trash|useless|talentless|lowest|loser|worthless)\b", "Weak / trash wording", "Leaning on 'weakest / trash' wording alone", "Overused; the twist needs something specific."),
    ("starts_he", r"^(he|his|he's|he’s)\b", "Titles that start with 'He'", "Starting every title with 'He…'", "A big share of the niche starts with 'He'; it blends in."),
]
Xt = np.array([[float(bool(re.search(rx, v["title"].lower()))) for _, rx, *_ in TITLE] + [math.log10(minutes(v["length"]) + 1)] for v in data])


def demean(X, y, groups):
    X = X.copy()
    y = y.copy()
    for c in set(groups):
        m = groups == c
        X[m] -= X[m].mean(0)
        y[m] -= y[m].mean()
    return X, y


def ridge(X, y, lam):
    return np.linalg.solve(X.T @ X + lam * len(y) * np.eye(X.shape[1]), X.T @ y)


def loco_titles(y):
    s = np.zeros(len(y))
    for c in channels:
        tr, te = hs_all != c, hs_all == c
        Xd, yd = demean(Xt[tr], y[tr], hs_all[tr])
        sd = Xd.std(0)
        sd[sd < 1e-9] = 1
        s[te] = Xt[te] / sd @ ridge(Xd / sd, yd, 1.0)
    return float(np.nanmean([rank_corr(s[hs_all == c], y[hs_all == c]) for c in channels if (hs_all == c).sum() >= 5]))


real_t = loco_titles(perf_all)
perm_t = []
for _ in range(8):
    yp = perf_all.copy()
    for c in channels:
        m = np.where(hs_all == c)[0]
        yp[m] = perf_all[rng.permutation(m)]
    perm_t.append(loco_titles(yp))
perm_t = np.array(perm_t)
print(f"titles + length, held out: rank corr {real_t:+.3f} vs shuffled {perm_t.mean():+.3f} ± {perm_t.std():.3f}")

# ---------------------------------------------------------------- rules with evidence


def example(v):
    return {"id": v["id"], "title": v["title"], "channel": v["channel"], "views": v["views"], "x": round(10 ** v["perf"], 2), "text": v.get("label", {}).get("text", "")}


chs = sorted(set(hs))
order = sorted(chs, key=lambda c: -med_vpd[c])
big, small = order[: len(order) // 3], order[-(len(order) // 3):]
rules = []
for k_i, k in enumerate(CONCEPT):
    w = float(w_c[k_i])
    x = Xc[:, k_i] == 1
    n = int(x.sum())
    d, m, pos = ev_in[k_i]
    b, r = ev_bt[k_i]
    if n < 8 or abs(w) < 0.02:
        continue
    # Shown only when the within-channel lift points the same way as the weight.
    if m >= 3 and abs(d) > 0.03 and (w > 0) != (d > 0):
        continue
    kind = ("write" if k in TEXT_FEATURES else "do") if w > 0 else "avoid"
    with_ = sorted([v for v, h in zip(lab, x) if h], key=lambda v: -v["perf"] if kind != "avoid" else v["perf"])
    do, avoid, why = WORDS[k]
    rules.append({
        "feature": k, "kind": kind, "text": avoid if kind == "avoid" else do, "why": why,
        "lift": round(10 ** d, 2), "n": n, "held": pos if w > 0 else m - pos, "channels": m,
        "big_share": round(float(np.mean([Xc[hs == c, k_i].mean() for c in big])), 2),
        "small_share": round(float(np.mean([Xc[hs == c, k_i].mean() for c in small])), 2),
        "weight": round(w, 4), "examples": [example(v) for v in with_[:3]],
    })
rules.sort(key=lambda r: (["do", "write", "avoid"].index(r["kind"]), -abs(r["weight"])))

won = sorted([v for v in lab if v["label"].get("text")], key=lambda v: -v["perf"])
text_wins = [example(v) for v in won[:24]]
text_losses = [example(v) for v in won[::-1][:12]]

# Titles: lift among long videos (the user's format) and the length rule.
long_vids = [v for v in data if minutes(v["length"]) >= 60]
tp = np.array([v["perf"] for v in long_vids])
title_rules = []
for key, rx, do, avoid, why in TITLE:
    x = np.array([bool(re.search(rx, v["title"].lower())) for v in long_vids])
    n = int(x.sum())
    if n < 12:
        continue
    lift = 10 ** (tp[x].mean() - tp[~x].mean())
    if 1 / 1.08 < lift < 1.08:
        continue
    kind = "do" if lift > 1 else "avoid"
    ex = sorted([v for v, h in zip(long_vids, x) if h], key=lambda v: -v["perf"] if kind == "do" else v["perf"])[:3]
    title_rules.append({"feature": key, "kind": kind, "text": do if kind == "do" else avoid, "why": why, "lift": round(float(lift), 2), "n": n, "examples": [example(v) for v in ex]})
lens = [(minutes(v["length"]), v) for v in data if minutes(v["length"]) > 0]
short_ = np.array([v["perf"] for m_, v in lens if m_ < 60])
long_ = [v for m_, v in lens if m_ >= 120]
if len(short_) >= 12 and long_:
    lift = 10 ** (np.mean([v["perf"] for v in long_]) - short_.mean())
    title_rules.append({"feature": "length", "kind": "do", "text": "Make it long: a 2-hour-plus recap of many chapters", "why": "Recaps over 2 hours average far more views than ones under an hour on the same channels; the binge is the product.",
                        "lift": round(float(lift), 2), "n": len(long_), "examples": [example(v) for v in sorted(long_, key=lambda v: -v["perf"])[:3]]})
title_rules.sort(key=lambda r: (r["kind"] != "do", -abs(math.log(r["lift"]))))

# The score's scale: where a thumbnail's sum sits among the labelled ones (0-100).
s_all = score_of(Xc, Zp_lab, w_c, w_p)
percentiles = [round(float(q), 4) for q in np.quantile(s_all, np.linspace(0, 1, 101))]

best = sorted(data, key=lambda v: -v["perf"])
hits = [v for v in best if v["views"] >= 500_000][:8]
recent = [v for v in best if v["age_days"] <= 45][:8]


def feature_rows(keys, ws, mean, std):
    return [{"key": k, "label": SHORT[k], "weight": round(float(w), 4), "mean": round(float(m), 4), "std": round(float(s_), 4)} for k, w, m, s_ in zip(keys, ws, mean, std)]


model = {
    "version": 2,
    "trained": date.today().isoformat(),
    "data": {"channels": len(channels), "videos": len(data), "labelled": len(lab)},
    "target": "log10 views against what the video's own channel usually gets at that age",
    "eval": {
        "within": round(real_in, 3), "within_shuffled": round(float(perm_in.mean()), 3), "within_sd": round(float(perm_in.std()), 3),
        "between": round(real_bt, 3), "between_channels": len(cs),
        "titles": round(real_t, 3), "titles_shuffled": round(float(perm_t.mean()), 3), "titles_sd": round(float(perm_t.std()), 3),
    },
    "pixel": {"intercept": 0, "features": feature_rows(PIXEL, w_p, mu_px, sd_px)},
    "concept": {"intercept": 0, "features": feature_rows(CONCEPT, w_c, np.zeros(len(CONCEPT)), np.ones(len(CONCEPT)))},
    "percentiles": percentiles,
    "big_channels": [lab[int(np.where(hs == c)[0][0])]["channel"] for c in big],
    "rules": rules,
    "title_rules": title_rules,
    "text_wins": text_wins,
    "text_losses": text_losses,
    "hits": [example(v) for v in hits],
    "recent": [example(v) for v in recent],
}
with open(os.path.join(HERE, "model.json"), "w", encoding="utf-8") as f:
    json.dump(model, f, ensure_ascii=False, indent=1)
print(f"{len(rules)} thumbnail rules, {len(title_rules)} title rules -> model.json")
for r in rules:
    print(f"  {r['kind']:5s} w={r['weight']:+.3f} within {r['lift']:.2f}x held {r['held']}/{r['channels']}  big {r['big_share']:.2f} small {r['small_share']:.2f}  {r['feature']}")
for r in title_rules:
    print(f"  title {r['kind']:5s} {r['lift']:.2f}x n={r['n']} {r['feature']}")
print("pixels:", {k: round(float(w), 3) for k, w in zip(PIXEL, w_p)})
