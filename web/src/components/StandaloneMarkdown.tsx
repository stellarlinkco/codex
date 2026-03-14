import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { cn } from '@/lib/utils'

export function StandaloneMarkdown(props: { content: string; className?: string }) {
    return (
        <div
            className={cn(
                'markdown-content min-w-0 max-w-full break-words text-sm leading-relaxed text-[var(--app-fg)]',
                props.className
            )}
        >
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {props.content}
            </ReactMarkdown>
        </div>
    )
}
