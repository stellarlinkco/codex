import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { I18nContext } from '@/lib/i18n-context'
import type { Machine } from '@/types/api'
import { NewSession } from './index'

const spawnSessionMock = vi.fn()
vi.mock('@/hooks/mutations/useSpawnSession', () => ({
    useSpawnSession: () => ({
        spawnSession: spawnSessionMock,
        isPending: false,
        error: null,
    }),
}))

const sessionsMock: unknown[] = []
const refetchSessionsMock = vi.fn()
vi.mock('@/hooks/queries/useSessions', () => ({
    useSessions: () => ({
        sessions: sessionsMock,
        isLoading: false,
        error: null,
        refetch: refetchSessionsMock,
    }),
}))

vi.mock('@/hooks/usePlatform', () => ({
    usePlatform: () => ({
        haptic: {
            notification: vi.fn(),
            impact: vi.fn(),
        },
    }),
}))

function renderWithSpyT(ui: React.ReactElement) {
    const spyT = vi.fn((key: string) => key)
    render(
        <I18nContext.Provider value={{ t: spyT, locale: 'en', setLocale: vi.fn() }}>
            {ui}
        </I18nContext.Provider>
    )
    return spyT
}

describe('NewSession', () => {
    beforeEach(() => {
        localStorage.clear()
        spawnSessionMock.mockReset()
        spawnSessionMock.mockResolvedValue({ type: 'success', sessionId: 's1' })
        sessionsMock.length = 0
    })

    it('includes reasoningEffort when creating a session', async () => {
        sessionsMock.push({
            id: 's0',
            active: false,
            thinking: false,
            activeAt: 0,
            updatedAt: 0,
            metadata: {
                path: '/repo',
                machineId: 'm1',
            },
        })

        const machines: Machine[] = [{
            id: 'm1',
            active: true,
            metadata: {
                host: 'h',
                platform: 'darwin',
                happyCliVersion: 'codex',
            },
        }]

        const api = {
            checkMachinePathsExists: vi.fn().mockResolvedValue({ exists: { '/repo': true } }),
        }

        renderWithSpyT(
            <NewSession
                api={api as never}
                machines={machines}
                onSuccess={vi.fn()}
                onCancel={vi.fn()}
            />
        )

        const selects = screen.getAllByRole('combobox')
        fireEvent.change(selects[0], { target: { value: 'm1' } })

        const directory = screen.getByPlaceholderText('newSession.placeholder')
        fireEvent.change(directory, { target: { value: '/repo' } })

        const reasoningEffortSelect = selects[2]
        fireEvent.change(reasoningEffortSelect, { target: { value: 'high' } })

        fireEvent.click(screen.getByRole('button', { name: 'newSession.create' }))

        await waitFor(() => {
            expect(spawnSessionMock).toHaveBeenCalled()
        })
        expect(spawnSessionMock).toHaveBeenCalledWith(expect.objectContaining({
            machineId: 'm1',
            directory: '/repo',
            reasoningEffort: 'high',
        }))
    })

    it('disables create when the directory does not exist', async () => {
        sessionsMock.push({
            id: 's0',
            active: false,
            thinking: false,
            activeAt: 0,
            updatedAt: 0,
            metadata: {
                path: '/missing',
                machineId: 'm1',
            },
        })

        const machines: Machine[] = [{
            id: 'm1',
            active: true,
            metadata: {
                host: 'h',
                platform: 'darwin',
                happyCliVersion: 'codex',
            },
        }]
        const api = {
            checkMachinePathsExists: vi.fn().mockResolvedValue({ exists: { '/missing': false } }),
        }

        renderWithSpyT(
            <NewSession
                api={api as never}
                machines={machines}
                onSuccess={vi.fn()}
                onCancel={vi.fn()}
            />
        )

        const selects = screen.getAllByRole('combobox')
        fireEvent.change(selects[0], { target: { value: 'm1' } })

        const directory = screen.getByPlaceholderText('newSession.placeholder')
        fireEvent.change(directory, { target: { value: '/missing' } })

        await waitFor(() => {
            expect(screen.getByText('newSession.directory.notFound')).toBeInTheDocument()
        })
        expect(screen.getByRole('button', { name: 'newSession.create' })).toBeDisabled()
    })

    it('removes and clears recent paths', async () => {
        localStorage.setItem('codex:lastMachineId', 'm1')
        localStorage.setItem('codex:recentPaths', JSON.stringify({ m1: ['/a', '/b'] }))

        const machines: Machine[] = [{
            id: 'm1',
            active: true,
            metadata: {
                host: 'h',
                platform: 'darwin',
                happyCliVersion: 'codex',
            },
        }]

        const api = {
            checkMachinePathsExists: vi.fn().mockResolvedValue({ exists: { '/a': true, '/b': true } }),
        }

        renderWithSpyT(
            <NewSession
                api={api as never}
                machines={machines}
                onSuccess={vi.fn()}
                onCancel={vi.fn()}
            />
        )

        const removeButtons = await screen.findAllByLabelText('newSession.recent.remove')
        fireEvent.click(removeButtons[0])
        await waitFor(() => {
            const stored = JSON.parse(localStorage.getItem('codex:recentPaths') ?? '{}') as Record<string, string[]>
            expect(stored.m1).toEqual(['/b'])
        })

        fireEvent.click(screen.getByRole('button', { name: 'newSession.recent.clear' }))
        await waitFor(() => {
            const stored = JSON.parse(localStorage.getItem('codex:recentPaths') ?? '{}') as Record<string, string[]>
            expect(stored.m1).toEqual([])
        })
    })

    it('checks directory existence after a short debounce', async () => {
        const machines: Machine[] = [{
            id: 'm1',
            active: true,
            metadata: {
                host: 'h',
                platform: 'darwin',
                happyCliVersion: 'codex',
            },
        }]

        const api = {
            checkMachinePathsExists: vi.fn().mockResolvedValue({ exists: { '/repo': true } }),
        }

        renderWithSpyT(
            <NewSession
                api={api as never}
                machines={machines}
                onSuccess={vi.fn()}
                onCancel={vi.fn()}
            />
        )

        const selects = screen.getAllByRole('combobox')
        fireEvent.change(selects[0], { target: { value: 'm1' } })

        const directory = screen.getByPlaceholderText('newSession.placeholder')
        fireEvent.change(directory, { target: { value: '/repo' } })

        await new Promise((resolve) => setTimeout(resolve, 250))
        await waitFor(() => {
            expect(api.checkMachinePathsExists).toHaveBeenCalledWith('m1', ['/repo'])
        })
    })
})
