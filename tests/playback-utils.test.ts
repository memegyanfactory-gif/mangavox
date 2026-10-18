import test from 'node:test'
import assert from 'node:assert/strict'
import { canonicalDuration, clampPlaybackPosition, parseProbeOutput, selectPlaybackMode } from '../src/shared/playback-utils.ts'
import type { MediaProbe } from '../src/shared/types.ts'

function probe(videoCodec: string, audioCodec: string): MediaProbe {
  return {
    durationSecs: 5400,
    tracks: [
      { index: 0, streamIndex: 0, kind: 'video', codec: videoCodec },
      { index: 0, streamIndex: 1, kind: 'audio', codec: audioCodec },
    ],
  }
}

test('probed duration remains canonical when browser duration grows', () => {
  assert.equal(canonicalDuration(5400, 20), 5400)
  assert.equal(canonicalDuration(5400, 40), 5400)
  assert.equal(canonicalDuration(5400, Number.POSITIVE_INFINITY), 5400)
})

test('browser duration is a fallback only when probing is unavailable', () => {
  assert.equal(canonicalDuration(null, 3600), 3600)
  assert.equal(canonicalDuration(null, Number.POSITIVE_INFINITY), 0)
})

test('playback mode prefers direct, audio conversion, then full conversion', () => {
  assert.equal(selectPlaybackMode(probe('h264', 'aac')), 'direct')
  assert.equal(selectPlaybackMode(probe('h264', 'eac3')), 'audio-transcode')
  assert.equal(selectPlaybackMode(probe('hevc', 'aac')), 'full-transcode')
})

test('HEVC without a decoder is re-encoded even when audio also needs conversion', () => {
  assert.equal(selectPlaybackMode(probe('hevc', 'eac3')), 'full-transcode')
})

test('hardware HEVC decode repackages instead of re-encoding', () => {
  assert.equal(selectPlaybackMode(probe('hevc', 'aac'), 0, { hevc: true }), 'remux')
  assert.equal(selectPlaybackMode(probe('hevc', 'eac3'), 0, { hevc: true }), 'audio-transcode')
  assert.equal(selectPlaybackMode(probe('av1', 'aac')), 'remux')
})

test('persisted playback positions are clamped to canonical duration', () => {
  assert.equal(clampPlaybackPosition(-10, 100), 0)
  assert.equal(clampPlaybackPosition(150, 100), 100)
  assert.equal(clampPlaybackPosition(42, 100), 42)
})

const FFMPEG_LISTING = `
Input #0, matroska,webm, from 'C:\Users\aayus\AppData\Local\Temp/mvtest/test.mkv':
  Metadata:
  Duration: 00:04:00.02, start: 0.000000, bitrate: 6590 kb/s
  Stream #0:0: Video: h264 (Constrained Baseline), yuv420p(tv, progressive), 1280x720 [SAR 1:1 DAR 16:9], 24 fps, 24 tbr, 1k tbn
    Metadata:
  Stream #0:1(jpn): Audio: aac (LC), 44100 Hz, mono, fltp (default)
    Metadata:
  Stream #0:2(eng): Audio: aac (LC), 44100 Hz, mono, fltp
    Metadata:
      title           : English Dub
  Stream #0:3(eng): Subtitle: subrip (srt) (default)
    Metadata:
  Stream #0:4(spa): Subtitle: subrip (srt)
    Metadata:
Stream mapping:
  Stream #0:0 -> #0:0 (copy)
  Stream #0:1 -> #0:1 (copy)
  Stream #0:2 -> #0:2 (copy)
  Stream #0:3 -> #0:3 (copy)
  Stream #0:4 -> #0:4 (copy)
Output #0, null, to 'pipe:':
  Metadata:
    encoder         : Lavf63.3.100
  Stream #0:0: Video: h264 (Constrained Baseline), yuv420p(tv, progressive), 1280x720 [SAR 1:1 DAR 16:9], q=2-31, 24 fps, 24 tbr, 1k tbn
    Metadata:
  Stream #0:1(jpn): Audio: aac (LC), 44100 Hz, mono, fltp (default)
    Metadata:
  Stream #0:2(eng): Audio: aac (LC), 44100 Hz, mono, fltp
    Metadata:
      title           : English Dub
  Stream #0:3(eng): Subtitle: subrip (default)
    Metadata:
  Stream #0:4(spa): Subtitle: subrip
    Metadata:
Press [q] to stop, [?] for help
`

test('probe parsing reads input streams once, with titles from metadata lines', () => {
  const probe = parseProbeOutput(FFMPEG_LISTING)
  assert.equal(probe.durationSecs, 240.02)
  const audio = probe.tracks.filter((t) => t.kind === 'audio')
  const subs = probe.tracks.filter((t) => t.kind === 'subtitle')
  assert.equal(audio.length, 2)
  assert.equal(subs.length, 2)
  assert.deepEqual(audio.map((t) => [t.index, t.language, t.title]), [[0, 'jpn', undefined], [1, 'eng', 'English Dub']])
  assert.equal(subs[0].title, undefined)
  assert.equal(subs[0].default, true)
  assert.equal(probe.tracks.find((t) => t.kind === 'video')?.height, 720)
})
