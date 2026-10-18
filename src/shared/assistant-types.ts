/** Assistant research, skills, and memory types (renderer + main safe) */

export type ResearchPhaseId = 'plan' | 'search' | 'crawl' | 'analyze' | 'refine' | 'synthesize'

export type AssistantSkill = 'chat' | 'research' | 'analyze' | 'visualize' | 'code'
export type ResearchDepth = 'basic' | 'mid' | 'extreme'

/** A long-form output that opens in the Canvas panel instead of a chat bubble. */
export type ArtifactKind = 'story' | 'code' | 'html' | 'doc'

export interface ResearchSource {
  title: string
  url: string
  summary: string
  score?: number
  /** Bare hostname, e.g. "en.wikipedia.org" */
  domain?: string
  /** Favicon / logo URL for the source's domain */
  favicon?: string
  /** Whether the full page body was fetched and read */
  read?: boolean
  /** The research-tree node that discovered this source. */
  nodeId?: string
}

/** One query node in the six-round deep-research tree. */
export interface ResearchRound {
  /** 1-based node index, kept for compatibility with the existing UI. */
  round: number
  nodeId?: string
  parentId?: string
  /** Tree depth, 1-6. */
  depth?: number
  query: string
  /** New unique sources discovered in this round */
  found: number
  /** Running total of unique sources after this round */
  total: number
}

export interface ResearchStats {
  /** Number of distinct search queries issued */
  searches: number
  /** Number of search rounds (planning iterations) */
  rounds: number
  /** Number of full pages crawled/read */
  pagesRead: number
  /** Total unique sources gathered */
  sourcesFound: number
  /** Sources kept after ranking */
  sourcesKept: number
  /** Max rounds allowed (the budget ceiling) */
  maxRounds: number
}

export interface ResearchPhase {
  id: ResearchPhaseId
  label: string
  status: 'running' | 'done' | 'skipped'
  detail?: string
}

/** One line in the live "what the agent is doing right now" timeline. */
export interface ResearchStep {
  phase: ResearchPhaseId
  label: string
  detail?: string
  status: 'running' | 'done' | 'skipped'
  ts: number
}

export interface ResearchImage {
  url: string
  alt?: string
  source?: string
  /** Page that supplied the image, when scraped from a research source. */
  sourceUrl?: string
  sourceTitle?: string
}

export interface ResearchBundle {
  query: string
  /** Detailed model-written interpretation of what the user actually needs. */
  interpretation?: string
  depth?: ResearchDepth
  queries: string[]
  skill: AssistantSkill
  phases: ResearchPhase[]
  sources: ResearchSource[]
  rounds?: ResearchRound[]
  stats?: ResearchStats
  /** Real image URLs gathered from keyed search providers (Tavily/Google/Bing…). */
  images?: ResearchImage[]
  /** Which search providers were actually used this run. */
  providers?: string[]
  /** On-disk workspace containing node evidence and the final report. */
  workspaceDir?: string
}

export interface ResearchProgressEvent {
  requestId?: string
  phase: ResearchPhaseId
  label: string
  status: 'running' | 'done' | 'skipped'
  detail?: string
  sources?: Array<{ title: string; url: string; summary?: string; score?: number; domain?: string; favicon?: string }>
  rounds?: ResearchRound[]
  stats?: ResearchStats
  skill?: AssistantSkill
  interpretation?: string
  depth?: ResearchDepth
}

export interface ConversationMemorySnapshot {
  summary: string
  facts: string[]
  messageCount: number
  topic?: string
}

/** A user-built skill: researched online, synthesized, and reusable later. */
export interface CustomSkill {
  id: string
  slug: string
  name: string
  icon: string
  description: string
  instructions: string
  knowledge: string
  triggers: string[]
  sources: ResearchSource[]
  usageCount: number
  createdAt: number
  updatedAt: number
}

export type SkillBuildPhaseId = 'research' | 'synthesize' | 'save'

export interface SkillBuildProgress {
  phase: SkillBuildPhaseId
  label: string
  status: 'running' | 'done' | 'error'
  detail?: string
  /** 0-100 overall completion for the progress bar. */
  percent?: number
  /** Human-readable parameters/metrics for what the build is doing right now. */
  params?: Record<string, string | number>
}
