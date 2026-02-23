import type { ToolViewProps } from '@/components/ToolCard/views/_all'
import { isObject } from '@codex/protocol'
import { basename, resolveDisplayPath } from '@/utils/path'

export function CodexPatchView(props: ToolViewProps) {
    const input = props.block.tool.input
    if (!isObject(input) || !isObject(input.changes)) return null

    const files = Object.keys(input.changes)
    if (files.length === 0) return null

    return (
        <div className="flex flex-col gap-1">
            {files.map((file) => {
                const display = resolveDisplayPath(file, props.metadata)
                return (
                    <div key={file} className="text-sm text-[var(--app-fg)] font-mono break-all">
                        {basename(display)}
                    </div>
                )
            })}
        </div>
    )
}
