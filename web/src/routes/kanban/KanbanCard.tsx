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
        disabled: isDragDisabled,
    })

    const style = {
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.5 : 1,
    }

    const { item, latestJob, settings } = card
    const hasConfig = settings.model || settings.reasoningEffort || settings.promptPrefix
    const isRunning = latestJob?.status === 'running'

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
            {/* Running pulse indicator */}
            {isRunning && (
                <div className="absolute top-2 right-2">
                    <span className="relative flex h-2.5 w-2.5">
                        <span className="absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75 animate-ping" />
                        <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-blue-500" />
                    </span>
                </div>
            )}

            <div className="px-3 py-2.5 space-y-1.5">
                {/* Top row: repo badge + number */}
                <div className="flex items-center gap-1.5 text-xs">
                    {repoColor && (
                        <span
                            className="inline-block w-2 h-2 rounded-full shrink-0"
                            style={{ backgroundColor: repoColor }}
                        />
                    )}
                    <span className="text-[var(--app-hint)] font-medium truncate">
                        {repoLabel || item.repo}
                    </span>
                    <span className="text-[var(--app-hint)]">#{item.number}</span>
                </div>

                {/* Title */}
                <div className="text-sm font-medium leading-snug text-[var(--app-fg)] line-clamp-2">
                    {item.title}
                </div>

                {/* Labels */}
                {item.labels.length > 0 && (
                    <div className="flex flex-wrap gap-1">
                        {item.labels.slice(0, 3).map(label => (
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
                        {item.labels.length > 3 && (
                            <span className="text-[10px] text-[var(--app-hint)]">
                                +{item.labels.length - 3}
                            </span>
                        )}
                    </div>
                )}

                {/* Bottom row: status + config indicator */}
                <div className="flex items-center justify-between gap-2 pt-0.5">
                    <div className="flex items-center gap-1.5">
                        {latestJob && (
                            <span
                                className="inline-flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide"
                                style={{ color: statusColor(latestJob.status) }}
                            >
                                <span
                                    className="w-1.5 h-1.5 rounded-full"
                                    style={{ backgroundColor: statusColor(latestJob.status) }}
                                />
                                {statusLabel(latestJob.status)}
                                {isRunning && latestJob.startedAt && (
                                    <span className="font-normal text-[var(--app-hint)] ml-0.5">
                                        {elapsed(latestJob.startedAt)}
                                    </span>
                                )}
                            </span>
                        )}
                    </div>

                    {hasConfig && (
                        <span className="text-[10px] text-[var(--app-hint)] font-mono">
                            {[
                                settings.model?.split('-').pop(),
                                settings.reasoningEffort,
                            ].filter(Boolean).join('/')}
                        </span>
                    )}
                </div>
            </div>
        </div>
    )
}, (prev, next) => {
    return (
        prev.card.key === next.card.key
        && prev.isSelected === next.isSelected
        && prev.isDragDisabled === next.isDragDisabled
        && prev.card.item.title === next.card.item.title
        && prev.card.item.state === next.card.item.state
        && prev.card.latestJob?.status === next.card.latestJob?.status
        && prev.card.latestJob?.startedAt === next.card.latestJob?.startedAt
        && prev.card.settings === next.card.settings
        && prev.repoColor === next.repoColor
        && prev.repoLabel === next.repoLabel
    )
})

export { KanbanCardInner as KanbanCard }
