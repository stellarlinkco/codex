import { memo, useCallback, useState } from 'react'
import type { KanbanScope, WorkspaceSummary } from './types'

type KanbanHeaderProps = {
    scope: KanbanScope
    onScopeChange: (scope: KanbanScope) => void
    workspaces: WorkspaceSummary[]
    selectedWorkspaceId: string | null
    onWorkspaceChange: (id: string | null) => void
    searchQuery: string
    onSearchChange: (query: string) => void
    repoFilter: string | null
    repos: string[]
    onRepoFilterChange: (repo: string | null) => void
    onSync: () => void
    syncing: boolean
    onNewWorkspace: () => void
    onEditWorkspace: () => void
    hasGithubWebhook: boolean
    onNavigateToSessions: () => void
}

const ScopeTab = memo(function ScopeTab({
    label,
    active,
    onClick,
}: {
    label: string
    active: boolean
    onClick: () => void
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            className={`
                px-3 py-1.5 text-xs font-medium rounded-md transition-all duration-150
                ${active
                    ? 'bg-[var(--app-fg)] text-[var(--app-bg)] shadow-sm'
                    : 'text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]'
                }
            `}
        >
            {label}
        </button>
    )
})

function SearchIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" />
        </svg>
    )
}

function SyncIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
        </svg>
    )
}

function PlusIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
        </svg>
    )
}

function SettingsIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
            <circle cx="12" cy="12" r="3" />
        </svg>
    )
}

function ListIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
            <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
        </svg>
    )
}

export const KanbanHeader = memo(function KanbanHeader({
    scope,
    onScopeChange,
    workspaces,
    selectedWorkspaceId,
    onWorkspaceChange,
    searchQuery,
    onSearchChange,
    repoFilter,
    repos,
    onRepoFilterChange,
    onSync,
    syncing,
    onNewWorkspace,
    onEditWorkspace,
    hasGithubWebhook,
    onNavigateToSessions,
}: KanbanHeaderProps) {
    const [searchFocused, setSearchFocused] = useState(false)

    const handleScopeChange = useCallback((newScope: KanbanScope) => {
        onScopeChange(newScope)
    }, [onScopeChange])

    return (
        <div className="border-b border-[var(--app-border)] bg-[var(--app-bg)]">
            {/* Top row: navigation + scope tabs + sync */}
            <div className="flex items-center justify-between gap-2 px-3 sm:px-4 py-2.5">
                <div className="flex items-center gap-2 sm:gap-3 shrink-0">
                    <button
                        type="button"
                        onClick={onNavigateToSessions}
                        className="flex items-center gap-1.5 text-sm text-[var(--app-hint)] hover:text-[var(--app-fg)] transition-colors"
                    >
                        <ListIcon className="w-4 h-4" />
                        <span className="hidden sm:inline">Sessions</span>
                    </button>
                    <div className="w-px h-4 bg-[var(--app-border)] hidden sm:block" />
                    <h1 className="text-sm font-semibold text-[var(--app-fg)] hidden sm:block">Board</h1>
                </div>

                {/* Scope tabs */}
                <div className="flex items-center gap-0.5 sm:gap-1 p-0.5 rounded-lg bg-[var(--app-subtle-bg)]">
                    <ScopeTab label="Sessions" active={scope === 'sessions'} onClick={() => handleScopeChange('sessions')} />
                    {hasGithubWebhook && (
                        <ScopeTab label="GitHub" active={scope === 'github'} onClick={() => handleScopeChange('github')} />
                    )}
                    <ScopeTab label="Workspace" active={scope === 'workspace'} onClick={() => handleScopeChange('workspace')} />
                </div>

                <div className="flex items-center gap-1.5 shrink-0">
                    {(scope === 'github' || scope === 'workspace') && (
                        <button
                            type="button"
                            onClick={onSync}
                            disabled={syncing}
                            className="flex items-center gap-1 sm:gap-1.5 px-2 sm:px-2.5 py-1.5 text-xs font-medium rounded-md text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors disabled:opacity-50"
                        >
                            <SyncIcon className={syncing ? 'animate-spin' : ''} />
                            <span className="hidden sm:inline">Sync</span>
                        </button>
                    )}
                </div>
            </div>

            {/* Second row: workspace selector + search + filters */}
            <div className="flex items-center gap-2 px-3 sm:px-4 py-2 border-t border-[var(--app-divider)] overflow-x-auto">
                {scope === 'workspace' && (
                    <div className="flex items-center gap-1.5 shrink-0">
                        <select
                            value={selectedWorkspaceId || ''}
                            onChange={e => onWorkspaceChange(e.target.value || null)}
                            className="h-7 text-xs rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-2 max-w-[140px] sm:max-w-none focus:outline-none focus:ring-1 focus:ring-[var(--app-link)]"
                        >
                            <option value="">Select workspace...</option>
                            {workspaces.map(ws => (
                                <option key={ws.id} value={ws.id}>{ws.name}</option>
                            ))}
                        </select>
                        <button
                            type="button"
                            onClick={onNewWorkspace}
                            className="flex items-center justify-center h-7 w-7 rounded-md text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                            title="New workspace"
                        >
                            <PlusIcon />
                        </button>
                        {selectedWorkspaceId && (
                            <button
                                type="button"
                                onClick={onEditWorkspace}
                                className="flex items-center justify-center h-7 w-7 rounded-md text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                                title="Workspace settings"
                            >
                                <SettingsIcon />
                            </button>
                        )}
                        <div className="w-px h-4 bg-[var(--app-border)] mx-1" />
                    </div>
                )}

                {/* Search */}
                <div className={`
                    relative flex items-center rounded-md border transition-colors shrink-0
                    ${searchFocused
                        ? 'border-[var(--app-link)] ring-1 ring-[var(--app-link)] ring-opacity-30'
                        : 'border-[var(--app-border)]'
                    }
                `}>
                    <SearchIcon className="absolute left-2 text-[var(--app-hint)]" />
                    <input
                        type="text"
                        placeholder="Filter..."
                        value={searchQuery}
                        onChange={e => onSearchChange(e.target.value)}
                        onFocus={() => setSearchFocused(true)}
                        onBlur={() => setSearchFocused(false)}
                        className="h-7 w-28 sm:w-48 pl-7 pr-2 text-xs bg-transparent text-[var(--app-fg)] placeholder-[var(--app-hint)] focus:outline-none"
                    />
                </div>

                {/* Repo filter pills - scrollable on mobile */}
                {repos.length > 1 && (
                    <div className="flex items-center gap-1 ml-1 shrink-0">
                        <button
                            type="button"
                            onClick={() => onRepoFilterChange(null)}
                            className={`
                                px-2 py-1 text-[10px] font-medium rounded-md transition-colors whitespace-nowrap
                                ${repoFilter === null
                                    ? 'bg-[var(--app-fg)] text-[var(--app-bg)]'
                                    : 'text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]'
                                }
                            `}
                        >
                            All
                        </button>
                        {repos.map(repo => (
                            <button
                                key={repo}
                                type="button"
                                onClick={() => onRepoFilterChange(repo)}
                                className={`
                                    px-2 py-1 text-[10px] font-medium rounded-md transition-colors truncate max-w-[80px] sm:max-w-[100px] whitespace-nowrap
                                    ${repoFilter === repo
                                        ? 'bg-[var(--app-fg)] text-[var(--app-bg)]'
                                        : 'text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]'
                                    }
                                `}
                            >
                                {repo.split('/')[1] || repo}
                            </button>
                        ))}
                    </div>
                )}
            </div>
        </div>
    )
})
