import test from 'node:test'
import assert from 'node:assert/strict'
import { filterMatchingStreams, matchStreamToTitle, streamAudioLanguages } from '../src/shared/stream-match.ts'

const inception = { titles: ['Inception'], year: 2010, kind: 'movie' as const }
const breakingBad = { titles: ['Breaking Bad'], kind: 'series' as const, season: 1, episode: 2 }

test('real Torrentio release names match their movie', () => {
  for (const title of [
    'Inception 2010 PROPER Bluray 2160p AV1 HDR10 EN/ITA/FR/ES/DE OPUS 5.1-UH [UserHEVC]\n👤 293 💾 8.91 GB ⚙️ 1337x\n🇬🇧 / 🇮🇹 / 🇪🇸 / 🇫🇷 / 🇩🇪',
    'Начало / Inception [2010 UHD BDRemux 2160p HDR10] Dub + Original (Eng) + Sub (Rus Ukr Eng)\n👤 43 💾 77.38 GB ⚙️ Rutracker',
    'Inception.2010.2160p.MA.WEB-DL.DTS-HD.MA.5.1.DV.HDR.H.265-TheFarm\n👤 40 💾 29.35 GB ⚙️ ThePirateBay',
    'Inception (2010) (2160p BluRay x265 HEVC 10bit HDR AAC 5.1 Tigole) [QxR]\n👤 39 💾 17.23 GB ⚙️ 1337x',
  ]) assert.equal(matchStreamToTitle({ title }, inception), 'match', title)
})

test('a different movie, a remake year or a sequel is rejected', () => {
  assert.equal(matchStreamToTitle({ title: 'Interstellar.2014.1080p.BluRay.x264\n👤 900 ⚙️ YTS' }, inception), 'mismatch')
  assert.equal(matchStreamToTitle({ title: 'Dune 1984 1080p BluRay' }, { titles: ['Dune'], year: 2021, kind: 'movie' }), 'mismatch')
  assert.equal(matchStreamToTitle({ title: 'Iron.Man.2.2010.1080p.BluRay' }, { titles: ['Iron Man'], year: 2008, kind: 'movie' }), 'mismatch')
})

test('transliteration differences still match', () => {
  assert.equal(matchStreamToTitle(
    { title: 'Prathama.Drishtya.Kuttakkar.2024.1080p.WEB-DL.Malayalam.x264\n👤 22' },
    { titles: ['Pradhama Drishtiya Kuttakkar'], year: 2024, kind: 'movie' },
  ), 'match')
})

test('series: wrong episode or season is rejected, season packs use the file line', () => {
  assert.equal(matchStreamToTitle({ title: 'Breaking Bad S01E02 The Cats in the Bag 2160p NF WEB-DL\n👤 25' }, breakingBad), 'match')
  assert.equal(matchStreamToTitle({ title: 'Breaking Bad (2008) S01 (2160p AMZN WEB-DL)\nBreaking Bad (2008) S01E02 (2160p).mkv\n👤 72' }, breakingBad), 'match')
  assert.equal(matchStreamToTitle({ title: 'Breaking Bad S01E05 1080p\n👤 25' }, breakingBad), 'mismatch')
  assert.equal(matchStreamToTitle({ title: 'Better Call Saul S01E02 1080p WEB\n👤 25' }, breakingBad), 'mismatch')
})

test('generic labels are never thrown away', () => {
  assert.equal(matchStreamToTitle({ title: '' }, inception), 'unknown')
  assert.equal(matchStreamToTitle({ title: '1080p\n👤 5' }, inception), 'unknown')
  const result = filterMatchingStreams([{ title: 'Interstellar 2014 1080p' }], inception)
  assert.equal(result.allMismatched, true)
  assert.equal(result.streams.length, 1)
})

test('audio languages come from flags and release words, not subtitles', () => {
  assert.deepEqual(
    streamAudioLanguages({ title: 'Inception 2010 Bluray 2160p EN/ITA/FR/ES/DE OPUS\n👤 293\n🇬🇧 / 🇮🇹 / 🇪🇸 / 🇫🇷 / 🇩🇪' }).sort(),
    ['de', 'en', 'es', 'fr', 'it'],
  )
  assert.deepEqual(streamAudioLanguages({ title: 'Movie (2023) 1080p WEB-DL [Hindi + English] DD5.1 x264 ESubs' }).sort(), ['en', 'hi'])
  assert.deepEqual(streamAudioLanguages({ title: 'Movie 2023 1080p Dub Original (Eng) Sub (Rus Ukr)' }), ['en'])
  assert.deepEqual(streamAudioLanguages({ title: 'Ben-Hur 1959 1080p BluRay' }), [])
})
