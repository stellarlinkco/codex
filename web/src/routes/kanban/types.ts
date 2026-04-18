import type {
    GithubJob,
    GithubKanbanCardSettings,
    GithubWorkItem,
    ReasoningEffort,
    SessionSummary,
    Workspace,
    WorkspaceSummary,
} from '@/types/api'

export type KanbanScope = 'sessions' | 'github' | 'workspace'

export type GithubCardData = {
    kind: 'github'
    key: string
    item: GithubWorkItem
    latestJob: GithubJob | null
    settings: GithubKanbanCardSettings
}

export type SessionCardData = {
    kind: 'session'
    key: string
    session: SessionSummary
    title: string
    path: string
    agentLabel: string
}

export type CardData = GithubCardData | SessionCardData

export type ColumnData = {
    id: string
    name: string
    position: number
    cardKeys: string[]
}

export type KanbanState = {
    scope: KanbanScope
    selectedWorkspaceId: string | null
    selectedCardKey: string | null
    searchQuery: string
    repoFilter: string | null
    showWorkspaceDialog: boolean
    editingWorkspace: Workspace | null
    showLogPanel: boolean
    logJobId: string | null
}

export type ConfigOverride = {
    model?: string | null
    reasoningEffort?: ReasoningEffort | null
    promptPrefix?: string | null
}

export type WorkspaceFormData = {
    name: string
    repos: Array<{ fullName: string; color?: string; shortLabel?: string }>
}

export { type GithubJob, type GithubWorkItem, type SessionSummary, type Workspace, type WorkspaceSummary }
