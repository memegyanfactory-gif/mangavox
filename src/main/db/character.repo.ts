import { getDb } from './database'
import { v4 as uuid } from 'uuid'

export interface Character {
  id: string
  manga_id: string
  name: string
  description: string | null
  voice: string | null
  face_crop_path: string | null
  color: string | null
  created_at: string
}

export interface AddCharacterInput {
  manga_id: string
  name: string
  description?: string | null
  voice?: string | null
  face_crop_path?: string | null
  color?: string | null
}

export function listCharacters(mangaId: string): Character[] {
  return getDb()
    .prepare('SELECT * FROM character WHERE manga_id = ? ORDER BY created_at ASC')
    .all(mangaId) as Character[]
}

export function getCharacter(id: string): Character | null {
  const row = getDb().prepare('SELECT * FROM character WHERE id = ?').get(id) as Character | undefined
  return row ?? null
}

export function addCharacter(input: AddCharacterInput): Character {
  const id = uuid()
  getDb()
    .prepare(`
      INSERT INTO character (id, manga_id, name, description, voice, face_crop_path, color)
      VALUES (?, ?, ?, ?, ?, ?, ?)
    `)
    .run(
      id, input.manga_id, input.name,
      input.description ?? null,
      input.voice ?? null,
      input.face_crop_path ?? null,
      input.color ?? null,
    )
  return getCharacter(id)!
}

export interface UpdateCharacterInput {
  id: string
  name?: string
  description?: string | null
  voice?: string | null
  face_crop_path?: string | null
  color?: string | null
}

export function updateCharacter(input: UpdateCharacterInput): Character | null {
  const sets: string[] = []
  const values: any[] = []
  if (input.name !== undefined)           { sets.push('name = ?');            values.push(input.name) }
  if (input.description !== undefined)    { sets.push('description = ?');     values.push(input.description) }
  if (input.voice !== undefined)          { sets.push('voice = ?');           values.push(input.voice) }
  if (input.face_crop_path !== undefined) { sets.push('face_crop_path = ?');  values.push(input.face_crop_path) }
  if (input.color !== undefined)          { sets.push('color = ?');           values.push(input.color) }
  if (sets.length === 0) return getCharacter(input.id)
  values.push(input.id)
  getDb().prepare(`UPDATE character SET ${sets.join(', ')} WHERE id = ?`).run(...values)
  return getCharacter(input.id)
}

export function deleteCharacter(id: string): void {
  getDb().prepare('DELETE FROM character WHERE id = ?').run(id)
}
