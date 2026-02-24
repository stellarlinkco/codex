import type { KeyboardEvent as ReactKeyboardEvent } from 'react'
import type { Suggestion } from '@/hooks/useActiveSuggestions'
import { Autocomplete } from '@/components/ChatInput/Autocomplete'
import { FloatingOverlay } from '@/components/ChatInput/FloatingOverlay'
import { useTranslation } from '@/lib/use-translation'

export function DirectorySection(props: {
    directory: string
    directoryExists?: boolean
    suggestions: readonly Suggestion[]
    selectedIndex: number
    isDisabled: boolean
    recentPaths: string[]
    onDirectoryChange: (value: string) => void
    onDirectoryFocus: () => void
    onDirectoryBlur: () => void
    onDirectoryKeyDown: (event: ReactKeyboardEvent<HTMLInputElement>) => void
    onSuggestionSelect: (index: number) => void
    onPathClick: (path: string) => void
    onPathRemove: (path: string) => void
    onClearRecentPaths: () => void
}) {
    const { t } = useTranslation()

    return (
        <div className="flex flex-col gap-1.5 px-3 py-3">
            <label className="text-xs font-medium text-[var(--app-hint)]">
                {t('newSession.directory')}
            </label>
            <div className="relative">
                <input
                    type="text"
                    placeholder={t('newSession.placeholder')}
                    value={props.directory}
                    onChange={(event) => props.onDirectoryChange(event.target.value)}
                    onKeyDown={props.onDirectoryKeyDown}
                    onFocus={props.onDirectoryFocus}
                    onBlur={props.onDirectoryBlur}
                    disabled={props.isDisabled}
                    className="w-full rounded-md border border-[var(--app-border)] bg-[var(--app-bg)] p-2 text-sm focus:outline-none focus:ring-2 focus:ring-[var(--app-link)] disabled:opacity-50"
                />
                {props.suggestions.length > 0 && (
                    <div className="absolute top-full left-0 right-0 z-10 mt-1">
                        <FloatingOverlay maxHeight={200}>
                            <Autocomplete
                                suggestions={props.suggestions}
                                selectedIndex={props.selectedIndex}
                                onSelect={props.onSuggestionSelect}
                            />
                        </FloatingOverlay>
                    </div>
                )}
            </div>

            {props.directory.trim().length > 0 && props.directoryExists === false ? (
                <div className="text-xs text-red-600">
                    {t('newSession.directory.notFound')}
                </div>
            ) : null}

            {props.recentPaths.length > 0 && (
                <div className="flex flex-col gap-1 mt-1">
                    <div className="flex items-center justify-between gap-2">
                        <span className="text-xs text-[var(--app-hint)]">
                            {t('newSession.recent')}:
                        </span>
                        <button
                            type="button"
                            onClick={props.onClearRecentPaths}
                            disabled={props.isDisabled}
                            className="text-xs text-[var(--app-link)] hover:underline disabled:opacity-50"
                        >
                            {t('newSession.recent.clear')}
                        </button>
                    </div>
                    <div className="flex flex-wrap gap-1">
                        {props.recentPaths.map((path) => (
                            <div
                                key={path}
                                className="flex items-stretch rounded bg-[var(--app-subtle-bg)] text-xs text-[var(--app-fg)] overflow-hidden max-w-[260px] disabled:opacity-50"
                            >
                                <button
                                    type="button"
                                    onClick={() => props.onPathClick(path)}
                                    disabled={props.isDisabled}
                                    className="px-2 py-1 hover:bg-[var(--app-secondary-bg)] transition-colors truncate max-w-[220px] disabled:opacity-50"
                                    title={path}
                                >
                                    {path}
                                </button>
                                <button
                                    type="button"
                                    onClick={() => props.onPathRemove(path)}
                                    disabled={props.isDisabled}
                                    className="px-2 py-1 text-[var(--app-hint)] hover:bg-[var(--app-secondary-bg)] hover:text-[var(--app-fg)] transition-colors disabled:opacity-50"
                                    aria-label={t('newSession.recent.remove')}
                                    title={t('newSession.recent.remove')}
                                >
                                    ×
                                </button>
                            </div>
                        ))}
                    </div>
                </div>
            )}
        </div>
    )
}
