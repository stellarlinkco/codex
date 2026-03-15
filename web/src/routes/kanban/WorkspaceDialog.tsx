import { memo, useCallback, useEffect, useState } from 'react'
import type { Workspace, WorkspaceFormData } from './types'

type WorkspaceDialogProps = {
    open: boolean
    editing: Workspace | null
    onClose: () => void
    onCreate: (data: WorkspaceFormData) => void
    onUpdate: (id: string, data: WorkspaceFormData) => void
    onDelete: (id: string) => void
}

function CloseIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
        </svg>
    )
}

function TrashIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="3 6 5 6 21 6" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
        </svg>
    )
}

export const WorkspaceDialog = memo(function WorkspaceDialog({
    open,
    editing,
    onClose,
    onCreate,
    onUpdate,
    onDelete,
}: WorkspaceDialogProps) {
    const [name, setName] = useState('')
    const [repoInput, setRepoInput] = useState('')
    const [repos, setRepos] = useState<Array<{ fullName: string; color?: string; shortLabel?: string }>>([])
    const [error, setError] = useState('')

    useEffect(() => {
        if (open) {
            if (editing) {
                setName(editing.name)
                setRepos(editing.repos.map(r => ({
                    fullName: r.fullName,
                    color: r.color,
                    shortLabel: r.shortLabel,
                })))
            } else {
                setName('')
                setRepos([])
            }
            setRepoInput('')
            setError('')
        }
    }, [open, editing])

    const addRepo = useCallback(() => {
        const trimmed = repoInput.trim()
        if (!trimmed) return
        if (!/^[^/\s]+\/[^/\s]+$/.test(trimmed)) {
            setError('Use format: owner/repo')
            return
        }
        if (repos.some(r => r.fullName === trimmed)) {
            setError('Repository already added')
            return
        }
        setRepos(prev => [...prev, { fullName: trimmed }])
        setRepoInput('')
        setError('')
    }, [repoInput, repos])

    const removeRepo = useCallback((fullName: string) => {
        setRepos(prev => prev.filter(r => r.fullName !== fullName))
    }, [])

    const handleSubmit = useCallback(() => {
        const trimmedName = name.trim()
        if (!trimmedName) {
            setError('Name is required')
            return
        }
        if (repos.length === 0) {
            setError('Add at least one repository')
            return
        }
        const data: WorkspaceFormData = { name: trimmedName, repos }
        if (editing) {
            onUpdate(editing.id, data)
        } else {
            onCreate(data)
        }
    }, [name, repos, editing, onCreate, onUpdate])

    const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
        if (e.key === 'Enter') {
            e.preventDefault()
            addRepo()
        }
    }, [addRepo])

    if (!open) return null

    return (
        <div className="fixed inset-0 z-50 flex items-end sm:items-center justify-center">
            {/* Backdrop */}
            <div
                className="absolute inset-0 bg-black/50 backdrop-blur-sm"
                onClick={onClose}
            />

            {/* Dialog - bottom sheet on mobile, centered on desktop */}
            <div className="relative w-full sm:max-w-md sm:mx-4 rounded-t-xl sm:rounded-xl border border-[var(--app-border)] bg-[var(--app-bg)] shadow-2xl max-h-[90vh] overflow-y-auto">
                {/* Header */}
                <div className="flex items-center justify-between px-5 py-4 border-b border-[var(--app-border)]">
                    <h2 className="text-sm font-semibold text-[var(--app-fg)]">
                        {editing ? 'Edit Workspace' : 'New Workspace'}
                    </h2>
                    <button
                        type="button"
                        onClick={onClose}
                        className="flex items-center justify-center w-7 h-7 rounded-md text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                    >
                        <CloseIcon />
                    </button>
                </div>

                {/* Body */}
                <div className="px-5 py-4 space-y-4">
                    {error && (
                        <div className="text-xs text-red-400 bg-red-500/10 border border-red-500/20 rounded-md px-3 py-2">
                            {error}
                        </div>
                    )}

                    <div>
                        <label className="block text-[11px] text-[var(--app-hint)] mb-1.5 uppercase tracking-wide font-medium">
                            Name
                        </label>
                        <input
                            type="text"
                            value={name}
                            onChange={e => setName(e.target.value)}
                            placeholder="My Project"
                            className="w-full h-9 text-sm rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-3 placeholder-[var(--app-hint)] focus:outline-none focus:ring-1 focus:ring-[var(--app-link)]"
                        />
                    </div>

                    <div>
                        <label className="block text-[11px] text-[var(--app-hint)] mb-1.5 uppercase tracking-wide font-medium">
                            Repositories
                        </label>
                        <div className="flex gap-2">
                            <input
                                type="text"
                                value={repoInput}
                                onChange={e => setRepoInput(e.target.value)}
                                onKeyDown={handleKeyDown}
                                placeholder="owner/repo"
                                className="flex-1 h-9 text-sm rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-3 placeholder-[var(--app-hint)] focus:outline-none focus:ring-1 focus:ring-[var(--app-link)]"
                            />
                            <button
                                type="button"
                                onClick={addRepo}
                                className="h-9 px-3 text-xs font-medium rounded-md border border-[var(--app-border)] text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                            >
                                Add
                            </button>
                        </div>

                        {repos.length > 0 && (
                            <div className="mt-2 space-y-1">
                                {repos.map(repo => (
                                    <div
                                        key={repo.fullName}
                                        className="flex items-center justify-between px-2.5 py-1.5 rounded-md bg-[var(--app-subtle-bg)] border border-[var(--app-border)]"
                                    >
                                        <span className="text-xs text-[var(--app-fg)] font-mono">
                                            {repo.fullName}
                                        </span>
                                        <button
                                            type="button"
                                            onClick={() => removeRepo(repo.fullName)}
                                            className="flex items-center justify-center w-5 h-5 rounded text-[var(--app-hint)] hover:text-red-400 transition-colors"
                                        >
                                            <CloseIcon className="w-3 h-3" />
                                        </button>
                                    </div>
                                ))}
                            </div>
                        )}
                    </div>
                </div>

                {/* Footer */}
                <div className="flex items-center justify-between px-5 py-4 border-t border-[var(--app-border)]">
                    <div>
                        {editing && (
                            <button
                                type="button"
                                onClick={() => onDelete(editing.id)}
                                className="flex items-center gap-1.5 text-xs text-red-400 hover:text-red-300 transition-colors"
                            >
                                <TrashIcon />
                                Delete
                            </button>
                        )}
                    </div>
                    <div className="flex items-center gap-2">
                        <button
                            type="button"
                            onClick={onClose}
                            className="h-8 px-4 text-xs font-medium rounded-md border border-[var(--app-border)] text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                        >
                            Cancel
                        </button>
                        <button
                            type="button"
                            onClick={handleSubmit}
                            className="h-8 px-4 text-xs font-medium rounded-md bg-[var(--app-fg)] text-[var(--app-bg)] hover:opacity-90 transition-opacity"
                        >
                            {editing ? 'Save' : 'Create'}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    )
})
