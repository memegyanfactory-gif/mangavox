export const IPC = {
  SOURCE_VERIFY_NEEDED: 'source:verify-needed',
  SOURCE_VERIFY_OPEN: 'source:verify-open',
  SOURCE_HEALTH: 'source:health',
  SOURCE_TEST: 'source:test',
  UPDATE_STATUS: 'update:status',
  UPDATE_GET_STATUS: 'update:get-status',
  UPDATE_CHECK: 'update:check',
  UPDATE_DOWNLOAD: 'update:download',
  UPDATE_INSTALL: 'update:install',
  UPDATE_OPEN_RELEASES: 'update:open-releases',
  STORAGE_GET: 'storage:get',
  STORAGE_SET: 'storage:set',
  STORAGE_PICK: 'storage:pick',
  MANGA_TRENDING: 'manga:trending',
  MANGA_RECOMMENDATIONS: 'manga:recommendations',
  WINDOW_SET_OVERLAY: 'window:set-overlay',
  // Library
  LIBRARY_LIST: 'library:list',
  LIBRARY_GET: 'library:get',
  LIBRARY_SEARCH: 'library:search',
  LIBRARY_ADD: 'library:add',
  LIBRARY_REMOVE: 'library:remove',
  LIBRARY_REMOVE_MANY: 'library:remove-many',
  LIBRARY_CLEAR: 'library:clear',
  LIBRARY_TOGGLE_FAVORITE: 'library:toggle-favorite',
  LIBRARY_UPDATE_NOTES: 'library:update-notes',
  LIBRARY_UPDATE_DETAILS: 'library:update-details',

  // Chapters
  CHAPTER_LIST: 'chapter:list',
  CHAPTER_SYNC: 'chapter:sync',
  CHAPTER_STAGE_STATUS: 'chapter:stage-status', // (mangaId) → per-chapter stage flags
  CHAPTER_STAGE_STATUS_ONE: 'chapter:stage-status-one', // (mangaId, chapterId) → single chapter
  CHAPTER_LIST_BY_SOURCE: 'chapter:list-by-source',
  CHAPTER_UPDATE_PROGRESS: 'chapter:update-progress',
  CHAPTER_MARK_READ: 'chapter:mark-read',

  // Reading History
  HISTORY_LIST: 'history:list',
  HISTORY_ADD: 'history:add',
  HISTORY_TOUCH: 'history:touch',
  HISTORY_REMOVE: 'history:remove',
  HISTORY_CLEAR: 'history:clear',

  // Plugins / Sources
  PLUGIN_LIST_SOURCES: 'plugin:list-sources',
  PLUGIN_SEARCH: 'plugin:search',
  PLUGIN_GET_DETAILS: 'plugin:get-details',
  PLUGIN_GET_CHAPTERS: 'plugin:get-chapters',
  PLUGIN_GET_PAGES: 'plugin:get-pages',
  PLUGIN_GET_POPULAR: 'plugin:get-popular',
  PLUGIN_GET_LATEST: 'plugin:get-latest',
  PLUGIN_GET_TAGS: 'plugin:get-tags',
  PLUGIN_GET_BY_TAG: 'plugin:get-by-tag',
  PLUGIN_INSTALL: 'plugin:install',
  PLUGIN_UNINSTALL: 'plugin:uninstall',
  PLUGIN_FIND_ACROSS_SOURCES: 'plugin:find-across-sources',

  // Extension Repos (Tachiyomi-compatible)
  REPO_ADD: 'repo:add',
  REPO_REMOVE: 'repo:remove',
  REPO_LIST: 'repo:list',
  REPO_BROWSE: 'repo:browse',
  REPO_INSTALL_EXT: 'repo:install-ext',

  // Downloads
  DOWNLOAD_CHAPTER: 'download:chapter',
  DOWNLOAD_CANCEL: 'download:cancel',
  DOWNLOAD_PROGRESS: 'download:progress', // main→renderer push

  // Pipeline
  PIPELINE_START: 'pipeline:start',
  PIPELINE_CANCEL: 'pipeline:cancel',
  PIPELINE_PAUSE: 'pipeline:pause',
  PIPELINE_RESUME: 'pipeline:resume',
  PIPELINE_ACTIVE_STATUS: 'pipeline:active-status',
  PIPELINE_LIST_JOBS: 'pipeline:list-jobs',
  PIPELINE_PROGRESS: 'pipeline:progress', // main→renderer push
  PIPELINE_RESET_CHAPTER: 'pipeline:reset-chapter',
  PIPELINE_GET_SCRIPT: 'pipeline:get-script',
  PIPELINE_SAVE_SCRIPT: 'pipeline:save-script',
  PIPELINE_EXPORT_TEXT: 'pipeline:export-text',

  // Reader
  READER_GET_PAGES: 'reader:get-pages',
  READER_SAVE_PROGRESS: 'reader:save-progress',

  // Player
  PLAYER_GET_SYNC_MAP: 'player:get-sync-map',
  PLAYER_GET_AUDIO_DATA: 'player:get-audio-data',
  AUDIO_LIST_HISTORY: 'audio:list-history',
  AUDIO_EXPORT: 'audio:export',
  AUDIO_RESET: 'audio:reset',
  VIDEO_EXPORT: 'video:export',

  // Local TTS (Kokoro-82M — Hindi, no API)
  TTS_KOKORO_STATUS: 'tts:kokoro-status',
  TTS_KOKORO_INSTALL: 'tts:kokoro-install',
  TTS_KOKORO_PROGRESS: 'tts:kokoro-progress', // main→renderer push
  TTS_KOKORO_CANCEL: 'tts:kokoro-cancel',
  TTS_VOICE_PREVIEW: 'tts:voice-preview',
  TTS_VOICE_READY: 'tts:voice-ready',

  // Logging (main→renderer push)
  LOG_ENTRY: 'log:entry',

  // Settings / Config
  CONFIG_GET: 'config:get',
  CONFIG_SET: 'config:set',
  CONFIG_PICK_AUDIO: 'config:pick-audio',
  TOKEN_USAGE_STATS: 'token-usage:stats',
  TOKEN_USAGE_CLEAR: 'token-usage:clear',
  SYSTEM_PROFILE: 'system:profile',
  AI_MODEL_CATALOG: 'ai-models:catalog',
  AI_MODEL_PREFERENCES_GET: 'ai-models:preferences-get',
  AI_MODEL_PREFERENCES_SET: 'ai-models:preferences-set',
  AI_MODELS_CHANGED: 'ai-models:changed',

  // Image Crop (auto-trim borders + content detect)
  CROP_START: 'crop:start',
  CROP_CANCEL: 'crop:cancel',
  CROP_RESTORE: 'crop:restore',
  CROP_STATUS: 'crop:status',
  CROP_PROGRESS: 'crop:progress', // main→renderer push
  CROP_PAGE: 'crop:page',         // single-page manual crop (user-drawn bbox)
  CROP_LIST_PAGES: 'crop:list-pages', // list page filenames for a chapter
  IMAGE_READ_DATA_URL: 'image:read-data-url', // read any local image as base64 data URL (renderer-safe)

  // Webtoon strip splitting (cut tall vertical strips into ~1:1 panels)
  STRIP_SPLIT_START: 'strip:split-start',
  STRIP_SPLIT_CANCEL: 'strip:split-cancel',
  STRIP_SPLIT_STATUS: 'strip:split-status',
  STRIP_SPLIT_PROGRESS: 'strip:split-progress', // main→renderer push

  // Manual Manga Import
  IMPORT_PICK_FOLDER: 'import:pick-folder',
  IMPORT_FROM_FOLDER: 'import:from-folder',
  IMPORT_FROM_URL: 'import:from-url',

  // Cast / Character Manager
  CHARACTER_LIST: 'character:list',
  CHARACTER_ADD: 'character:add',           // takes (mangaId, sourceImage, bbox, name, …) → crops + saves
  CHARACTER_UPDATE: 'character:update',
  CHARACTER_DELETE: 'character:delete',

  // Cloud LLM keys (encrypted via Electron safeStorage)
  GEMINI_HAS_KEY: 'gemini:has-key',
  GEMINI_SAVE_KEY: 'gemini:save-key',
  GEMINI_DELETE_KEY: 'gemini:delete-key',

  // Text Extraction (Vision LLM)
  TEXT_EXTRACT_START: 'text:extract-start',
  TEXT_EXTRACT_CANCEL: 'text:extract-cancel',
  TEXT_GET_RESULTS: 'text:get-results',
  TEXT_HAS_RESULTS: 'text:has-results',
  TEXT_ENHANCE_START: 'text:enhance-start',
  TEXT_ENHANCE_CANCEL: 'text:enhance-cancel',
  TEXT_GET_ENHANCED: 'text:get-enhanced',
  TEXT_COMBINE_START: 'text:combine-start',
  TEXT_GET_COMBINED: 'text:get-combined',
  TEXT_SAVE_COMBINED: 'text:save-combined', // renderer-side combine results (Puter, etc.)
  TEXT_ENHANCE_SEGMENT: 'text:enhance-segment',
  TEXT_SAVE_EDITED: 'text:save-edited',
  TEXT_EXPORT: 'text:export',
  TEXT_SEND_TO_TTS: 'text:send-to-tts',
  TEXT_TRANSLATE_HINGLISH: 'text:translate-hinglish',
  TEXT_TRANSLATE: 'text:translate', // generic multi-language (free Google endpoint)
  TEXT_PROGRESS: 'text:progress', // main→renderer push
  // ── Scene narrator (Phase 2: whole-chapter vision narration + memory) ──
  TEXT_NARRATE_START: 'text:narrate-start',
  TEXT_NARRATE_CANCEL: 'text:narrate-cancel',
  TEXT_GET_NARRATION: 'text:get-narration',
  TEXT_GET_NARRATION_PARTIAL: 'text:get-narration-partial',
  TEXT_NARRATION_DISCARD_PARTIAL: 'text:narration-discard-partial',
  TEXT_NARRATION_DISCARD: 'text:narration-discard',
  TEXT_SAVE_NARRATION: 'text:save-narration',
  // g0i.ai connectivity probe — Settings → AI Models "Test key" button.
  G0I_TEST_KEY: 'g0i:test-key',
  // Fired when the main process auto-disables g0i after a trial-expired
  // response, so the renderer can hide the Narrate button immediately.
  G0I_DISABLED: 'g0i:disabled',
  // OpenRouter connectivity probe — Settings → AI Models "Test key" button.
  OPENROUTER_TEST_KEY: 'openrouter:test-key',
  // Fetch available models from OpenRouter API for the model picker.
  OPENROUTER_LIST_MODELS: 'openrouter:list-models',
  // Fired when the main process detects the OpenRouter key is invalid/expired.
  OPENROUTER_DISABLED: 'openrouter:disabled',
  MEMORY_GET: 'memory:get',
  MEMORY_SET: 'memory:set',
  MEMORY_RESET: 'memory:reset',

  // Reader OCR Overlay (translation-on-image)
  READER_GET_OCR_OVERLAY: 'reader:get-ocr-overlay',

  // Ollama
  OLLAMA_STATUS: 'ollama:status',
  OLLAMA_LIST_MODELS: 'ollama:list-models',
  OLLAMA_PULL_MODEL: 'ollama:pull-model',
  OLLAMA_PULL_PROGRESS: 'ollama:pull-progress', // main→renderer push
  OLLAMA_DELETE_MODEL: 'ollama:delete-model',
  OLLAMA_PICK_MODEL_FILE: 'ollama:pick-model-file',
  OLLAMA_IMPORT_LOCAL: 'ollama:import-local',
  OLLAMA_PICK_MODELS_DIR: 'ollama:pick-models-dir',
  OLLAMA_LIST_RUNNING: 'ollama:list-running',
  OLLAMA_KILL_MODEL: 'ollama:kill-model',
  OLLAMA_KILL_ALL: 'ollama:kill-all',
  APP_READY: 'app:ready',
  // User Profile
  USER_UPLOAD_AVATAR: 'user:upload-avatar',

  // YouTube Download
  YTDL_CHECK: 'ytdl:check',           // check yt-dlp is installed
  YTDL_LIST_FORMATS: 'ytdl:list-formats', // get available formats for a URL
  YTDL_START: 'ytdl:start',           // start download
  YTDL_CANCEL: 'ytdl:cancel',         // cancel download
  YTDL_PROGRESS: 'ytdl:progress',     // main→renderer push
  YTDL_PICK_FOLDER: 'ytdl:pick-folder', // open folder picker dialog

  // Anime Catalog (Jikan/MAL built-in)
  ANIME_TOP: 'anime:top',
  ANIME_AIRING: 'anime:airing',
  ANIME_UPCOMING: 'anime:upcoming',
  ANIME_SEARCH: 'anime:search',
  ANIME_BY_GENRE: 'anime:by-genre',
  ANIME_GENRES: 'anime:genres',
  ANIME_DETAILS: 'anime:details',
  ANIME_EPISODES: 'anime:episodes',
  ANIME_RELATIONS: 'anime:relations',
  ANIME_GET_IMDB: 'anime:get-imdb',
  ANIME_SEASON: 'anime:season',
  ANIME_SCHEDULE: 'anime:schedule',
  WATCHLIST_LIST: 'watchlist:list',
  WATCHLIST_SET: 'watchlist:set',
  WATCHLIST_REMOVE: 'watchlist:remove',

  // Movies Catalog (Cinemeta — real movies, not anime)
  MOVIE_CATALOG: 'movie:catalog',
  MOVIE_SEARCH: 'movie:search',
  MOVIE_DETAILS: 'movie:details',
  SERIES_CATALOG: 'series:catalog',
  SERIES_SEARCH: 'series:search',
  SERIES_DETAILS: 'series:details',
  SERIES_EPISODES: 'series:episodes',

  MEDIA_PROVIDERS: 'media:providers',
  MEDIA_PROVIDER_CATALOG: 'media:provider-catalog',
  MEDIA_TMDB_DETAILS: 'media:tmdb-details',
  MEDIA_AVAILABILITY: 'media:availability',
  MEDIA_OPEN_PROVIDER: 'media:open-provider',
  MEDIA_EVENT_RECORD: 'media:event-record',
  MEDIA_HISTORY: 'media:history',
  MEDIA_EVENT_TITLES: 'media:event-titles',
  MEDIA_HISTORY_REMOVE: 'media:history-remove',
  MEDIA_EVENT_REMOVE: 'media:event-remove',
  MEDIA_SHELF_CLEAR: 'media:shelf-clear',
  MEDIA_PROFILE: 'media:profile',
  MEDIA_RECOMMENDATIONS: 'media:recommendations',
  MEDIA_PERSONALIZATION_RESET: 'media:personalization-reset',
  MEDIA_IMPORT_PICK: 'media:import-pick',
  MEDIA_IMPORT_COMMIT: 'media:import-commit',
  MEDIA_TRAKT_BEGIN: 'media:trakt-begin',
  MEDIA_TRAKT_POLL: 'media:trakt-poll',
  MEDIA_TRAKT_STATUS: 'media:trakt-status',
  MEDIA_TRAKT_SYNC: 'media:trakt-sync',
  MEDIA_SIMILAR_ITEMS: 'media:similar-items',

  // Anime Addons (Stremio-compatible, for streams)
  ADDON_LIST: 'addon:list',
  ADDON_INSTALL: 'addon:install',
  ADDON_UNINSTALL: 'addon:uninstall',
  ADDON_GET_CATALOG: 'addon:get-catalog',
  ADDON_SEARCH: 'addon:search',
  ADDON_GET_EPISODES: 'addon:get-episodes',
  ADDON_GET_STREAMS: 'addon:get-streams',
  ADDON_FIND_STREAMS: 'addon:find-streams',
  ADDON_STREAMS_UPDATED: 'addon:streams-updated', // main→renderer push when slow addons finish

  // Torrent Streaming
  TORRENT_START: 'torrent:start',
  TORRENT_STOP: 'torrent:stop',
  TORRENT_STATUS: 'torrent:status',
  TORRENT_PROGRESS: 'torrent:progress', // main→renderer push
  TORRENT_PRE_NEXT: 'torrent:pre-next', // pre-download next episode in background
  TORRENT_SEEK: 'torrent:seek',         // hint: prioritize pieces at seek ratio (0–1)

  // Supervised playback session
  PLAYBACK_SESSION_CREATE: 'playback-session:create',
  PLAYBACK_SESSION_GET: 'playback-session:get',
  PLAYBACK_SESSION_STOP: 'playback-session:stop',
  PLAYBACK_SESSION_UPDATE: 'playback-session:update',
  PLAYBACK_SESSION_SWITCH_SOURCE: 'playback-session:switch-source',
  PLAYBACK_SESSION_SEEK: 'playback-session:seek',
  PLAYBACK_SESSION_FAILURE: 'playback-session:failure',
  PLAYBACK_RECOVERY_GET: 'playback-session:recovery-get',
  PLAYBACK_RECOVERY_CLEAR: 'playback-session:recovery-clear',
  PLAYBACK_PREFERENCES_GET: 'playback-session:preferences-get',
  PLAYBACK_PREFERENCES_SET: 'playback-session:preferences-set',
  PLAYBACK_SUBTITLE_EXTRACT: 'playback-session:subtitle-extract',

  // Terminal Agent (AI-powered error diagnosis + shell commands + auto-fix)
  AGENT_DIAGNOSE: 'agent:diagnose',       // send error logs → get AI diagnosis
  AGENT_EXEC: 'agent:exec',               // execute a shell command
  AGENT_EXEC_OUTPUT: 'agent:exec-output', // main→renderer push (streaming stdout/stderr)
  AGENT_CHAT: 'agent:chat',               // free-form chat with agent about the project
  AGENT_AUTO_FIX: 'agent:auto-fix',       // start autonomous fix loop
  AGENT_FIX_STATUS: 'agent:fix-status',   // main→renderer push (fix progress updates)
  AGENT_APPROVE_ACTION: 'agent:approve-action', // renderer→main: approve/reject a pending action
  AGENT_CANCEL_FIX: 'agent:cancel-fix',   // cancel ongoing auto-fix
  AGENT_CHAT_STREAM: 'agent:chat-stream', // main→renderer push (streaming chat tokens)
  AGENT_CHAT_CLEAR: 'agent:chat-clear',   // reset conversation history
  AGENT_TOOL_CALL: 'agent:tool-call',     // main→renderer push (live tool-call events)
  AGENT_PARSE_INPUT: 'agent:parse-input',
  AGENT_GET_SUGGESTIONS: 'agent:get-suggestions',
  AGENT_GET_CONTEXTUAL_TAGS: 'agent:get-contextual-tags',
  AGENT_SEARCH_TORRENTS: 'agent:search-torrents',
  AGENT_FIND_SIMILAR: 'agent:find-similar',
  AGENT_START_DOWNLOAD: 'agent:start-download',
  AGENT_CANCEL_DOWNLOAD: 'agent:cancel-download',
  AGENT_DOWNLOAD_PROGRESS: 'agent:download-progress',
  AGENT_GET_MEMORY: 'agent:get-memory',
  AGENT_CLEAR_MEMORY: 'agent:clear-memory',
  AGENT_PICK_FOLDER: 'agent:pick-folder',
  AGENT_RECORD_PLAY: 'agent:record-play',
  AGENT_MATCH_TITLES: 'agent:match-titles',
  AGENT_PLAY_TITLE: 'agent:play-title',
  AGENT_PREFETCH_PLAY: 'agent:prefetch-play',

  // Media torrent downloads (anime / movies)
  MEDIA_TORRENT_DOWNLOAD: 'media:torrent-download',
  MEDIA_DOWNLOAD_PROGRESS: 'media:download-progress',
  MEDIA_DOWNLOAD_CANCEL: 'media:download-cancel',
  MEDIA_DOWNLOAD_REVEAL: 'media:download-reveal',
  BULK_DOWNLOAD_START: 'bulk-download:start',
  BULK_DOWNLOAD_STATUS: 'bulk-download:status',
  BULK_DOWNLOAD_CANCEL: 'bulk-download:cancel',

  // Hero banner clips
  HERO_GET_CLIP: 'hero:get-clip',

  // Media Assistant
  MEDIA_CHAT_SEND: 'media-chat:send',
  MEDIA_CHAT_STREAM: 'media-chat:stream',
  MEDIA_CHAT_RESEARCH: 'media-chat:research',
  MEDIA_CHAT_STATUS: 'media-chat:status',
  MEDIA_CHAT_STOP: 'media-chat:stop',
  MEDIA_CHAT_RENAMED: 'media-chat:renamed',
  ASSISTANT_CLEAR_SESSION: 'assistant:clear-session',
  MEDIA_CHAT_LOG: 'media-chat:log',
  MEDIA_CHAT_LIST: 'media-chat:list',
  MEDIA_CHAT_GET: 'media-chat:get',
  MEDIA_CHAT_DELETE: 'media-chat:delete',
  MEDIA_CHAT_CLEAR: 'media-chat:clear',
  MEDIA_CHAT_PREPARE_ADULT: 'media-chat:prepare-adult',
  MEDIA_CHAT_CREATE: 'media-chat:create',
  MEDIA_CHAT_RENAME: 'media-chat:rename',

  // Custom AI skills (researched + built on demand, reusable later)
  SKILL_BUILD: 'skill:build',
  SKILL_LIST: 'skill:list',
  SKILL_GET: 'skill:get',
  SKILL_DELETE: 'skill:delete',

  // Subtitles (OpenSubtitles integration)
  SUBTITLE_SEARCH: 'subtitle:search',
  SUBTITLE_DOWNLOAD: 'subtitle:download',
  SUBTITLE_PICK_FILE: 'subtitle:pick-file',

  // Privacy / Proxy
  PRIVACY_TOGGLE: 'privacy:toggle',
  PRIVACY_STATUS: 'privacy:status',
  PRIVACY_CHECK_IP: 'privacy:check-ip',

  // Cache Management
  CACHE_GET_STATS: 'cache:get-stats',
  CACHE_CLEAR_COVERS: 'cache:clear-covers',
  CACHE_CLEAR_ALL: 'cache:clear-all',
  CACHE_CLEAR_DOWNLOADS: 'cache:clear-downloads',
  CACHE_OPEN_DATA_DIR: 'cache:open-data-dir',
  CACHE_RESET_APP: 'cache:reset-app',

  // Audio Model Daemon & Video Discard
  TTS_LOAD_MODEL: 'tts:load-model',
  VIDEO_DISCARD: 'video:discard',

  // Library categories
  CATEGORY_LIST: 'category:list',
  CATEGORY_CREATE: 'category:create',
  CATEGORY_RENAME: 'category:rename',
  CATEGORY_DELETE: 'category:delete',
  CATEGORY_SET_FOR_MANGA: 'category:set-for-manga',
  CATEGORY_MANGA_MAP: 'category:manga-map',

  // Smart skip (AniSkip)
  SKIP_TIMES: 'skip:times',

  // Multi-provider assistant (docs/AI_ASSISTANT_PLAN.md)
  AI_PROVIDERS: 'ai:providers',
  AI_MODELS: 'ai:models',
  AI_STEP: 'ai:step',
  AI_STOP: 'ai:stop',
  AI_EVENT: 'ai:event',                     // main→renderer push (stream deltas)
  AI_PROVIDER_UPDATE: 'ai:provider-update',
  AI_PROVIDER_TEST: 'ai:provider-test',
  AI_HUB_SEARCH: 'ai-hub:search',
  AI_HUB_DETAIL: 'ai-hub:detail',
  AI_HUB_FIT: 'ai-hub:fit',
  AI_HUB_DOWNLOAD: 'ai-hub:download',
  AI_HUB_DOWNLOAD_URL: 'ai-hub:download-url',
  AI_HUB_CANCEL: 'ai-hub:cancel',
  AI_HUB_DOWNLOADS: 'ai-hub:downloads',
  AI_HUB_PROGRESS: 'ai-hub:download-progress', // main→renderer push
  AI_HUB_LOCAL: 'ai-hub:local',
  AI_HUB_DELETE: 'ai-hub:delete',
  AI_HUB_REVEAL: 'ai-hub:reveal',
  AI_HUB_IMPORT_OLLAMA: 'ai-hub:import-ollama',
  AI_HUB_CONFIG: 'ai-hub:config',
  AI_HUB_SET_CONFIG: 'ai-hub:set-config',

  // Debugging (docs/DEBUGGING.md)
  DEBUG_LOG: 'debug:log',           // renderer → main: forward an error/warning to renderer.log
  DEBUG_SNAPSHOT: 'debug:snapshot', // → diagnostics bundle (versions, redacted config, source health, recent errors)
} as const

export type IpcChannel = (typeof IPC)[keyof typeof IPC]
