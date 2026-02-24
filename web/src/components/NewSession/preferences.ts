import type { AgentType } from './types'
import type { ReasoningEffort } from '@/types/api'

const AGENT_STORAGE_KEY = 'codex:newSession:agent'
const YOLO_STORAGE_KEY = 'codex:newSession:yolo'
const REASONING_EFFORT_STORAGE_KEY = 'codex:newSession:reasoningEffort'

const VALID_AGENTS: AgentType[] = ['codex']
const VALID_REASONING_EFFORTS: Array<ReasoningEffort | 'auto'> = [
    'auto',
    'none',
    'minimal',
    'low',
    'medium',
    'high',
    'xhigh',
]

export function loadPreferredAgent(): AgentType {
    try {
        const stored = localStorage.getItem(AGENT_STORAGE_KEY)
        if (stored && VALID_AGENTS.includes(stored as AgentType)) {
            return stored as AgentType
        }
    } catch {
        // Ignore storage errors
    }
    return 'codex'
}

export function savePreferredAgent(agent: AgentType): void {
    try {
        localStorage.setItem(AGENT_STORAGE_KEY, agent)
    } catch {
        // Ignore storage errors
    }
}

export function loadPreferredYoloMode(): boolean {
    try {
        return localStorage.getItem(YOLO_STORAGE_KEY) === 'true'
    } catch {
        return false
    }
}

export function savePreferredYoloMode(enabled: boolean): void {
    try {
        localStorage.setItem(YOLO_STORAGE_KEY, enabled ? 'true' : 'false')
    } catch {
        // Ignore storage errors
    }
}

export function loadPreferredReasoningEffort(): ReasoningEffort | 'auto' {
    try {
        const stored = localStorage.getItem(REASONING_EFFORT_STORAGE_KEY)
        if (stored && VALID_REASONING_EFFORTS.includes(stored as ReasoningEffort | 'auto')) {
            return stored as ReasoningEffort | 'auto'
        }
    } catch {
        // Ignore storage errors
    }
    return 'auto'
}

export function savePreferredReasoningEffort(effort: ReasoningEffort | 'auto'): void {
    try {
        localStorage.setItem(REASONING_EFFORT_STORAGE_KEY, effort)
    } catch {
        // Ignore storage errors
    }
}
