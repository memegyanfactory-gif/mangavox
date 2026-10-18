/** Slash-command parse types — safe for renderer + main */

export interface AgentCommand {
  type: 'text' | 'command'
  command?: 'play' | 'download' | 'yt' | 'search' | 'history' | 'memory' | 'cancel' | 'settings' | 'help'
    | 'recommend' | 'continue' | 'trending' | 'random' | 'mood' | 'narrate' | 'complete'
  args: string[]
  raw: string
}

export interface ParsedInput {
  hasCommand: boolean
  command: AgentCommand | null
  text: string
}
