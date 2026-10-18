import test from 'node:test'
import assert from 'node:assert/strict'
import { isAnimeAddon } from '../src/main/addons/addon-loader.ts'
import type { AnimeAddonManifest } from '../src/shared/anime-addon-interface.ts'

test('isAnimeAddon correctly filters anime-specific addons', () => {
  const kitsuAddon: AnimeAddonManifest = {
    id: 'community.anime.kitsu',
    name: 'Kitsu Anime',
    version: '1.0.0',
    description: 'Kitsu metadata addon',
    catalogs: [{ id: 'kitsu-anime', type: 'anime', name: 'Kitsu Anime' }],
    endpoint: 'https://anime-kitsu.strem.fun',
    resources: ['catalog', 'meta'],
    types: ['anime'],
    idPrefixes: ['kitsu:']
  }

  const torrentioAddon: AnimeAddonManifest = {
    id: 'com.stremio.torrentio.addon',
    name: 'Torrentio',
    version: '1.0.0',
    description: 'Torrent streams provider',
    catalogs: [],
    endpoint: 'https://torrentio.strem.fun',
    resources: ['stream'],
    types: ['movie', 'series'],
    idPrefixes: []
  }

  const netflixCatalogAddon: AnimeAddonManifest = {
    id: 'pw.ers.netflix-catalog',
    name: 'Netflix Catalog',
    version: '1.0.0',
    description: 'Netflix catalog provider',
    catalogs: [{ id: 'netflix-catalog', type: 'series', name: 'Netflix Series' }],
    endpoint: 'https://netflix.strem.fun',
    resources: ['catalog'],
    types: ['series'],
    idPrefixes: []
  }

  const hanimeAddon: AnimeAddonManifest = {
    id: 'hanime-addon',
    name: 'HAnime Addon',
    version: '1.0.0',
    description: 'HAnime streaming',
    catalogs: [{ id: 'hanime-popular', type: 'anime', name: 'HAnime Catalog' }],
    endpoint: 'https://hanime.strem.fun',
    resources: ['catalog'],
    types: ['anime'],
    idPrefixes: []
  }

  assert.ok(isAnimeAddon(kitsuAddon), 'Kitsu addon should be matched')
  assert.ok(isAnimeAddon(hanimeAddon), 'HAnime addon should be matched')
  assert.ok(!isAnimeAddon(torrentioAddon), 'Torrentio addon should NOT be matched as an anime catalog provider')
  assert.ok(!isAnimeAddon(netflixCatalogAddon), 'Netflix Catalog addon should NOT be matched as an anime catalog provider')
})
