import { memo, useCallback, useEffect, useRef, useState } from 'react'

type JobLogViewerProps = {
    jobId: string | null
    logText: string | null
    logLoading: boolean
    truncated: boolean
    onClose: () => void
    onRefresh: () => void
}

function CloseIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
        </svg>
    )
}

function RefreshIcon({ className }: { className?: string }) {
    return (
        <svg className={className} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M21.5 2v6h-6M2.5 22v-6h6M2 11.5a10 10 0 0 1 18.8-4.3M22 12.5a10 10 0 0 1-18.8 4.2" />
        </svg>
    )
}

export const JobLogViewer = memo(function JobLogViewer({
    jobId,
    logText,
    logLoading,
    truncated,
    onClose,
    onRefresh,
}: JobLogViewerProps) {
    const scrollRef = useRef<HTMLPreElement>(null)
    const [autoScroll, setAutoScroll] = useState(true)

    useEffect(() => {
        if (autoScroll && scrollRef.current) {
            scrollRef.current.scrollTop = scrollRef.current.scrollHeight
        }
    }, [logText, autoScroll])

    const handleScroll = useCallback(() => {
        if (!scrollRef.current) return
        const { scrollTop, scrollHeight, clientHeight } = scrollRef.current
        const atBottom = scrollHeight - scrollTop - clientHeight < 40
        setAutoScroll(atBottom)
    }, [])

    if (!jobId) return null

    return (
        <div className="border-t border-[var(--app-border)] bg-[var(--app-bg)] flex flex-col h-[50vh] sm:h-[280px]">
            {/* Log header */}
            <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--app-border)] shrink-0">
                <div className="flex items-center gap-2">
                    <h3 className="text-xs font-semibold text-[var(--app-fg)]">Job Log</h3>
                    <span className="text-[10px] text-[var(--app-hint)] font-mono">{jobId.slice(0, 8)}</span>
                    {truncated && (
                        <span className="text-[10px] text-yellow-400">(truncated)</span>
                    )}
                </div>
                <div className="flex items-center gap-1.5">
                    {!autoScroll && (
                        <button
                            type="button"
                            onClick={() => {
                                setAutoScroll(true)
                                if (scrollRef.current) {
                                    scrollRef.current.scrollTop = scrollRef.current.scrollHeight
                                }
                            }}
                            className="text-[10px] text-[var(--app-link)] hover:underline"
                        >
                            Scroll to bottom
                        </button>
                    )}
                    <button
                        type="button"
                        onClick={onRefresh}
                        className="flex items-center justify-center w-6 h-6 rounded text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                        title="Refresh log"
                    >
                        <RefreshIcon />
                    </button>
                    <button
                        type="button"
                        onClick={onClose}
                        className="flex items-center justify-center w-6 h-6 rounded text-[var(--app-hint)] hover:text-[var(--app-fg)] hover:bg-[var(--app-subtle-bg)] transition-colors"
                    >
                        <CloseIcon />
                    </button>
                </div>
            </div>

            {/* Log content */}
            <pre
                ref={scrollRef}
                onScroll={handleScroll}
                className="flex-1 overflow-auto p-3 text-[11px] leading-relaxed font-mono text-[var(--app-fg)] whitespace-pre-wrap break-all bg-[var(--app-code-bg)]"
            >
                {logLoading ? (
                    <span className="text-[var(--app-hint)]">Loading log...</span>
                ) : logText ? (
                    logText
                ) : (
                    <span className="text-[var(--app-hint)]">No log output yet.</span>
                )}
            </pre>
        </div>
    )
})
