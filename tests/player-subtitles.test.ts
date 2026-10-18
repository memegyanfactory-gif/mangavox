import test from 'node:test'
import assert from 'node:assert/strict'
import { activeCuesAt, parseSubtitleText } from '../src/renderer/components/player/subtitles.ts'
import { audioFormat, resolveLanguage, sameLanguage } from '../src/renderer/components/player/languages.ts'

test('parses SRT with comma milliseconds, tags kept for styling, top-positioned cues', () => {
  const cues = parseSubtitleText('﻿1\r\n00:00:01,500 --> 00:00:03,000\r\n<i>Hello</i>\r\n\r\n2\r\n00:00:00,200 --> 00:00:01,000\r\n{\\an8}Sign text\r\n')
  assert.equal(cues.length, 2)
  assert.deepEqual(cues[0], { start: 0.2, end: 1, text: 'Sign text', top: true })
  assert.equal(cues[1].text, '<i>Hello</i>')
  assert.equal(cues[1].start, 1.5)
})

test('parses ASS dialogue with commas in text, line breaks and override tags', () => {
  const ass = [
    '[Script Info]', 'Title: x', '',
    '[Events]',
    'Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text',
    'Dialogue: 0,0:00:05.10,0:00:07.50,Default,,0,0,0,,{\\i1}Well,{\\i0} hello\\Nthere',
    'Dialogue: 0,0:00:08.00,0:00:09.00,Sign,,0,0,0,,{\\p1}m 0 0 l 10 10{\\p0}',
    'Dialogue: 0,0:00:10.00,0:00:11.00,Top,,0,0,0,,{\\an8}Up here',
  ].join('\n')
  const cues = parseSubtitleText(ass)
  assert.equal(cues.length, 2)
  assert.deepEqual(cues[0], { start: 5.1, end: 7.5, text: 'Well, hello\nthere', top: undefined })
  assert.equal(cues[1].top, true)
})

test('activeCuesAt finds overlapping cues', () => {
  const cues = [
    { start: 0, end: 10, text: 'long' },
    { start: 2, end: 3, text: 'a' },
    { start: 4, end: 5, text: 'b' },
  ]
  assert.deepEqual(activeCuesAt(cues, 4.5).map((c) => c.text), ['long', 'b'])
  assert.deepEqual(activeCuesAt(cues, 11), [])
  assert.deepEqual(activeCuesAt([], 1), [])
})

test('resolves language codes, regional variants and title hints to flags', () => {
  assert.deepEqual(resolveLanguage('jpn'), { code: 'ja', name: 'Japanese', country: 'jp' })
  assert.equal(resolveLanguage('pt-BR').country, 'br')
  assert.equal(resolveLanguage('pob').name, 'Portuguese (Brazil)')
  assert.equal(resolveLanguage('es-419').country, 'mx')
  assert.equal(resolveLanguage('und', 'English [Dub]').code, 'en')
  assert.equal(resolveLanguage('', 'Track 1').code, '')
  assert.ok(sameLanguage('eng', 'en'))
  assert.ok(!sameLanguage('jpn', 'eng'))
  assert.equal(audioFormat('eac3', 6), 'Dolby Digital+ · 5.1')
})
