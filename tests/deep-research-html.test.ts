import assert from 'node:assert/strict'
import test from 'node:test'
import { buildDeepResearchHtml } from '../src/shared/deep-research-html.ts'

test('deep research HTML uses scraped assets and renders the complete source directory', () => {
  const html = buildDeepResearchHtml({
    heroTitle: 'A researched topic',
    heroImage: 'https://example.com/hero.jpg',
    category: 'RESEARCH',
    author: 'MangaVox Research',
    publishedDate: 'June 13, 2026',
    readTime: '12 min',
    authorBio: 'A research desk.',
    introParagraphs: ['An evidence-based introduction.'],
    sections: [{
      title: 'Evidence',
      image: 'https://example.com/evidence.jpg',
      imageCredit: 'example.com',
      paragraphs: ['A detailed, non-repeated finding.'],
    }],
    keyTakeaways: ['Use primary evidence.'],
    tags: ['Evidence'],
    trendingSidebar: [],
    recommendedCards: [],
    sources: [
      { title: 'Primary source', url: 'https://example.com/source-1', domain: 'example.com' },
      { title: 'Second source', url: 'https://example.org/source-2', domain: 'example.org' },
    ],
  })

  assert.match(html, /https:\/\/example\.com\/hero\.jpg/)
  assert.match(html, /Sources and further reading/)
  assert.match(html, /Primary source/)
  assert.match(html, /Second source/)
  assert.doesNotMatch(html, /loremflickr|pravatar/)
})

test('deep research HTML has a readable fallback without inventing an image', () => {
  const html = buildDeepResearchHtml({
    heroTitle: 'No image topic',
    heroImage: '',
    category: 'RESEARCH',
    author: 'MangaVox Research',
    publishedDate: 'June 13, 2026',
    readTime: '5 min',
    authorBio: 'A research desk.',
    introParagraphs: ['Introduction.'],
    sections: [],
    trendingSidebar: [],
    recommendedCards: [],
    sources: [],
    tags: [],
  })

  assert.match(html, /radial-gradient/)
  assert.doesNotMatch(html, /<img src="" alt="No image topic">/)
})
