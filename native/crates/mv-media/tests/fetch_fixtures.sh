#!/usr/bin/env bash
# Re-record the response fixtures used by the offline tests.
# Jikan has a 3 req/s limit, so the script sleeps between calls. Run from anywhere:
#   bash native/crates/mv-media/tests/fetch_fixtures.sh
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)/fixtures"
C=(curl -sSfL -m 30 -A "MangaVox-mv-media-fixtures/0.1")
get() { "${C[@]}" -o "$DIR/$1" "$2"; echo "saved $1"; sleep 1; }

# Cinemeta / Stremio
get cinemeta_manifest.json "https://v3-cinemeta.strem.io/manifest.json"
get cinemeta_catalog_movie_top.json "https://v3-cinemeta.strem.io/catalog/movie/top.json"
get cinemeta_catalog_movie_top_skip100.json "https://v3-cinemeta.strem.io/catalog/movie/top/skip=100.json"
get cinemeta_catalog_series_top.json "https://v3-cinemeta.strem.io/catalog/series/top.json"
get cinemeta_catalog_movie_search.json "https://v3-cinemeta.strem.io/catalog/movie/top/search=inception.json"
get cinemeta_catalog_series_search.json "https://v3-cinemeta.strem.io/catalog/series/top/search=breaking%20bad.json"
get cinemeta_meta_movie.json "https://v3-cinemeta.strem.io/meta/movie/tt1375666.json"
get cinemeta_meta_series.json "https://v3-cinemeta.strem.io/meta/series/tt0903747.json"
get imdb_suggestion_inception.json "https://v3.sg.media-imdb.com/suggestion/i/inception.json"
get opensubtitles_manifest.json "https://opensubtitles-v3.strem.io/manifest.json"
get opensubtitles_subtitles_movie_inception.json "https://opensubtitles-v3.strem.io/subtitles/movie/tt1375666.json"

# AniSkip / arm
get aniskip_skip_times_16498_ep1.json "https://api.aniskip.com/v2/skip-times/16498/1?types=op&types=ed&types=recap&types=mixed-op&types=mixed-ed&episodeLength=1440"
get arm_ids_mal_16498.json "https://arm.haglund.dev/api/v2/ids?source=myanimelist&id=16498"

# Jikan (MyAnimeList). Not recorded yet: api.jikan.moe was unreachable from the machine that
# wrote these tests. Uncomment and run when it is reachable, then add parsing tests against them.
# get jikan_top_anime.json "https://api.jikan.moe/v4/top/anime?page=1&limit=25"
# get jikan_seasons_now.json "https://api.jikan.moe/v4/seasons/now?page=1&limit=25"
# get jikan_seasons_2013_spring.json "https://api.jikan.moe/v4/seasons/2013/spring?page=1&limit=25"
# get jikan_seasons_upcoming.json "https://api.jikan.moe/v4/seasons/upcoming?page=1&limit=25"
# get jikan_schedules_monday.json "https://api.jikan.moe/v4/schedules?filter=monday&page=1&limit=25"
# get jikan_search_aot.json "https://api.jikan.moe/v4/anime?q=attack%20on%20titan&page=1&limit=25&order_by=score&sort=desc"
# get jikan_genres_anime.json "https://api.jikan.moe/v4/genres/anime"
# get jikan_anime_full_16498.json "https://api.jikan.moe/v4/anime/16498/full"
# get jikan_anime_episodes_16498.json "https://api.jikan.moe/v4/anime/16498/episodes?page=1"
# get jikan_anime_relations_16498.json "https://api.jikan.moe/v4/anime/16498/relations"
