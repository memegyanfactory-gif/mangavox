import { useCallback, useMemo } from 'react'
import type { ParsedInput } from '../../shared/agent-command-types'

// Client-side parsing (no IPC needed for better UX)
const COMMAND_REGEX = /^\/([a-z-]+)\s*(.*)/i

export function useCommandParser() {
  const parseInput = useCallback((input: string): ParsedInput => {
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

    const validCommands = [
      'play', 'download', 'yt', 'youtube', 'search', 'history', 'memory', 'cancel', 'settings', 'help',
      'recommend', 'continue', 'trending', 'random', 'mood', 'skill', 'skills', 'deepresearch', 'narrate', 'complete',
    ]
    if (!validCommands.includes(cmdLower)) {
      return {
        hasCommand: false,
        command: null,
        text: trimmed,
      }
    }

    const normalizedCmd = cmdLower === 'youtube' ? 'yt' : cmdLower
    
    return {
      hasCommand: true,
      command: {
        type: 'command',
        command: normalizedCmd as any,
        args,
        raw: trimmed,
      },
      text: argsStr.trim(),
    }
  }, [])

  const getSuggestions = useCallback((input: string): string[] => {
    const trimmed = input.trim()
    
    if (trimmed === '/') {
      return [
        '/play', '/download', '/yt', '/search', '/skill', '/deepresearch', '/narrate', '/complete', '/recommend', '/continue', '/trending',
        '/random', '/mood', '/memory', '/history', '/help', '/settings', '/cancel',
      ]
    }

    const match = trimmed.match(/^\/([a-z-]*)/i)
    if (!match) return []

    const prefix = match[1].toLowerCase()
    const commands = [
      'play', 'download', 'yt', 'youtube', 'search', 'history', 'memory', 'cancel', 'settings', 'help',
      'recommend', 'continue', 'trending', 'random', 'mood', 'skill', 'skills', 'deepresearch', 'narrate', 'complete',
    ]
    return commands
      .filter((cmd) => cmd.startsWith(prefix))
      .map((cmd) => `/${cmd}`)
  }, [])

  const getContextualTags = useCallback((command: string, input: string): string[] => {
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
  }, [])

  const getRecommendations = useCallback((input: string, hasHistory: boolean, userMemory: any): string[] => {
    const recommendations: string[] = []
    
    const lowerInput = input.toLowerCase()
    
    if (lowerInput.includes('how') || lowerInput.includes('what') || lowerInput.includes('can')) {
      recommendations.push('Try /play to search torrents')
      recommendations.push('Use /help to see all commands')
    }
    
    if (hasHistory) {
      recommendations.push('View search history with /history')
      recommendations.push('Check your preferences with /memory')
    }
    
    if (userMemory?.preferences?.preferredGenre) {
      recommendations.push(`/play ${userMemory.preferences.preferredGenre}`)
    }
    
    if (recommendations.length === 0) {
      recommendations.push('/play Search for movies & anime')
      recommendations.push('/download Download from URLs')
      recommendations.push('/search Find content info')
    }
    
    return recommendations.slice(0, 3)
  }, [])

  return { parseInput, getSuggestions, getContextualTags, getRecommendations }
}
