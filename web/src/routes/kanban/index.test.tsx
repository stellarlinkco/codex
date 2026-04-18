import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { KanbanPage } from './index'

const navigateMock = vi.fn()
const addToastMock = vi.fn()

const sessionsState = {
    sessions: [] as Array<{
        id: string
        active: boolean
        thinking: boolean
        activeAt: number
        updatedAt: number
        metadata: {
            name?: string
            path?: string
            flavor?: string | null
            summary?: { text: string }
            worktree?: { basePath: string }
        } | null
        todoProgress: { completed: number; total: number } | null
        pendingRequestsCount: number
    }>,
}

const sessionKanbanState = {
    data: {
        columns: [
            { id: 'backlog', name: 'Backlog', position: 0 },
            { id: 'in-progress', name: 'In Progress', position: 1 },
        ],
        cardPositions: {
            'session-1': { columnId: 'backlog', position: 0 },
        },
    },
}

vi.mock('@tanstack/react-router', () => ({
    useNavigate: () => navigateMock,
}))

vi.mock('@/lib/app-context', () => ({
    useAppContext: () => ({
        api: {
            moveKanbanCard: vi.fn(),
        },
    }),
}))

vi.mock('@/lib/toast-context', () => ({
    useToast: () => ({
        addToast: addToastMock,
    }),
}))

vi.mock('@/hooks/queries/useSessions', () => ({
    useSessions: () => ({
        sessions: sessionsState.sessions,
        isLoading: false,
        error: null,
        refetch: vi.fn(),
    }),
}))

vi.mock('@/hooks/queries/useKanban', () => ({
    useKanban: () => ({
        data: sessionKanbanState.data,
        refetch: vi.fn(),
    }),
}))

vi.mock('@/hooks/queries/useGithubRepos', () => ({
    useGithubRepos: () => ({
        data: { repos: [] },
    }),
}))

vi.mock('@/hooks/queries/useGithubWorkItems', () => ({
    useGithubWorkItems: () => ({ data: { items: [] } }),
}))

vi.mock('@/hooks/queries/useGithubKanban', () => ({
    useGithubKanban: () => ({ data: null, refetch: vi.fn() }),
}))

vi.mock('@/hooks/queries/useGithubJobs', () => ({
    useGithubJobs: () => ({ data: { jobs: [] } }),
}))

vi.mock('@/hooks/queries/useGithubJobLog', () => ({
    useGithubJobLog: () => ({ data: null, refetch: vi.fn() }),
}))

vi.mock('@/hooks/queries/useModelsCatalog', () => ({
    useModelsCatalog: () => ({ data: { models: [] } }),
}))

vi.mock('@/hooks/queries/useGithubWorkItemDetail', () => ({
    useGithubWorkItemDetail: () => ({ data: null, isLoading: false }),
}))

vi.mock('@/hooks/queries/useWorkspaces', () => ({
    useWorkspaces: () => ({ data: [], refetch: vi.fn() }),
}))

vi.mock('@/hooks/queries/useWorkspace', () => ({
    useWorkspace: () => ({ data: null }),
}))

vi.mock('@/hooks/queries/useWorkspaceWorkItems', () => ({
    useWorkspaceWorkItems: () => ({ data: { items: [] } }),
}))

vi.mock('@/hooks/queries/useWorkspaceKanban', () => ({
    useWorkspaceKanban: () => ({ data: null, refetch: vi.fn() }),
}))

vi.mock('@/hooks/queries/useWorkspaceJobs', () => ({
    useWorkspaceJobs: () => ({ data: { jobs: [] } }),
}))

vi.mock('@/hooks/queries/useWorkspaceJobLog', () => ({
    useWorkspaceJobLog: () => ({ data: null, refetch: vi.fn() }),
}))

vi.mock('./CardDetailPanel', () => ({
    CardDetailPanel: () => null,
}))

vi.mock('./WorkspaceDialog', () => ({
    WorkspaceDialog: () => null,
}))

vi.mock('./JobLogViewer', () => ({
    JobLogViewer: () => null,
}))

function renderPage() {
    const queryClient = new QueryClient({
        defaultOptions: {
            queries: {
                retry: false,
            },
        },
    })

    return render(
        <QueryClientProvider client={queryClient}>
            <KanbanPage />
        </QueryClientProvider>
    )
}

describe('KanbanPage', () => {
    beforeEach(() => {
        navigateMock.mockReset()
        addToastMock.mockReset()
        sessionsState.sessions = [{
            id: 'session-1',
            active: true,
            thinking: false,
            activeAt: 1,
            updatedAt: 1_710_000_000,
            metadata: {
                name: 'Ubuntu verification session',
                path: '/tmp',
                flavor: 'codex',
            },
            todoProgress: null,
            pendingRequestsCount: 0,
        }]
    })

    it('renders session cards after switching to Sessions scope', () => {
        renderPage()

        fireEvent.click(screen.getAllByRole('button', { name: 'Sessions' })[1])

        expect(screen.getByText('Backlog')).toBeInTheDocument()
        expect(screen.getByText('Ubuntu verification session')).toBeInTheDocument()
        expect(screen.getByText('/tmp')).toBeInTheDocument()
    })
})
