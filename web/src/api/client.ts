import type {
    AttachmentMetadata,
    AuthResponse,
    BatchMoveKanbanCardsRequest,
    DeleteUploadResponse,
    ListDirectoryResponse,
    FileReadResponse,
    FileSearchResponse,
    GitCommandResponse,
    GithubJobsResponse,
    GithubJobLogResponse,
    GithubKanbanConfig,
    GithubReposResponse,
    GithubWorkItemDetail,
    GithubWorkItemsSnapshot,
    KanbanConfig,
    CloseGithubWorkItemRequest,
    CreateWorkspaceRequest,
    MachinePathsExistsResponse,
    MachinesResponse,
    MessagesResponse,
    MoveGithubKanbanCardRequest,
    MoveKanbanCardRequest,
    ModelMode,
    ModelsCatalogResponse,
    PermissionMode,
    PushSubscriptionPayload,
    PushUnsubscribePayload,
    PushVapidPublicKeyResponse,
    ReasoningEffort,
    SlashCommandsResponse,
    SkillsResponse,
    SpawnResponse,
    SetGithubReposRequest,
    UpdateWorkspaceRequest,
    UploadFileResponse,
    UpdateGithubKanbanCardSettingsRequest,
    VisibilityPayload,
    SessionResponse,
    SessionsResponse,
    Workspace,
    WorkspaceSummary
} from '@/types/api'

type ApiClientOptions = {
    baseUrl?: string
    getToken?: () => string | null
    onUnauthorized?: () => Promise<string | null>
}

type ErrorPayload = {
    error?: unknown
}

function parseErrorCode(bodyText: string): string | undefined {
    try {
        const parsed = JSON.parse(bodyText) as ErrorPayload
        return typeof parsed.error === 'string' ? parsed.error : undefined
    } catch {
        return undefined
    }
}

export class ApiError extends Error {
    status: number
    code?: string
    body?: string

    constructor(message: string, status: number, code?: string, body?: string) {
        super(message)
        this.name = 'ApiError'
        this.status = status
        this.code = code
        this.body = body
    }
}

export class ApiClient {
    private token: string
    private readonly baseUrl: string | null
    private readonly getToken: (() => string | null) | null
    private readonly onUnauthorized: (() => Promise<string | null>) | null

    constructor(token: string, options?: ApiClientOptions) {
        this.token = token
        this.baseUrl = options?.baseUrl ?? null
        this.getToken = options?.getToken ?? null
        this.onUnauthorized = options?.onUnauthorized ?? null
    }

    private buildUrl(path: string): string {
        if (!this.baseUrl) {
            return path
        }
        try {
            return new URL(path, this.baseUrl).toString()
        } catch {
            return path
        }
    }

    private async request<T>(
        path: string,
        init?: RequestInit,
        attempt: number = 0,
        overrideToken?: string | null
    ): Promise<T> {
        const headers = new Headers(init?.headers)
        const liveToken = this.getToken ? this.getToken() : null
        const authToken = overrideToken !== undefined
            ? (overrideToken ?? (liveToken ?? this.token))
            : (liveToken ?? this.token)
        if (authToken) {
            headers.set('authorization', `Bearer ${authToken}`)
        }
        if (init?.body !== undefined && !headers.has('content-type')) {
            headers.set('content-type', 'application/json')
        }

        const res = await fetch(this.buildUrl(path), {
            ...init,
            headers
        })

        if (res.status === 401) {
            if (attempt === 0 && this.onUnauthorized) {
                const refreshed = await this.onUnauthorized()
                if (refreshed) {
                    this.token = refreshed
                    return await this.request<T>(path, init, attempt + 1, refreshed)
                }
            }
            throw new Error('Session expired. Please sign in again.')
        }

        if (!res.ok) {
            const body = await res.text().catch(() => '')
            throw new Error(`HTTP ${res.status} ${res.statusText}: ${body}`)
        }

        return await res.json() as T
    }

    async authenticate(auth: { initData: string } | { accessToken: string }): Promise<AuthResponse> {
        const res = await fetch(this.buildUrl('/api/auth'), {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(auth)
        })

        if (!res.ok) {
            const body = await res.text().catch(() => '')
            const code = parseErrorCode(body)
            const detail = body ? `: ${body}` : ''
            throw new ApiError(`Auth failed: HTTP ${res.status} ${res.statusText}${detail}`, res.status, code, body || undefined)
        }

        return await res.json() as AuthResponse
    }

    async bind(auth: { initData: string; accessToken: string }): Promise<AuthResponse> {
        const res = await fetch(this.buildUrl('/api/bind'), {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(auth)
        })

        if (!res.ok) {
            const body = await res.text().catch(() => '')
            const code = parseErrorCode(body)
            const detail = body ? `: ${body}` : ''
            throw new ApiError(`Bind failed: HTTP ${res.status} ${res.statusText}${detail}`, res.status, code, body || undefined)
        }

        return await res.json() as AuthResponse
    }

    async getSessions(): Promise<SessionsResponse> {
        return await this.request<SessionsResponse>('/api/sessions')
    }

    async getKanban(): Promise<KanbanConfig> {
        return await this.request<KanbanConfig>('/api/kanban')
    }

    async getModelsCatalog(): Promise<ModelsCatalogResponse> {
        return await this.request<ModelsCatalogResponse>('/api/models/catalog')
    }

    async listWorkspaces(): Promise<WorkspaceSummary[]> {
        return await this.request<WorkspaceSummary[]>('/api/workspaces')
    }

    async createWorkspace(payload: CreateWorkspaceRequest): Promise<Workspace> {
        return await this.request<Workspace>('/api/workspaces', {
            method: 'POST',
            body: JSON.stringify(payload)
        })
    }

    async getWorkspace(workspaceId: string): Promise<Workspace> {
        return await this.request<Workspace>(`/api/workspaces/${encodeURIComponent(workspaceId)}`)
    }

    async updateWorkspace(workspaceId: string, payload: UpdateWorkspaceRequest): Promise<Workspace> {
        return await this.request<Workspace>(`/api/workspaces/${encodeURIComponent(workspaceId)}`, {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async deleteWorkspace(workspaceId: string): Promise<void> {
        await this.request<{}>(`/api/workspaces/${encodeURIComponent(workspaceId)}`, { method: 'DELETE' })
    }

    async syncWorkspace(workspaceId: string): Promise<void> {
        await this.request<{}>(`/api/workspaces/${encodeURIComponent(workspaceId)}/sync`, { method: 'POST' })
    }

    async getWorkspaceWorkItems(workspaceId: string): Promise<GithubWorkItemsSnapshot> {
        return await this.request<GithubWorkItemsSnapshot>(`/api/workspaces/${encodeURIComponent(workspaceId)}/work-items`)
    }

    async getWorkspaceKanban(workspaceId: string): Promise<GithubKanbanConfig> {
        return await this.request<GithubKanbanConfig>(`/api/workspaces/${encodeURIComponent(workspaceId)}/kanban`)
    }

    async moveWorkspaceKanbanCard(workspaceId: string, payload: MoveGithubKanbanCardRequest): Promise<void> {
        await this.request<{}>(`/api/workspaces/${encodeURIComponent(workspaceId)}/kanban/cards`, {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async updateWorkspaceKanbanCardSettings(workspaceId: string, payload: UpdateGithubKanbanCardSettingsRequest): Promise<void> {
        await this.request<{}>(`/api/workspaces/${encodeURIComponent(workspaceId)}/kanban/cards/settings`, {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async getWorkspaceJobs(workspaceId: string): Promise<GithubJobsResponse> {
        return await this.request<GithubJobsResponse>(`/api/workspaces/${encodeURIComponent(workspaceId)}/jobs`)
    }

    async getWorkspaceJobLog(workspaceId: string, jobId: string): Promise<GithubJobLogResponse> {
        return await this.request<GithubJobLogResponse>(
            `/api/workspaces/${encodeURIComponent(workspaceId)}/jobs/${encodeURIComponent(jobId)}/log`
        )
    }

    async getGithubWorkItems(): Promise<GithubWorkItemsSnapshot> {
        return await this.request<GithubWorkItemsSnapshot>('/api/github/work-items')
    }

    async getGithubRepos(): Promise<GithubReposResponse> {
        return await this.request<GithubReposResponse>('/api/github/repos')
    }

    async setGithubRepos(payload: SetGithubReposRequest): Promise<void> {
        await this.request<{}>('/api/github/repos', {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async syncGithubWorkItems(): Promise<void> {
        await this.request<{}>('/api/github/sync', { method: 'POST' })
    }

    async getGithubKanban(): Promise<GithubKanbanConfig> {
        return await this.request<GithubKanbanConfig>('/api/github/kanban')
    }

    async moveGithubKanbanCard(payload: MoveGithubKanbanCardRequest): Promise<void> {
        await this.request<{}>('/api/github/kanban/cards', {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async updateGithubKanbanCardSettings(payload: UpdateGithubKanbanCardSettingsRequest): Promise<void> {
        await this.request<{}>('/api/github/kanban/cards/settings', {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async getGithubWorkItemDetail(workItemKey: string): Promise<GithubWorkItemDetail> {
        const params = new URLSearchParams()
        params.set('workItemKey', workItemKey)
        return await this.request<GithubWorkItemDetail>(`/api/github/work-items/detail?${params.toString()}`)
    }

    async closeGithubWorkItem(payload: CloseGithubWorkItemRequest): Promise<void> {
        await this.request<{}>('/api/github/work-items/close', {
            method: 'POST',
            body: JSON.stringify(payload)
        })
    }

    async getGithubJobs(): Promise<GithubJobsResponse> {
        return await this.request<GithubJobsResponse>('/api/github/jobs')
    }

    async getGithubJobLog(jobId: string): Promise<GithubJobLogResponse> {
        return await this.request<GithubJobLogResponse>(`/api/github/jobs/${encodeURIComponent(jobId)}/log`)
    }

    async moveKanbanCard(sessionId: string, payload: MoveKanbanCardRequest): Promise<void> {
        await this.request(`/api/kanban/cards/${encodeURIComponent(sessionId)}`, {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async batchMoveKanbanCards(payload: BatchMoveKanbanCardsRequest): Promise<void> {
        await this.request('/api/kanban/cards/batch', {
            method: 'PUT',
            body: JSON.stringify(payload)
        })
    }

    async getPushVapidPublicKey(): Promise<PushVapidPublicKeyResponse> {
        return await this.request<PushVapidPublicKeyResponse>('/api/push/vapid-public-key')
    }

    async subscribePushNotifications(payload: PushSubscriptionPayload): Promise<void> {
        await this.request('/api/push/subscribe', {
            method: 'POST',
            body: JSON.stringify(payload)
        })
    }

    async unsubscribePushNotifications(payload: PushUnsubscribePayload): Promise<void> {
        await this.request('/api/push/subscribe', {
            method: 'DELETE',
            body: JSON.stringify(payload)
        })
    }

    async setVisibility(payload: VisibilityPayload): Promise<void> {
        await this.request('/api/visibility', {
            method: 'POST',
            body: JSON.stringify(payload)
        })
    }

    async getSession(sessionId: string): Promise<SessionResponse> {
        return await this.request<SessionResponse>(`/api/sessions/${encodeURIComponent(sessionId)}`)
    }

    async getMessages(sessionId: string, options: { beforeSeq?: number | null; limit?: number }): Promise<MessagesResponse> {
        const params = new URLSearchParams()
        if (options.beforeSeq !== undefined && options.beforeSeq !== null) {
            params.set('beforeSeq', `${options.beforeSeq}`)
        }
        if (options.limit !== undefined && options.limit !== null) {
            params.set('limit', `${options.limit}`)
        }

        const qs = params.toString()
        const url = `/api/sessions/${encodeURIComponent(sessionId)}/messages${qs ? `?${qs}` : ''}`
        return await this.request<MessagesResponse>(url)
    }

    async getGitStatus(sessionId: string): Promise<GitCommandResponse> {
        return await this.request<GitCommandResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/git-status`)
    }

    async getGitDiffNumstat(sessionId: string, staged: boolean): Promise<GitCommandResponse> {
        const params = new URLSearchParams()
        params.set('staged', staged ? 'true' : 'false')
        return await this.request<GitCommandResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/git-diff-numstat?${params.toString()}`)
    }

    async getGitDiffFile(sessionId: string, path: string, staged?: boolean): Promise<GitCommandResponse> {
        const params = new URLSearchParams()
        params.set('path', path)
        if (staged !== undefined) {
            params.set('staged', staged ? 'true' : 'false')
        }
        return await this.request<GitCommandResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/git-diff-file?${params.toString()}`)
    }

    async searchSessionFiles(sessionId: string, query: string, limit?: number): Promise<FileSearchResponse> {
        const params = new URLSearchParams()
        if (query) {
            params.set('query', query)
        }
        if (limit !== undefined) {
            params.set('limit', `${limit}`)
        }
        const qs = params.toString()
        return await this.request<FileSearchResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/files${qs ? `?${qs}` : ''}`)
    }

    async readSessionFile(sessionId: string, path: string): Promise<FileReadResponse> {
        const params = new URLSearchParams()
        params.set('path', path)
        return await this.request<FileReadResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/file?${params.toString()}`)
    }

    async listSessionDirectory(sessionId: string, path?: string): Promise<ListDirectoryResponse> {
        const params = new URLSearchParams()
        if (path) {
            params.set('path', path)
        }

        const qs = params.toString()
        return await this.request<ListDirectoryResponse>(
            `/api/sessions/${encodeURIComponent(sessionId)}/directory${qs ? `?${qs}` : ''}`
        )
    }

    async uploadFile(sessionId: string, filename: string, content: string, mimeType: string): Promise<UploadFileResponse> {
        return await this.request<UploadFileResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/upload`, {
            method: 'POST',
            body: JSON.stringify({ filename, content, mimeType })
        })
    }

    async deleteUploadFile(sessionId: string, path: string): Promise<DeleteUploadResponse> {
        return await this.request<DeleteUploadResponse>(`/api/sessions/${encodeURIComponent(sessionId)}/upload/delete`, {
            method: 'POST',
            body: JSON.stringify({ path })
        })
    }

    async resumeSession(sessionId: string): Promise<string> {
        const response = await this.request<{ sessionId: string }>(
            `/api/sessions/${encodeURIComponent(sessionId)}/resume`,
            { method: 'POST' }
        )
        return response.sessionId
    }

    async sendMessage(sessionId: string, text: string, localId?: string | null, attachments?: AttachmentMetadata[]): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, {
            method: 'POST',
            body: JSON.stringify({
                text,
                localId: localId ?? undefined,
                attachments: attachments ?? undefined
            })
        })
    }

    async abortSession(sessionId: string): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/abort`, {
            method: 'POST',
            body: JSON.stringify({})
        })
    }

    async archiveSession(sessionId: string): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/archive`, {
            method: 'POST',
            body: JSON.stringify({})
        })
    }

    async switchSession(sessionId: string): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/switch`, {
            method: 'POST',
            body: JSON.stringify({})
        })
    }

    async setPermissionMode(sessionId: string, mode: PermissionMode): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/permission-mode`, {
            method: 'POST',
            body: JSON.stringify({ mode })
        })
    }

    async setModelMode(sessionId: string, model: ModelMode): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/model`, {
            method: 'POST',
            body: JSON.stringify({ model })
        })
    }

    async approvePermission(
        sessionId: string,
        requestId: string,
        modeOrOptions?: 'default' | 'acceptEdits' | 'bypassPermissions' | 'plan' | {
            mode?: 'default' | 'acceptEdits' | 'bypassPermissions' | 'plan'
            allowTools?: string[]
            decision?: 'approved' | 'approved_for_session' | 'denied' | 'abort'
            answers?: Record<string, string[]> | Record<string, { answers: string[] }>
        }
    ): Promise<void> {
        const body = typeof modeOrOptions === 'string' || modeOrOptions === undefined
            ? { mode: modeOrOptions }
            : modeOrOptions
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/permissions/${encodeURIComponent(requestId)}/approve`, {
            method: 'POST',
            body: JSON.stringify(body)
        })
    }

    async denyPermission(
        sessionId: string,
        requestId: string,
        options?: {
            decision?: 'approved' | 'approved_for_session' | 'denied' | 'abort'
        }
    ): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}/permissions/${encodeURIComponent(requestId)}/deny`, {
            method: 'POST',
            body: JSON.stringify(options ?? {})
        })
    }

    async getMachines(): Promise<MachinesResponse> {
        return await this.request<MachinesResponse>('/api/machines')
    }

    async checkMachinePathsExists(
        machineId: string,
        paths: string[]
    ): Promise<MachinePathsExistsResponse> {
        return await this.request<MachinePathsExistsResponse>(
            `/api/machines/${encodeURIComponent(machineId)}/paths/exists`,
            {
                method: 'POST',
                body: JSON.stringify({ paths })
            }
        )
    }

    async spawnSession(
        machineId: string,
        directory: string,
        agent?: 'claude' | 'codex' | 'gemini' | 'opencode',
        model?: string,
        yolo?: boolean,
        sessionType?: 'simple' | 'worktree',
        worktreeName?: string,
        reasoningEffort?: ReasoningEffort
    ): Promise<SpawnResponse> {
        return await this.request<SpawnResponse>(`/api/machines/${encodeURIComponent(machineId)}/spawn`, {
            method: 'POST',
            body: JSON.stringify({ directory, agent, model, yolo, sessionType, worktreeName, reasoningEffort })
        })
    }

    async getSlashCommands(sessionId: string): Promise<SlashCommandsResponse> {
        return await this.request<SlashCommandsResponse>(
            `/api/sessions/${encodeURIComponent(sessionId)}/slash-commands`
        )
    }

    async getSkills(sessionId: string): Promise<SkillsResponse> {
        return await this.request<SkillsResponse>(
            `/api/sessions/${encodeURIComponent(sessionId)}/skills`
        )
    }

    async renameSession(sessionId: string, name: string): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}`, {
            method: 'PATCH',
            body: JSON.stringify({ name })
        })
    }

    async deleteSession(sessionId: string): Promise<void> {
        await this.request(`/api/sessions/${encodeURIComponent(sessionId)}`, {
            method: 'DELETE'
        })
    }

    async fetchVoiceToken(options?: { customAgentId?: string; customApiKey?: string }): Promise<{
        allowed: boolean
        token?: string
        agentId?: string
        error?: string
    }> {
        return await this.request('/api/voice/token', {
            method: 'POST',
            body: JSON.stringify(options || {})
        })
    }
}
