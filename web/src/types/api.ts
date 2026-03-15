import type {
    DecryptedMessage as ProtocolDecryptedMessage,
    KanbanConfig as ProtocolKanbanConfig,
    Session,
    SessionSummary,
    SyncEvent as ProtocolSyncEvent,
    WorktreeMetadata
} from '@codex/protocol/types'

export type {
    AgentState,
    AttachmentMetadata,
    ModelMode,
    PermissionMode,
    Session,
    SessionSummary,
    SessionSummaryMetadata,
    TodoItem,
    WorktreeMetadata
} from '@codex/protocol/types'

export type SessionMetadataSummary = {
    path: string
    host: string
    version?: string
    name?: string
    os?: string
    summary?: { text: string; updatedAt: number }
    machineId?: string
    tools?: string[]
    flavor?: string | null
    worktree?: WorktreeMetadata
}

export type MessageStatus = 'sending' | 'sent' | 'failed'

export type DecryptedMessage = ProtocolDecryptedMessage & {
    status?: MessageStatus
    originalText?: string
}

export type ReasoningEffort = 'none' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh'

export type ReasoningEffortPreset = {
    effort: ReasoningEffort
    description: string
}

export type ModelCatalogModel = {
    id: string
    displayName: string
    description: string
    isDefault: boolean
    showInPicker: boolean
    defaultReasoningEffort: ReasoningEffort
    supportedReasoningEfforts: ReasoningEffortPreset[]
}

export type ModelsCatalogResponse = {
    models: ModelCatalogModel[]
}

export type Machine = {
    id: string
    active: boolean
    metadata: {
        host: string
        platform: string
        happyCliVersion: string
        displayName?: string
    } | null
}

export type AuthResponse = {
    token: string
    user: {
        id: number
        username?: string
        firstName?: string
        lastName?: string
    }
}

export type SessionsResponse = { sessions: SessionSummary[] }
export type SessionResponse = { session: Session }
export type MessagesResponse = {
    messages: DecryptedMessage[]
    page: {
        limit: number
        beforeSeq: number | null
        nextBeforeSeq: number | null
        hasMore: boolean
    }
}

export type MachinesResponse = { machines: Machine[] }
export type MachinePathsExistsResponse = { exists: Record<string, boolean> }

export type SpawnResponse =
    | { type: 'success'; sessionId: string }
    | { type: 'error'; message: string }

export type GitCommandResponse = {
    success: boolean
    stdout?: string
    stderr?: string
    exitCode?: number
    error?: string
}

export type FileSearchItem = {
    fileName: string
    filePath: string
    fullPath: string
    fileType: 'file' | 'folder'
}

export type FileSearchResponse = {
    success: boolean
    files?: FileSearchItem[]
    error?: string
}

export type DirectoryEntry = {
    name: string
    type: 'file' | 'directory' | 'other'
    size?: number
    modified?: number
}

export type ListDirectoryResponse = {
    success: boolean
    entries?: DirectoryEntry[]
    error?: string
}

export type FileReadResponse = {
    success: boolean
    content?: string
    error?: string
}

export type UploadFileResponse = {
    success: boolean
    path?: string
    error?: string
}

export type DeleteUploadResponse = {
    success: boolean
    error?: string
}

export type GitFileStatus = {
    fileName: string
    filePath: string
    fullPath: string
    status: 'modified' | 'added' | 'deleted' | 'renamed' | 'untracked' | 'conflicted'
    isStaged: boolean
    linesAdded: number
    linesRemoved: number
    oldPath?: string
}

export type GitStatusFiles = {
    stagedFiles: GitFileStatus[]
    unstagedFiles: GitFileStatus[]
    branch: string | null
    totalStaged: number
    totalUnstaged: number
}

export type SlashCommand = {
    name: string
    description?: string
    source: 'builtin' | 'user' | 'plugin'
    content?: string  // Expanded content for Codex user prompts
    pluginName?: string
}

export type SlashCommandsResponse = {
    success: boolean
    commands?: SlashCommand[]
    error?: string
}

export type SkillSummary = {
    name: string
    description?: string
}

export type SkillsResponse = {
    success: boolean
    skills?: SkillSummary[]
    error?: string
}

export type PushSubscriptionKeys = {
    p256dh: string
    auth: string
}

export type PushSubscriptionPayload = {
    endpoint: string
    keys: PushSubscriptionKeys
}

export type PushUnsubscribePayload = {
    endpoint: string
}

export type PushVapidPublicKeyResponse = {
    publicKey: string
}

export type VisibilityPayload = {
    subscriptionId: string
    visibility: 'visible' | 'hidden'
}

export type SyncEvent = ProtocolSyncEvent

export type KanbanConfig = ProtocolKanbanConfig

export type WorkspaceSummary = {
    id: string
    name: string
    repoCount: number
}

export type WorkspaceRepoRefInput = {
    fullName: string
    color?: string | null
    shortLabel?: string | null
    defaultBranch?: string | null
}

export type WorkspaceRepoRef = {
    fullName: string
    color: string
    shortLabel: string
    defaultBranch: string
}

export type WorkspaceBoardColumn = {
    id: string
    name: string
    position: number
    autoTrigger?: 'startExecution' | 'closeIssue' | null
}

export type WorkspaceIssueRef = {
    repo: string
    number: number
}

export type WorkspaceBoardFilters = {
    repos?: string[] | null
    epics?: WorkspaceIssueRef[] | null
    labels?: string[] | null
    assignees?: string[] | null
}

export type WorkspaceBoardConfig = {
    columns: WorkspaceBoardColumn[]
    swimlaneMode: 'byEpic' | 'byRepo' | 'byAssignee' | 'none'
    wipLimits: Record<string, number>
    filters: WorkspaceBoardFilters
}

export type WorkspaceExecConfig = {
    model?: string | null
    reasoningEffort?: ReasoningEffort | null
    sandbox?: 'readOnly' | 'workspaceWrite' | 'fullAccess' | null
    systemPrompt?: string | null
    prompt?: string | null
    timeoutMinutes?: number | null
    autoPr?: boolean | null
    autoTest?: boolean | null
}

export type Workspace = {
    id: string
    name: string
    repos: WorkspaceRepoRef[]
    board: WorkspaceBoardConfig
    defaultExec: WorkspaceExecConfig
    createdAtMs: number
    updatedAtMs: number
}

export type CreateWorkspaceRequest = {
    name: string
    repos: WorkspaceRepoRefInput[]
    board?: WorkspaceBoardConfig | null
    defaultExec?: WorkspaceExecConfig | null
}

export type UpdateWorkspaceRequest = {
    name?: string | null
    repos?: WorkspaceRepoRefInput[] | null
    board?: WorkspaceBoardConfig | null
    defaultExec?: WorkspaceExecConfig | null
}

export type GithubLabel = {
    name: string
    color: string
}

export type GithubKanbanCardSettings = {
    promptPrefix?: string | null
    model?: string | null
    reasoningEffort?: ReasoningEffort | null
}

export type GithubKanbanConfig = {
    columns: ProtocolKanbanConfig['columns']
    cardPositions: ProtocolKanbanConfig['cardPositions']
    cardSettings: Record<string, GithubKanbanCardSettings>
}

export type GithubWorkItem = {
    workItemKey: string
    repo: string
    kind: 'issue' | 'pull' | string
    number: number
    title: string
    state: string
    url: string
    updatedAt: number
    labels: GithubLabel[]
    comments: number
}

export type GithubWorkItemsSnapshot = {
    fetchedAt: number
    items: GithubWorkItem[]
}

export type GithubJobStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'canceled' | string

export type GithubJob = {
    jobId: string
    workItemKey: string
    status: GithubJobStatus
    createdAt: number
    startedAt?: number | null
    finishedAt?: number | null
    lastError?: string | null
    resultSummary?: string | null
    threadId?: string | null
    logPath?: string | null
}

export type GithubJobsResponse = {
    jobs: GithubJob[]
}

export type GithubReposResponse = {
    repos: string[]
}

export type SetGithubReposRequest = {
    repos: string[]
}

export type GithubWorkItemDetail = {
    repo: string
    number: number
    title: string
    state: string
    url: string
    updatedAt: string
    body: string
}

export type UpdateGithubKanbanCardSettingsRequest = {
    workItemKey: string
    promptPrefix?: string
    model?: string
    reasoningEffort?: ReasoningEffort | null
}

export type CloseGithubWorkItemRequest = {
    workItemKey: string
}

export type GithubJobLogResponse = {
    jobId: string
    logText: string
    truncated: boolean
}

export type MoveKanbanCardRequest = {
    columnId: string
    position: number
}

export type MoveGithubKanbanCardRequest = {
    workItemKey: string
    columnId: string
    position: number
    promptPrefix?: string
    model?: string
    reasoningEffort?: ReasoningEffort | null
}

export type BatchMoveKanbanCardsRequest = {
    moves: Array<{
        sessionId: string
        columnId: string
        position: number
    }>
}
