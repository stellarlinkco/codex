import { memo, useCallback, useEffect, useState } from 'react'
import { StandaloneMarkdown } from '@/components/StandaloneMarkdown'
import type { CardData, GithubJob } from './types'
import type { GithubWorkItemDetail, ModelCatalogModel, ReasoningEffort } from '@/types/api'

type CardDetailPanelProps = {
    card: CardData | null
    detail: GithubWorkItemDetail | null
    detailLoading: boolean
    jobs: GithubJob[]
    models: ModelCatalogModel[]
    onClose: () => void
    onUpdateSettings: (key: string, settings: { promptPrefix?: string; model?: string; reasoningEffort?: ReasoningEffort | null }) => void
    onCloseIssue: (key: string) => void
    onViewLog: (jobId: string) => void
}

function CloseIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
        </svg>
    )
}

function ExternalLinkIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" /><polyline points="15 3 21 3 21 9" /><line x1="10" y1="14" x2="21" y2="3" />
        </svg>
    )
}

function LogIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline points="14 2 14 8 20 8" /><line x1="16" y1="13" x2="8" y2="13" /><line x1="16" y1="17" x2="8" y2="17" />
        </svg>
    )
}

function statusBadgeClass(status: string): string {
    switch (status) {
        case 'running': return 'bg-blue-500/15 text-blue-400 border-blue-500/30'
        case 'queued': return 'bg-yellow-500/15 text-yellow-400 border-yellow-500/30'
        case 'succeeded': return 'bg-green-500/15 text-green-400 border-green-500/30'
        case 'failed': return 'bg-red-500/15 text-red-400 border-red-500/30'
        default: return 'bg-[var(--app-subtle-bg)] text-[var(--app-hint)] border-[var(--app-border)]'
    }
}

function formatTime(ms: number): string {
    return new Date(ms).toLocaleString(undefined, {
        month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'
    })
}

export const CardDetailPanel = memo(function CardDetailPanel({
    card,
    detail,
    detailLoading,
    jobs,
    models,
    onClose,
    onUpdateSettings,
    onCloseIssue,
    onViewLog,
}: CardDetailPanelProps) {
    const [promptDraft, setPromptDraft] = useState('')
    const [modelDraft, setModelDraft] = useState('')
    const [effortDraft, setEffortDraft] = useState<ReasoningEffort | ''>('')

    // Reset drafts when card changes
    useEffect(() => {
        if (card) {
            setPromptDraft(card.settings.promptPrefix || '')
            setModelDraft(card.settings.model || '')
            setEffortDraft(card.settings.reasoningEffort || '')
        }
    }, [card?.key]) // eslint-disable-line react-hooks/exhaustive-deps

    const handleSaveSettings = useCallback(() => {
        if (!card) return
        onUpdateSettings(card.key, {
            promptPrefix: promptDraft || undefined,
            model: modelDraft || undefined,
            reasoningEffort: (effortDraft as ReasoningEffort) || null,
        })
    }, [card, promptDraft, modelDraft, effortDraft, onUpdateSettings])

    if (!card) return null

    const { item } = card
    const cardJobs = jobs.filter(j => j.workItemKey === card.key)
        .sort((a, b) => b.createdAt - a.createdAt)

    const settingsChanged = (promptDraft || '') !== (card.settings.promptPrefix || '')
        || (modelDraft || '') !== (card.settings.model || '')
        || (effortDraft || '') !== (card.settings.reasoningEffort || '')

    return (
        <div className="fixed inset-0 z-40 sm:relative sm:inset-auto sm:z-auto w-full sm:w-[400px] xl:w-[440px] shrink-0 sm:border-l border-[var(--app-border)] bg-[var(--app-bg)] flex flex-col h-full overflow-hidden">
            {/* Panel header */}
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--app-border)]">
                <div className="flex items-center gap-2 min-w-0">
                    <span className="text-xs text-[var(--app-hint)] font-medium shrink-0">
                        {item.repo}#{item.number}
                    </span>
                    {item.url && (
                        <a
                            href={item.url}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="text-[var(--app-hint)] hover:text-[var(--app-fg)] transition-colors shrink-0"
                        >
                            <ExternalLinkIcon />
                        </a>
                    )}
                </div>
                <button
                    type="button"
                    onClick={onClose}
                    className="flex items-center justify-center w-7 h-7 rounded-md text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                >
                    <CloseIcon />
                </button>
            </div>

            {/* Scrollable body */}
            <div className="flex-1 overflow-y-auto">
                <div className="px-4 py-4 space-y-5">
                    {/* Title + state */}
                    <div>
                        <h2 className="text-base font-semibold text-[var(--app-fg)] leading-snug">
                            {item.title}
                        </h2>
                        <div className="flex items-center gap-2 mt-2">
                            <span className={`
                                inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-semibold uppercase tracking-wide border
                                ${item.state === 'open'
                                    ? 'bg-green-500/15 text-green-400 border-green-500/30'
                                    : 'bg-purple-500/15 text-purple-400 border-purple-500/30'
                                }
                            `}>
                                {item.state}
                            </span>
                            {item.labels.map(label => (
                                <span
                                    key={label.name}
                                    className="inline-flex items-center px-1.5 py-0.5 rounded-full text-[10px] font-medium"
                                    style={{
                                        backgroundColor: `#${label.color}22`,
                                        color: `#${label.color}`,
                                        border: `1px solid #${label.color}44`,
                                    }}
                                >
                                    {label.name}
                                </span>
                            ))}
                        </div>
                    </div>

                    {/* Issue body */}
                    {detailLoading ? (
                        <div className="text-xs text-[var(--app-hint)]">Loading details...</div>
                    ) : detail?.body ? (
                        <div className="prose prose-sm max-w-none text-[var(--app-fg)]">
                            <StandaloneMarkdown content={detail.body} />
                        </div>
                    ) : null}

                    {/* Execution config */}
                    <div className="space-y-3">
                        <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--app-hint)]">
                            Execution Config
                        </h3>

                        <div className="space-y-2.5">
                            <div>
                                <label className="block text-[11px] text-[var(--app-hint)] mb-1">Model</label>
                                <select
                                    value={modelDraft}
                                    onChange={e => setModelDraft(e.target.value)}
                                    className="w-full h-8 text-xs rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-2 focus:outline-none focus:ring-1 focus:ring-[var(--app-link)]"
                                >
                                    <option value="">Default</option>
                                    {models.map(m => (
                                        <option key={m.id} value={m.id}>{m.displayName}</option>
                                    ))}
                                </select>
                            </div>

                            <div>
                                <label className="block text-[11px] text-[var(--app-hint)] mb-1">Reasoning Effort</label>
                                <select
                                    value={effortDraft}
                                    onChange={e => setEffortDraft(e.target.value as ReasoningEffort | '')}
                                    className="w-full h-8 text-xs rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-2 focus:outline-none focus:ring-1 focus:ring-[var(--app-link)]"
                                >
                                    <option value="">Default</option>
                                    <option value="low">Low</option>
                                    <option value="medium">Medium</option>
                                    <option value="high">High</option>
                                </select>
                            </div>

                            <div>
                                <label className="block text-[11px] text-[var(--app-hint)] mb-1">Prompt Prefix</label>
                                <textarea
                                    value={promptDraft}
                                    onChange={e => setPromptDraft(e.target.value)}
                                    rows={3}
                                    placeholder="Additional instructions for the agent..."
                                    className="w-full text-xs rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] text-[var(--app-fg)] px-2 py-1.5 placeholder-[var(--app-hint)] focus:outline-none focus:ring-1 focus:ring-[var(--app-link)] resize-none"
                                />
                            </div>

                            {settingsChanged && (
                                <button
                                    type="button"
                                    onClick={handleSaveSettings}
                                    className="w-full h-8 text-xs font-medium rounded-md bg-[var(--app-fg)] text-[var(--app-bg)] hover:opacity-90 transition-opacity"
                                >
                                    Save Settings
                                </button>
                            )}
                        </div>
                    </div>

                    {/* Jobs history */}
                    {cardJobs.length > 0 && (
                        <div className="space-y-2">
                            <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--app-hint)]">
                                Job History
                            </h3>
                            <div className="space-y-1.5">
                                {cardJobs.map(job => (
                                    <div
                                        key={job.jobId}
                                        className="flex items-center justify-between px-2.5 py-2 rounded-md border border-[var(--app-border)] bg-[var(--app-subtle-bg)]"
                                    >
                                        <div className="flex items-center gap-2">
                                            <span className={`
                                                inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wide border
                                                ${statusBadgeClass(job.status)}
                                            `}>
                                                {job.status}
                                            </span>
                                            <span className="text-[10px] text-[var(--app-hint)]">
                                                {formatTime(job.createdAt)}
                                            </span>
                                        </div>
                                        {job.logPath && (
                                            <button
                                                type="button"
                                                onClick={() => onViewLog(job.jobId)}
                                                className="flex items-center gap-1 text-[10px] text-[var(--app-hint)] hover:text-[var(--app-fg)] transition-colors"
                                            >
                                                <LogIcon />
                                                Log
                                            </button>
                                        )}
                                    </div>
                                ))}
                            </div>
                        </div>
                    )}

                    {/* Actions */}
                    {item.state === 'open' && (
                        <div className="pt-2 border-t border-[var(--app-divider)]">
                            <button
                                type="button"
                                onClick={() => onCloseIssue(card.key)}
                                className="w-full h-8 text-xs font-medium rounded-md border border-red-500/30 text-red-400 hover:bg-red-500/10 transition-colors"
                            >
                                Close Issue
                            </button>
                        </div>
                    )}
                </div>
            </div>
        </div>
    )
})
