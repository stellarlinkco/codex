import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { I18nContext } from '@/lib/i18n-context'
import { getToolResultViewComponent } from './_results'

function makeToolBlock(toolName: string, result: unknown) {
    return {
        kind: 'tool-call',
        id: 'block-1',
        localId: null,
        createdAt: 0,
        tool: {
            id: 'call-1',
            name: toolName,
            state: 'completed',
            input: {},
            createdAt: 0,
            startedAt: 0,
            completedAt: 0,
            description: null,
            result,
        },
        children: [],
    } as const
}

function renderWithT(ui: JSX.Element) {
    return render(
        <I18nContext.Provider value={{ t: (key: string) => key, locale: 'en', setLocale: vi.fn() }}>
            {ui}
        </I18nContext.Provider>
    )
}

describe('tool result views', () => {
    it('renders spawn_team results in a friendly format', () => {
        const View = getToolResultViewComponent('spawn_team')
        const block = makeToolBlock(
            'spawn_team',
            JSON.stringify({
                team_id: 't1',
                members: [{ name: 'alice', agent_id: 'a1', status: 'running' }],
            })
        )

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('Team: t1')).toBeInTheDocument()
        expect(screen.getByText('alice')).toBeInTheDocument()
        expect(screen.getByText('running')).toBeInTheDocument()
    })

    it('renders wait_team results in a friendly format', () => {
        const View = getToolResultViewComponent('wait_team')
        const block = makeToolBlock(
            'wait_team',
            JSON.stringify({
                team_id: 't1',
                completed: false,
                mode: 'all',
                triggered_member: { name: 'triggered' },
                member_statuses: [{ name: 'alice', agent_id: 'a1', state: { errored: 'boom' } }],
            })
        )

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('Team: t1')).toBeInTheDocument()
        expect(screen.getByText('Completed: no · Mode: all · Triggered: triggered')).toBeInTheDocument()
        expect(screen.getByText('alice')).toBeInTheDocument()
        expect(screen.getByText('errored: boom')).toBeInTheDocument()
    })

    it('falls back to GenericResultView for invalid JSON strings', () => {
        const View = getToolResultViewComponent('spawn_team')
        const block = makeToolBlock('spawn_team', '{bad}')

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('{bad}')).toBeInTheDocument()
    })

    it('renders task lists from object results', () => {
        const View = getToolResultViewComponent('team_task_list')
        const block = makeToolBlock('team_task_list', {
            teamId: 't1',
            tasks: [
                { task_id: 'task-1', title: 'Do thing', state: 'pending' },
                { title: 'No id' },
            ],
        })

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('Team: t1')).toBeInTheDocument()
        expect(screen.getByText('task-1 · Do thing')).toBeInTheDocument()
        expect(screen.getByText(/pending/)).toBeInTheDocument()
        expect(screen.getByText('task-2 · No id')).toBeInTheDocument()
    })

    it('renders closed member results and status detail objects', () => {
        const View = getToolResultViewComponent('close_team')
        const block = makeToolBlock(
            'close_team',
            JSON.stringify({
                team_id: 't1',
                closed: [
                    { name: 'alice', ok: true, status: { completed: 'ok' }, error: 'extra' },
                    { name: 'bob', ok: false, status: { queued: 'waiting' } },
                ],
            })
        )

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('Team: t1')).toBeInTheDocument()
        expect(screen.getByText('alice')).toBeInTheDocument()
        expect(screen.getByText('completed: ok · extra')).toBeInTheDocument()
        expect(screen.getByText('bob')).toBeInTheDocument()
        expect(screen.getByText('queued: waiting')).toBeInTheDocument()
    })

    it('formats non-string statuses via safeStringify', () => {
        const View = getToolResultViewComponent('spawn_team')
        const block = makeToolBlock(
            'spawn_team',
            JSON.stringify({
                team_id: 't1',
                members: [{ name: 'alice', agent_id: 'a1', status: 42 }],
            })
        )

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('42')).toBeInTheDocument()
    })

    it('falls back to GenericResultView when team id is missing', () => {
        const View = getToolResultViewComponent('spawn_team')
        const block = makeToolBlock('spawn_team', JSON.stringify({ foo: 'bar' }))

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.queryByText(/Team:/)).not.toBeInTheDocument()
        expect(screen.getByText(/foo/)).toBeInTheDocument()
    })

    it('falls back to JSON display when shape is unknown', () => {
        const View = getToolResultViewComponent('team_cleanup')
        const block = makeToolBlock('team_cleanup', JSON.stringify({ team_id: 't1', hello: 'world' }))

        renderWithT(<View block={block as never} metadata={null} />)
        expect(screen.getByText('Team: t1')).toBeInTheDocument()
        expect(screen.getByText(/hello/)).toBeInTheDocument()
    })
})
