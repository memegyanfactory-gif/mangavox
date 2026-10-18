import { createCipheriv, createDecipheriv, randomBytes } from 'crypto'

/**
 * App-specific AES-256-GCM encryption for MangaVox text/JSON files.
 * Uses a fixed app key — prevents casual reading in text editors.
 * Files encrypted with this are saved with .mvx extension.
 *
 * Binary format: iv(12) | authTag(16) | ciphertext(N)
 */

const APP_KEY = Buffer.from('MangaVox-v1-key!1234567890ABCDEF', 'utf-8') // exactly 32 bytes

export function encryptText(text: string): Buffer {
  const iv = randomBytes(12)
  const cipher = createCipheriv('aes-256-gcm', APP_KEY, iv)
  const encrypted = Buffer.concat([cipher.update(text, 'utf-8'), cipher.final()])
  const tag = cipher.getAuthTag()
  return Buffer.concat([iv, tag, encrypted])
}

export function decryptText(data: Buffer): string {
  const iv = data.subarray(0, 12)
  const tag = data.subarray(12, 28)
  const ciphertext = data.subarray(28)
  const decipher = createDecipheriv('aes-256-gcm', APP_KEY, iv)
  decipher.setAuthTag(tag)
  return decipher.update(ciphertext).toString('utf-8') + decipher.final('utf-8')
}

/** Write text to an encrypted .mvx file */
export function writeEncrypted(filePath: string, text: string): void {
  const { writeFileSync } = require('fs')
  writeFileSync(filePath, encryptText(text))
}

/**
 * Read an encrypted .mvx file.
 * Returns null if file doesn't exist.
 */
export function readEncrypted(filePath: string): string | null {
  const { existsSync, readFileSync } = require('fs')
  if (!existsSync(filePath)) return null
  try {
    return decryptText(readFileSync(filePath))
  } catch {
    return null
  }
}
