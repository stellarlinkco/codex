import type { ToolViewProps } from '@/components/ToolCard/views/_all'
import { isObject } from '@codex/protocol'
import { MarkdownRenderer } from '@/components/MarkdownRenderer'

export function ExitPlanModeView(props: ToolViewProps) {
    const input = props.block.tool.input
    if (!isObject(input)) return null
    const plan = typeof input.plan === 'string' ? input.plan : null
    if (!plan) return null
    return <MarkdownRenderer content={plan} />
}
