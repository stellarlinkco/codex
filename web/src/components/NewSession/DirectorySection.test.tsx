import { describe, it, expect, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { I18nContext } from '@/lib/i18n-context'
import { DirectorySection } from './DirectorySection'

function renderWithSpyT(ui: React.ReactElement) {
    const spyT = vi.fn((key: string) => key)
    render(
        <I18nContext.Provider value={{ t: spyT, locale: 'en', setLocale: vi.fn() }}>
            {ui}
        </I18nContext.Provider>
    )
    return spyT
}

describe('DirectorySection', () => {
    it('shows a not-found message when directoryExists is false', () => {
        renderWithSpyT(
            <DirectorySection
                directory="/missing"
                directoryExists={false}
                suggestions={[]}
                selectedIndex={-1}
                isDisabled={false}
                recentPaths={[]}
                onDirectoryChange={vi.fn()}
                onDirectoryFocus={vi.fn()}
                onDirectoryBlur={vi.fn()}
                onDirectoryKeyDown={vi.fn()}
                onSuggestionSelect={vi.fn()}
                onPathClick={vi.fn()}
                onPathRemove={vi.fn()}
                onClearRecentPaths={vi.fn()}
            />
        )

        expect(screen.getByText('newSession.directory.notFound')).toBeInTheDocument()
    })

    it('supports removing and clearing recent paths', () => {
        const onPathClick = vi.fn()
        const onPathRemove = vi.fn()
        const onClearRecentPaths = vi.fn()

        renderWithSpyT(
            <DirectorySection
                directory=""
                suggestions={[]}
                selectedIndex={-1}
                isDisabled={false}
                recentPaths={['/a']}
                onDirectoryChange={vi.fn()}
                onDirectoryFocus={vi.fn()}
                onDirectoryBlur={vi.fn()}
                onDirectoryKeyDown={vi.fn()}
                onSuggestionSelect={vi.fn()}
                onPathClick={onPathClick}
                onPathRemove={onPathRemove}
                onClearRecentPaths={onClearRecentPaths}
            />
        )

        fireEvent.click(screen.getByRole('button', { name: '/a' }))
        expect(onPathClick).toHaveBeenCalledWith('/a')

        fireEvent.click(screen.getByRole('button', { name: 'newSession.recent.clear' }))
        expect(onClearRecentPaths).toHaveBeenCalledTimes(1)

        fireEvent.click(screen.getByLabelText('newSession.recent.remove'))
        expect(onPathRemove).toHaveBeenCalledWith('/a')
    })
})
