// ── Manga & Chapter Types ──

export interface Manga {
  id: string
  source_plugin: string
  source_id: string
  title: string
  author: string | null
  artist: string | null
  description: string | null
  cover_path: string | null
  cover_url?: string
  status: 'ongoing' | 'completed' | 'hiatus' | 'cancelled' | null
  genres: string[]
  is_favorite: boolean
  notes: string | null
  added_at: string
  last_read_at: string | null
  chapter_count?: number
  unread_count?: number
}

export interface LibraryCategory {
  id: string
  name: string
  sort_order: number
}

export interface Chapter {
  id: string
  manga_id: string
  source_plugin: string
  source_id: string
  number: number | null
  title: string | null
  language: string
  page_count: number | null
  download_status: PipelineStageStatus
  ocr_status: PipelineStageStatus
  tts_status: PipelineStageStatus
  video_status: PipelineStageStatus
  read_progress: number
  is_read: boolean
  pages_dir: string | null
  added_at: string
}

export type PipelineStageStatus = 'none' | 'queued' | 'running' | 'complete' | 'failed' | 'downloading'

// ── Plugin Types ──

export interface MangaSearchResult {
  source_id: string
  title: string
  alt_titles?: string[]
  cover_url?: string
  author?: string
  status?: string
}

export interface MangaDetails {
  source_id: string
  title: string
  author?: string
  artist?: string
  description?: string
  cover_url?: string
  status?: string
  genres?: string[]
}

export interface ChapterInfo {
  source_id: string
  number: number | null
  title?: string
  language: string
  scanlation_group?: string
  published_at?: string
  page_count?: number
}

export interface PluginManifest {
  id: string
  name: string
  version: string
  base_url: string
  lang: string
  entry_point: string
}

// ── Extension Repo Types (Tachiyomi-compatible) ──

export interface RepoExtension {
  name: string
  pkg: string
  apk: string
  lang: string
  code: number
  version: string
  nsfw: number
  sources: RepoSource[]
}

export interface RepoSource {
  name: string
  lang: string
  id: string
  baseUrl: string
}

// ── Pipeline Types ──

export interface PipelineJob {
  id: string
  manga_id: string | null
  chapter_id: string
  status: 'queued' | 'running' | 'paused' | 'completed' | 'failed' | 'cancelled'
  current_stage: string | null
  progress: number
  config_json: string
  result_json: string | null
  error_message: string | null
  created_at: string
  updated_at: string
  chapter_number?: number | null
  chapter_title?: string | null
}

export interface SyncMapEntry {
  page: number
  text: string
  audio_file: string
  start_ms: number
  end_ms: number
}

export interface SyncMap {
  chapter_id: string
  segments: SyncMapEntry[]
}

// ── UI Navigation ──

export type Page = 'chat' | 'browse' | 'library' | 'manga-detail' | 'reader' | 'queue' | 'anime' | 'anime-detail' | 'anime-player' | 'movies' | 'player' | 'dashboard' | 'settings' | 'history' | 'text' | 'youtube' | 'subtitles' | 'discover' | 'pipeline' | 'audio' | 'video'

export type MediaChatRole = 'user' | 'assistant'
export type MediaChatMode = 'mangavox' | 'open'

export type MediaKind = 'anime' | 'movie' | 'series'
export type MediaEventType = 'open' | 'play' | 'progress' | 'complete' | 'like' | 'dislike' | 'skip' | 'search'

export interface MediaTitle {
  id: string
  kind: MediaKind
  title: string
  imdbId?: string
  tmdbId?: number
  malId?: number
  coverUrl?: string
  bannerUrl?: string
  year?: number
  genres: string[]
  languages: string[]
  description?: string
  score?: number
}

export interface MediaEvent {
  id?: string
  mediaId?: string
  kind: MediaKind
  type: MediaEventType
  title?: MediaTitle
  query?: string
  providerId?: string
  episodeNumber?: number
  positionSecs?: number
  durationSecs?: number
  createdAt?: number
}

export interface PlaybackState {
  mediaId: string
  kind: MediaKind
  title: string
  coverUrl?: string
  providerId?: string
  episodeNumber?: number
  positionSecs: number
  durationSecs: number
  completed: boolean
  updatedAt: number
}

export type PlaybackMode = 'direct' | 'remux' | 'audio-transcode' | 'full-transcode'
export type PlaybackLifecycle = 'creating' | 'connecting' | 'ready' | 'playing' | 'stopped' | 'failed'

export interface MediaTrack {
  index: number
  streamIndex: number
  kind: 'video' | 'audio' | 'subtitle'
  codec: string
  language?: string
  title?: string
  channels?: number
  channelLayout?: string
  width?: number
  height?: number
  forced?: boolean
  default?: boolean
  imageBased?: boolean
}

export interface MediaProbe {
  durationSecs: number | null
  format?: string
  bitrate?: number
  frameRate?: number
  tracks: MediaTrack[]
}

export interface PlaybackSource {
  infoHash: string
  fileIdx?: number
  title: string
  quality?: string
  source?: string
  seeders?: number
  size?: number
}

export interface PlaybackReturnRoute {
  page: Page
  params: Record<string, unknown>
}

export interface PlaybackFailure {
  code: string
  message: string
  recoverable: boolean
  occurredAt: number
}

export interface PlaybackSession {
  id: string
  state: PlaybackLifecycle
  source: PlaybackSource
  sources: PlaybackSource[]
  streamUrl?: string
  mediaId?: string
  mediaKind?: MediaKind
  title: string
  episodeNumber?: number
  mode: PlaybackMode
  probe?: MediaProbe
  selectedAudioIndex: number
  selectedSubtitleIndex?: number
  positionSecs: number
  transcodeOffsetSecs: number
  returnRoute: PlaybackReturnRoute
  failure?: PlaybackFailure
  createdAt: number
  updatedAt: number
}

export interface PlaybackPreferences {
  playbackSpeed: number
  volume: number
  quality: 'auto' | 'data-saver' | '720p' | '1080p' | 'best'
  audioLanguage: string
  subtitleLanguage: string
  subtitlesEnabled: boolean
  subtitleSize: number
  subtitleVerticalPosition: number
  subtitleOffsetSecs: number
}

export interface RecoveryState {
  session: PlaybackSession
  failure: PlaybackFailure
  recentLogs: string[]
}

export interface UserMediaProfile {
  favoriteGenres: { name: string; score: number }[]
  favoriteLanguages: { name: string; score: number }[]
  favoriteProviders: { id: string; score: number }[]
  totals: Record<MediaEventType, number>
}

export interface ProviderTheme {
  accent: string
  accent2: string
  background: string
  surface: string
  cardRadius: number
  heroStyle: 'cinematic' | 'editorial' | 'bright'
}

export interface StreamingProvider {
  id: string
  tmdbProviderId?: number
  name: string
  logoUrl?: string
  regions: string[]
  deepLinkUrl: string
  enabled: boolean
  priority: number
  refreshedAt: number
  theme: ProviderTheme
}

export interface ProviderAvailability {
  mediaId: string
  providerId: string
  region: string
  type: 'subscription' | 'free' | 'ads' | 'rent' | 'buy'
  url: string
}

export interface RecommendationRow {
  id: string
  title: string
  reason: string
  items: MediaTitle[]
}

export interface ImportMatch {
  row: number
  sourceTitle: string
  watchedAt?: string
  matched?: MediaTitle
  confidence: number
  status: 'matched' | 'review' | 'unmatched'
}

export interface MediaChatMessage {
  id: string
  conversation_id: string | null
  role: MediaChatRole
  content: string
  created_at: string
  /** JSON persisted with assistant messages: artifact info, research bundle, agent action logs. */
  meta_json?: string
}

export interface MediaChatConversation {
  id: string
  title: string
  /** Emoji that captures the topic, e.g. "🎬" or "🧠". */
  icon?: string
  /** One-line description of what the conversation is about. */
  subtitle?: string
  model: string
  online_enabled: boolean
  chat_mode: MediaChatMode
  persona_id: string
  adult_enabled: boolean
  created_at: string
  updated_at: string
}

// ── Local AI Models ──

export type AIModelRole = 'vision' | 'text'
export type AIHardwareTier = 'basic' | 'standard' | 'performance' | 'workstation'

export interface SystemProfile {
  platform: string
  arch: string
  cpu: string
  cpuCores: number
  ramGb: number
  gpu: string
  gpuVendor: 'apple' | 'nvidia' | 'amd' | 'intel' | 'unknown'
  vramGb: number | null
  unifiedMemory: boolean
  availableModelMemoryGb: number
  tier: AIHardwareTier
}

export interface ModelCatalogEntry {
  name: string
  label: string
  roles: AIModelRole[]
  downloadGb: number
  minMemoryGb: number
  recommendedMemoryGb: number
  vision: boolean
  multilingual: boolean
  hindi: boolean
  quality: number
  description: string
  sourceUrl: string
}

export interface CompatibleModel extends ModelCatalogEntry {
  compatible: boolean
  recommended: boolean
  reason: string
  installed: boolean
}

export interface AIModelPreferences {
  visionModel: string
  textModel: string
  autoSelect: boolean
  migratedLegacy: boolean
}

// ── Reading History ──

export interface ReadingHistory {
  id: string
  manga_id: string
  chapter_id: string
  source_plugin: string
  source_chapter_id: string
  manga_title: string
  chapter_number: number | null
  chapter_title: string | null
  cover_url: string | null
  page: number
  read_at: string
}
