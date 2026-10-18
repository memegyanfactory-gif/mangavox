import { BrowserWindow, ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import { AnimeSearchResult } from '../../shared/anime-addon-interface'
import { existsSync, readFileSync } from 'fs'
import { AppPaths } from '../services/app-paths'

function loadConfig(): any {
  try {
    return existsSync(AppPaths.config) ? JSON.parse(readFileSync(AppPaths.config, 'utf8')) : {}
  } catch {
    return {}
  }
}
import {
  installAddon, uninstallAddon, listAddons,
  addonGetCatalog, addonSearch, addonGetEpisodes, addonGetStreams, findBestStreams,
  searchAnimeAddons, getAddon, getAnimeAddonsCatalog, findAddonForId,
  setStreamsUpdatedListener,
} from '../addons/addon-loader'
import { fetchStremioMeta } from '../addons/stremio-adapter'
import { fetchJSON } from '../services/jikan-api'
// Anime catalog facade: AniList → Kitsu → Jikan (circuit breaker); ani.zip for episodes/ids.
import {
  getTopAnime, getAiringAnime, getUpcomingAnime, searchAnime,
  getAnimeByGenre, getGenres, getAnimeDetails, getAnimeEpisodes,
  getAnimeRelations, getImdbId, getSimilarAnime, resolveAnimeStreamIds,
} from '../services/anime-catalog'
import {
  getMoviesCatalog, getSeriesCatalog, searchMovies, getMovieDetails, getSeriesDetails,
} from '../services/cinemeta-api'

export function registerAddonHandlers(): void {
  // ── Built-in Anime Catalog (Jikan/MAL) ──

  ipcMain.handle(IPC.ANIME_TOP, async (_e, page?: number, type?: string) => {
    try {
      const results = await getTopAnime(page, type)
      if (results && results.length > 0) return results
      return await getAnimeAddonsCatalog('popular', page || 1)
    }
    catch (err: any) {
      console.error(`[anime:top] Failed to fetch top anime (page=${page ?? 1}, type=${type ?? 'all'}): ${err?.message}`)
      try {
        return await getAnimeAddonsCatalog('popular', page || 1)
      } catch {
        return []
      }
    }
  })

  ipcMain.handle(IPC.ANIME_AIRING, async (_e, page?: number) => {
    try {
      const results = await getAiringAnime(page)
      if (results && results.length > 0) return results
      return await getAnimeAddonsCatalog('airing', page || 1)
    }
    catch (err: any) {
      console.error(`[anime:airing] Failed to fetch currently airing (page=${page ?? 1}): ${err?.message}`)
      try {
        return await getAnimeAddonsCatalog('airing', page || 1)
      } catch {
        return []
      }
    }
  })

  ipcMain.handle(IPC.ANIME_UPCOMING, async (_e, page?: number) => {
    try {
      const results = await getUpcomingAnime(page)
      if (results && results.length > 0) return results
      return await getAnimeAddonsCatalog('upcoming', page || 1)
    }
    catch (err: any) {
      console.error(`[anime:upcoming] Failed to fetch upcoming anime (page=${page ?? 1}): ${err?.message}`)
      try {
        return await getAnimeAddonsCatalog('upcoming', page || 1)
      } catch {
        return []
      }
    }
  })

  ipcMain.handle(IPC.ANIME_SEARCH, async (_e, query: string, page?: number, type?: string) => {
    try {
      let results = await searchAnime(query, page, type)
      // On the first page, search installed anime addons and merge them
      if (page === 1 || !page) {
        try {
          const addonResults = await searchAnimeAddons(query)
          if (addonResults.length > 0) {
            results = [
              ...results,
              ...addonResults.filter(
                (ar) => !results.some((r) => r.id === ar.id || r.title.toLowerCase() === ar.title.toLowerCase())
              ),
            ]
          }
        } catch (addonErr) {
          console.warn('[anime:search] Addon search fallback failed:', addonErr)
        }
      }
      return results
    } catch (err: any) {
      console.error(`[anime:search] Search failed for "${query}" (page=${page ?? 1}, type=${type ?? 'all'}): ${err?.message}`)
      try {
        return await searchAnimeAddons(query)
      } catch {
        return []
      }
    }
  })

  ipcMain.handle(IPC.ANIME_BY_GENRE, async (_e, genreId: number, page?: number, type?: string) => {
    try {
      const results = await getAnimeByGenre(genreId, page, type)
      if (results && results.length > 0) return results
      return await getAnimeAddonsCatalog('popular', page || 1)
    }
    catch (err: any) {
      console.error(`[anime:genre] Failed to fetch genre=${genreId} (page=${page ?? 1}, type=${type ?? 'all'}): ${err?.message}`)
      try {
        return await getAnimeAddonsCatalog('popular', page || 1)
      } catch {
        return []
      }
    }
  })

  ipcMain.handle(IPC.ANIME_GENRES, async () => {
    try { return await getGenres() }
    catch (err: any) { console.error(`[anime:genres] Failed to fetch genre list: ${err?.message}`); return [] }
  })

  ipcMain.handle(IPC.ANIME_DETAILS, async (_e, malId: string) => {
    try { return await getAnimeDetails(malId) }
    catch (err: any) { console.error(`[anime:details] Failed to fetch details for MAL ID ${malId}: ${err?.message}`); throw err }
  })

  ipcMain.handle(IPC.ANIME_EPISODES, async (_e, malId: string, page?: number) => {
    try {
      if (typeof malId === 'string' && !/^\d+$/.test(malId) && !malId.startsWith('al-')) {
        const addon = findAddonForId(malId)
        if (addon) {
          const { episodes } = await fetchStremioMeta(addon.endpoint, malId, 'anime')
          return episodes
        }
      }
      return await getAnimeEpisodes(String(malId), page)
    }
    catch (err: any) { console.error(`[anime:episodes] Failed to fetch episodes for MAL ID ${malId} (page=${page ?? 1}): ${err?.message}`); return [] }
  })

  ipcMain.handle(IPC.ANIME_RELATIONS, async (_e, malId: string) => {
    try { return await getAnimeRelations(malId) }
    catch (err: any) { console.error(`[anime:relations] Failed to fetch relations for MAL ID ${malId}: ${err?.message}`); return [] }
  })

  ipcMain.handle(IPC.ANIME_GET_IMDB, async (_e, malId: string) => {
    try {
      return await getImdbId(malId)
    }
    catch (err: any) { console.error(`[anime:imdb] Failed to resolve IMDB ID for MAL ID ${malId}: ${err?.message}`); return null }
  })

  // ── Movies Catalog (Cinemeta — real movies) ──

  ipcMain.handle(IPC.MOVIE_CATALOG, async (_e, catalogId?: 'top' | 'year' | 'popular' | 'imdbRating', page?: number, genre?: string) => {
    try { return await getMoviesCatalog(catalogId || 'top', page || 1, genre) }
    catch (err: any) { console.error(`[movie:catalog] Failed: ${err?.message}`); return [] }
  })

  ipcMain.handle(IPC.MOVIE_SEARCH, async (_e, query: string) => {
    try { return await searchMovies(query) }
    catch (err: any) { console.error(`[movie:search] Failed for "${query}": ${err?.message}`); return [] }
  })

  ipcMain.handle(IPC.MOVIE_DETAILS, async (_e, imdbId: string) => {
    try { return await getMovieDetails(imdbId) }
    catch (err: any) { console.error(`[movie:details] Failed for ${imdbId}: ${err?.message}`); return null }
  })

  // ── Stremio Addons (for torrent streams) ──

  ipcMain.handle(IPC.ADDON_LIST, () => {
    return listAddons()
  })

  ipcMain.handle(IPC.ADDON_INSTALL, async (_e, url: string) => {
    try {
      return await installAddon(url)
    } catch (err: any) {
      console.error(`[addon:install] Failed to install addon from "${url}": ${err?.message || err}`)
      throw err
    }
  })

  ipcMain.handle(IPC.ADDON_UNINSTALL, (_e, addonId: string) => {
    console.log(`[addon:uninstall] Removing addon "${addonId}"`)
    return uninstallAddon(addonId)
  })

  ipcMain.handle(IPC.ADDON_GET_CATALOG, async (_e, addonId: string, catalogId: string, page?: number) => {
    try {
      return await addonGetCatalog(addonId, catalogId, page)
    } catch (err: any) {
      console.error(`[addon:catalog] Failed to fetch catalog "${catalogId}" from addon "${addonId}": ${err?.message || err}`)
      throw err
    }
  })

  ipcMain.handle(IPC.ADDON_SEARCH, async (_e, addonId: string, query: string) => {
    try {
      return await addonSearch(addonId, query)
    } catch (err: any) {
      console.error(`[addon:search] Search failed for "${query}" on addon "${addonId}": ${err?.message || err}`)
      return []
    }
  })

  ipcMain.handle(IPC.ADDON_GET_EPISODES, async (_e, addonId: string, animeId: string) => {
    try {
      return await addonGetEpisodes(addonId, animeId)
    } catch (err: any) {
      console.error(`[addon:episodes] Failed to fetch episodes for anime=${animeId} from addon "${addonId}": ${err?.message || err}`)
      throw err
    }
  })

  ipcMain.handle(IPC.ADDON_GET_STREAMS, async (_e, addonId: string, animeId: string, episodeId: string, type?: string) => {
    try {
      return await addonGetStreams(addonId, animeId, episodeId, type || 'series')
    } catch (err: any) {
      console.error(`[addon:streams] Failed to fetch streams for episode="${episodeId}" type=${type ?? 'series'} from addon "${addonId}": ${err?.message || err}`)
      throw err
    }
  })

  setStreamsUpdatedListener((resourceId, type, streams) => {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send(IPC.ADDON_STREAMS_UPDATED, { resourceId, type, streams })
    }
  })

  ipcMain.handle(IPC.ADDON_FIND_STREAMS, async (_e, resourceId: string, type?: string) => {
    try {
      // Anime ids (al-…, MAL:ep, kitsu fallback, tt resolved for an anime) → kitsu-first Torrentio queries.
      const animeIds = await resolveAnimeStreamIds(resourceId).catch(() => null)
      if (animeIds && animeIds.length > 0) {
        const lists = await Promise.all(animeIds.map((id) =>
          findBestStreams(id, id.startsWith('kitsu:') ? 'series' : (type || 'series')).catch(() => [])))
        const merged = lists.flat()
        return merged.filter((s, i) => merged.findIndex((o) => o.infoHash === s.infoHash && o.fileIdx === s.fileIdx) === i)
      }
      return await findBestStreams(resourceId, type || 'series')
    } catch (err: any) {
      console.error(`[addon:find-streams] Failed for "${resourceId}" type=${type ?? 'series'}: ${err?.message || err}`)
      return []
    }
  })

  ipcMain.handle(IPC.MEDIA_SIMILAR_ITEMS, async (_e, mediaId: string, mediaKind: 'anime' | 'manga' | 'movie' | 'series') => {
    return getSimilarItems(mediaId, mediaKind)
  })

  ipcMain.handle('anime:nsfw', async (_e, page?: number) => {
    try {
      const addonId = 'hanime-addon'
      const addon = getAddon(addonId)
      if (!addon) {
        console.warn(`[anime:nsfw] HAnime addon is not installed or loaded.`)
        return []
      }
      const catalog = addon.catalogs[0]
      if (!catalog) {
        console.warn(`[anime:nsfw] HAnime addon has no catalog.`)
        return []
      }
      return await addonGetCatalog(addonId, catalog.id, page || 1)
    } catch (err: any) {
      console.error(`[anime:nsfw] Failed to fetch HAnime catalog (page=${page ?? 1}):`, err?.message)
      return []
    }
  })
}

async function getSimilarItems(mediaId: string, mediaKind: 'anime' | 'manga' | 'movie' | 'series'): Promise<AnimeSearchResult[]> {
  try {
    if (mediaKind === 'anime') {
      // AniList recommendations → same-genre → top (see services/anime-catalog.ts)
      return await getSimilarAnime(mediaId, 6)
    }

    if (mediaKind === 'manga') {
      let items: any[] = []
      try {
        const data: any = await fetchJSON(`https://api.jikan.moe/v4/manga/${mediaId}/recommendations`)
        items = (data.data || []).map((item: any) => ({
          id: String(item.entry.mal_id),
          title: item.entry.title,
          coverUrl: item.entry.images?.jpg?.large_image_url || item.entry.images?.jpg?.image_url,
          animeType: 'Manga',
        }))
      } catch (e) {
        console.warn('[similar-items] Jikan manga recommendations failed:', e)
      }

      if (items.length === 0) {
        try {
          const detailData: any = await fetchJSON(`https://api.jikan.moe/v4/manga/${mediaId}`)
          const genres = detailData.data?.genres || []
          if (genres.length > 0) {
            const genreId = genres[0].mal_id
            const searchData: any = await fetchJSON(`https://api.jikan.moe/v4/manga?genres=${genreId}&order_by=score&sort=desc&limit=6`)
            items = (searchData.data || []).map((item: any) => ({
              id: String(item.mal_id),
              title: item.title_english || item.title || 'Unknown',
              coverUrl: item.images?.jpg?.large_image_url || item.images?.jpg?.image_url,
              animeType: 'Manga',
            }))
          }
        } catch (e) {
          console.warn('[similar-items] Jikan manga fallback search failed:', e)
        }
      }

      if (items.length === 0) {
        try {
          const topData: any = await fetchJSON('https://api.jikan.moe/v4/top/manga?limit=6')
          items = (topData.data || []).map((item: any) => ({
            id: String(item.mal_id),
            title: item.title_english || item.title || 'Unknown',
            coverUrl: item.images?.jpg?.large_image_url || item.images?.jpg?.image_url,
            animeType: 'Manga',
          }))
        } catch {}
      }

      return items.slice(0, 6)
    }

    if (mediaKind === 'movie' || mediaKind === 'series') {
      const imdbId = mediaId
      
      const apiKey = loadConfig()?.tmdb?.apiKey || process.env.TMDB_API_KEY
      if (apiKey && imdbId.startsWith('tt')) {
        const headers = { Authorization: `Bearer ${apiKey}`, accept: 'application/json' }
        const findRes = await fetch(`https://api.themoviedb.org/3/find/${imdbId}?external_source=imdb_id`, { headers })
        if (findRes.ok) {
          const findData: any = await findRes.json()
          const movie = findData.movie_results?.[0]
          const tv = findData.tv_results?.[0]
          const tmdbId = movie?.id || tv?.id
          const endpoint = movie ? 'movie' : 'tv'
          if (tmdbId) {
            const recRes = await fetch(`https://api.themoviedb.org/3/${endpoint}/${tmdbId}/recommendations`, { headers })
            if (recRes.ok) {
              const recData: any = await recRes.json()
              return (recData.results || []).slice(0, 6).map((item: any) => ({
                id: `tmdb:${item.id}`,
                imdbId: item.imdb_id,
                tmdbId: item.id,
                title: item.title || item.name,
                coverUrl: item.poster_path ? `https://image.tmdb.org/t/p/w500${item.poster_path}` : undefined,
                year: Number(String(item.release_date || item.first_air_date || '').slice(0, 4)) || undefined,
                animeType: movie ? 'Movie' : 'TV',
              }))
            }
          }
        }
      }

      // Cinemeta Genre filter fallback
      let genres: string[] = []
      try {
        const details = mediaKind === 'movie' 
          ? await getMovieDetails(imdbId)
          : await getSeriesDetails(imdbId)
        if (details?.genres) {
          genres = details.genres
        }
      } catch {}

      const firstGenre = genres[0]
      if (mediaKind === 'movie') {
        const recs = await getMoviesCatalog('popular', 1, firstGenre)
        const filtered = recs.filter((m) => m.id !== imdbId)
        if (filtered.length > 0) return filtered.slice(0, 6)
        return recs.slice(0, 6)
      } else {
        const recs = await getSeriesCatalog('popular', 1, firstGenre)
        const filtered = recs.filter((m) => m.id !== imdbId)
        if (filtered.length > 0) return filtered.slice(0, 6)
        return recs.slice(0, 6)
      }
    }
  } catch (err: any) {
    console.error(`[similar-items] Failed to get similar items for ${mediaId} (${mediaKind}):`, err?.message)
  }
  return []
}
