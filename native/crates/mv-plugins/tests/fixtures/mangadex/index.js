// MangaDex Source Plugin for MangaVox
// Uses the public MangaDex API v5 (api.mangadex.org)

var BASE = 'https://api.mangadex.org'

async function apiFetch(path, params) {
  var url = BASE + path
  if (params) {
    var qs = new URLSearchParams()
    for (var key in params) {
      var val = params[key]
      if (Array.isArray(val)) {
        for (var i = 0; i < val.length; i++) {
          qs.append(key + '[]', val[i])
        }
      } else if (val !== undefined && val !== null) {
        qs.append(key, String(val))
      }
    }
    url += '?' + qs.toString()
  }
  var resp = await __fetch(url)
  if (resp.status !== 200) {
    throw new Error('MangaDex API error: ' + resp.status + ' for ' + path)
  }
  return resp.data
}

function extractCoverUrl(manga) {
  var rels = manga.relationships || []
  for (var i = 0; i < rels.length; i++) {
    if (rels[i].type === 'cover_art' && rels[i].attributes) {
      var fileName = rels[i].attributes.fileName
      return 'https://uploads.mangadex.org/covers/' + manga.id + '/' + fileName + '.256.jpg'
    }
  }
  return null
}

function extractAuthor(manga) {
  var rels = manga.relationships || []
  for (var i = 0; i < rels.length; i++) {
    if (rels[i].type === 'author' && rels[i].attributes) {
      return rels[i].attributes.name
    }
  }
  return null
}

function extractArtist(manga) {
  var rels = manga.relationships || []
  for (var i = 0; i < rels.length; i++) {
    if (rels[i].type === 'artist' && rels[i].attributes) {
      return rels[i].attributes.name
    }
  }
  return null
}

function getTitle(attrs) {
  if (attrs.title) {
    return attrs.title.en || attrs.title['ja-ro'] || attrs.title.ja || Object.values(attrs.title)[0] || 'Unknown'
  }
  return 'Unknown'
}

function getAltTitles(attrs) {
  var alts = []
  if (attrs.altTitles) {
    for (var i = 0; i < attrs.altTitles.length; i++) {
      var obj = attrs.altTitles[i]
      var vals = Object.values(obj)
      if (vals.length > 0) alts.push(vals[0])
    }
  }
  return alts
}

function getDescription(attrs) {
  if (attrs.description) {
    return attrs.description.en || Object.values(attrs.description)[0] || ''
  }
  return ''
}

function getTags(attrs) {
  return (attrs.tags || []).map(function (t) {
    return t.attributes && t.attributes.name ? (t.attributes.name.en || Object.values(t.attributes.name)[0]) : ''
  }).filter(function (t) { return t })
}

function parseMangaList(items) {
  var results = []
  for (var i = 0; i < items.length; i++) {
    var manga = items[i]
    var attrs = manga.attributes
    results.push({
      source_id: manga.id,
      title: getTitle(attrs),
      alt_titles: getAltTitles(attrs),
      cover_url: extractCoverUrl(manga),
      author: extractAuthor(manga),
      status: attrs.status || null,
    })
  }
  return results
}

registerSource({
  searchManga: async function (query, page) {
    var offset = ((page || 1) - 1) * 20
    var data = await apiFetch('/manga', {
      title: query,
      limit: 20,
      offset: offset,
      includes: ['cover_art', 'author'],
      'order[relevance]': 'desc',
      'contentRating': ['safe', 'suggestive'],
    })
    return parseMangaList(data.data || [])
  },

  getPopularManga: async function (page) {
    var offset = ((page || 1) - 1) * 20
    var data = await apiFetch('/manga', {
      limit: 20,
      offset: offset,
      includes: ['cover_art', 'author'],
      'order[followedCount]': 'desc',
      'contentRating': ['safe', 'suggestive'],
      hasAvailableChapters: 'true',
    })
    return parseMangaList(data.data || [])
  },

  getLatestManga: async function (page) {
    var offset = ((page || 1) - 1) * 20
    var data = await apiFetch('/manga', {
      limit: 20,
      offset: offset,
      includes: ['cover_art', 'author'],
      'order[latestUploadedChapter]': 'desc',
      'contentRating': ['safe', 'suggestive'],
      hasAvailableChapters: 'true',
    })
    return parseMangaList(data.data || [])
  },

  getTagList: async function () {
    var data = await apiFetch('/manga/tag')
    var tags = []
    var items = data.data || []
    for (var i = 0; i < items.length; i++) {
      var t = items[i]
      var name = t.attributes && t.attributes.name ? (t.attributes.name.en || Object.values(t.attributes.name)[0]) : ''
      if (name && t.attributes.group === 'genre') {
        tags.push({ id: t.id, name: name })
      }
    }
    // Also add themes and formats
    for (var j = 0; j < items.length; j++) {
      var t2 = items[j]
      var name2 = t2.attributes && t2.attributes.name ? (t2.attributes.name.en || Object.values(t2.attributes.name)[0]) : ''
      if (name2 && (t2.attributes.group === 'theme' || t2.attributes.group === 'format')) {
        tags.push({ id: t2.id, name: name2 })
      }
    }
    tags.sort(function (a, b) { return a.name.localeCompare(b.name) })
    return tags
  },

  getMangaByTag: async function (tagId, page) {
    var offset = ((page || 1) - 1) * 20
    var data = await apiFetch('/manga', {
      limit: 20,
      offset: offset,
      includes: ['cover_art', 'author'],
      includedTags: [tagId],
      'order[followedCount]': 'desc',
      'contentRating': ['safe', 'suggestive'],
      hasAvailableChapters: 'true',
    })
    return parseMangaList(data.data || [])
  },

  getMangaDetails: async function (mangaId) {
    var data = await apiFetch('/manga/' + mangaId, {
      includes: ['cover_art', 'author', 'artist'],
    })

    var manga = data.data
    var attrs = manga.attributes

    return {
      source_id: manga.id,
      title: getTitle(attrs),
      author: extractAuthor(manga),
      artist: extractArtist(manga),
      description: getDescription(attrs),
      cover_url: extractCoverUrl(manga),
      status: attrs.status || null,
      genres: getTags(attrs),
    }
  },

  getChapterList: async function (mangaId) {
    var allChapters = []
    var offset = 0
    var limit = 500
    var hasMore = true

    while (hasMore) {
      var data = await apiFetch('/manga/' + mangaId + '/feed', {
        translatedLanguage: ['en'],
        'order[chapter]': 'asc',
        'order[volume]': 'asc',
        limit: limit,
        offset: offset,
        includes: ['scanlation_group'],
        'contentRating': ['safe', 'suggestive', 'erotica', 'pornographic'],
      })

      var items = data.data || []
      for (var i = 0; i < items.length; i++) {
        var ch = items[i]
        var attrs = ch.attributes

        // Skip external URL chapters (they can't be read in-app)
        if (attrs.externalUrl) continue

        var group = null
        var rels = ch.relationships || []
        for (var j = 0; j < rels.length; j++) {
          if (rels[j].type === 'scanlation_group' && rels[j].attributes) {
            group = rels[j].attributes.name
            break
          }
        }

        allChapters.push({
          source_id: ch.id,
          number: attrs.chapter ? parseFloat(attrs.chapter) : null,
          title: attrs.title || null,
          language: attrs.translatedLanguage || 'en',
          scanlation_group: group,
          published_at: attrs.publishAt || null,
          page_count: attrs.pages || null,
        })
      }

      offset += limit
      hasMore = items.length === limit && offset < (data.total || 0)
    }

    // Deduplicate by chapter number — keep first upload per number
    // but keep all chapters with null number (oneshots/specials)
    var seen = {}
    var deduped = []
    for (var k = 0; k < allChapters.length; k++) {
      var ch = allChapters[k]
      if (ch.number === null) {
        deduped.push(ch)
      } else {
        var key = String(ch.number)
        if (!seen[key]) {
          seen[key] = true
          deduped.push(ch)
        }
      }
    }

    return deduped
  },

  getPageUrls: async function (chapterId) {
    var data = await apiFetch('/at-home/server/' + chapterId)

    if (!data || !data.chapter) {
      // Check if chapter is external (hosted outside MangaDex)
      try {
        var chInfo = await apiFetch('/chapter/' + chapterId)
        if (chInfo && chInfo.data && chInfo.data.attributes && chInfo.data.attributes.externalUrl) {
          throw new Error('This chapter is hosted externally at: ' + chInfo.data.attributes.externalUrl + ' — it cannot be read in-app')
        }
      } catch (e) {
        if (e.message && e.message.indexOf('externally') !== -1) throw e
      }
      throw new Error('MangaDex returned no page data for this chapter')
    }

    var baseUrl = data.baseUrl
    var hash = data.chapter.hash
    var pages = data.chapter.data
    if (!pages || pages.length === 0) {
      pages = data.chapter.dataSaver || []
    }

    // If hash is empty or no pages, the chapter may be external
    if (!hash || pages.length === 0) {
      try {
        var chInfo2 = await apiFetch('/chapter/' + chapterId)
        if (chInfo2 && chInfo2.data && chInfo2.data.attributes && chInfo2.data.attributes.externalUrl) {
          throw new Error('This chapter is hosted externally at: ' + chInfo2.data.attributes.externalUrl + ' — it cannot be read in-app')
        }
      } catch (e) {
        if (e.message && e.message.indexOf('externally') !== -1) throw e
      }
      throw new Error('No pages available for this chapter on MangaDex')
    }

    if (!baseUrl) {
      throw new Error('MangaDex CDN is temporarily unavailable — try again later')
    }

    var urls = []
    var quality = (data.chapter.data && data.chapter.data.length > 0) ? '/data/' : '/data-saver/'
    for (var i = 0; i < pages.length; i++) {
      urls.push(baseUrl + quality + hash + '/' + pages[i])
    }
    return urls
  },
})
