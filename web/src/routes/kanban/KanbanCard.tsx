import { memo } from 'react'
import { useSortable } from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import type { CardData } from './types'

type KanbanCardProps = {
    card: CardData
    isSelected: boolean
    isDragDisabled: boolean
    repoColor?: string
    repoLabel?: string
    onSelect: (key: string) => void
}

function statusColor(status: string | undefined): string {
    if (!status) return 'var(--app-hint)'
    switch (status) {
        case 'running': return '#3B82F6'
        case 'queued': return '#F59E0B'
        case 'succeeded': return '#22C55E'
        case 'failed': return '#EF4444'
        case 'canceled': return '#6B7280'
        default: return 'var(--app-hint)'
    }
}

function statusLabel(status: string | undefined): string {
    if (!status) return ''
    switch (status) {
        case 'running': return 'Running'
        case 'queued': return 'Queued'
        case 'succeeded': return 'Done'
        case 'failed': return 'Failed'
        case 'canceled': return 'Canceled'
        default: return status
    }
}

function elapsed(startedAt: number | null | undefined): string {
    if (!startedAt) return ''
    const sec = Math.floor((Date.now() - startedAt) / 1000)
    if (sec < 60) return `${sec}s`
    const min = Math.floor(sec / 60)
    if (min < 60) return `${min}m`
    return `${Math.floor(min / 60)}h ${min % 60}m`
}

function formatRelativeTime(value: number): string {
    const ms = value < 1_000_000_000_000 ? value * 1000 : value
    if (!Number.isFinite(ms)) return ''
    const delta = Date.now() - ms
    if (delta < 60_000) return 'just now'
    const minutes = Math.floor(delta / 60_000)
    if (minutes < 60) return `${minutes}m ago`
    const hours = Math.floor(minutes / 60)
    if (hours < 24) return `${hours}h ago`
    const days = Math.floor(hours / 24)
    if (days < 7) return `${days}d ago`
    return new Date(ms).toLocaleDateString()
}

const KanbanCardInner = memo(function KanbanCardInner({
    card,
    isSelected,
    isDragDisabled,
    repoColor,
    repoLabel,
    onSelect,
}: KanbanCardProps) {
    const {
        attributes,
        listeners,
        setNodeRef,
        transform,
        transition,
        isDragging,
    } = useSortable({
        id: card.key,
        data: {
            type: 'card',
            cardKey: card.key,
        },
        disabled: isDragDisabled,
    })

    const style = {
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.5 : 1,
    }

    const isGithubCard = card.kind === 'github'
    const githubSettings = isGithubCard ? card.settings : null
    const githubLatestJob = isGithubCard ? card.latestJob : null
    const hasConfig = Boolean(
        githubSettings?.model || githubSettings?.reasoningEffort || githubSettings?.promptPrefix
    )
    const isRunning = githubLatestJob?.status === 'running'

    return (
        <div
            ref={setNodeRef}
            style={style}
            {...attributes}
            {...listeners}
            onClick={() => onSelect(card.key)}
            className={`
                group relative rounded-lg border transition-all duration-150 cursor-pointer
                ${isSelected
                    ? 'border-[var(--app-link)] bg-[color-mix(in_srgb,var(--app-link)_8%,var(--app-bg))]'
                    : 'border-[var(--app-border)] bg-[var(--app-bg)] hover:border-[var(--app-hint)]'
                }
                ${isDragging ? 'shadow-lg ring-2 ring-[var(--app-link)] ring-opacity-30' : 'shadow-sm'}
                ${isDragDisabled ? 'cursor-default' : 'cursor-grab active:cursor-grabbing'}
            `}
        >
            {isGithubCard ? (
                <>
                    {isRunning && (
                        <div className="absolute top-2 right-2">
                            <span className="relative flex h-2.5 w-2.5">
                                <span className="absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75 animate-ping" />
                                <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-blue-500" />
                            </span>
                        </div>
                    )}

                    <div className="px-3 py-2.5 space-y-1.5">
                        <div className="flex items-center gap-1.5 text-xs">
                            {repoColor && (
                                <span
                                    className="inline-block w-2 h-2 rounded-full shrink-0"
                                    style={{ backgroundColor: repoColor }}
                                />
                            )}
                            <span className="text-[var(--app-hint)] font-medium truncate">
                                {repoLabel || card.item.repo}
                            </span>
                            <span className="text-[var(--app-hint)]">#{card.item.number}</span>
                        </div>

                        <div className="text-sm font-medium leading-snug text-[var(--app-fg)] line-clamp-2">
                            {card.item.title}
                        </div>

                        {card.item.labels.length > 0 && (
                            <div className="flex flex-wrap gap-1">
                                {card.item.labels.slice(0, 3).map(label => (
                                    <span
                                        key={label.name}
                                        className="inline-flex items-center px-1.5 py-0.5 rounded-full text-[10px] font-medium leading-none"
                                        style={{
                                            backgroundColor: `#${label.color}22`,
                                            color: `#${label.color}`,
                                            border: `1px solid #${label.color}44`,
                                        }}
                                    >
                                        {label.name}
                                    </span>
                                ))}
                                {card.item.labels.length > 3 && (
                                    <span className="text-[10px] text-[var(--app-hint)]">
                                        +{card.item.labels.length - 3}
                                    </span>
                                )}
                            </div>
                        )}

                        <div className="flex items-center justify-between gap-2 pt-0.5">
                            <div className="flex items-center gap-1.5">
                                {githubLatestJob && (
                                    <span
                                        className="inline-flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide"
                                        style={{ color: statusColor(githubLatestJob.status) }}
                                    >
                                        <span
                                            className="w-1.5 h-1.5 rounded-full"
                                            style={{ backgroundColor: statusColor(githubLatestJob.status) }}
                                        />
                                        {statusLabel(githubLatestJob.status)}
                                        {isRunning && githubLatestJob.startedAt && (
                                            <span className="font-normal text-[var(--app-hint)] ml-0.5">
                                                {elapsed(githubLatestJob.startedAt)}
                                            </span>
                                        )}
                                    </span>
                                )}
                            </div>

                            {hasConfig && (
                                <span className="text-[10px] text-[var(--app-hint)] font-mono">
                                    {[
                                        githubSettings?.model?.split('-').pop(),
                                        githubSettings?.reasoningEffort,
                                    ].filter(Boolean).join('/')}
                                </span>
                            )}
                        </div>
                    </div>
                </>
            ) : (
                <div className="px-3 py-2.5 space-y-2">
                    <div className="flex items-center justify-between gap-2 text-xs">
                        <span className="text-[var(--app-hint)] font-medium truncate">
                            {card.agentLabel}
                        </span>
                        <span className="text-[var(--app-hint)] shrink-0">
                            {formatRelativeTime(card.session.updatedAt)}
                        </span>
                    </div>

                    <div className="text-sm font-medium leading-snug text-[var(--app-fg)] line-clamp-2">
                        {card.title}
                    </div>

                    <div className="text-xs text-[var(--app-hint)] truncate">
                        {card.path}
                    </div>

                    <div className="flex items-center justify-between gap-2 pt-0.5 text-[10px]">
                        <div className="flex items-center gap-2 flex-wrap">
                            <span className="inline-flex items-center gap-1 font-semibold uppercase tracking-wide text-[var(--app-hint)]">
                                <span
                                    className={`w-1.5 h-1.5 rounded-full ${card.session.thinking ? 'bg-blue-500' : card.session.active ? 'bg-green-500' : 'bg-[var(--app-hint)]'}`}
                                />
                                {card.session.thinking ? 'Thinking' : card.session.active ? 'Active' : 'Idle'}
                            </span>
                            {card.session.pendingRequestsCount > 0 && (
                                <span className="text-[var(--app-hint)]">
                                    Pending {card.session.pendingRequestsCount}
                                </span>
                            )}
                            {card.session.todoProgress && card.session.todoProgress.completed !== card.session.todoProgress.total && (
                                <span className="text-[var(--app-hint)]">
                                    Todos {card.session.todoProgress.completed}/{card.session.todoProgress.total}
                                </span>
                            )}
                        </div>
                    </div>
                </div>
            )}
        </div>
    )
}, (prev, next) => {
    return (
        prev.card.key === next.card.key
        && prev.card.kind === next.card.kind
        && prev.isSelected === next.isSelected
        && prev.isDragDisabled === next.isDragDisabled
        && prev.repoColor === next.repoColor
        && prev.repoLabel === next.repoLabel
        && (
            prev.card.kind === 'github' && next.card.kind === 'github'
                ? (
                    prev.card.item.title === next.card.item.title
                    && prev.card.item.state === next.card.item.state
                    && prev.card.latestJob?.status === next.card.latestJob?.status
                    && prev.card.latestJob?.startedAt === next.card.latestJob?.startedAt
                    && prev.card.settings === next.card.settings
                )
                : prev.card.kind === 'session' && next.card.kind === 'session'
                    ? (
                        prev.card.title === next.card.title
                        && prev.card.path === next.card.path
                        && prev.card.agentLabel === next.card.agentLabel
                        && prev.card.session.active === next.card.session.active
                        && prev.card.session.thinking === next.card.session.thinking
                        && prev.card.session.pendingRequestsCount === next.card.session.pendingRequestsCount
                        && prev.card.session.updatedAt === next.card.session.updatedAt
                        && prev.card.session.todoProgress?.completed === next.card.session.todoProgress?.completed
                        && prev.card.session.todoProgress?.total === next.card.session.todoProgress?.total
                    )
                    : false
        )
    )
})

export { KanbanCardInner as KanbanCard }
