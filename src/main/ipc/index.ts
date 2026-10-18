import { registerLibraryHandlers } from './library.ipc'
import { registerPluginHandlers } from './plugin.ipc'
import { registerRepoHandlers } from './repo.ipc'
import { registerDownloadHandlers } from './download.ipc'
import { registerPipelineHandlers } from './pipeline.ipc'
import { registerCacheHandlers } from './cache.ipc'
import { registerConfigHandlers } from './config.ipc'
import { registerPlayerHandlers } from './player.ipc'
import { registerTextHandlers } from './text.ipc'
import { registerYoutubeHandlers } from './youtube.ipc'
import { registerAddonHandlers } from './addon.ipc'
import { registerTorrentHandlers } from './torrent.ipc'
import { registerAgentHandlers } from './agent.ipc'
import { registerSubtitleHandlers } from './subtitle.ipc'
import { registerPrivacyHandlers } from './privacy.ipc'
import { registerCropImportHandlers } from './crop-import.ipc'
import { registerReaderOverlayHandlers } from './reader-overlay.ipc'
import { registerCharacterHandlers } from './character.ipc'
import { registerTtsHandlers } from './tts.ipc'
import { registerMediaChatHandlers } from './media-chat.ipc'
import { registerModelHandlers } from './models.ipc'
import { registerMediaHandlers } from './media.ipc'
import { registerPlaybackSessionHandlers } from './playback-session.ipc'
import { registerHeroHandlers } from './hero.ipc'
import { registerSkillHandlers } from './skills.ipc'
import { registerWatchlistHandlers } from './watchlist.ipc'
import { registerMangaRecommendationHandlers } from './manga-recs.ipc'
import { registerStorageHandlers } from './storage.ipc'
import { registerSkipTimesHandlers } from '../services/skip-times'
import { registerUpdateHandlers } from '../services/app-updater'

import { registerCategoryHandlers } from './category.ipc'
import { registerAiHandlers } from './ai.ipc'
import { registerSourceAccessHandlers } from './source-access.ipc'

export function registerAllHandlers(): void {
  registerLibraryHandlers()
  registerCategoryHandlers()
  registerPluginHandlers()
  registerRepoHandlers()
  registerDownloadHandlers()
  registerPipelineHandlers()
  registerCacheHandlers()
  registerConfigHandlers()
  registerPlayerHandlers()
  registerTextHandlers()
  registerYoutubeHandlers()
  registerAddonHandlers()
  registerTorrentHandlers()
  registerAgentHandlers()
  registerSubtitleHandlers()
  registerPrivacyHandlers()
  registerCropImportHandlers()
  registerReaderOverlayHandlers()
  registerCharacterHandlers()
  registerTtsHandlers()
  registerMediaChatHandlers()
  registerModelHandlers()
  registerMediaHandlers()
  registerPlaybackSessionHandlers()
  registerHeroHandlers()
  registerSkillHandlers()
  registerWatchlistHandlers()
  registerMangaRecommendationHandlers()
  registerStorageHandlers()
  registerSkipTimesHandlers()
  registerSourceAccessHandlers()
  registerAiHandlers()
  registerUpdateHandlers()
  import('./source-health.ipc').then((m) => m.registerSourceHealthHandlers()).catch((e) => console.error('[source-health] register failed:', e))
}
