import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { mediaEventWeight, normalizedMediaKey } from '../src/shared/media-intelligence.ts'
import { parseHistoryFile } from '../src/main/services/media-import.ts'

test('preference weights prioritize completion and likes while penalizing dislikes', () => {
  assert.equal(mediaEventWeight('complete'), 6)
  assert.equal(mediaEventWeight('like'), 6)
  assert.equal(mediaEventWeight('progress'), 3)
  assert.equal(mediaEventWeight('skip'), -2)
  assert.equal(mediaEventWeight('dislike'), -8)
})

test('normalized keys keep anime, movies, and series in independent namespaces', () => {
  assert.equal(normalizedMediaKey('anime', '1'), 'anime:mal:1')
  assert.equal(normalizedMediaKey('movie', 'tt1'), 'movie:imdb:tt1')
  assert.equal(normalizedMediaKey('series', 'tt1'), 'series:imdb:tt1')
})

test('Netflix-style CSV import parses quoted titles and dates', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mangavox-media-'))
  const path = join(dir, 'ViewingActivity.csv')
  writeFileSync(path, 'Title,Date\n"Example Show: Season 1: Episode 2",6/1/26\n"A Film, Part Two",5/2/26\n')
  try {
    const rows = parseHistoryFile(path, 'series')
    assert.equal(rows.length, 2)
    assert.equal(rows[0].sourceTitle, 'Example Show')
    assert.equal(rows[1].sourceTitle, 'A Film, Part Two')
    assert.equal(rows[0].status, 'unmatched')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
