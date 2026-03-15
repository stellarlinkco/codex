import { useCallback, useMemo } from 'react'
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core'
import type { DragEndEvent, DragStartEvent } from '@dnd-kit/core'
import { KanbanColumn } from './KanbanColumn'
import { KanbanCard } from './KanbanCard'
import type { CardData, ColumnData } from './types'

type KanbanBoardProps = {
    columns: ColumnData[]
    cardsByKey: Map<string, CardData>
    selectedCardKey: string | null
    isDragDisabled: boolean
    repoColors: Map<string, string>
    repoLabels: Map<string, string>
    activeCardKey: string | null
    onDragStart: (key: string) => void
    onDragEnd: (key: string, columnId: string, position: number) => void
    onDragCancel: () => void
    onSelectCard: (key: string) => void
}

export function KanbanBoard({
    columns,
    cardsByKey,
    selectedCardKey,
    isDragDisabled,
    repoColors,
    repoLabels,
    activeCardKey,
    onDragStart,
    onDragEnd,
    onDragCancel,
    onSelectCard,
}: KanbanBoardProps) {
    const sensors = useSensors(
        useSensor(PointerSensor, {
            activationConstraint: { distance: 5 },
        })
    )

    const handleDragStart = useCallback((event: DragStartEvent) => {
        onDragStart(String(event.active.id))
    }, [onDragStart])

    const handleDragEnd = useCallback((event: DragEndEvent) => {
        const { active, over } = event
        if (!over) {
            onDragCancel()
            return
        }

        const cardKey = String(active.id)
        const overId = String(over.id)

        // Determine target column and position
        let targetColumnId: string
        let position: number

        // Check if dropped on a column directly
        const targetColumn = columns.find(c => c.id === overId)
        if (targetColumn) {
            targetColumnId = targetColumn.id
            position = targetColumn.cardKeys.length
        } else {
            // Dropped on another card - find its column
            const overCard = cardsByKey.get(overId)
            if (!overCard) {
                onDragCancel()
                return
            }
            const col = columns.find(c => c.cardKeys.includes(overId))
            if (!col) {
                onDragCancel()
                return
            }
            targetColumnId = col.id
            position = col.cardKeys.indexOf(overId)
        }

        onDragEnd(cardKey, targetColumnId, position)
    }, [columns, cardsByKey, onDragEnd, onDragCancel])

    const columnCards = useMemo(() => {
        return columns.map(col => ({
            column: col,
            cards: col.cardKeys
                .map(key => cardsByKey.get(key))
                .filter((c): c is CardData => c !== undefined),
        }))
    }, [columns, cardsByKey])

    const activeCard = activeCardKey ? cardsByKey.get(activeCardKey) : null

    return (
        <DndContext
            sensors={sensors}
            onDragStart={handleDragStart}
            onDragEnd={handleDragEnd}
            onDragCancel={onDragCancel}
        >
            <div className="flex-1 overflow-x-auto overflow-y-hidden">
                <div className="flex gap-3 sm:gap-4 p-3 sm:p-4 h-full min-w-max sm:min-w-max snap-x snap-mandatory sm:snap-none">
                    {columnCards.map(({ column, cards }) => (
                        <KanbanColumn
                            key={column.id}
                            column={column}
                            cards={cards}
                            selectedCardKey={selectedCardKey}
                            isDragDisabled={isDragDisabled}
                            repoColors={repoColors}
                            repoLabels={repoLabels}
                            onSelectCard={onSelectCard}
                        />
                    ))}
                </div>
            </div>

            <DragOverlay>
                {activeCard && (
                    <div className="w-[320px] rotate-2 scale-105 opacity-90">
                        <KanbanCard
                            card={activeCard}
                            isSelected={false}
                            isDragDisabled
                            repoColor={repoColors.get(activeCard.item.repo)}
                            repoLabel={repoLabels.get(activeCard.item.repo)}
                            onSelect={() => {}}
                        />
                    </div>
                )}
            </DragOverlay>
        </DndContext>
    )
}
