import type { CSSProperties, ReactNode } from 'react'
import { memo, useCallback, useDeferredValue, useEffect, useMemo, useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { DndContext, DragOverlay, PointerSensor, useDroppable, useSensor, useSensors } from '@dnd-kit/core'
import type { DragEndEvent, DragStartEvent, UniqueIdentifier } from '@dnd-kit/core'
import { SortableContext, arrayMove, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { LoadingState } from '@/components/LoadingState'
import { StandaloneMarkdown } from '@/components/StandaloneMarkdown'
import { useGithubJobs } from '@/hooks/queries/useGithubJobs'
import { useGithubKanban } from '@/hooks/queries/useGithubKanban'
import { useGithubJobLog } from '@/hooks/queries/useGithubJobLog'
import { useModelsCatalog } from '@/hooks/queries/useModelsCatalog'
import { useGithubRepos } from '@/hooks/queries/useGithubRepos'
import { useGithubWorkItems } from '@/hooks/queries/useGithubWorkItems'
import { useGithubWorkItemDetail } from '@/hooks/queries/useGithubWorkItemDetail'
import { useAppContext } from '@/lib/app-context'
import { useToast } from '@/lib/toast-context'
import type { GithubJob, GithubWorkItem, ReasoningEffort } from '@/types/api'

type CardsByColumn = Record<string, string[]>
type BadgeTone = 'ok' | 'warn' | 'info' | 'error'
type CardBadge = { text: string; tone: BadgeTone }
type ReasoningEffortPreference = ReasoningEffort | 'auto'
type DragHandleProps = {
    attributes: ReturnType<typeof useSortable>['attributes']
    listeners: ReturnType<typeof useSortable>['listeners']
}

const VIEW_KEY = 'codex.sessions.view'
const GITHUB_SHOW_CLOSED_KEY = 'codex.github.kanban.showClosed'

function setPreferredView(view: 'list' | 'kanban') {
    try {
        window.localStorage.setItem(VIEW_KEY, view)
    } catch {
    }
}

function findContainer(columns: Array<{ id: string }>, cardsByColumn: CardsByColumn, id: UniqueIdentifier): string | null {
    const asString = `${id}`
    if (columns.some(c => c.id === asString)) {
        return asString
    }
    for (const [columnId, cards] of Object.entries(cardsByColumn)) {
        if (cards.includes(asString)) {
            return columnId
        }
    }
    return null
}

function cardsByColumnEqual(a: CardsByColumn, b: CardsByColumn): boolean {
    if (a === b) {
        return true
    }
    const aKeys = Object.keys(a)
    const bKeys = Object.keys(b)
    if (aKeys.length !== bKeys.length) {
        return false
    }
    for (const key of bKeys) {
        const aList = a[key]
        const bList = b[key]
        if (!aList || !bList) {
            return false
        }
        if (aList.length !== bList.length) {
            return false
        }
        for (let i = 0; i < bList.length; i += 1) {
            if (aList[i] !== bList[i]) {
                return false
            }
        }
    }
    return true
}

function formatUpdatedAt(updatedAt: number | undefined): string {
    if (updatedAt === undefined) {
        return ''
    }
    try {
        return new Date(updatedAt).toLocaleString()
    } catch {
        return ''
    }
}

function formatShortId(id: string): string {
    if (id.length <= 18) {
        return id
    }
    return `${id.slice(0, 8)}…${id.slice(-8)}`
}

function SearchIcon(props: { className?: string }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className={props.className}
        >
            <circle cx="11" cy="11" r="8" />
            <path d="m21 21-4.3-4.3" />
        </svg>
    )
}

function DotsIcon(props: { className?: string }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="currentColor"
            className={props.className}
        >
            <circle cx="5" cy="12" r="1.8" />
            <circle cx="12" cy="12" r="1.8" />
            <circle cx="19" cy="12" r="1.8" />
        </svg>
    )
}

function GripIcon(props: { className?: string }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="currentColor"
            className={props.className}
        >
            <circle cx="9" cy="6.5" r="1.3" />
            <circle cx="15" cy="6.5" r="1.3" />
            <circle cx="9" cy="12" r="1.3" />
            <circle cx="15" cy="12" r="1.3" />
            <circle cx="9" cy="17.5" r="1.3" />
            <circle cx="15" cy="17.5" r="1.3" />
        </svg>
    )
}

function CommentIcon(props: { className?: string }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className={props.className}
        >
            <path d="M21 15a4 4 0 0 1-4 4H7l-4 4V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z" />
        </svg>
    )
}

function StatusDot(props: { tone: BadgeTone }) {
    const cls = props.tone === 'ok'
        ? 'bg-emerald-500/90'
        : props.tone === 'warn'
            ? 'bg-amber-400/90'
            : props.tone === 'error'
                ? 'bg-rose-500/90'
                : 'bg-sky-400/90'

    return (
        <span className={`mt-0.5 inline-block h-2.5 w-2.5 shrink-0 rounded-full ${cls}`} />
    )
}

function Badge(props: { badge: CardBadge }) {
    const className = props.badge.tone === 'warn'
        ? 'border-[var(--app-badge-warning-border)] bg-[var(--app-badge-warning-bg)] text-[var(--app-badge-warning-text)]'
        : props.badge.tone === 'ok'
            ? 'border-[var(--app-badge-success-border)] bg-[var(--app-badge-success-bg)] text-[var(--app-badge-success-text)]'
            : props.badge.tone === 'error'
                ? 'border-[var(--app-badge-error-border)] bg-[var(--app-badge-error-bg)] text-[var(--app-badge-error-text)]'
                : 'border-[var(--app-divider)] bg-[var(--app-subtle-bg)] text-[var(--app-hint)]'

    return (
        <span
            className={[
                'inline-flex items-center rounded-full border px-2 py-0.5 text-[10px] font-medium tracking-wide',
                className
            ].join(' ')}
        >
            {props.badge.text}
        </span>
    )
}

const Column = memo(function Column(props: {
    id: string
    name: string
    workItemKeys: string[]
    renderCard: (workItemKey: string) => ReactNode
}) {
    const { setNodeRef, isOver } = useDroppable({ id: props.id })

    const accent = props.id === 'done'
        ? 'bg-emerald-500/80'
        : props.id === 'review'
            ? 'bg-fuchsia-400/75'
            : props.id === 'in-progress'
                ? 'bg-amber-400/75'
                : 'bg-slate-400/70'

    return (
        <div className="w-[320px] shrink-0 flex flex-col h-full min-h-0">
            <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-secondary-bg)] shadow-sm flex flex-col h-full min-h-0">
                <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-[var(--app-divider)]">
                    <div className="min-w-0 flex items-center gap-2">
                        <span className={`h-2.5 w-2.5 rounded-full ${accent}`} />
                        <div className="truncate text-sm font-semibold text-[var(--app-fg)]">
                            {props.name}
                        </div>
                        <span className="shrink-0 rounded-full border border-[var(--app-border)] bg-[var(--app-subtle-bg)] px-2 py-0.5 text-[11px] font-medium text-[var(--app-hint)]">
                            {props.workItemKeys.length}
                        </span>
                    </div>
                    <button
                        type="button"
                        className="rounded-md p-1 text-[var(--app-hint)] hover:bg-[var(--app-subtle-bg)] hover:text-[var(--app-fg)] transition-colors"
                        title="菜单"
                        disabled
                    >
                        <DotsIcon className="h-4 w-4" />
                    </button>
                </div>

                <div
                    ref={setNodeRef}
                    className={[
                        'flex-1 min-h-0 p-2 flex flex-col gap-2 overflow-y-auto',
                        isOver ? 'ring-2 ring-[var(--app-link)] ring-offset-2 ring-offset-[var(--app-bg)]' : ''
                    ].join(' ')}
                >
                    {props.workItemKeys.length === 0 ? (
                        <div className="rounded-md border border-dashed border-[var(--app-border)] p-3 text-xs text-[var(--app-hint)]">
                            拖拽卡片到此列
                        </div>
                    ) : null}
                    <SortableContext items={props.workItemKeys} strategy={verticalListSortingStrategy}>
                        {props.workItemKeys.map(props.renderCard)}
                    </SortableContext>
                </div>
            </div>
        </div>
    )
})

function normalizeLabelColor(input: string): string | null {
    const trimmed = input.trim()
    if (!trimmed) {
        return null
    }
    if (trimmed.startsWith('#')) {
        return trimmed
    }
    if (/^[0-9a-fA-F]{3}$/.test(trimmed) || /^[0-9a-fA-F]{6}$/.test(trimmed)) {
        return `#${trimmed}`
    }
    return null
}

function jobStatusToBadge(statusRaw: string | null | undefined): CardBadge | undefined {
    const status = (statusRaw ?? '').toLowerCase()
    if (!status) {
        return undefined
    }
    if (status === 'running') {
        return { text: 'RUNNING', tone: 'warn' }
    }
    if (status === 'queued') {
        return { text: 'QUEUED', tone: 'info' }
    }
    if (status === 'succeeded') {
        return { text: 'DONE', tone: 'ok' }
    }
    if (status === 'failed') {
        return { text: 'FAILED', tone: 'error' }
    }
    if (status === 'canceled' || status === 'cancelled') {
        return { text: 'CANCELED', tone: 'info' }
    }
    return { text: status.toUpperCase() || 'JOB', tone: 'info' }
}

function jobToBadge(job: GithubJob | null): CardBadge | undefined {
    return jobStatusToBadge(job?.status)
}

function kindShort(kind: string): string {
    const s = kind.trim().toLowerCase()
    if (s === 'pull' || s === 'pr' || s === 'pull_request') {
        return 'PR'
    }
    return 'ISSUE'
}

function CardContent(props: {
    id: string
    title: string
    subtitle: string
    badge?: CardBadge
    dndEnabled: boolean
    onSelect: (workItemKey: string) => void
    dragHandleProps?: DragHandleProps
    labels?: Array<{ name: string; color: string }>
    secondaryBadges?: CardBadge[]
    rightMeta?: ReactNode
}) {
    const tone = props.badge?.tone ?? 'info'

    return (
        <>
            <button
                type="button"
                onClick={() => props.onSelect(props.id)}
                className="w-full text-left"
            >
                <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0 flex items-start gap-2">
                        <StatusDot tone={tone} />
                        <div className="min-w-0">
                            <div className="truncate text-sm font-semibold text-[var(--app-fg)]">
                                {props.title}
                            </div>
                            <div className="mt-1 truncate text-xs text-[var(--app-hint)]">
                                {props.subtitle}
                            </div>
                        </div>
                    </div>
                    {props.badge ? (
                        <div className="shrink-0">
                            <Badge badge={props.badge} />
                        </div>
                    ) : null}
                </div>

                {props.secondaryBadges && props.secondaryBadges.length ? (
                    <div className="mt-2 flex flex-wrap items-center gap-1.5">
                        {props.secondaryBadges.map((badge) => (
                            <Badge key={`${badge.text}:${badge.tone}`} badge={badge} />
                        ))}
                    </div>
                ) : null}

                {props.labels && props.labels.length ? (
                    <div className="mt-2 flex flex-wrap items-center gap-1.5">
                        {props.labels.slice(0, 4).map((label) => {
                            const hex = normalizeLabelColor(label.color)
                            const style = hex
                                ? ({
                                    backgroundColor: `${hex}20`,
                                    borderColor: `${hex}40`,
                                    color: hex
                                } satisfies CSSProperties)
                                : undefined

                            return (
                                <span
                                    key={label.name}
                                    className="inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-medium"
                                    style={style}
                                >
                                    {label.name}
                                </span>
                            )
                        })}
                    </div>
                ) : null}
            </button>

            <div className="mt-2 flex items-center justify-between gap-2">
                <div className="min-w-0 flex items-center gap-2">
                    <span className="truncate font-mono text-[10px] text-[var(--app-hint)]">
                        {formatShortId(props.id)}
                    </span>
                    {props.rightMeta ? (
                        <span className="shrink-0">
                            {props.rightMeta}
                        </span>
                    ) : null}
                    {!props.dndEnabled ? (
                        <span className="shrink-0 text-[10px] text-[var(--app-hint)]">
                            搜索中禁用拖拽
                        </span>
                    ) : null}
                </div>
                <button
                    type="button"
                    className={[
                        'inline-flex items-center gap-1 rounded-md border border-[var(--app-border)] bg-[var(--app-subtle-bg)] px-2 py-1 text-[10px] font-medium text-[var(--app-hint)]',
                        props.dndEnabled ? 'cursor-grab active:cursor-grabbing hover:text-[var(--app-fg)] hover:bg-[var(--app-secondary-bg)]' : 'cursor-not-allowed opacity-60'
                    ].join(' ')}
                    title={props.dndEnabled ? '拖拽排序' : '清空搜索后可拖拽'}
                    {...(props.dndEnabled && props.dragHandleProps ? props.dragHandleProps.attributes : {})}
                    {...(props.dndEnabled && props.dragHandleProps ? (props.dragHandleProps.listeners ?? {}) : {})}
                    disabled={!props.dndEnabled}
                >
                    <GripIcon className="h-4 w-4" />
                    拖拽
                </button>
            </div>
        </>
    )
}

const Card = memo(function Card(props: {
    id: string
    title: string
    repo: string
    number: number
    updatedAt: number | undefined
    kind: string
    state: string
    comments: number
    jobStatus?: string | null
    dndEnabled: boolean
    onSelect: (workItemKey: string) => void
    labels?: Array<{ name: string; color: string }>
}) {
    const {
        attributes,
        listeners,
        setNodeRef,
        transform,
        transition,
        isDragging
    } = useSortable({ id: props.id, disabled: !props.dndEnabled })

    const style: CSSProperties = {
        transform: CSS.Transform.toString(transform),
        transition
    }

    const subtitle = useMemo(() => {
        return `${props.repo} · #${props.number} · ${formatUpdatedAt(props.updatedAt)}`
    }, [props.number, props.repo, props.updatedAt])

    const badge = useMemo(() => {
        return jobStatusToBadge(props.jobStatus ?? null)
    }, [props.jobStatus])

    const secondaryBadges = useMemo((): CardBadge[] => {
        return [
            { text: kindShort(props.kind), tone: 'info' },
            props.state.toLowerCase() === 'open'
                ? { text: 'OPEN', tone: 'ok' }
                : { text: props.state.toUpperCase() || 'STATE', tone: 'info' }
        ]
    }, [props.kind, props.state])

    const rightMeta = useMemo(() => {
        return (
            <span className="inline-flex items-center gap-1 text-[10px] text-[var(--app-hint)]">
                <CommentIcon className="h-3.5 w-3.5" />
                {props.comments}
            </span>
        )
    }, [props.comments])

    return (
        <div
            ref={setNodeRef}
            style={style}
            className={[
                'rounded-lg border border-[var(--app-border)] bg-[var(--app-bg)] p-3 shadow-sm transition-colors',
                'hover:border-[var(--app-divider)]',
                isDragging ? 'opacity-70' : 'opacity-100',
            ].join(' ')}
        >
            <CardContent
                id={props.id}
                title={props.title}
                subtitle={subtitle}
                badge={badge}
                dndEnabled={props.dndEnabled}
                onSelect={props.onSelect}
                dragHandleProps={{ attributes, listeners }}
                labels={props.labels}
                secondaryBadges={secondaryBadges}
                rightMeta={rightMeta}
            />
        </div>
    )
}, (prev, next) => {
    return prev.id === next.id
        && prev.title === next.title
        && prev.repo === next.repo
        && prev.number === next.number
        && prev.updatedAt === next.updatedAt
        && prev.kind === next.kind
        && prev.state === next.state
        && prev.comments === next.comments
        && prev.jobStatus === next.jobStatus
        && prev.dndEnabled === next.dndEnabled
        && prev.labels === next.labels
        && prev.onSelect === next.onSelect
})

export function KanbanPage() {
    const { api } = useAppContext()
    const navigate = useNavigate()
    const { addToast } = useToast()
    const reposQuery = useGithubRepos(api)
    const workItemsQuery = useGithubWorkItems(api)
    const kanbanQuery = useGithubKanban(api)
    const jobsQuery = useGithubJobs(api)
    const modelsCatalogQuery = useModelsCatalog(api)
    const [selectedWorkItemKey, setSelectedWorkItemKey] = useState<string | null>(null)
    const [selectedJobId, setSelectedJobId] = useState<string | null>(null)
    const [activeId, setActiveId] = useState<string | null>(null)
    const [cardsByColumn, setCardsByColumn] = useState<CardsByColumn>({})
    const [search, setSearch] = useState('')
    const [repoFilter, setRepoFilter] = useState<string>('__all__')
    const [showClosed, setShowClosed] = useState(() => {
        try {
            return window.localStorage.getItem(GITHUB_SHOW_CLOSED_KEY) === '1'
        } catch {
            return false
        }
    })
    const [isRepoEditorOpen, setIsRepoEditorOpen] = useState(false)
    const [repoEditorText, setRepoEditorText] = useState('')
    const [repoEditorBusy, setRepoEditorBusy] = useState(false)

    const sensors = useSensors(
        useSensor(PointerSensor, {
            activationConstraint: { distance: 5 }
        })
    )

    const columns = useMemo(() => {
        const cfg = kanbanQuery.data
        if (!cfg) {
            return []
        }
        return [...cfg.columns].sort((a, b) => a.position - b.position)
    }, [kanbanQuery.data])

    useEffect(() => {
        try {
            window.localStorage.setItem(GITHUB_SHOW_CLOSED_KEY, showClosed ? '1' : '0')
        } catch {
        }
    }, [showClosed])
    const detailQuery = useGithubWorkItemDetail(api, selectedWorkItemKey)

    const allItems = workItemsQuery.data?.items ?? []
    const items = useMemo(() => {
        let out = showClosed
            ? allItems
            : allItems.filter((i) => i.state?.toLowerCase() === 'open')
        if (repoFilter !== '__all__') {
            out = out.filter((i) => i.repo === repoFilter)
        }
        return out
    }, [allItems, repoFilter, showClosed])

    const repoOptions = useMemo(() => {
        return Array.from(new Set(items.map(i => i.repo))).sort()
    }, [items])

    const derivedCardsByColumn = useMemo(() => {
        const cfg = kanbanQuery.data
        if (!cfg || !items.length) {
            const empty: CardsByColumn = {}
            for (const col of columns) {
                empty[col.id] = []
            }
            return empty
        }

        const buckets: Record<string, Array<{ workItemKey: string; position: number }>> = {}
        for (const col of cfg.columns) {
            buckets[col.id] = []
        }

        const defaultColumnId = columns[0]?.id ?? cfg.columns[0]?.id
        for (const item of items) {
            const pos = cfg.cardPositions[item.workItemKey]
            const columnId = pos?.columnId ?? defaultColumnId
            if (!columnId) {
                continue
            }
            if (!buckets[columnId]) {
                buckets[columnId] = []
            }
            buckets[columnId].push({ workItemKey: item.workItemKey, position: pos?.position ?? Number.MAX_SAFE_INTEGER })
        }

        const result: CardsByColumn = {}
        for (const [columnId, items] of Object.entries(buckets)) {
            items.sort((a, b) => a.position - b.position)
            result[columnId] = items.map(i => i.workItemKey)
        }
        return result
    }, [columns, items, kanbanQuery.data])

    useEffect(() => {
        setCardsByColumn((current) => {
            if (cardsByColumnEqual(current, derivedCardsByColumn)) {
                return current
            }
            return derivedCardsByColumn
        })
    }, [derivedCardsByColumn])

    const itemsByKey = useMemo(() => {
        const map = new Map<string, GithubWorkItem>()
        for (const item of items) {
            map.set(item.workItemKey, item)
        }
        return map
    }, [items])

    const latestJobByKey = useMemo(() => {
        const map = new Map<string, GithubJob>()
        for (const job of jobsQuery.data?.jobs ?? []) {
            if (!map.has(job.workItemKey)) {
                map.set(job.workItemKey, job)
            }
        }
        return map
    }, [jobsQuery.data?.jobs])

    const selectedItem = useMemo(() => {
        if (!selectedWorkItemKey) {
            return null
        }
        return itemsByKey.get(selectedWorkItemKey) ?? null
    }, [itemsByKey, selectedWorkItemKey])

    const selectedSettings = useMemo(() => {
        if (!selectedWorkItemKey) {
            return null
        }
        return kanbanQuery.data?.cardSettings?.[selectedWorkItemKey] ?? null
    }, [kanbanQuery.data?.cardSettings, selectedWorkItemKey])

    const [draftPromptPrefix, setDraftPromptPrefix] = useState('')
    const [draftModel, setDraftModel] = useState('')
    const [draftReasoningEffort, setDraftReasoningEffort] = useState<ReasoningEffortPreference>('auto')
    const [settingsBusy, setSettingsBusy] = useState(false)

    useEffect(() => {
        if (!selectedWorkItemKey) {
            setDraftPromptPrefix('')
            setDraftModel('')
            setDraftReasoningEffort('auto')
            return
        }
        setDraftPromptPrefix(selectedSettings?.promptPrefix ?? '')
        setDraftModel(selectedSettings?.model ?? '')
        setDraftReasoningEffort(selectedSettings?.reasoningEffort ?? 'auto')
    }, [selectedSettings?.model, selectedSettings?.promptPrefix, selectedSettings?.reasoningEffort, selectedWorkItemKey])

    const availableModels = modelsCatalogQuery.data?.models ?? []
    const defaultModel = useMemo(() => {
        return availableModels.find(m => m.isDefault) ?? availableModels[0] ?? null
    }, [availableModels])

    const effectiveModelIdForEffort = draftModel.trim() || defaultModel?.id || ''
    const effectiveModelForEffort = useMemo(() => {
        if (!effectiveModelIdForEffort) {
            return null
        }
        return availableModels.find(m => m.id === effectiveModelIdForEffort) ?? null
    }, [availableModels, effectiveModelIdForEffort])

    const supportedEfforts = effectiveModelForEffort?.supportedReasoningEfforts ?? []

    const selectedJobs = useMemo(() => {
        if (!selectedWorkItemKey) {
            return []
        }
        return (jobsQuery.data?.jobs ?? []).filter(j => j.workItemKey === selectedWorkItemKey)
    }, [jobsQuery.data?.jobs, selectedWorkItemKey])

    const selectedJob = useMemo(() => {
        if (!selectedJobId) {
            return null
        }
        return selectedJobs.find(j => j.jobId === selectedJobId) ?? null
    }, [selectedJobId, selectedJobs])

    const selectedJobIsLive = useMemo(() => {
        const status = (selectedJob?.status ?? '').toLowerCase()
        return status === 'queued' || status === 'running'
    }, [selectedJob?.status])

    const jobLogQuery = useGithubJobLog(api, selectedJobId, {
        refetchIntervalMs: selectedJobIsLive ? 1500 : false
    })

    useEffect(() => {
        if (!selectedWorkItemKey) {
            setSelectedJobId(null)
            return
        }
        if (selectedJobId && selectedJobs.some(j => j.jobId === selectedJobId)) {
            return
        }
        const live = selectedJobs.find(j => {
            const status = (j.status ?? '').toLowerCase()
            return status === 'running' || status === 'queued'
        })
        setSelectedJobId(live?.jobId ?? selectedJobs[0]?.jobId ?? null)
    }, [selectedJobId, selectedJobs, selectedWorkItemKey])

    const normalizedSearch = useMemo(() => search.trim().toLowerCase(), [search])
    const deferredNormalizedSearch = useDeferredValue(normalizedSearch)
    const dndEnabled = normalizedSearch.length === 0

    const visibleWorkItemKeys = useMemo(() => {
        if (!deferredNormalizedSearch) {
            return null
        }
        const keys = new Set<string>()
        for (const item of items) {
            const haystack = [
                item.repo,
                kindShort(item.kind),
                `${item.number}`,
                item.title,
                item.state,
                item.workItemKey,
                ...(item.labels ?? []).map(l => l.name)
            ]
                .filter(Boolean)
                .join(' ')
                .toLowerCase()
            if (haystack.includes(deferredNormalizedSearch)) {
                keys.add(item.workItemKey)
            }
        }
        return keys
    }, [deferredNormalizedSearch, items])

    const visibleCardsByColumn = useMemo(() => {
        if (!visibleWorkItemKeys) {
            return cardsByColumn
        }
        const next: CardsByColumn = {}
        for (const col of columns) {
            const list = cardsByColumn[col.id] ?? []
            next[col.id] = list.filter(key => visibleWorkItemKeys.has(key))
        }
        return next
    }, [cardsByColumn, columns, visibleWorkItemKeys])

    const onSelectWorkItemKey = useCallback((workItemKey: string) => {
        setSelectedWorkItemKey(workItemKey)
    }, [])

    const renderCard = useCallback((workItemKey: string) => {
        const item = itemsByKey.get(workItemKey)
        if (!item) {
            return null
        }
        const job = latestJobByKey.get(workItemKey) ?? null

        return (
            <Card
                key={workItemKey}
                id={workItemKey}
                title={item.title}
                repo={item.repo}
                number={item.number}
                updatedAt={item.updatedAt}
                kind={item.kind}
                state={item.state}
                comments={item.comments}
                jobStatus={job?.status ?? null}
                dndEnabled={dndEnabled && item.state?.toLowerCase() === 'open'}
                labels={item.labels}
                onSelect={onSelectWorkItemKey}
            />
        )
    }, [dndEnabled, itemsByKey, latestJobByKey, onSelectWorkItemKey])

    const overlayCard = useMemo(() => {
        if (!activeId) {
            return null
        }
        const item = itemsByKey.get(activeId)
        if (!item) {
            return null
        }
        const job = latestJobByKey.get(activeId) ?? null
        const title = item.title
        const subtitle = `${item.repo} · #${item.number} · ${formatUpdatedAt(item.updatedAt)}`
        const badge = jobStatusToBadge(job?.status ?? null) ?? { text: kindShort(item.kind), tone: 'info' }
        return { title, subtitle, badge }
    }, [activeId, itemsByKey, latestJobByKey])

    const persistMove = useCallback(async (workItemKey: string, columnId: string, position: number) => {
        if (!api) {
            return
        }
        await api.moveGithubKanbanCard({
            workItemKey,
            columnId,
            position,
        })
    }, [api])

    const settingsDirty = (draftPromptPrefix.trim() !== (selectedSettings?.promptPrefix ?? '').trim())
        || (draftModel.trim() !== (selectedSettings?.model ?? '').trim())
        || (draftReasoningEffort !== (selectedSettings?.reasoningEffort ?? 'auto'))

    const saveSelectedSettings = useCallback(async () => {
        if (!api || !selectedWorkItemKey) {
            return
        }
        setSettingsBusy(true)
        try {
            await api.updateGithubKanbanCardSettings({
                workItemKey: selectedWorkItemKey,
                promptPrefix: draftPromptPrefix,
                model: draftModel,
                reasoningEffort: draftReasoningEffort === 'auto' ? null : draftReasoningEffort
            })
            await kanbanQuery.refetch()
        } finally {
            setSettingsBusy(false)
        }
    }, [api, draftModel, draftPromptPrefix, draftReasoningEffort, kanbanQuery, selectedWorkItemKey])

    const startSelected = useCallback(async () => {
        if (!api || !selectedWorkItemKey) {
            return
        }
        await api.moveGithubKanbanCard({
            workItemKey: selectedWorkItemKey,
            columnId: 'in-progress',
            position: 0,
        })
        void Promise.allSettled([kanbanQuery.refetch(), jobsQuery.refetch()])
    }, [api, jobsQuery, kanbanQuery, selectedWorkItemKey])

    const closeSelected = useCallback(async () => {
        if (!api || !selectedWorkItemKey) {
            return
        }
        await api.closeGithubWorkItem({ workItemKey: selectedWorkItemKey })
        setSelectedWorkItemKey(null)
        void Promise.allSettled([workItemsQuery.refetch(), kanbanQuery.refetch(), jobsQuery.refetch()])
    }, [api, jobsQuery, kanbanQuery, selectedWorkItemKey, workItemsQuery])

    const onDragStart = useCallback((event: DragStartEvent) => {
        if (!dndEnabled) {
            return
        }
        setActiveId(`${event.active.id}`)
    }, [dndEnabled])

    const onDragEnd = useCallback(async (event: DragEndEvent) => {
        if (!dndEnabled) {
            setActiveId(null)
            return
        }
        const active = event.active?.id
        const over = event.over?.id
        setActiveId(null)
        if (!active || !over) {
            return
        }

        setCardsByColumn((current) => {
            const fromCol = findContainer(columns, current, active)
            const toCol = findContainer(columns, current, over)
            if (!fromCol || !toCol) {
                return current
            }

            const next: CardsByColumn = { ...current }
            const fromItems = [...(next[fromCol] ?? [])]
            const toItems = [...(next[toCol] ?? [])]

            const activeId = `${active}`
            const overId = `${over}`

            const fromIndex = fromItems.indexOf(activeId)
            if (fromIndex === -1) {
                return current
            }

            if (fromCol === toCol) {
                const overIndex = fromItems.indexOf(overId)
                if (overIndex === -1) {
                    return current
                }
                next[fromCol] = arrayMove(fromItems, fromIndex, overIndex)
                void (async () => {
                    try {
                        await persistMove(activeId, fromCol, overIndex)
                    } catch (error) {
                        const message = error instanceof Error ? error.message : 'Move failed'
                        const item = itemsByKey.get(activeId)
                        addToast({ title: 'Kanban move failed', body: message, sessionId: activeId, url: item?.url ?? '' })
                        void Promise.allSettled([kanbanQuery.refetch(), workItemsQuery.refetch(), jobsQuery.refetch()])
                    }
                })()
                return next
            }

            fromItems.splice(fromIndex, 1)
            const insertIndex = columns.some(c => c.id === overId) ? toItems.length : toItems.indexOf(overId)
            const at = insertIndex === -1 ? toItems.length : insertIndex
            toItems.splice(at, 0, activeId)

            next[fromCol] = fromItems
            next[toCol] = toItems

            void (async () => {
                try {
                    await persistMove(activeId, toCol, at)
                } catch (error) {
                    const message = error instanceof Error ? error.message : 'Move failed'
                    const item = itemsByKey.get(activeId)
                    addToast({ title: 'Kanban move failed', body: message, sessionId: activeId, url: item?.url ?? '' })
                    void Promise.allSettled([kanbanQuery.refetch(), workItemsQuery.refetch(), jobsQuery.refetch()])
                }
            })()

            return next
        })
    }, [addToast, columns, dndEnabled, itemsByKey, jobsQuery, kanbanQuery, persistMove, workItemsQuery])

    const isLoading = reposQuery.isLoading || workItemsQuery.isLoading || kanbanQuery.isLoading || jobsQuery.isLoading
    const error = (reposQuery.error ?? workItemsQuery.error ?? kanbanQuery.error ?? jobsQuery.error) as Error | null
    const githubNotEnabled = Boolean(error && error.message.includes('HTTP 404'))

    if (isLoading) {
        return (
            <div className="flex h-full items-center justify-center p-4">
                <LoadingState label="Loading kanban…" className="text-sm" />
            </div>
        )
    }

    if (error) {
        return (
            <div className="p-4">
                {githubNotEnabled ? (
                    <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-secondary-bg)] p-4 text-sm text-[var(--app-fg)]">
                        <div className="font-semibold">GitHub webhook 未启用</div>
                        <div className="mt-1 text-xs text-[var(--app-hint)]">
                            在 `config.toml` 配置 `[github_webhook] enabled = true` 后重启 `codex serve`。
                        </div>
                    </div>
                ) : (
                    <div className="text-sm text-red-600">
                        {error.message}
                    </div>
                )}
            </div>
        )
    }

    const fetchedAt = workItemsQuery.data?.fetchedAt
    const workItemCount = items.length
    const repoCount = repoOptions.length
    const visibleCount = visibleWorkItemKeys ? visibleWorkItemKeys.size : workItemCount

    return (
        <div className="flex h-full min-h-0 flex-col bg-[var(--app-bg)]">
            <div className="bg-[var(--app-bg)] pt-[env(safe-area-inset-top)] border-b border-[var(--app-divider)]">
                <div className="mx-auto w-full max-w-content px-3 py-2">
                    <div className="flex items-center justify-between gap-3">
                        <div className="min-w-0 flex items-center gap-3">
                            <div className="min-w-0">
                                <div className="truncate text-sm font-semibold text-[var(--app-fg)]">
                                    GitHub · Kanban
                                </div>
                                <div className="truncate text-xs text-[var(--app-hint)]">
                                    {repoCount ? `${repoCount} 个仓库 · ` : ''}{workItemCount} 个 work item{fetchedAt ? ` · ${formatUpdatedAt(fetchedAt)}` : ''}{normalizedSearch ? ' · 搜索中已禁用拖拽' : ''}
                                </div>
                                {workItemCount === 0 ? (
                                    <div className="mt-1 text-xs text-[var(--app-hint)]">
                                        {fetchedAt
                                            ? '未拉取到任何 issue/pr。'
                                            : '尚未同步 issue/pr：点击右侧“同步”。如果仍为空，请在 config.toml 配置 github_webhook.allow_repos，或在目标仓库目录启动 codex serve。'}
                                    </div>
                                ) : null}
                            </div>

                            <div className="hidden sm:flex items-center rounded-lg border border-[var(--app-border)] bg-[var(--app-secondary-bg)] p-1">
                                <button
                                    type="button"
                                    onClick={() => {
                                        setPreferredView('list')
                                        navigate({ to: '/sessions' })
                                    }}
                                    className="rounded-md px-2 py-1 text-xs font-medium text-[var(--app-hint)] hover:bg-[var(--app-subtle-bg)] hover:text-[var(--app-fg)]"
                                    title="会话"
                                >
                                    会话
                                </button>
                                <div className="rounded-md bg-[var(--app-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] shadow-sm">
                                    看板
                                </div>
                            </div>
                        </div>

                        <div className="flex items-center gap-2">
                            <button
                                type="button"
                                onClick={() => {
                                    void (async () => {
                                        try {
                                            if (api) {
                                                await api.syncGithubWorkItems()
                                            }
                                            await Promise.allSettled([workItemsQuery.refetch(), kanbanQuery.refetch(), jobsQuery.refetch()])
                                        } catch (error) {
                                            const message = error instanceof Error ? error.message : 'Sync failed'
                                            addToast({ title: 'GitHub sync failed', body: message, sessionId: 'github-sync', url: '' })
                                        }
                                    })()
                                }}
                                className="rounded-md border border-[var(--app-border)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                title="同步 GitHub"
                                disabled={!api}
                            >
                                同步
                            </button>
                            <button
                                type="button"
                                onClick={() => setShowClosed((v) => !v)}
                                className="rounded-md border border-[var(--app-border)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                title="过滤关闭的 issue/pr"
                            >
                                {showClosed ? '隐藏已关闭' : '显示已关闭'}
                            </button>
                            <button
                                type="button"
                                onClick={() => navigate({ to: '/settings' })}
                                className="rounded-md border border-[var(--app-border)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                            >
                                设置
                            </button>
                        </div>
                    </div>

                    <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                        <div className="relative w-full sm:max-w-[420px]">
                            <div className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--app-hint)]">
                                <SearchIcon className="h-4 w-4" />
                            </div>
                            <input
                                value={search}
                                onChange={(e) => setSearch(e.target.value)}
                                placeholder="搜索 repo / 标题 / #号 / 标签"
                                className={[
                                    'w-full rounded-lg border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-9 py-2 text-sm text-[var(--app-fg)]',
                                    'placeholder:text-[var(--app-hint)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] focus:ring-offset-2 focus:ring-offset-[var(--app-bg)]'
                                ].join(' ')}
                            />
                        </div>
                        <div className="text-xs text-[var(--app-hint)]">
                            {visibleWorkItemKeys ? `${visibleCount} / ${workItemCount}` : `${workItemCount}`} 个
                        </div>
                    </div>

                    <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
                        <div className="flex items-center gap-2">
                            <label className="text-xs font-medium text-[var(--app-hint)]">
                                Repo
                            </label>
                            <select
                                value={repoFilter}
                                onChange={(e) => setRepoFilter(e.target.value)}
                                className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)]"
                                title="按仓库过滤"
                            >
                                <option value="__all__">All</option>
                                {repoOptions.map((repo) => (
                                    <option key={repo} value={repo}>{repo}</option>
                                ))}
                            </select>
                            <button
                                type="button"
                                onClick={() => {
                                    const repos = reposQuery.data?.repos ?? []
                                    setRepoEditorText(repos.join('\n'))
                                    setIsRepoEditorOpen(true)
                                }}
                                className="rounded-md border border-[var(--app-border)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                title="配置要同步的仓库列表"
                            >
                                管理仓库
                            </button>
                        </div>
                        <div className="text-[11px] text-[var(--app-hint)]">
                            同步仓库：{reposQuery.data?.repos?.length ? reposQuery.data.repos.join(', ') : '未配置（将尝试从 git remote 推断）'}
                        </div>
                    </div>
                </div>
            </div>

            <div className="flex-1 min-h-0 flex">
                <div className="flex-1 min-h-0 overflow-x-auto overflow-y-hidden p-3 pb-[calc(0.75rem+env(safe-area-inset-bottom))]">
                    <DndContext sensors={sensors} onDragStart={onDragStart} onDragEnd={onDragEnd}>
                        <div className="flex h-full items-start gap-3">
                            {columns.map(col => (
                                <Column
                                    key={col.id}
                                    id={col.id}
                                    name={col.name}
                                    workItemKeys={visibleCardsByColumn[col.id] ?? []}
                                    renderCard={renderCard}
                                />
                            ))}
                        </div>
                        <DragOverlay>
                            {dndEnabled && overlayCard ? (
                                <div className="w-[320px] opacity-95 rotate-[0.4deg]">
                                    <div className="rounded-lg border border-[var(--app-divider)] bg-[var(--app-bg)] p-3 shadow-lg">
                                        <div className="flex items-start justify-between gap-3">
                                            <div className="min-w-0 flex items-start gap-2">
                                                <StatusDot tone={overlayCard.badge.tone} />
                                                <div className="min-w-0">
                                                    <div className="truncate text-sm font-semibold text-[var(--app-fg)]">
                                                        {overlayCard.title}
                                                    </div>
                                                    <div className="mt-1 truncate text-xs text-[var(--app-hint)]">
                                                        {overlayCard.subtitle}
                                                    </div>
                                                </div>
                                            </div>
                                            <Badge badge={overlayCard.badge} />
                                        </div>
                                    </div>
                                </div>
                            ) : null}
                        </DragOverlay>
                    </DndContext>
                </div>

                {selectedItem ? (
                    <>
                        <button
                            type="button"
                            onClick={() => setSelectedWorkItemKey(null)}
                            className="lg:hidden fixed inset-0 z-40 bg-black/40"
                            aria-label="Close drawer backdrop"
                        />
                        <aside className="fixed inset-y-0 right-0 z-50 w-full sm:w-[420px] lg:static lg:z-auto lg:w-[420px] shrink-0 border-l border-[var(--app-divider)] bg-[var(--app-secondary-bg)] min-h-0 flex flex-col">
                        <div className="px-4 pt-4 pb-3 border-b border-[var(--app-divider)] bg-[var(--app-bg)]">
                            <div className="flex items-start justify-between gap-3">
                                <div className="min-w-0">
                                    <div className="text-xs text-[var(--app-hint)]">
                                        {selectedItem.repo} · {kindShort(selectedItem.kind)} #{selectedItem.number}
                                    </div>
                                    <div className="mt-1 text-sm font-semibold text-[var(--app-fg)] leading-snug">
                                        {selectedItem.title}
                                    </div>
                                    <div className="mt-2 flex flex-wrap items-center gap-2">
                                        {jobToBadge(latestJobByKey.get(selectedItem.workItemKey) ?? null) ? (
                                            <Badge badge={jobToBadge(latestJobByKey.get(selectedItem.workItemKey) ?? null)!} />
                                        ) : null}
                                        <Badge badge={{ text: (selectedItem.state || 'STATE').toUpperCase(), tone: selectedItem.state?.toLowerCase() === 'open' ? 'ok' : 'info' }} />
                                        {draftModel.trim() ? <Badge badge={{ text: `MODEL:${draftModel.trim()}`, tone: 'info' }} /> : null}
                                        {draftReasoningEffort !== 'auto' ? <Badge badge={{ text: `EFFORT:${draftReasoningEffort}`, tone: 'info' }} /> : null}
                                    </div>
                                </div>
                                <button
                                    type="button"
                                    onClick={() => setSelectedWorkItemKey(null)}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                    title="关闭详情"
                                >
                                    关闭
                                </button>
                            </div>

                            <div className="mt-3 flex flex-wrap items-center gap-2">
                                <button
                                    type="button"
                                    onClick={() => {
                                        try {
                                            window.open(selectedItem.url, '_blank', 'noreferrer')
                                        } catch {
                                            window.location.href = selectedItem.url
                                        }
                                    }}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                >
                                    打开 GitHub
                                </button>
                                <button
                                    type="button"
                                    onClick={() => void (async () => {
                                        if (settingsDirty) {
                                            await saveSelectedSettings()
                                        }
                                        await startSelected()
                                    })().catch((error) => {
                                        const message = error instanceof Error ? error.message : 'Start failed'
                                        addToast({ title: 'Start failed', body: message, sessionId: selectedItem.workItemKey, url: selectedItem.url })
                                    })}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-fg)] px-2 py-1 text-xs font-semibold text-[var(--app-bg)] hover:opacity-90 disabled:opacity-50"
                                    disabled={selectedItem.state?.toLowerCase() !== 'open'}
                                    title="移动到 In Progress 并触发 job"
                                >
                                    Start
                                </button>
                                <button
                                    type="button"
                                    onClick={() => void closeSelected().catch((error) => {
                                        const message = error instanceof Error ? error.message : 'Close failed'
                                        addToast({ title: 'Close failed', body: message, sessionId: selectedItem.workItemKey, url: selectedItem.url })
                                    })}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] disabled:opacity-50"
                                    disabled={selectedItem.state?.toLowerCase() !== 'open'}
                                    title="关闭 GitHub issue/pr"
                                >
                                    Close
                                </button>
                                <button
                                    type="button"
                                    onClick={() => void Promise.allSettled([workItemsQuery.refetch(), jobsQuery.refetch(), detailQuery.refetch()])}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                    title="刷新"
                                >
                                    刷新
                                </button>
                            </div>
                        </div>

                        <div className="flex-1 min-h-0 overflow-y-auto px-4 py-4 space-y-4">
                            <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-bg)] p-3">
                                <div className="flex items-center justify-between gap-3">
                                    <div className="text-xs font-semibold text-[var(--app-fg)]">Run Settings</div>
                                    <button
                                        type="button"
                                        onClick={() => void saveSelectedSettings().catch((error) => {
                                            const message = error instanceof Error ? error.message : 'Save failed'
                                            addToast({ title: 'Save failed', body: message, sessionId: selectedItem.workItemKey, url: selectedItem.url })
                                        })}
                                        disabled={!settingsDirty || settingsBusy}
                                        className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] disabled:opacity-50"
                                    >
                                        保存
                                    </button>
                                </div>
	                                <div className="mt-2 grid gap-2">
	                                    <div>
	                                        <div className="text-[11px] font-medium text-[var(--app-hint)]">模型（per-item）</div>
	                                        <input
                                            value={draftModel}
                                            onChange={(e) => setDraftModel(e.target.value)}
                                            list="codex-kanban-models"
                                            placeholder="例如：gpt-5.2 / gpt-4.1 / o3"
                                            className="mt-1 w-full rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs text-[var(--app-fg)] placeholder:text-[var(--app-hint)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] focus:ring-offset-2 focus:ring-offset-[var(--app-bg)]"
                                        />
                                        <datalist id="codex-kanban-models">
                                            {availableModels.map((model) => (
                                                <option key={model.id} value={model.id}>
                                                    {model.displayName}
                                                </option>
                                            ))}
                                        </datalist>
                                        <div className="mt-1 text-[10px] text-[var(--app-hint)]">
                                            {defaultModel ? `默认：${defaultModel.displayName} (${defaultModel.id})` : ''}
                                        </div>
                                    </div>
                                    <div>
                                        <div className="text-[11px] font-medium text-[var(--app-hint)]">思考等级（per-item）</div>
                                        <select
                                            value={draftReasoningEffort}
                                            onChange={(e) => setDraftReasoningEffort(e.target.value as ReasoningEffortPreference)}
                                            className="mt-1 w-full rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs text-[var(--app-fg)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] focus:ring-offset-2 focus:ring-offset-[var(--app-bg)]"
                                        >
                                            <option value="auto">auto（模型默认）</option>
                                            {supportedEfforts.length ? supportedEfforts.map((preset) => (
                                                <option key={preset.effort} value={preset.effort}>
                                                    {preset.effort}{preset.description ? ` — ${preset.description}` : ''}
                                                </option>
                                            )) : (
                                                <>
                                                    <option value="none">none</option>
                                                    <option value="minimal">minimal</option>
                                                    <option value="low">low</option>
                                                    <option value="medium">medium</option>
                                                    <option value="high">high</option>
                                                    <option value="xhigh">xhigh</option>
                                                </>
                                            )}
                                        </select>
                                    </div>
                                    <div>
                                        <div className="text-[11px] font-medium text-[var(--app-hint)]">执行 Prompt 前缀（per-item）</div>
                                        <textarea
                                            value={draftPromptPrefix}
                                            onChange={(e) => setDraftPromptPrefix(e.target.value)}
                                            placeholder="例如：你是我的 GitHub 工单助理。优先产出可合并的最小 PR，并在最后给出测试命令。"
                                            rows={4}
                                            className="mt-1 w-full rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-2 text-xs text-[var(--app-fg)] placeholder:text-[var(--app-hint)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] focus:ring-offset-2 focus:ring-offset-[var(--app-bg)]"
                                        />
                                    </div>
	                                </div>
	                            </div>

	                            <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-bg)] p-3">
	                                <div className="flex items-center justify-between gap-3">
	                                    <div className="text-xs font-semibold text-[var(--app-fg)]">Run Status</div>
	                                    <div className="flex items-center gap-2">
	                                        {selectedJobIsLive ? (
	                                            <span className="text-[11px] font-medium text-[var(--app-hint)]">自动刷新中…</span>
	                                        ) : null}
	                                        {selectedJob ? (
	                                            (() => {
	                                                const badge = jobStatusToBadge(selectedJob.status)
	                                                return badge ? <Badge badge={badge} /> : null
	                                            })()
	                                        ) : null}
	                                    </div>
	                                </div>

	                                {selectedJob ? (
	                                    <>
	                                        <div className="mt-2 grid gap-1 text-[11px] text-[var(--app-hint)]">
	                                            <div>job: {formatShortId(selectedJob.jobId)}</div>
	                                            {selectedJob.createdAt ? <div>created: {formatUpdatedAt(selectedJob.createdAt)}</div> : null}
	                                            {selectedJob.startedAt ? <div>started: {formatUpdatedAt(selectedJob.startedAt)}</div> : null}
	                                            {selectedJob.finishedAt ? <div>finished: {formatUpdatedAt(selectedJob.finishedAt)}</div> : null}
	                                            {selectedJob.threadId ? <div>thread: {formatShortId(selectedJob.threadId)}</div> : null}
	                                        </div>

	                                        <div className="mt-2 flex items-center justify-between gap-2">
	                                            <div className="text-[11px] font-medium text-[var(--app-hint)]">
	                                                Log
	                                            </div>
	                                            <div className="flex items-center gap-2">
	                                                <button
	                                                    type="button"
	                                                    onClick={() => void jobLogQuery.refetch()}
	                                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-[11px] font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
	                                                >
	                                                    刷新日志
	                                                </button>
	                                                <button
	                                                    type="button"
	                                                    onClick={() => {
	                                                        const threadId = selectedJob.threadId
	                                                        if (!threadId) {
	                                                            return
	                                                        }
	                                                        window.location.href = `/sessions/${encodeURIComponent(threadId)}`
	                                                    }}
	                                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-[11px] font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] disabled:opacity-50"
	                                                    disabled={!selectedJob.threadId}
	                                                >
	                                                    打开 Thread
	                                                </button>
	                                            </div>
	                                        </div>

	                                        <pre className="mt-2 max-h-[320px] overflow-auto rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] p-2 text-[11px] leading-relaxed text-[var(--app-fg)] whitespace-pre-wrap">
	                                            {jobLogQuery.data?.logText ?? (jobLogQuery.isLoading ? '加载日志…' : '暂无日志或加载失败。')}
	                                            {jobLogQuery.data?.truncated ? '\n\n[truncated]' : ''}
	                                        </pre>
	                                    </>
	                                ) : (
	                                    <div className="mt-2 text-xs text-[var(--app-hint)]">
	                                        暂无运行记录：点击 Start 或拖到 In Progress 后将自动展示日志。
	                                    </div>
	                                )}
	                            </div>

	                            <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-bg)] p-3">
	                                <div className="flex items-center justify-between gap-3">
	                                    <div className="text-xs font-semibold text-[var(--app-fg)]">Preview</div>
	                                    <div className="text-[11px] text-[var(--app-hint)]">
                                        {detailQuery.isFetching ? '加载中…' : ''}
                                    </div>
                                </div>
                                <div className="mt-2">
                                    {detailQuery.data?.body ? (
                                        <StandaloneMarkdown content={detailQuery.data.body} />
                                    ) : (
                                        <div className="text-xs text-[var(--app-hint)]">
                                            {detailQuery.isLoading ? '加载正文…' : '暂无正文或加载失败。'}
                                        </div>
                                    )}
                                </div>
                            </div>

                            <div className="rounded-lg border border-[var(--app-border)] bg-[var(--app-bg)] p-3">
                                <div className="flex items-center justify-between gap-3">
                                    <div className="text-xs font-semibold text-[var(--app-fg)]">Jobs</div>
                                    <div className="text-[11px] text-[var(--app-hint)]">{selectedJobs.length} 条</div>
                                </div>
	                                <div className="mt-2 grid gap-1">
	                                    {selectedJobs.length ? selectedJobs.slice(0, 12).map((job) => (
	                                        <button
	                                            key={job.jobId}
	                                            type="button"
	                                            onClick={() => setSelectedJobId(job.jobId)}
	                                            className={[
	                                                'w-full rounded-md border px-2 py-1 text-left text-xs transition-colors',
	                                                selectedJobId === job.jobId
	                                                    ? 'border-[var(--app-link)] bg-[var(--app-subtle-bg)]'
	                                                    : 'border-[var(--app-border)] hover:bg-[var(--app-subtle-bg)]'
	                                            ].join(' ')}
	                                        >
	                                            <div className="flex items-center justify-between gap-2">
	                                                <div className="min-w-0 truncate font-medium text-[var(--app-fg)]">
	                                                    {formatShortId(job.jobId)}
	                                                </div>
	                                                <div className="shrink-0 flex items-center gap-2">
	                                                    {(() => {
	                                                        const badge = jobStatusToBadge(job.status)
	                                                        return badge ? <Badge badge={badge} /> : null
	                                                    })()}
	                                                    <div className="text-[10px] text-[var(--app-hint)]">
	                                                        {job.createdAt ? formatUpdatedAt(job.createdAt) : ''}
	                                                    </div>
	                                                </div>
	                                            </div>
	                                            {job.threadId ? (
	                                                <div className="mt-0.5 text-[10px] text-[var(--app-hint)]">
	                                                    thread: {formatShortId(job.threadId)}
	                                                </div>
                                            ) : null}
                                            {job.lastError ? (
                                                <div className="mt-0.5 text-[10px] text-rose-600">
                                                    {job.lastError}
                                                </div>
                                            ) : null}
                                        </button>
                                    )) : (
                                        <div className="text-xs text-[var(--app-hint)]">
                                            暂无 job：拖到 In Progress 或点击 Start。
                                        </div>
	                                    )}
	                                </div>
	                            </div>
	                        </div>
	                    </aside>
                    </>
                ) : null}
            </div>

            {isRepoEditorOpen ? (
                <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
                    <div className="w-full max-w-[640px] rounded-xl border border-[var(--app-divider)] bg-[var(--app-bg)] shadow-xl">
                        <div className="flex items-center justify-between gap-3 border-b border-[var(--app-divider)] px-4 py-3">
                            <div className="text-sm font-semibold text-[var(--app-fg)]">同步仓库（多 repo）</div>
                            <button
                                type="button"
                                onClick={() => setIsRepoEditorOpen(false)}
                                className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-2 py-1 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                            >
                                关闭
                            </button>
                        </div>
                        <div className="px-4 py-4">
                            <div className="text-xs text-[var(--app-hint)]">
                                每行一个 `owner/repo`。保存后将立即触发一次同步。
                            </div>
                            <textarea
                                value={repoEditorText}
                                onChange={(e) => setRepoEditorText(e.target.value)}
                                placeholder="openai/codex\nBloopAI/vibe-kanban"
                                rows={8}
                                className="mt-2 w-full rounded-lg border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-3 py-2 text-sm text-[var(--app-fg)] placeholder:text-[var(--app-hint)] focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] focus:ring-offset-2 focus:ring-offset-[var(--app-bg)]"
                            />
                            <div className="mt-3 flex items-center justify-end gap-2">
                                <button
                                    type="button"
                                    onClick={() => setIsRepoEditorOpen(false)}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-secondary-bg)] px-3 py-1.5 text-xs font-medium text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)]"
                                    disabled={repoEditorBusy}
                                >
                                    取消
                                </button>
                                <button
                                    type="button"
                                    onClick={() => void (async () => {
                                        if (!api) {
                                            return
                                        }
                                        setRepoEditorBusy(true)
                                        try {
                                            const repos = repoEditorText
                                                .split('\n')
                                                .map(s => s.trim())
                                                .filter(Boolean)
                                            await api.setGithubRepos({ repos })
                                            await Promise.allSettled([reposQuery.refetch(), workItemsQuery.refetch(), jobsQuery.refetch(), kanbanQuery.refetch()])
                                            setIsRepoEditorOpen(false)
                                        } catch (error) {
                                            const message = error instanceof Error ? error.message : 'Save repos failed'
                                            addToast({ title: 'Save repos failed', body: message, sessionId: '', url: '' })
                                        } finally {
                                            setRepoEditorBusy(false)
                                        }
                                    })()}
                                    className="rounded-md border border-[var(--app-border)] bg-[var(--app-fg)] px-3 py-1.5 text-xs font-semibold text-[var(--app-bg)] hover:opacity-90 disabled:opacity-50"
                                    disabled={repoEditorBusy}
                                >
                                    保存并同步
                                </button>
                            </div>
                        </div>
                    </div>
                </div>
            ) : null}
        </div>
    )
}
