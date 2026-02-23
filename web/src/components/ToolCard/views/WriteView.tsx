import type { ToolViewProps } from '@/components/ToolCard/views/_all'
import { isObject } from '@codex/protocol'
import { DiffView } from '@/components/DiffView'

export function WriteView(props: ToolViewProps) {
    const input = props.block.tool.input
    if (!isObject(input)) return null

    const content = typeof input.content === 'string' ? input.content : typeof input.text === 'string' ? input.text : null
    if (content === null) return null

    return (
        <DiffView
            oldString=""
            newString={content}
            variant="inline"
        />
    )
}
