# Thumbnail click model

The data and the scripts behind Studio › Thumbnails' **Thumbnail rules** tab and the click score
on every AI idea and creation. `model.json` is compiled into the app (`src/ytpack/click.rs`).

## Data

`dataset.json`: 2,288 public videos from 25 manhwa / manga recap channels (Mangoro Manhwa Recap,
MrManhwas, Webtoon Library, Manhwa Plague, Manhwa Fresh, ManhwaDealer, Tobs Manhwa, Mamoru Manhwa,
Frieren Manhwa and others). For each channel: its latest 120 uploads plus its 60 most popular.
Each row has the title, views, upload age, length, channel, thumbnail URL and:

- `perf`: log10(views) minus the channel's fit of log views on log age, so 10^perf
  (`x_expected`) is the video's views over what its own channel usually gets at that age;
- `x_channel_median`, `views_per_day`;
- `px`: pixel features of the thumbnail (colour, contrast, clutter, see `pixels.py`);
- `label` (363 videos): what the thumbnail shows. Per channel, the 7 best and 5 worst against
  expectation, 2 from the middle and 2 recent outperformers were labelled by hand with
  `LABELING.md`: face size, expression, characters, onlookers, text (exact words, colour, kind),
  arrows, aura, weapon, monster, split frame, before/after, system window, setting, and so on.

Only public data: no images are stored, no personal data.

## What the data says

`train.py` first fitted a logistic model on the labels. **Tested on channels it never saw, it did
no better than chance** at telling which of a channel's videos would win. Within one channel, the
thumbnails all look alike; the title, the premise and the length decide. So the shipped model is a
checklist. Each feature's weight is its effect × how reliably it held, from two views:

- within channels: the views lift where a channel used it both ways, × how many channels agreed;
- between channels: how much more the biggest third of the channels use it than the smallest third.

Held-out checks (in `model.json` → `eval`):

| Test | Rank correlation | Shuffled |
|---|---|---|
| Checklist score vs a channel's own videos | +0.01 | −0.04 ± 0.06 |
| Checklist score vs channel size, 25 channels | **+0.35** | |
| Titles + length vs a channel's own videos | **+0.20** | +0.01 ± 0.03 |

Main findings:

- **Avoid the generic power pose.** Aura and lightning are on 75% of small channels' thumbnails
  and 41% of big ones', and those videos did worse in 15 of 22 channels (0.55x). The same goes for
  a weapon as the focus (15 of 20 channels, 0.51x), split frames, the lone hero, and dark voids.
- **Do what the big channels do:** heroines in the shot (82% vs 49%), romance beats, arrows at the
  key detail (better in 10 of 17 channels, 1.35x) and yellow outlined text.
- **Titles:** recaps of 2 hours and more get about 2x the views of shorter ones. The villain angle
  (1.6x), 'FULL' or 'Completed' (1.3x), first-person titles (1.3x), an ellipsis, the reincarnation
  premise and big numbers help. Chapter ranges like '(1-40)' (0.67x) and generic 'OP / strongest'
  wording hurt.

## Rebuild

```bash
py collect.py              # raw.json: channel lists (no API key)
py score_videos.py         # rows.json + img/ (thumbnails, not committed)
py pixels.py               # adds pixel features
py sheets.py               # picks videos to label (picked.json), draws sheets/; label them with LABELING.md into labels_*.jsonl
py build_dataset.py .      # dataset.json
py train.py                # model.json
```

Run the first five in a scratch folder: they write their outputs into the current folder.
