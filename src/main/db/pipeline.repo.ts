import { getDb } from './database'
import { v4 as uuid } from 'uuid'
import type { PipelineJob } from '../../shared/types'

export interface CreateJobInput {
  manga_id: string
  chapter_id: string
  config_json: string
}

export function createJob(input: CreateJobInput): string {
  const id = uuid()
  getDb()
    .prepare(
      `INSERT INTO pipeline_job (id, manga_id, chapter_id, config_json) VALUES (?, ?, ?, ?)`
    )
    .run(id, input.manga_id, input.chapter_id, input.config_json)
  return id
}

export function getJob(id: string): PipelineJob | null {
  const row = getDb().prepare('SELECT * FROM pipeline_job WHERE id = ?').get(id) as any
  return row ? rowToJob(row) : null
}

export function listJobs(limit = 50): PipelineJob[] {
  const rows = getDb()
    .prepare(`
      SELECT pj.*, c.number as chapter_number, c.title as chapter_title
      FROM pipeline_job pj
      LEFT JOIN chapter c ON pj.chapter_id = c.id
      ORDER BY pj.created_at DESC LIMIT ?
    `)
    .all(limit) as any[]
  return rows.map(rowToJob)
}

export function listJobsByChapter(chapterId: string): PipelineJob[] {
  const rows = getDb()
    .prepare('SELECT * FROM pipeline_job WHERE chapter_id = ? ORDER BY created_at DESC')
    .all(chapterId) as any[]
  return rows.map(rowToJob)
}

export function updateJob(id: string, updates: Partial<{
  status: string
  current_stage: string | null
  progress: number
  result_json: string | null
  error_message: string | null
}>): void {
  const fields: string[] = []
  const values: any[] = []

  if (updates.status !== undefined) { fields.push('status = ?'); values.push(updates.status) }
  if (updates.current_stage !== undefined) { fields.push('current_stage = ?'); values.push(updates.current_stage) }
  if (updates.progress !== undefined) { fields.push('progress = ?'); values.push(updates.progress) }
  if (updates.result_json !== undefined) { fields.push('result_json = ?'); values.push(updates.result_json) }
  if (updates.error_message !== undefined) { fields.push('error_message = ?'); values.push(updates.error_message) }

  if (fields.length === 0) return

  fields.push("updated_at = datetime('now')")
  values.push(id)

  getDb()
    .prepare(`UPDATE pipeline_job SET ${fields.join(', ')} WHERE id = ?`)
    .run(...values)
}

export function deleteJob(id: string): void {
  getDb().prepare('DELETE FROM pipeline_job WHERE id = ?').run(id)
}

function rowToJob(row: any): PipelineJob {
  return {
    id: row.id,
    manga_id: row.manga_id,
    chapter_id: row.chapter_id,
    status: row.status,
    current_stage: row.current_stage,
    progress: row.progress,
    config_json: row.config_json,
    result_json: row.result_json,
    error_message: row.error_message,
    created_at: row.created_at,
    updated_at: row.updated_at,
    chapter_number: row.chapter_number ?? null,
    chapter_title: row.chapter_title ?? null,
  }
}
