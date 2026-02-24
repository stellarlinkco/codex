import { describe, it, expect, beforeEach } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { useRecentPaths } from './useRecentPaths'

describe('useRecentPaths', () => {
    beforeEach(() => {
        localStorage.clear()
    })

    it('adds, removes, and clears recent paths per machine', () => {
        const { result } = renderHook(() => useRecentPaths())

        act(() => {
            result.current.addRecentPath('m1', '/a')
            result.current.addRecentPath('m1', '/b')
        })
        expect(result.current.getRecentPaths('m1')).toEqual(['/b', '/a'])

        act(() => {
            result.current.removeRecentPath('m1', '/a')
        })
        expect(result.current.getRecentPaths('m1')).toEqual(['/b'])

        act(() => {
            result.current.clearRecentPaths('m1')
        })
        expect(result.current.getRecentPaths('m1')).toEqual([])
        expect(result.current.getRecentPaths('m2')).toEqual([])
    })

    it('dedupes paths and caps at max paths per machine', () => {
        const { result } = renderHook(() => useRecentPaths())

        act(() => {
            for (const path of ['/1', '/2', '/3', '/4', '/5', '/6']) {
                result.current.addRecentPath('m1', path)
            }
        })
        expect(result.current.getRecentPaths('m1')).toEqual(['/6', '/5', '/4', '/3', '/2'])

        act(() => {
            result.current.addRecentPath('m1', '/4')
        })
        expect(result.current.getRecentPaths('m1')).toEqual(['/4', '/6', '/5', '/3', '/2'])
    })

    it('no-ops when clearing an empty machine', () => {
        const { result } = renderHook(() => useRecentPaths())
        expect(localStorage.getItem('codex:recentPaths')).toBeNull()

        act(() => {
            result.current.clearRecentPaths('m1')
        })
        expect(result.current.getRecentPaths('m1')).toEqual([])
        expect(localStorage.getItem('codex:recentPaths')).toBeNull()
    })
})
