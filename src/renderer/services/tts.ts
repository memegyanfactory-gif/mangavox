// Web Speech API does not reliably match voices when given a short ISO code
// like "hi" — Chromium picks an English voice and you hear English with a
// Hindi accent. Map to BCP-47 and explicitly select a matching SpeechSynthesisVoice.

const BCP47: Record<string, string> = {
  en: 'en-US',
  hi: 'hi-IN',
  es: 'es-ES',
  fr: 'fr-FR',
  de: 'de-DE',
  ja: 'ja-JP',
  ko: 'ko-KR',
  pt: 'pt-BR',
  ru: 'ru-RU',
  id: 'id-ID',
  ar: 'ar-SA',
  'zh-CN': 'zh-CN',
}

export function toBcp47(code: string): string {
  return BCP47[code] ?? code
}

export function pickVoice(code: string): SpeechSynthesisVoice | null {
  const target = toBcp47(code).toLowerCase()
  const prefix = target.split('-')[0]
  const voices = window.speechSynthesis.getVoices()
  if (voices.length === 0) return null
  return (
    voices.find((v) => v.lang.toLowerCase() === target) ??
    voices.find((v) => v.lang.toLowerCase().startsWith(prefix + '-')) ??
    voices.find((v) => v.lang.toLowerCase().startsWith(prefix)) ??
    null
  )
}

// Chromium populates getVoices() asynchronously on first launch. Resolve once
// the list is ready so callers don't have to deal with an empty array.
export function whenVoicesReady(): Promise<SpeechSynthesisVoice[]> {
  return new Promise((resolve) => {
    const existing = window.speechSynthesis.getVoices()
    if (existing.length > 0) return resolve(existing)
    const handler = () => {
      window.speechSynthesis.removeEventListener('voiceschanged', handler)
      resolve(window.speechSynthesis.getVoices())
    }
    window.speechSynthesis.addEventListener('voiceschanged', handler)
  })
}
