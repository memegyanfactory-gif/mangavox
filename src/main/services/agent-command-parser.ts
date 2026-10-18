/**
 * Parse agent commands from user input
 * Supports: /play, /download, /yt, /search, /history, /memory, /cancel, /settings
 */

import type { AgentCommand, ParsedInput } from '../../shared/agent-command-types'

export type { AgentCommand, ParsedInput } from '../../shared/agent-command-types'

const COMMAND_REGEX = /^\/([a-z-]+)\s*(.*)/i

export function parseAgentInput(input: string): ParsedInput {
  const trimmed = input.trim()
  const match = trimmed.match(COMMAND_REGEX)

  if (!match) {
    return {
      hasCommand: false,
      command: null,
      text: trimmed,
    }
  }

  const [, cmd, argsStr] = match
  const cmdLower = cmd.toLowerCase()
  const args = argsStr.trim() ? argsStr.split(/\s+/).filter(Boolean) : []

  // Validate command type
  const validCommands = [
    'play', 'download', 'yt', 'youtube', 'search', 'history', 'memory', 'cancel', 'settings', 'help',
    'recommend', 'continue', 'trending', 'random', 'mood',
  ]
  if (!validCommands.includes(cmdLower)) {
    return {
      hasCommand: false,
      command: null,
      text: trimmed,
    }
  }

  // Normalize youtube -> yt
  const normalizedCmd = cmdLower === 'youtube' ? 'yt' : cmdLower as AgentCommand['command']
  
  return {
    hasCommand: true,
    command: {
      type: 'command',
      command: normalizedCmd,
      args,
      raw: trimmed,
    },
    text: argsStr.trim(),
  }
}

export function getSuggestions(input: string): string[] {
  const trimmed = input.trim()
  
  if (trimmed === '/') {
    return [
      '/play', '/download', '/yt', '/search', '/recommend', '/continue', '/trending',
      '/random', '/mood', '/memory', '/history', '/help', '/settings', '/cancel',
    ]
  }

  const match = trimmed.match(/^\/([a-z-]*)/i)
  if (!match) return []

  const prefix = match[1].toLowerCase()
  const commands = [
    'play', 'download', 'yt', 'youtube', 'search', 'history', 'memory', 'cancel', 'settings', 'help',
    'recommend', 'continue', 'trending', 'random', 'mood',
  ]
  return commands
    .filter((cmd) => cmd.startsWith(prefix) || prefix.length === 0)
    .map((cmd) => `/${cmd}`)
}

export function getCommandHints(command: string): string {
  const hints: Record<string, string> = {
    'play': 'Search and play torrents for movies, series, anime, or Indian content. Shows quality, peers, and seeds.',
    'download': 'Download media from URLs with format and resolution options. Supports YouTube, Instagram, and more.',
    'yt': 'Quick YouTube download shorthand. Automatically selects best quality.',
    'youtube': 'Full YouTube download with complete format and resolution options.',
    'search': 'Search online for movie info, cast, plot, ratings, and recommendations.',
    'history': 'View your search history, downloads, and past interactions.',
    'memory': 'Manage your AI personality, preferences, watch history, and recommendations.',
    'cancel': 'Stop the current download or operation in progress.',
    'settings': 'Configure default download format, quality, save location, and other preferences.',
    'help': 'Show this comprehensive help panel with all available commands and usage examples.',
    'recommend': 'Get personalized picks from your watch history, favorites, and taste profile.',
    'continue': 'Jump back to what you were watching — manga, anime, or movies.',
    'trending': 'Surface what is hot right now across anime and streaming catalogs.',
    'random': 'Surprise me — pick something great you have not seen yet.',
    'mood': 'Tell me your vibe (cozy, action, cry, laugh) and I will match content.',
  }
  return hints[command.toLowerCase()] || 'Unknown command'
}

/** Extract tags/hints for /play and /download commands */
export function getContextualTags(command: string, input: string): string[] {
  const tags: string[] = []

  if (command === 'play') {
    tags.push('🎬 Movies', '📺 Series', '⚡ Anime', '🎭 Indian Content', '🌍 International')
    if (/anime/i.test(input)) tags.unshift('⚡ Anime')
    if (/movie|film/i.test(input)) tags.unshift('🎬 Movies')
    if (/series|show|tv/i.test(input)) tags.unshift('📺 Series')
    if (/indian|hindi|tamil|bollywood/i.test(input)) tags.unshift('🎭 Indian')
  } else if (command === 'download') {
    tags.push('📥 Direct Link', '📹 YouTube', '📸 Instagram', '🎵 TikTok', '🌐 Other')
    if (/youtube|youtu.be/i.test(input)) tags.unshift('📹 YouTube')
    if (/instagram|insta/i.test(input)) tags.unshift('📸 Instagram')
    if (/tiktok|tik.tok/i.test(input)) tags.unshift('🎵 TikTok')
  }

  return tags
}

/** Generate recommendations based on context and user message */
export function getRecommendations(input: string, hasHistory: boolean, userMemory: any): string[] {
  const recommendations: string[] = []
  
  const lowerInput = input.toLowerCase()
  
  // If user is asking about commands
  if (lowerInput.includes('how') || lowerInput.includes('what') || lowerInput.includes('can')) {
    recommendations.push('Try /play to search torrents')
    recommendations.push('Use /help to see all commands')
  }
  
  // If they searched before
  if (hasHistory) {
    recommendations.push('View search history with /history')
    recommendations.push('Check your preferences with /memory')
  }
  
  // If user memory exists with preferences
  if (userMemory?.preferences?.preferredGenre) {
    recommendations.push(`/play ${userMemory.preferences.preferredGenre}`)
  }
  
  // Generic recommendations
  if (recommendations.length === 0) {
    recommendations.push('/play Search for movies & anime')
    recommendations.push('/download Download from URLs')
    recommendations.push('/search Find content info')
  }
  
  return recommendations.slice(0, 3) // Limit to 3 recommendations
}

/** Extract format/resolution hints for download command */
export function getDownloadFormatHints(input: string): string[] {
  const formats: string[] = []
  
  if (/format|mp4|mp3|webm|mkv/i.test(input)) {
    formats.push('MP4', 'MP3', 'WebM', 'MKV', 'AVI')
  }
  
  if (/resolution|quality|1080|720|480|4k/i.test(input)) {
    formats.push('4K (2160p)', '1080p', '720p', '480p', 'Audio Only')
  }
  
  if (formats.length === 0) {
    formats.push('MP4', '1080p', 'MP3', '720p', '480p')
  }
  
  return formats
}
