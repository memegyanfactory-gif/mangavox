import { registerSchema, getDb } from '../db/database'

registerSchema((db) => {
  db.exec(`
    CREATE TABLE IF NOT EXISTS ai_token_usage (
      request_id TEXT PRIMARY KEY,
      conversation_id TEXT REFERENCES media_chat_conversation(id) ON DELETE CASCADE,
      tokens INTEGER NOT NULL,
      private INTEGER NOT NULL DEFAULT 0,
      mode TEXT NOT NULL DEFAULT 'chat',
      label TEXT NOT NULL DEFAULT '',
      status TEXT NOT NULL DEFAULT 'done',
      provider TEXT NOT NULL DEFAULT '',
      created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_ai_token_usage_created ON ai_token_usage(created_at DESC);
  `)
  // Migration: add provider column if upgrading from an older schema.
  try {
    db.exec(`ALTER TABLE ai_token_usage ADD COLUMN provider TEXT NOT NULL DEFAULT ''`)
  } catch { /* column already exists */ }
})

export interface TokenUsageStats {
  totalTokens: number
  privateTokens: number
  publicTokens: number
  requestCount: number
  daily: Array<{ day: string; tokens: number; privateTokens: number }>
  activities: Array<{ label: string; mode: string; tokens: number; createdAt: string; provider: string }>
  byProvider: Array<{ provider: string; tokens: number; count: number }>
}

export function recordTokenUsage(input: {
  requestId: string
  conversationId?: string | null
  tokens: number
  privateMode: boolean
  mode: string
  label: string
  status: string
  provider?: string
}): void {
  getDb().prepare(`
    INSERT INTO ai_token_usage
      (request_id, conversation_id, tokens, private, mode, label, status, provider)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(request_id) DO UPDATE SET
      tokens = excluded.tokens,
      status = excluded.status,
      provider = excluded.provider
  `).run(
    input.requestId,
    input.privateMode ? null : input.conversationId || null,
    Math.max(0, Math.round(input.tokens)),
    input.privateMode ? 1 : 0,
    input.mode,
    input.privateMode ? '' : input.label.slice(0, 140),
    input.status,
    input.provider || '',
  )
}

export function getTokenUsageStats(): TokenUsageStats {
  const totals = getDb().prepare(`
    SELECT
      COALESCE(SUM(tokens), 0) AS total_tokens,
      COALESCE(SUM(CASE WHEN private = 1 THEN tokens ELSE 0 END), 0) AS private_tokens,
      COUNT(*) AS request_count
    FROM ai_token_usage
  `).get() as any
  const daily = getDb().prepare(`
    SELECT
      date(created_at) AS day,
      SUM(tokens) AS tokens
    FROM ai_token_usage
    WHERE private = 0 AND created_at >= datetime('now', '-29 days')
    GROUP BY date(created_at)
    ORDER BY day ASC
  `).all() as any[]
  const activities = getDb().prepare(`
    SELECT label, mode, tokens, created_at, provider
    FROM ai_token_usage
    WHERE private = 0 AND label <> ''
    ORDER BY created_at DESC
    LIMIT 30
  `).all() as any[]
  const byProvider = getDb().prepare(`
    SELECT
      CASE WHEN provider = '' THEN 'unknown' ELSE provider END AS provider,
      COALESCE(SUM(tokens), 0) AS tokens,
      COUNT(*) AS count
    FROM ai_token_usage
    GROUP BY 1
    ORDER BY tokens DESC
  `).all() as any[]
  const totalTokens = Number(totals.total_tokens || 0)
  const privateTokens = Number(totals.private_tokens || 0)
  return {
    totalTokens,
    privateTokens,
    publicTokens: totalTokens - privateTokens,
    requestCount: Number(totals.request_count || 0),
    daily: daily.map((row) => ({
      day: row.day,
      tokens: Number(row.tokens || 0),
      privateTokens: 0,
    })),
    activities: activities.map((row) => ({
      label: row.label,
      mode: row.mode,
      tokens: Number(row.tokens || 0),
      createdAt: row.created_at,
      provider: row.provider || 'unknown',
    })),
    byProvider: byProvider.map((row) => ({
      provider: row.provider,
      tokens: Number(row.tokens || 0),
      count: Number(row.count || 0),
    })),
  }
}

export function clearTokenUsage(): void {
  getDb().prepare('DELETE FROM ai_token_usage').run()
}
