import type { AiStepRequest, AiStepResult, AiStreamEvent } from '../../../shared/ai-types'

type EventBody<T> = T extends unknown ? Omit<T, 'requestId'> : never

export interface StepContext {
  req: AiStepRequest
  signal: AbortSignal
  apiKey?: string
  baseUrl: string
  emit: (event: EventBody<AiStreamEvent>) => void
}

export type StepFn = (ctx: StepContext) => Promise<AiStepResult>
