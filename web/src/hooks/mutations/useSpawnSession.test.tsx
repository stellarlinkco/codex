import { describe, it, expect, vi } from 'vitest'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook } from '@testing-library/react'
import { useSpawnSession } from './useSpawnSession'

describe('useSpawnSession', () => {
    it('passes reasoningEffort through to ApiClient.spawnSession', async () => {
        const api = {
            spawnSession: vi.fn().mockResolvedValue({ type: 'success', sessionId: 's1' }),
        }

        const queryClient = new QueryClient()
        const wrapper = ({ children }: { children: React.ReactNode }) => (
            <QueryClientProvider client={queryClient}>
                {children}
            </QueryClientProvider>
        )

        const { result } = renderHook(() => useSpawnSession(api as never), { wrapper })

        await act(async () => {
            await result.current.spawnSession({
                machineId: 'm1',
                directory: '/repo',
                reasoningEffort: 'high',
            })
        })

        expect(api.spawnSession).toHaveBeenCalledWith(
            'm1',
            '/repo',
            undefined,
            undefined,
            undefined,
            undefined,
            undefined,
            'high'
        )
    })
})

