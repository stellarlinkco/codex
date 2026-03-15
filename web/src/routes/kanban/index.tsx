import { useCallback, useDeferredValue, useMemo, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { LoadingState } from '@/components/LoadingState'
import { useGithubJobs } from '@/hooks/queries/useGithubJobs'
import { useGithubKanban } from '@/hooks/queries/useGithubKanban'
import { useGithubJobLog } from '@/hooks/queries/useGithubJobLog'
import { useModelsCatalog } from '@/hooks/queries/useModelsCatalog'
import { useGithubRepos } from '@/hooks/queries/useGithubRepos'
import { useGithubWorkItems } from '@/hooks/queries/useGithubWorkItems'
import { useGithubWorkItemDetail } from '@/hooks/queries/useGithubWorkItemDetail'
import { useWorkspaces } from '@/hooks/queries/useWorkspaces'
import { useWorkspaceKanban } from '@/hooks/queries/useWorkspaceKanban'
import { useWorkspaceWorkItems } from '@/hooks/queries/useWorkspaceWorkItems'
import { useWorkspaceJobs } from '@/hooks/queries/useWorkspaceJobs'
import { useWorkspaceJobLog } from '@/hooks/queries/useWorkspaceJobLog'
import { useWorkspace } from '@/hooks/queries/useWorkspace'
import { useAppContext } from '@/lib/app-context'
import { useToast } from '@/lib/toast-context'
import { queryKeys } from '@/lib/query-keys'
import { KanbanHeader } from './KanbanHeader'
import { KanbanBoard } from './KanbanBoard'
import { CardDetailPanel } from './CardDetailPanel'
import { WorkspaceDialog } from './WorkspaceDialog'
import { JobLogViewer } from './JobLogViewer'
import type { CardData, ColumnData, KanbanScope, WorkspaceFormData } from './types'
import type { GithubJob, GithubKanbanCardSettings, GithubWorkItem, ReasoningEffort, Workspace } from '@/types/api'

export function KanbanPage() {
    const { api } = useAppContext()
    const navigate = useNavigate()
    const queryClient = useQueryClient()
    const { addToast } = useToast()

    // State
    const [scope, setScope] = useState<KanbanScope>('github')
    const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null)
    const [selectedCardKey, setSelectedCardKey] = useState<string | null>(null)
    const [searchQuery, setSearchQuery] = useState('')
    const [repoFilter, setRepoFilter] = useState<string | null>(null)
    const [activeCardKey, setActiveCardKey] = useState<string | null>(null)
    const [showWorkspaceDialog, setShowWorkspaceDialog] = useState(false)
    const [editingWorkspace, setEditingWorkspace] = useState<Workspace | null>(null)
    const [logJobId, setLogJobId] = useState<string | null>(null)
    const [syncing, setSyncing] = useState(false)

    const deferredSearch = useDeferredValue(searchQuery)

    // Detect GitHub webhook availability (always fetch repos to know)
    const { data: githubReposData } = useGithubRepos(api)
    const hasGithubWebhook = (githubReposData?.repos?.length ?? 0) > 0 || scope === 'github'

    // Data queries - GitHub scope
    const githubEnabled = scope === 'github'
    const { data: githubWorkItems } = useGithubWorkItems(githubEnabled ? api : null)
    const { data: githubKanban, refetch: refetchGithubKanban } = useGithubKanban(githubEnabled ? api : null)
    const { data: githubJobsData } = useGithubJobs(githubEnabled ? api : null)

    // Data queries - Workspace scope
    const workspaceEnabled = scope === 'workspace'
    const { data: workspaces = [], refetch: refetchWorkspaces } = useWorkspaces(workspaceEnabled ? api : null)
    const { data: workspace } = useWorkspace(workspaceEnabled && selectedWorkspaceId ? api : null, selectedWorkspaceId || '')
    const { data: wsWorkItems } = useWorkspaceWorkItems(workspaceEnabled && selectedWorkspaceId ? api : null, selectedWorkspaceId || '')
    const { data: wsKanban, refetch: refetchWsKanban } = useWorkspaceKanban(workspaceEnabled && selectedWorkspaceId ? api : null, selectedWorkspaceId || '')
    const { data: wsJobsData } = useWorkspaceJobs(workspaceEnabled && selectedWorkspaceId ? api : null, selectedWorkspaceId || '')

    // Shared queries
    const { data: modelsCatalog } = useModelsCatalog(api)
    const models = modelsCatalog?.models ?? []

    // Selected card detail (uses github endpoint for all scopes - it fetches from GitHub API directly)
    const detailEnabled = Boolean(selectedCardKey) && (scope === 'github' || (scope === 'workspace' && hasGithubWebhook))
    const { data: itemDetail, isLoading: detailLoading } = useGithubWorkItemDetail(
        detailEnabled && api ? api : null,
        selectedCardKey
    )

    // Log query
    const { data: githubLogData, refetch: refetchGithubLog } = useGithubJobLog(
        githubEnabled && logJobId ? api : null,
        logJobId || ''
    )
    const { data: wsLogData, refetch: refetchWsLog } = useWorkspaceJobLog(
        workspaceEnabled && selectedWorkspaceId && logJobId ? api : null,
        selectedWorkspaceId || '',
        logJobId || ''
    )
    const logData = githubEnabled ? githubLogData : wsLogData

    // Resolved data based on scope
    const workItems: GithubWorkItem[] = useMemo(() => {
        if (scope === 'github') return githubWorkItems?.items ?? []
        if (scope === 'workspace') return wsWorkItems?.items ?? []
        return []
    }, [scope, githubWorkItems, wsWorkItems])

    const kanbanConfig = useMemo(() => {
        if (scope === 'github') return githubKanban
        if (scope === 'workspace') return wsKanban
        return null
    }, [scope, githubKanban, wsKanban])

    const jobs: GithubJob[] = useMemo(() => {
        if (scope === 'github') return githubJobsData?.jobs ?? []
        if (scope === 'workspace') return wsJobsData?.jobs ?? []
        return []
    }, [scope, githubJobsData, wsJobsData])

    const repos: string[] = useMemo(() => {
        if (scope === 'github') return githubReposData?.repos ?? []
        if (scope === 'workspace') return workspace?.repos.map(r => r.fullName) ?? []
        return []
    }, [scope, githubReposData?.repos, workspace?.repos])

    // Repo color/label maps (workspace repos have colors)
    const repoColors = useMemo(() => {
        const map = new Map<string, string>()
        if (workspace?.repos) {
            for (const r of workspace.repos) {
                map.set(r.fullName, r.color)
            }
        }
        return map
    }, [workspace])

    const repoLabels = useMemo(() => {
        const map = new Map<string, string>()
        if (workspace?.repos) {
            for (const r of workspace.repos) {
                map.set(r.fullName, r.shortLabel)
            }
        }
        return map
    }, [workspace])

    // Build card data map
    const cardsByKey = useMemo(() => {
        const map = new Map<string, CardData>()
        const jobsByItem = new Map<string, GithubJob>()
        for (const job of jobs) {
            const existing = jobsByItem.get(job.workItemKey)
            if (!existing || job.createdAt > existing.createdAt) {
                jobsByItem.set(job.workItemKey, job)
            }
        }

        const settings = kanbanConfig?.cardSettings ?? {}
        for (const item of workItems) {
            map.set(item.workItemKey, {
                key: item.workItemKey,
                item,
                latestJob: jobsByItem.get(item.workItemKey) ?? null,
                settings: settings[item.workItemKey] ?? {},
            })
        }
        return map
    }, [workItems, jobs, kanbanConfig])

    // Build columns with card keys
    const columns: ColumnData[] = useMemo(() => {
        if (!kanbanConfig) return []

        const cols = [...kanbanConfig.columns].sort((a, b) => a.position - b.position)
        const positions = kanbanConfig.cardPositions ?? {}
        const firstColId = cols[0]?.id ?? ''

        // Filter matching search/repo
        const matchingKeys = new Set<string>()
        for (const [key, card] of cardsByKey) {
            const matchSearch = !deferredSearch
                || card.item.title.toLowerCase().includes(deferredSearch.toLowerCase())
                || card.item.repo.toLowerCase().includes(deferredSearch.toLowerCase())
                || `#${card.item.number}`.includes(deferredSearch)
            const matchRepo = !repoFilter || card.item.repo === repoFilter
            if (matchSearch && matchRepo) {
                matchingKeys.add(key)
            }
        }

        return cols.map(col => {
            const cardsInCol: Array<{ key: string; position: number }> = []
            for (const [key, pos] of Object.entries(positions)) {
                if (pos.columnId === col.id && matchingKeys.has(key)) {
                    cardsInCol.push({ key, position: pos.position })
                }
            }
            // Add unpositioned items to first column
            if (col.id === firstColId) {
                for (const key of matchingKeys) {
                    if (!positions[key]) {
                        cardsInCol.push({ key, position: cardsInCol.length + 1000 })
                    }
                }
            }
            cardsInCol.sort((a, b) => a.position - b.position)
            return {
                id: col.id,
                name: col.name,
                position: col.position,
                cardKeys: cardsInCol.map(c => c.key),
            }
        })
    }, [kanbanConfig, cardsByKey, deferredSearch, repoFilter])

    // Handlers
    const handleSync = useCallback(async () => {
        if (!api) return
        setSyncing(true)
        try {
            if (scope === 'github') {
                await api.syncGithubWorkItems()
                void queryClient.invalidateQueries({ queryKey: queryKeys.githubWorkItems })
                void queryClient.invalidateQueries({ queryKey: queryKeys.githubKanban })
            } else if (scope === 'workspace' && selectedWorkspaceId) {
                await api.syncWorkspace(selectedWorkspaceId)
                void queryClient.invalidateQueries({ queryKey: queryKeys.workspaceWorkItems(selectedWorkspaceId) })
                void queryClient.invalidateQueries({ queryKey: queryKeys.workspaceKanban(selectedWorkspaceId) })
            }
        } catch {
            addToast({ title: 'Sync failed', body: 'Could not sync items', sessionId: '', url: '' })
        } finally {
            setSyncing(false)
        }
    }, [api, scope, selectedWorkspaceId, queryClient, addToast])

    const handleDragStart = useCallback((key: string) => {
        setActiveCardKey(key)
    }, [])

    const handleDragEnd = useCallback(async (cardKey: string, columnId: string, position: number) => {
        setActiveCardKey(null)
        if (!api) return

        const card = cardsByKey.get(cardKey)
        try {
            if (scope === 'github') {
                await api.moveGithubKanbanCard({
                    workItemKey: cardKey,
                    columnId,
                    position,
                    promptPrefix: card?.settings.promptPrefix ?? undefined,
                    model: card?.settings.model ?? undefined,
                    reasoningEffort: card?.settings.reasoningEffort ?? undefined,
                })
                void refetchGithubKanban()
                void queryClient.invalidateQueries({ queryKey: queryKeys.githubJobs })
            } else if (scope === 'workspace' && selectedWorkspaceId) {
                await api.moveWorkspaceKanbanCard(selectedWorkspaceId, {
                    workItemKey: cardKey,
                    columnId,
                    position,
                    promptPrefix: card?.settings.promptPrefix ?? undefined,
                    model: card?.settings.model ?? undefined,
                    reasoningEffort: card?.settings.reasoningEffort ?? undefined,
                })
                void refetchWsKanban()
                void queryClient.invalidateQueries({ queryKey: queryKeys.workspaceJobs(selectedWorkspaceId) })
            }
        } catch {
            addToast({ title: 'Move failed', body: 'Could not move card', sessionId: '', url: '' })
        }
    }, [api, scope, selectedWorkspaceId, cardsByKey, queryClient, refetchGithubKanban, refetchWsKanban, addToast])

    const handleDragCancel = useCallback(() => {
        setActiveCardKey(null)
    }, [])

    const handleUpdateSettings = useCallback(async (key: string, settings: { promptPrefix?: string; model?: string; reasoningEffort?: ReasoningEffort | null }) => {
        if (!api) return
        try {
            if (scope === 'github') {
                await api.updateGithubKanbanCardSettings({
                    workItemKey: key,
                    promptPrefix: settings.promptPrefix,
                    model: settings.model,
                    reasoningEffort: settings.reasoningEffort,
                })
                void refetchGithubKanban()
            } else if (scope === 'workspace' && selectedWorkspaceId) {
                await api.updateWorkspaceKanbanCardSettings(selectedWorkspaceId, {
                    workItemKey: key,
                    promptPrefix: settings.promptPrefix,
                    model: settings.model,
                    reasoningEffort: settings.reasoningEffort,
                })
                void refetchWsKanban()
            }
            addToast({ title: 'Settings saved', body: '', sessionId: '', url: '' })
        } catch {
            addToast({ title: 'Save failed', body: 'Could not save settings', sessionId: '', url: '' })
        }
    }, [api, scope, selectedWorkspaceId, refetchGithubKanban, refetchWsKanban, addToast])

    const handleCloseIssue = useCallback(async (key: string) => {
        if (!api) return
        try {
            await api.closeGithubWorkItem({ workItemKey: key })
            if (scope === 'github') {
                void queryClient.invalidateQueries({ queryKey: queryKeys.githubWorkItems })
            } else if (selectedWorkspaceId) {
                void queryClient.invalidateQueries({ queryKey: queryKeys.workspaceWorkItems(selectedWorkspaceId) })
            }
            setSelectedCardKey(null)
            addToast({ title: 'Issue closed', body: '', sessionId: '', url: '' })
        } catch {
            addToast({ title: 'Close failed', body: 'Could not close issue', sessionId: '', url: '' })
        }
    }, [api, scope, selectedWorkspaceId, queryClient, addToast])

    const handleCreateWorkspace = useCallback(async (data: WorkspaceFormData) => {
        if (!api) return
        try {
            const ws = await api.createWorkspace({
                name: data.name,
                repos: data.repos.map(r => ({ fullName: r.fullName, color: r.color, shortLabel: r.shortLabel })),
            })
            void refetchWorkspaces()
            setSelectedWorkspaceId(ws.id)
            setShowWorkspaceDialog(false)
        } catch {
            addToast({ title: 'Create failed', body: 'Could not create workspace', sessionId: '', url: '' })
        }
    }, [api, refetchWorkspaces, addToast])

    const handleUpdateWorkspace = useCallback(async (id: string, data: WorkspaceFormData) => {
        if (!api) return
        try {
            await api.updateWorkspace(id, {
                name: data.name,
                repos: data.repos.map(r => ({ fullName: r.fullName, color: r.color, shortLabel: r.shortLabel })),
            })
            void refetchWorkspaces()
            void queryClient.invalidateQueries({ queryKey: queryKeys.workspace(id) })
            setShowWorkspaceDialog(false)
            setEditingWorkspace(null)
        } catch {
            addToast({ title: 'Update failed', body: 'Could not update workspace', sessionId: '', url: '' })
        }
    }, [api, refetchWorkspaces, queryClient, addToast])

    const handleDeleteWorkspace = useCallback(async (id: string) => {
        if (!api) return
        try {
            await api.deleteWorkspace(id)
            void refetchWorkspaces()
            if (selectedWorkspaceId === id) {
                setSelectedWorkspaceId(null)
            }
            setShowWorkspaceDialog(false)
            setEditingWorkspace(null)
        } catch {
            addToast({ title: 'Delete failed', body: 'Could not delete workspace', sessionId: '', url: '' })
        }
    }, [api, selectedWorkspaceId, refetchWorkspaces, addToast])

    const handleViewLog = useCallback((jobId: string) => {
        setLogJobId(jobId)
    }, [])

    const handleRefreshLog = useCallback(() => {
        if (scope === 'github') void refetchGithubLog()
        else void refetchWsLog()
    }, [scope, refetchGithubLog, refetchWsLog])

    const handleScopeChange = useCallback((newScope: KanbanScope) => {
        setScope(newScope)
        setSelectedCardKey(null)
        setLogJobId(null)
        setRepoFilter(null)
        setSearchQuery('')
    }, [])

    const isDragDisabled = deferredSearch.length > 0

    const selectedCard = selectedCardKey ? cardsByKey.get(selectedCardKey) ?? null : null

    // Empty state for workspace scope
    const showEmptyWorkspace = scope === 'workspace' && !selectedWorkspaceId
    const showNoData = (scope === 'github' || (scope === 'workspace' && selectedWorkspaceId)) && workItems.length === 0 && !kanbanConfig

    if (!api) {
        return (
            <div className="flex items-center justify-center h-full">
                <LoadingState label="Connecting..." className="text-sm" />
            </div>
        )
    }

    return (
        <div className="flex flex-col h-full bg-[var(--app-secondary-bg)]">
            <KanbanHeader
                scope={scope}
                onScopeChange={handleScopeChange}
                workspaces={workspaces}
                selectedWorkspaceId={selectedWorkspaceId}
                onWorkspaceChange={setSelectedWorkspaceId}
                searchQuery={searchQuery}
                onSearchChange={setSearchQuery}
                repoFilter={repoFilter}
                repos={repos}
                onRepoFilterChange={setRepoFilter}
                onSync={handleSync}
                syncing={syncing}
                onNewWorkspace={() => { setEditingWorkspace(null); setShowWorkspaceDialog(true) }}
                onEditWorkspace={() => { if (workspace) { setEditingWorkspace(workspace); setShowWorkspaceDialog(true) } }}
                hasGithubWebhook
                onNavigateToSessions={() => {
                    try { window.localStorage.setItem('codex.sessions.view', 'sessions') } catch {}
                    navigate({ to: '/sessions' })
                }}
            />

            <div className="flex flex-1 min-h-0">
                {/* Board area */}
                {showEmptyWorkspace ? (
                    <div className="flex-1 flex flex-col items-center justify-center gap-3 text-center px-4">
                        <div className="text-sm text-[var(--app-hint)]">
                            Select a workspace or create a new one to get started.
                        </div>
                        <button
                            type="button"
                            onClick={() => { setEditingWorkspace(null); setShowWorkspaceDialog(true) }}
                            className="h-8 px-4 text-xs font-medium rounded-md bg-[var(--app-fg)] text-[var(--app-bg)] hover:opacity-90 transition-opacity"
                        >
                            Create Workspace
                        </button>
                    </div>
                ) : showNoData ? (
                    <div className="flex-1 flex items-center justify-center">
                        <div className="text-sm text-[var(--app-hint)]">
                            {scope === 'github' ? 'No GitHub work items. Configure repos and sync.' : 'No items found. Try syncing.'}
                        </div>
                    </div>
                ) : (
                    <KanbanBoard
                        columns={columns}
                        cardsByKey={cardsByKey}
                        selectedCardKey={selectedCardKey}
                        isDragDisabled={isDragDisabled}
                        repoColors={repoColors}
                        repoLabels={repoLabels}
                        activeCardKey={activeCardKey}
                        onDragStart={handleDragStart}
                        onDragEnd={handleDragEnd}
                        onDragCancel={handleDragCancel}
                        onSelectCard={setSelectedCardKey}
                    />
                )}

                {/* Detail panel */}
                {selectedCard && (
                    <CardDetailPanel
                        card={selectedCard}
                        detail={itemDetail ?? null}
                        detailLoading={detailLoading}
                        jobs={jobs}
                        models={models}
                        onClose={() => setSelectedCardKey(null)}
                        onUpdateSettings={handleUpdateSettings}
                        onCloseIssue={handleCloseIssue}
                        onViewLog={handleViewLog}
                    />
                )}
            </div>

            {/* Log panel */}
            {logJobId && (
                <JobLogViewer
                    jobId={logJobId}
                    logText={logData?.logText ?? null}
                    logLoading={!logData}
                    truncated={logData?.truncated ?? false}
                    onClose={() => setLogJobId(null)}
                    onRefresh={handleRefreshLog}
                />
            )}

            {/* Workspace dialog */}
            <WorkspaceDialog
                open={showWorkspaceDialog}
                editing={editingWorkspace}
                onClose={() => { setShowWorkspaceDialog(false); setEditingWorkspace(null) }}
                onCreate={handleCreateWorkspace}
                onUpdate={handleUpdateWorkspace}
                onDelete={handleDeleteWorkspace}
            />
        </div>
    )
}
