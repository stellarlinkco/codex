import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { ApiClient } from './client'

type FetchResponse = {
    ok: boolean
    status: number
    statusText: string
    json: () => Promise<unknown>
    text: () => Promise<string>
}

describe('ApiClient', () => {
    const originalFetch = globalThis.fetch

    beforeEach(() => {
        vi.restoreAllMocks()
    })

    afterEach(() => {
        globalThis.fetch = originalFetch
    })

    it('includes reasoningEffort when spawning a session', async () => {
        const fetchMock = vi.fn<Parameters<typeof fetch>, Promise<FetchResponse>>()
            .mockResolvedValue({
                ok: true,
                status: 200,
                statusText: 'OK',
                json: async () => ({ type: 'success', sessionId: 's1' }),
                text: async () => '',
            })
        globalThis.fetch = fetchMock as unknown as typeof fetch

        const api = new ApiClient('t0')
        await api.spawnSession('m1', '/repo', 'codex', 'gpt-5.2', false, 'simple', undefined, 'high')

        const [url, init] = fetchMock.mock.calls[0]
        expect(url).toBe('/api/machines/m1/spawn')

        const body = JSON.parse(String(init?.body))
        expect(body.reasoningEffort).toBe('high')
    })

    it('omits reasoningEffort when not provided', async () => {
        const fetchMock = vi.fn<Parameters<typeof fetch>, Promise<FetchResponse>>()
            .mockResolvedValue({
                ok: true,
                status: 200,
                statusText: 'OK',
                json: async () => ({ type: 'success', sessionId: 's1' }),
                text: async () => '',
            })
        globalThis.fetch = fetchMock as unknown as typeof fetch

        const api = new ApiClient('t0')
        await api.spawnSession('m1', '/repo', 'codex', 'gpt-5.2', false)

        const [, init] = fetchMock.mock.calls[0]
        const body = JSON.parse(String(init?.body))
        expect(body).not.toHaveProperty('reasoningEffort')
    })
})

