import { memo } from 'react'
import { useDroppable } from '@dnd-kit/core'
import { SortableContext, verticalListSortingStrategy } from '@dnd-kit/sortable'
import { KanbanCard } from './KanbanCard'
import type { CardData, ColumnData } from './types'

type KanbanColumnProps = {
    column: ColumnData
    cards: CardData[]
    selectedCardKey: string | null
    isDragDisabled: boolean
    repoColors: Map<string, string>
    repoLabels: Map<string, string>
    onSelectCard: (key: string) => void
}

const KanbanColumn = memo(function KanbanColumn({
    column,
    cards,
    selectedCardKey,
    isDragDisabled,
    repoColors,
    repoLabels,
    onSelectCard,
}: KanbanColumnProps) {
    const { setNodeRef, isOver } = useDroppable({ id: column.id })

    return (
        <div className="flex flex-col shrink-0 w-[85vw] sm:w-[320px] min-w-[260px] sm:min-w-[280px] max-w-[360px] h-full snap-start">
            {/* Column header */}
            <div className="flex items-center gap-2 px-2 pb-3">
                <h3 className="text-xs font-semibold uppercase tracking-wider text-[var(--app-hint)]">
                    {column.name}
                </h3>
                <span className="inline-flex items-center justify-center min-w-[20px] h-5 px-1.5 rounded-full text-[10px] font-semibold bg-[var(--app-subtle-bg)] text-[var(--app-hint)]">
                    {cards.length}
                </span>
            </div>

            {/* Drop zone */}
            <div
                ref={setNodeRef}
                className={`
                    flex-1 overflow-y-auto space-y-2 px-1 pb-4 rounded-lg transition-colors duration-150 min-h-[120px]
                    ${isOver
                        ? 'bg-[color-mix(in_srgb,var(--app-link)_6%,transparent)] ring-1 ring-[var(--app-link)] ring-opacity-20 rounded-lg'
                        : ''
                    }
                `}
            >
                <SortableContext
                    items={column.cardKeys}
                    strategy={verticalListSortingStrategy}
                >
                    {cards.map(card => (
                        <KanbanCard
                            key={card.key}
                            card={card}
                            isSelected={selectedCardKey === card.key}
                            isDragDisabled={isDragDisabled}
                            repoColor={repoColors.get(card.item.repo)}
                            repoLabel={repoLabels.get(card.item.repo)}
                            onSelect={onSelectCard}
                        />
                    ))}
                </SortableContext>

                {cards.length === 0 && !isOver && (
                    <div className="flex items-center justify-center h-20 rounded-lg border border-dashed border-[var(--app-border)] text-xs text-[var(--app-hint)]">
                        Drag cards here
                    </div>
                )}
            </div>
        </div>
    )
}, (prev, next) => {
    return (
        prev.column.id === next.column.id
        && prev.column.name === next.column.name
        && prev.selectedCardKey === next.selectedCardKey
        && prev.isDragDisabled === next.isDragDisabled
        && prev.cards === next.cards
        && prev.repoColors === next.repoColors
        && prev.repoLabels === next.repoLabels
    )
})

export { KanbanColumn }
