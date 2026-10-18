import { create } from 'zustand'

export type LogLevel = 'info' | 'warn' | 'error' | 'debug'

export interface LogEntry {
  id: number
  level: LogLevel
  message: string
  timestamp: string
  source: string
}

export interface AgentMessage {
  id: number
  role: 'user' | 'agent' | 'command' | 'output' | 'fix-status'
  text: string
  timestamp: string
  isRunning?: boolean
  fixPhase?: string
  action?: any
}

export type PermissionMode = 'auto' | 'manual' | 'decline'

const MAX_ENTRIES = 500

type PanelView = 'logs' | 'agent'

interface LoggingState {
  entries: LogEntry[]
  isOpen: boolean
  filter: LogLevel | 'all'
  unreadErrors: number
  panelView: PanelView

  // Agent chat
  agentMessages: AgentMessage[]
  agentLoading: boolean

  // Auto-fix state
  permissionMode: PermissionMode
  fixInProgress: boolean
  pendingAction: any | null

  addEntry: (entry: LogEntry) => void
  addRendererLog: (level: LogLevel, message: string) => void
  togglePanel: () => void
  setOpen: (open: boolean) => void
  setFilter: (filter: LogLevel | 'all') => void
  clearLogs: () => void
  setPanelView: (view: PanelView) => void

  // Agent actions
  addAgentMessage: (msg: AgentMessage) => void
  updateLastAgentMessage: (text: string) => void
  setAgentLoading: (loading: boolean) => void
  clearAgentChat: () => void

  // Auto-fix actions
  setPermissionMode: (mode: PermissionMode) => void
  setFixInProgress: (inProgress: boolean) => void
  setPendingAction: (action: any | null) => void
  clearFixedErrors: () => void
}

let rendererLogId = 100_000
let agentMsgId = 0

export const useLoggingStore = create<LoggingState>((set, get) => ({
  entries: [],
  isOpen: false,
  filter: 'all',
  unreadErrors: 0,
  panelView: 'logs',

  agentMessages: [],
  agentLoading: false,

  permissionMode: 'manual',
  fixInProgress: false,
  pendingAction: null,

  addEntry: (entry) => {
    set((state) => ({
      entries: [...state.entries.slice(-(MAX_ENTRIES - 1)), entry],
      unreadErrors: !state.isOpen && entry.level === 'error'
        ? state.unreadErrors + 1
        : state.unreadErrors,
    }))
  },

  addRendererLog: (level, message) => {
    const entry: LogEntry = {
      id: ++rendererLogId,
      level,
      message,
      timestamp: new Date().toISOString(),
      source: 'renderer',
    }
    get().addEntry(entry)
  },

  togglePanel: () => set((state) => ({
    isOpen: !state.isOpen,
    unreadErrors: !state.isOpen ? 0 : state.unreadErrors,
  })),

  setOpen: (open) => set({
    isOpen: open,
    unreadErrors: open ? 0 : get().unreadErrors,
  }),

  setFilter: (filter) => set({ filter }),

  clearLogs: () => set({ entries: [], unreadErrors: 0 }),

  setPanelView: (panelView) => set({ panelView, isOpen: true }),

  // Agent
  addAgentMessage: (msg) => {
    msg.id = ++agentMsgId
    set((state) => ({ agentMessages: [...state.agentMessages, msg] }))
  },

  updateLastAgentMessage: (text) => {
    set((state) => {
      const msgs = [...state.agentMessages]
      if (msgs.length > 0) {
        msgs[msgs.length - 1] = { ...msgs[msgs.length - 1], text: msgs[msgs.length - 1].text + text }
      }
      return { agentMessages: msgs }
    })
  },

  setAgentLoading: (agentLoading) => set({ agentLoading }),

  clearAgentChat: () => set({ agentMessages: [], agentLoading: false, fixInProgress: false, pendingAction: null }),

  // Auto-fix
  setPermissionMode: (permissionMode) => set({ permissionMode }),
  setFixInProgress: (fixInProgress) => set({ fixInProgress }),
  setPendingAction: (pendingAction) => set({ pendingAction }),

  clearFixedErrors: () => {
    // Remove error entries that were fixed
    set((state) => ({
      entries: state.entries.filter((e) => e.level !== 'error'),
      unreadErrors: 0,
    }))
  },
}))
