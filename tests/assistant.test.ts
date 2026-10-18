import test from 'node:test'
import assert from 'node:assert/strict'
import { groupGgufFiles, parseQuant } from '../src/shared/gguf-files.ts'
import { buildTranscript } from '../src/main/ai/transcript.ts'
import { parseBlocks } from '../src/renderer/components/assistant-dock/markdown-blocks.ts'

test('quant names are read from common GGUF file names', () => {
  assert.deepEqual(parseQuant('Qwen3.5-4B-Q4_K_M.gguf'), { quant: 'Q4_K_M', bits: 4.8 })
  assert.deepEqual(parseQuant('gemma-4-12b-it-UD-Q4_K_XL.gguf'), { quant: 'Q4_K_XL', bits: 5 })
  assert.equal(parseQuant('model.IQ3_XXS.gguf').quant, 'IQ3_XXS')
  assert.equal(parseQuant('gpt-oss-20b-MXFP4.gguf').quant, 'MXFP4')
  assert.equal(parseQuant('Llama-3.2-3B-Instruct-f16.gguf').quant, 'F16')
  assert.deepEqual(parseQuant('README.md'), {})
})

test('split GGUF shards become one download option with summed size', () => {
  const files = groupGgufFiles([
    { path: 'Q8_0/big-Q8_0-00002-of-00002.gguf', size: 4 },
    { path: 'Q8_0/big-Q8_0-00001-of-00002.gguf', size: 6 },
    { path: 'big-Q4_K_M.gguf', size: 5 },
    { path: 'mmproj-F16.gguf', size: 1 },
    { path: 'config.json', size: 1 },
  ])
  assert.equal(files.length, 2)
  assert.equal(files[0].path, 'big-Q4_K_M.gguf')
  const q8 = files[1]
  assert.equal(q8.size, 10)
  assert.equal(q8.path, 'Q8_0/big-Q8_0-00001-of-00002.gguf')
  assert.deepEqual(q8.parts, ['Q8_0/big-Q8_0-00001-of-00002.gguf', 'Q8_0/big-Q8_0-00002-of-00002.gguf'])
  assert.equal(q8.quant, 'Q8_0')
})

test('CLI transcript keeps instructions, history and the latest message', () => {
  const out = buildTranscript([
    { role: 'system', content: 'Be brief.' },
    { role: 'user', content: 'Hi' },
    { role: 'assistant', content: 'Hello!' },
    { role: 'user', content: 'Recommend an anime' },
  ])
  assert.match(out, /<instructions>\nBe brief\.\n<\/instructions>/)
  assert.match(out, /User: Hi\n\nAssistant: Hello!/)
  assert.ok(out.endsWith('Reply to this latest message:\nRecommend an anime'))
})

test('CLI transcript drops the oldest turns when too long', () => {
  const long = 'x'.repeat(20_000)
  const out = buildTranscript([
    { role: 'user', content: `old ${long}` },
    { role: 'assistant', content: long },
    { role: 'user', content: 'newest question' },
  ])
  assert.ok(!out.includes('old x'))
  assert.ok(out.endsWith('newest question'))
})

test('markdown blocks: fences, tables, lists, headings and paragraphs', () => {
  const blocks = parseBlocks([
    '# Title',
    '',
    'Some **text**',
    'continues here',
    '',
    '- one',
    '- two',
    '  more of two',
    '',
    '1. first',
    '2. second',
    '',
    '| A | B |',
    '|---|:-:|',
    '| 1 | 2 |',
    '',
    '```ts',
    'const a = 1',
    '```',
    '> quoted',
    '---',
  ].join('\n'))
  assert.deepEqual(blocks.map((b) => b.kind), ['heading', 'para', 'list', 'list', 'table', 'code', 'quote', 'rule'])
  const ul = blocks[2] as { kind: 'list'; items: string[]; ordered: boolean }
  assert.deepEqual(ul.items, ['one', 'two\nmore of two'])
  assert.equal((blocks[3] as { ordered: boolean }).ordered, true)
  assert.deepEqual((blocks[4] as { rows: string[][] }).rows, [['1', '2']])
  assert.equal((blocks[5] as { lang: string; text: string }).text, 'const a = 1')
})

test('an unterminated code fence while streaming still renders as code', () => {
  const blocks = parseBlocks('Here:\n```python\nprint(1)')
  assert.equal(blocks[1].kind, 'code')
})
