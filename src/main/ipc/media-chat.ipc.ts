import { ipcMain } from 'electron'
import { IPC } from '../../shared/ipc-channels'
import type { MediaChatMessage, MediaChatMode } from '../../shared/types'
import type { ArtifactKind, ResearchBundle, ResearchDepth, ResearchProgressEvent, ResearchStep } from '../../shared/assistant-types'
import { detectAssistantSkill } from '../../shared/assistant-skills'
import * as chatRepo from '../db/media-chat.repo'
import {
  buildMediaContext,
  generateConversationMeta,
  generateDeepResearchArticle,
  interpretResearchRequest,
  prepareAdultChatModel,
  researchOnlineDeep,
  streamMediaChat,
  type MediaWatchContext,
} from '../services/media-chat'
import { clearTokenUsage, getTokenUsageStats, recordTokenUsage } from '../services/token-usage'
import { getActiveSkillContext } from '../services/custom-skills'
import {
  absorbConversationTurn,
  clearConversationMemory,
  clearPrivateSessionMemory,
  formatMemoryForPrompt,
  getConversationMemory,
  getPrivateSessionMemory,
} from '../services/conversation-memory'

interface SendRequest {
  requestId: string
  conversationId?: string | null
  sessionId?: string | null
  message: string
  researchQuery?: string
  model: string
  online: boolean
  privateMode: boolean
  sessionMessages?: Pick<MediaChatMessage, 'role' | 'content'>[]
  watchHistory?: MediaWatchContext[]
  chatMode?: MediaChatMode
  personaId?: string
  adultEnabled?: boolean
  artifact?: ArtifactKind
  artifactTitle?: string
  deepResearch?: boolean
  researchDepth?: ResearchDepth
}

/** Compact research bundle stored with the message — enough to re-render the
 * research panel when the chat is reopened, without the heavy crawl payload. */
function trimResearchForStorage(bundle: ResearchBundle): Partial<ResearchBundle> {
  return {
    query: bundle.query,
    interpretation: bundle.interpretation,
    depth: bundle.depth,
    queries: bundle.queries,
    skill: bundle.skill,
    phases: [],
    stats: bundle.stats,
    providers: bundle.providers,
    images: bundle.images,
    rounds: bundle.rounds,
    workspaceDir: bundle.workspaceDir,
    sources: bundle.sources.slice(0, 30).map((s) => ({
      title: s.title,
      url: s.url,
      summary: (s.summary || '').slice(0, 300),
      score: s.score,
      domain: s.domain,
      favicon: s.favicon,
    })),
  }
}

// In-flight generations, keyed by requestId, so the renderer can stop them.
const activeStreams = new Map<string, AbortController>()

export function registerMediaChatHandlers(): void {
  ipcMain.handle(IPC.MEDIA_CHAT_STOP, (_event, requestId: string) => {
    activeStreams.get(requestId)?.abort()
    return true
  })

  ipcMain.handle(IPC.MEDIA_CHAT_LIST, () => chatRepo.listConversations())
  ipcMain.handle(IPC.MEDIA_CHAT_GET, (_event, id: string) => ({
    conversation: chatRepo.getConversation(id),
    messages: chatRepo.listMessages(id),
  }))
  ipcMain.handle(IPC.MEDIA_CHAT_DELETE, (_event, id: string) => {
    clearConversationMemory(id)
    chatRepo.deleteConversation(id)
    return true
  })
  ipcMain.handle(IPC.MEDIA_CHAT_CLEAR, () => {
    chatRepo.clearConversations()
    clearTokenUsage()
    return true
  })
  ipcMain.handle(IPC.TOKEN_USAGE_STATS, () => getTokenUsageStats())
  ipcMain.handle(IPC.TOKEN_USAGE_CLEAR, () => {
    clearTokenUsage()
    return true
  })
  ipcMain.handle(IPC.MEDIA_CHAT_PREPARE_ADULT, () => prepareAdultChatModel())
  // Persist an agent action line (download started, watched-list update, skill
  // built, …) into the conversation so reopening the chat shows what was done.
  ipcMain.handle(IPC.MEDIA_CHAT_LOG, (_event, conversationId: string, content: string, meta?: unknown, role: 'user' | 'assistant' = 'assistant') => {
    if (!conversationId || !content?.trim()) return null
    if (!chatRepo.getConversation(conversationId)) return null
    return chatRepo.addMessage(conversationId, role === 'user' ? 'user' : 'assistant', content, meta)
  })
  ipcMain.handle(IPC.ASSISTANT_CLEAR_SESSION, (_event, sessionId: string) => {
    clearPrivateSessionMemory(sessionId)
    return true
  })

  ipcMain.handle(IPC.MEDIA_CHAT_CREATE, (_event, title: string, model: string, online: boolean, chatMode?: any, personaId?: string, adultEnabled?: boolean) => {
    return chatRepo.createConversation(
      title,
      model,
      online,
      chatMode || 'mangavox',
      personaId || 'nova',
      !!adultEnabled,
    )
  })

  ipcMain.handle(IPC.MEDIA_CHAT_RENAME, (_event, conversationId: string, title: string, icon?: string, subtitle?: string) => {
    chatRepo.updateConversationMeta(conversationId, title, icon || '', subtitle || '')
    return true
  })

  ipcMain.handle(IPC.MEDIA_CHAT_SEND, async (event, request: SendRequest) => {
    const message = request.message.trim()
    if (!message) throw new Error('Message cannot be empty')
    if (!request.model) throw new Error('Install and select an Ollama text model first')

    let provider = 'ollama'
    if (request.model.includes('/') || request.model.toLowerCase().includes('openrouter')) {
      provider = 'openrouter'
    } else if (request.model.toLowerCase().includes('gemini')) {
      provider = 'gemini'
    } else if (request.model.toLowerCase().includes('g0i')) {
      provider = 'g0i'
    }

    const skill = detectAssistantSkill(message, request.online)

    let conversationId = request.privateMode ? null : request.conversationId || null
    if (!request.privateMode && !conversationId) {
      conversationId = chatRepo.createConversation(
        makeTitle(message),
        request.model,
        request.online,
        request.chatMode || 'mangavox',
        request.personaId || 'nova',
        !!request.adultEnabled,
      ).id
    }
    if (conversationId) {
      chatRepo.updateConversationSettings(
        conversationId,
        request.model,
        request.online,
        request.chatMode || 'mangavox',
        request.personaId || 'nova',
        !!request.adultEnabled,
      )
    }
    if (conversationId) chatRepo.addMessage(conversationId, 'user', message)

    const turns = request.privateMode
      ? [...(request.sessionMessages || []), { role: 'user' as const, content: message }]
      : chatRepo.listMessages(conversationId!).map(({ role, content }) => ({ role, content }))
    // The very first user/assistant exchange in a saved conversation.
    const isFirstTurn = !request.privateMode && turns.length <= 1

    const chatMode = request.chatMode || 'mangavox'
    const context = chatMode === 'mangavox' ? buildMediaContext(request.watchHistory || []) : ''

    const memorySnapshot = request.privateMode && request.sessionId
      ? getPrivateSessionMemory(request.sessionId)
      : conversationId
        ? getConversationMemory(conversationId)
        : { summary: '', facts: [], messageCount: 0, topic: '' }
    const conversationMemory = formatMemoryForPrompt(memorySnapshot)
    const skillContext = getActiveSkillContext(message)
    const artifactMeta = request.artifact
      ? { kind: request.artifact, title: request.artifactTitle || makeTitle(message) }
      : undefined
    const jobSteps: ResearchStep[] = []
    let partialAnswer = ''
    const researchDepth = request.researchDepth || 'mid'
    let tokenUsage = Math.max(1, Math.ceil(turns.reduce((sum, turn) => sum + turn.content.length, 0) / 4))
    const tokenLimit = request.deepResearch
      ? researchDepth === 'extreme' ? 64_000 : researchDepth === 'basic' ? 12_000 : 32_000
      : 8_000
    let persistedResearch: Partial<ResearchBundle> | undefined
    let lastPersistedAt = 0
    const assistantMessage = conversationId
      ? chatRepo.addMessage(conversationId, 'assistant', '', {
        artifact: artifactMeta,
        job: {
          requestId: request.requestId,
          status: request.online ? 'researching' : 'thinking',
          label: request.online ? 'Planning research' : 'Thinking',
          tokenUsage,
          tokenLimit,
          depth: request.deepResearch ? researchDepth : undefined,
          updatedAt: Date.now(),
        },
        steps: jobSteps,
      })
      : null

    const jobPayload = (status: string, label: string, error?: string) => ({
      artifact: artifactMeta,
      research: persistedResearch,
      steps: jobSteps,
      job: {
        requestId: request.requestId,
        status,
        label,
        error,
        updatedAt: Date.now(),
        tokenUsage,
        tokenLimit,
        depth: request.deepResearch ? researchDepth : undefined,
      },
    })
    const persistJob = (status: string, label: string, force = false, error?: string) => {
      if (!assistantMessage) return
      const now = Date.now()
      if (!force && now - lastPersistedAt < 400) return
      lastPersistedAt = now
      chatRepo.updateMessage(assistantMessage.id, partialAnswer, jobPayload(status, label, error))
    }
    const emitJob = (channel: string, payload: Record<string, unknown>) => {
      let provider = 'ollama'
      if (request.model.includes('/') || request.model.toLowerCase().includes('openrouter')) {
        provider = 'openrouter'
      } else if (request.model.toLowerCase().includes('gemini')) {
        provider = 'gemini'
      } else if (request.model.toLowerCase().includes('g0i')) {
        provider = 'g0i'
      }
      event.sender.send(channel, {
        requestId: request.requestId,
        conversationId,
        messageId: assistantMessage?.id,
        model: request.model,
        provider,
        ...payload,
      })
    }
    const seenResearchSources = new Set<string>()
    const seenResearchDetails = new Set<string>()
    let accountedRounds = 0
    const accountResearchProgress = (progress: ResearchProgressEvent) => {
      let chars = 0
      if (progress.detail && !seenResearchDetails.has(progress.detail)) {
        seenResearchDetails.add(progress.detail)
        chars += progress.detail.length
      }
      const rounds = progress.rounds || []
      for (const round of rounds.slice(accountedRounds)) chars += round.query.length + 24
      accountedRounds = Math.max(accountedRounds, rounds.length)
      for (const source of progress.sources || []) {
        if (seenResearchSources.has(source.url)) continue
        seenResearchSources.add(source.url)
        chars += source.title.length + (source.summary || '').length + source.url.length
      }
      tokenUsage += Math.max(1, Math.ceil(chars / 4))
    }
    let activeUsageLabel = request.online ? 'Planning research' : 'Thinking'
    let activeUsageStatus: 'researching' | 'thinking' | 'writing' = request.online ? 'researching' : 'thinking'
    let lastUsageEmit = 0
    const accountModelUsage = (tokens: number) => {
      tokenUsage += Math.max(1, tokens)
      const now = Date.now()
      if (now - lastUsageEmit < 160) return
      lastUsageEmit = now
      persistJob(activeUsageStatus, activeUsageLabel)
      emitJob(IPC.MEDIA_CHAT_STATUS, {
        status: activeUsageStatus,
        label: activeUsageLabel,
        skill,
        tokenUsage,
        depth: request.deepResearch ? researchDepth : undefined,
      })
    }

    emitJob(IPC.MEDIA_CHAT_STATUS, {
      status: request.online ? 'researching' : 'thinking',
      label: request.online ? 'Planning research' : 'Thinking',
      skill,
      tokenUsage,
      tokenLimit,
      depth: request.deepResearch ? researchDepth : undefined,
    })

    let researchBundle: ResearchBundle = {
      query: message,
      queries: [message],
      skill,
      phases: [],
      sources: [],
    }

    // Register the abort controller BEFORE research starts so the stop button
    // also cancels the (potentially long) crawl, not just the model stream.
    const controller = new AbortController()
    activeStreams.set(request.requestId, controller)

    try {
      let interpretation = ''
      if (request.deepResearch) {
        activeUsageLabel = 'Understanding your request'
        emitJob(IPC.MEDIA_CHAT_STATUS, {
          status: 'researching',
          label: 'Understanding your request',
          skill,
          tokenUsage,
          tokenLimit,
          depth: researchDepth,
        })
        interpretation = await interpretResearchRequest(
          request.model,
          request.researchQuery || message,
          researchDepth,
          controller.signal,
          accountModelUsage,
        )
        persistJob('researching', 'Understanding your request', true)
        emitJob(IPC.MEDIA_CHAT_STATUS, {
          status: 'researching',
          label: 'Understanding your request',
          skill,
          tokenUsage,
          depth: researchDepth,
        })
      }
      if (request.online) {
        researchBundle = await researchOnlineDeep(request.researchQuery || message, (progress) => {
        accountResearchProgress(progress)
        if (progress.phase && progress.label) {
          const last = jobSteps[jobSteps.length - 1]
          const isNew = !last || last.phase !== progress.phase || (!!progress.detail && progress.detail !== last.detail)
          if (isNew) {
            for (const step of jobSteps) {
              if (step.status === 'running') step.status = 'done'
            }
            jobSteps.push({
              phase: progress.phase,
              label: progress.label,
              detail: progress.detail,
              status: progress.status,
              ts: Date.now(),
            })
          } else {
            jobSteps[jobSteps.length - 1] = {
              ...last,
              label: progress.label,
              detail: progress.detail || last.detail,
              status: progress.status,
            }
          }
        }
        persistedResearch = {
          ...(persistedResearch || {
            query: request.researchQuery || message,
            queries: [],
            skill,
            phases: [],
            sources: [],
          }),
          skill: progress.skill || persistedResearch?.skill || skill,
          rounds: progress.rounds || persistedResearch?.rounds,
          stats: progress.stats || persistedResearch?.stats,
          sources: progress.sources?.length
            ? progress.sources.map((source) => ({
              title: source.title,
              url: source.url,
              summary: source.summary || '',
              score: source.score,
              domain: source.domain,
              favicon: source.favicon,
            }))
            : persistedResearch?.sources || [],
        }
        persistJob('researching', progress.label)
        emitJob(IPC.MEDIA_CHAT_RESEARCH, {
          ...(progress as unknown as Record<string, unknown>),
          tokenUsage,
          depth: researchDepth,
        })
        }, controller.signal, {
          deep: !!request.deepResearch,
          depth: researchDepth,
          interpretation,
        })
        persistedResearch = trimResearchForStorage(researchBundle)
        persistJob('writing', 'Writing answer', true)
        emitJob(IPC.MEDIA_CHAT_RESEARCH, {
          phase: 'synthesize',
          label: 'Writing answer',
          status: 'done',
          skill: researchBundle.skill,
          bundle: {
            query: researchBundle.query,
            queries: researchBundle.queries,
            skill: researchBundle.skill,
            phases: researchBundle.phases,
            sources: researchBundle.sources,
            rounds: researchBundle.rounds,
            stats: researchBundle.stats,
            images: researchBundle.images,
            workspaceDir: researchBundle.workspaceDir,
          },
        })
      }

      emitJob(IPC.MEDIA_CHAT_STATUS, {
        status: 'thinking',
        label: request.deepResearch ? 'Writing research report' : 'Thinking',
        skill,
        tokenUsage,
        tokenLimit,
        depth: request.deepResearch ? researchDepth : undefined,
      })

      // Stopped during research → don't generate; close out the turn cleanly.
      if (controller.signal.aborted) {
        const stopped = '⏹ Stopped — research cancelled before the answer was written.'
        partialAnswer = stopped
        persistJob('stopped', 'Stopped', true)
        emitJob(IPC.MEDIA_CHAT_STATUS, { status: 'stopped', label: 'Stopped', skill, tokenUsage, tokenLimit, depth: researchDepth })
        recordTokenUsage({
          requestId: request.requestId,
          conversationId,
          tokens: tokenUsage,
          privateMode: request.privateMode,
          mode: request.deepResearch ? `deep-${researchDepth}` : request.online ? 'research' : 'chat',
          label: request.researchQuery || makeTitle(message),
          status: 'stopped',
          provider,
        })
        return {
          conversationId,
          messageId: assistantMessage?.id || null,
          answer: stopped,
          research: request.online ? researchBundle : null,
          skill,
        }
      }
      const emitToken = (token: string) => {
        activeUsageStatus = 'writing'
        activeUsageLabel = request.deepResearch ? 'Writing research report' : 'Writing answer'
        partialAnswer += token
        tokenUsage += Math.max(1, Math.ceil(token.length / 4))
        persistJob('writing', request.deepResearch ? 'Writing research report' : 'Writing answer')
        emitJob(IPC.MEDIA_CHAT_STREAM, { token, tokenUsage, tokenLimit, depth: researchDepth })
      }
      const emitArticleProgress = (label: string, detail?: string) => {
        activeUsageStatus = 'writing'
        activeUsageLabel = label
        const progress: ResearchProgressEvent = {
          phase: 'synthesize',
          label,
          detail,
          status: label === 'Research document complete' ? 'done' : 'running',
          skill,
          depth: researchDepth,
        }
        const last = jobSteps[jobSteps.length - 1]
        if (last?.status === 'running') last.status = 'done'
        jobSteps.push({ phase: 'synthesize', label, detail, status: progress.status, ts: Date.now() })
        persistJob('writing', label, true)
        emitJob(IPC.MEDIA_CHAT_RESEARCH, { ...progress, tokenUsage, tokenLimit })
        emitJob(IPC.MEDIA_CHAT_STATUS, {
          status: progress.status === 'done' ? 'writing' : 'writing',
          label,
          skill,
          tokenUsage,
          depth: researchDepth,
        })
      }

      // Deep research uses the staged generator: code assembles the article
      // structure; the model writes the text in small recoverable calls. A
      // renderable page comes out even if individual model calls fail.
      const answer = request.deepResearch
        ? await generateDeepResearchArticle(
          request.model,
          request.researchQuery || message,
          researchBundle,
          emitToken,
          controller.signal,
          emitArticleProgress,
          accountModelUsage,
        )
        : await streamMediaChat(
          request.model,
          turns,
          context,
          researchBundle.sources,
          {
            chatMode,
            personaId: request.personaId || 'nova',
            adultEnabled: !!request.adultEnabled,
            skill: request.online ? researchBundle.skill : skill,
            conversationMemory,
            skillContext,
            artifactKind: request.artifact,
            researchImages: researchBundle.images?.map((i) => i.url),
            online: request.online,
            signal: controller.signal,
          },
          emitToken,
        )
      absorbConversationTurn(
        conversationId,
        request.sessionId || null,
        message,
        answer,
        request.privateMode,
      )

      if (conversationId) {
        const meta = {
          artifact: artifactMeta,
          research: request.online ? trimResearchForStorage(researchBundle) : undefined,
          steps: jobSteps.map((step) => ({ ...step, status: step.status === 'running' ? 'done' : step.status })),
          job: {
            requestId: request.requestId,
            status: 'done',
            label: 'Completed',
            updatedAt: Date.now(),
            tokenUsage,
            tokenLimit,
            depth: request.deepResearch ? researchDepth : undefined,
          },
        }
        chatRepo.updateMessage(assistantMessage!.id, answer, meta)
        emitJob(IPC.MEDIA_CHAT_STATUS, { status: 'done', label: 'Completed', skill, tokenUsage, tokenLimit, depth: researchDepth })
        // First exchange → give the conversation a smart, contextual name with
        // an icon + one-line description. Fire-and-forget so we don't delay.
        if (isFirstTurn && answer.trim()) {
          void titleConversation(conversationId, request.model, message, answer).then((meta) => {
            if (meta) event.sender.send(IPC.MEDIA_CHAT_RENAMED, { conversationId, ...meta })
          })
        }
      }
      recordTokenUsage({
        requestId: request.requestId,
        conversationId,
        tokens: tokenUsage,
        privateMode: request.privateMode,
        mode: request.deepResearch ? `deep-${researchDepth}` : request.online ? 'research' : 'chat',
        label: request.researchQuery || makeTitle(message),
        status: 'done',
        provider,
      })
      return {
        conversationId,
        messageId: assistantMessage?.id || null,
        answer,
        research: request.online ? researchBundle : null,
        skill,
      }
    } catch (error: any) {
      const detail = error?.message || 'The assistant could not respond'
      partialAnswer = partialAnswer || `I could not answer: ${detail}`
      persistJob('error', 'Failed', true, detail)
      emitJob(IPC.MEDIA_CHAT_STREAM, { error: detail })
      emitJob(IPC.MEDIA_CHAT_STATUS, { status: 'error', label: 'Failed', error: detail, skill, tokenUsage, tokenLimit, depth: researchDepth })
      recordTokenUsage({
        requestId: request.requestId,
        conversationId,
        tokens: tokenUsage,
        privateMode: request.privateMode,
        mode: request.deepResearch ? `deep-${researchDepth}` : request.online ? 'research' : 'chat',
        label: request.researchQuery || makeTitle(message),
        status: controller.signal.aborted ? 'stopped' : 'error',
        provider,
      })
      throw new Error(detail)
    } finally {
      activeStreams.delete(request.requestId)
    }
  })
}

function makeTitle(message: string): string {
  const clean = message.replace(/\s+/g, ' ').trim()
  return clean.length > 48 ? `${clean.slice(0, 48)}...` : clean
}

async function titleConversation(
  conversationId: string,
  model: string,
  userMessage: string,
  assistantMessage: string,
): Promise<{ title: string; icon: string; subtitle: string } | null> {
  try {
    const meta = await generateConversationMeta(model, userMessage, assistantMessage)
    chatRepo.updateConversationMeta(conversationId, meta.title, meta.icon, meta.subtitle)
    return meta
  } catch {
    return null
  }
}
