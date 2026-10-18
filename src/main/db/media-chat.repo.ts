import { v4 as uuid } from 'uuid'
import type { MediaChatConversation, MediaChatMessage, MediaChatMode, MediaChatRole } from '../../shared/types'
import { getDb } from './database'

export function listConversations(): MediaChatConversation[] {
  return getDb().prepare(`
    SELECT id, title, icon, subtitle, model, online_enabled, chat_mode, persona_id, adult_enabled, created_at, updated_at
    FROM media_chat_conversation
    ORDER BY updated_at DESC
  `).all().map(rowToConversation)
}

export function createConversation(
  title: string,
  model: string,
  online: boolean,
  chatMode: MediaChatMode,
  personaId: string,
  adultEnabled: boolean,
): MediaChatConversation {
  const id = uuid()
  getDb().prepare(`
    INSERT INTO media_chat_conversation
      (id, title, model, online_enabled, chat_mode, persona_id, adult_enabled)
    VALUES (?, ?, ?, ?, ?, ?, ?)
  `).run(id, title, model, online ? 1 : 0, chatMode, personaId, adultEnabled ? 1 : 0)
  return getConversation(id)!
}

export function getConversation(id: string): MediaChatConversation | null {
  const row = getDb().prepare(`
    SELECT id, title, icon, subtitle, model, online_enabled, chat_mode, persona_id, adult_enabled, created_at, updated_at
    FROM media_chat_conversation WHERE id = ?
  `).get(id)
  return row ? rowToConversation(row) : null
}

export function listMessages(conversationId: string): MediaChatMessage[] {
  return getDb().prepare(`
    SELECT id, conversation_id, role, content, created_at, meta_json
    FROM media_chat_message
    WHERE conversation_id = ?
    ORDER BY created_at ASC, rowid ASC
  `).all(conversationId) as MediaChatMessage[]
}

export function addMessage(conversationId: string, role: MediaChatRole, content: string, meta?: unknown): MediaChatMessage {
  const id = uuid()
  let metaJson = ''
  if (meta) {
    try { metaJson = JSON.stringify(meta) } catch { metaJson = '' }
  }
  getDb().transaction(() => {
    getDb().prepare(`
      INSERT INTO media_chat_message (id, conversation_id, role, content, meta_json)
      VALUES (?, ?, ?, ?, ?)
    `).run(id, conversationId, role, content, metaJson)
    getDb().prepare(`
      UPDATE media_chat_conversation SET updated_at = datetime('now') WHERE id = ?
    `).run(conversationId)
  })()
  return getDb().prepare(`
    SELECT id, conversation_id, role, content, created_at, meta_json
    FROM media_chat_message WHERE id = ?
  `).get(id) as MediaChatMessage
}

export function updateMessage(id: string, content: string, meta?: unknown): MediaChatMessage | null {
  let metaJson = ''
  if (meta) {
    try { metaJson = JSON.stringify(meta) } catch { metaJson = '' }
  }
  getDb().transaction(() => {
    getDb().prepare(`
      UPDATE media_chat_message
      SET content = ?, meta_json = ?
      WHERE id = ?
    `).run(content, metaJson, id)
    getDb().prepare(`
      UPDATE media_chat_conversation
      SET updated_at = datetime('now')
      WHERE id = (SELECT conversation_id FROM media_chat_message WHERE id = ?)
    `).run(id)
  })()
  return getDb().prepare(`
    SELECT id, conversation_id, role, content, created_at, meta_json
    FROM media_chat_message WHERE id = ?
  `).get(id) as MediaChatMessage | null
}

export function updateConversationSettings(
  id: string,
  model: string,
  online: boolean,
  chatMode: MediaChatMode,
  personaId: string,
  adultEnabled: boolean,
): void {
  getDb().prepare(`
    UPDATE media_chat_conversation
    SET model = ?, online_enabled = ?, chat_mode = ?, persona_id = ?,
        adult_enabled = ?, updated_at = datetime('now')
    WHERE id = ?
  `).run(model, online ? 1 : 0, chatMode, personaId, adultEnabled ? 1 : 0, id)
}

export function updateConversationMeta(id: string, title: string, icon: string, subtitle: string): void {
  getDb().prepare(`
    UPDATE media_chat_conversation
    SET title = ?, icon = ?, subtitle = ?
    WHERE id = ?
  `).run(title, icon, subtitle, id)
}

export function deleteConversation(id: string): void {
  getDb().prepare('DELETE FROM media_chat_conversation WHERE id = ?').run(id)
}

export function clearConversations(): void {
  getDb().prepare('DELETE FROM media_chat_conversation').run()
}

function rowToConversation(row: any): MediaChatConversation {
  return {
    ...row,
    online_enabled: Boolean(row.online_enabled),
    adult_enabled: Boolean(row.adult_enabled),
  }
}
