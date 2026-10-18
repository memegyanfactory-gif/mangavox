/**
 * User memory and personality system for AI assistant
 * Persists to database for long-term learning
 */

import { existsSync, readFileSync } from 'fs'
import { getDb, registerSchema } from '../db/database'
import { AppPaths } from './app-paths'
import type { AgentUserMemory } from '../../shared/agent-types'

export type UserMemory = AgentUserMemory

function shouldRecordLearning(): boolean {
  try {
    if (!existsSync(AppPaths.config)) return true
    const config = JSON.parse(readFileSync(AppPaths.config, 'utf-8'))
    return config?.assistant?.privacyMode !== true
  } catch {
    return true
  }
}

function defaultMemory(userId: string): UserMemory {
  return {
    userId,
    createdAt: Date.now(),
    updatedAt: Date.now(),
    preferences: {
      downloadFormat: 'MP4',
      downloadResolution: '1080p',
      defaultSaveLocation: '',
      favoriteGenres: [],
      favoriteLanguages: [],
      preferredQuality: '1080p',
      favoriteCategory: 'mixed',
    },
    personality: {
      communicationStyle: 'friendly',
      interests: [],
      favoriteContentTypes: [],
      favoriteGenres: [],
    },
    history: {
      searches: [],
      downloads: [],
      plays: [],
    },
  }
}

function normalizeMemory(raw: Partial<UserMemory>, userId: string): UserMemory {
  const base = defaultMemory(userId)
  return {
    userId,
    createdAt: raw.createdAt || base.createdAt,
    updatedAt: raw.updatedAt || Date.now(),
    preferences: {
      ...base.preferences,
      ...(raw.preferences || {}),
      favoriteGenres: raw.preferences?.favoriteGenres || base.preferences.favoriteGenres,
      favoriteLanguages: raw.preferences?.favoriteLanguages || base.preferences.favoriteLanguages,
    },
    personality: {
      ...base.personality,
      ...(raw.personality || {}),
      interests: raw.personality?.interests || base.personality.interests,
      favoriteContentTypes: raw.personality?.favoriteContentTypes || base.personality.favoriteContentTypes,
      favoriteGenres: raw.personality?.favoriteGenres || base.personality.favoriteGenres,
    },
    history: {
      searches: raw.history?.searches || [],
      downloads: raw.history?.downloads || [],
      plays: raw.history?.plays || [],
    },
  }
}

registerSchema((db) => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS agent_memory (
      user_id TEXT PRIMARY KEY,
      data TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      updated_at INTEGER NOT NULL
    )
  `)
})

export function initializeMemory(userId: string): UserMemory {
  const db = getDb()

  try {
    const stmt = db.prepare('SELECT data FROM agent_memory WHERE user_id = ?')
    const row = stmt.get(userId) as { data?: string } | undefined
    if (row?.data) {
      return normalizeMemory(JSON.parse(row.data), userId)
    }
  } catch (error) {
    console.error('[Agent Memory] Get error:', error)
  }

  const memory = defaultMemory(userId)

  try {
    const stmt = db.prepare(`
      INSERT INTO agent_memory (user_id, data, created_at, updated_at)
      VALUES (?, ?, ?, ?)
    `)
    stmt.run(userId, JSON.stringify(memory), memory.createdAt, memory.updatedAt)
  } catch (error) {
    console.error('[Agent Memory] Insert error:', error)
  }

  return memory
}

function saveMemory(memory: UserMemory): void {
  const db = getDb()
  memory.updatedAt = Date.now()

  try {
    const stmt = db.prepare(`
      UPDATE agent_memory
      SET data = ?, updated_at = ?
      WHERE user_id = ?
    `)
    stmt.run(JSON.stringify(memory), memory.updatedAt, memory.userId)
  } catch (error) {
    console.error('[Agent Memory] Save error:', error)
  }
}

export function getMemory(userId: string): UserMemory {
  return initializeMemory(userId)
}

export function updatePreferences(userId: string, updates: Partial<UserMemory['preferences']>): void {
  const memory = getMemory(userId)
  memory.preferences = { ...memory.preferences, ...updates }
  saveMemory(memory)
}

export function updatePersonality(userId: string, updates: Partial<UserMemory['personality']>): void {
  const memory = getMemory(userId)
  memory.personality = { ...memory.personality, ...updates }
  saveMemory(memory)
}

function inferFavoriteCategory(memory: UserMemory): void {
  const counts = new Map<string, number>()
  for (const play of memory.history.plays) {
    counts.set(play.type, (counts.get(play.type) || 0) + 1)
  }
  const top = Array.from(counts.entries()).sort((a, b) => b[1] - a[1])[0]?.[0]
  if (top) memory.preferences.favoriteCategory = top
}

export function recordSearch(userId: string, query: string): void {
  if (!shouldRecordLearning() || !query.trim()) return
  const memory = getMemory(userId)
  memory.history.searches.push({ query: query.trim(), timestamp: Date.now() })
  if (memory.history.searches.length > 100) memory.history.searches.shift()
  saveMemory(memory)
}

export function recordDownload(userId: string, name: string, format: string): void {
  if (!shouldRecordLearning()) return
  const memory = getMemory(userId)
  memory.history.downloads.push({ name, format, timestamp: Date.now() })
  if (memory.history.downloads.length > 50) memory.history.downloads.shift()

  const formatCounts = new Map<string, number>()
  for (const dl of memory.history.downloads) {
    formatCounts.set(dl.format, (formatCounts.get(dl.format) || 0) + 1)
  }
  const mostUsedFormat = Array.from(formatCounts.entries()).sort((a, b) => b[1] - a[1])[0]?.[0]
  if (mostUsedFormat) memory.preferences.downloadFormat = mostUsedFormat

  saveMemory(memory)
}

export function recordPlay(userId: string, title: string, type: string): void {
  if (!shouldRecordLearning()) return
  const memory = getMemory(userId)
  memory.history.plays.push({ title, type, timestamp: Date.now() })
  if (memory.history.plays.length > 50) memory.history.plays.shift()

  if (!memory.personality.favoriteContentTypes.includes(type)) {
    memory.personality.favoriteContentTypes.push(type)
  }
  inferFavoriteCategory(memory)
  saveMemory(memory)
}

export function clearMemory(userId: string): void {
  const db = getDb()
  try {
    const stmt = db.prepare('DELETE FROM agent_memory WHERE user_id = ?')
    stmt.run(userId)
  } catch (error) {
    console.error('[Agent Memory] Clear error:', error)
  }
}

export function getPersonalityDescription(userId: string): string {
  const memory = getMemory(userId)
  const style = memory.personality.communicationStyle
  const types = memory.personality.favoriteContentTypes.slice(0, 3).join(', ') || 'various content'
  const searches = memory.history.searches.length
  return `User prefers ${style} conversations, enjoys ${types}, and has made ${searches} searches. Last used ${memory.preferences.downloadFormat} format for downloads.`
}

export function getAgentMemoryContext(userId: string): string {
  if (!shouldRecordLearning()) {
    return 'ASSISTANT MEMORY: Privacy mode is on — searches, plays, and preferences are not stored.'
  }
  const memory = getMemory(userId)
  const recentSearches = memory.history.searches.slice(-8).map((s) => s.query).join(', ') || 'none'
  const recentPlays = memory.history.plays.slice(-8).map((p) => `${p.title} (${p.type})`).join(', ') || 'none'
  const recentDownloads = memory.history.downloads.slice(-6).map((d) => `${d.name} [${d.format}]`).join(', ') || 'none'
  const genres = memory.personality.favoriteGenres.join(', ') || memory.preferences.favoriteGenres.join(', ') || 'learning'
  const contentTypes = memory.personality.favoriteContentTypes.join(', ') || 'mixed'

  return [
    'ASSISTANT LEARNED MEMORY (from user behavior in MangaVox):',
    `Communication style: ${memory.personality.communicationStyle}`,
    `Favorite content types: ${contentTypes}`,
    `Favorite category: ${memory.preferences.favoriteCategory}`,
    `Preferred download format: ${memory.preferences.downloadFormat}`,
    `Preferred quality: ${memory.preferences.preferredQuality}`,
    `Genres/interests: ${genres}`,
    `Recent searches: ${recentSearches}`,
    `Recent plays: ${recentPlays}`,
    `Recent downloads: ${recentDownloads}`,
  ].join('\n')
}
