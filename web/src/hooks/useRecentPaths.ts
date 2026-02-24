import { useCallback, useMemo, useState } from 'react'

const STORAGE_KEY = 'codex:recentPaths'
const LAST_MACHINE_ID_KEY = 'codex:lastMachineId'
const MAX_PATHS_PER_MACHINE = 5

type RecentPathsData = Record<string, string[]>

function loadRecentPaths(): RecentPathsData {
    try {
        const stored = localStorage.getItem(STORAGE_KEY)
        return stored ? JSON.parse(stored) : {}
    } catch {
        return {}
    }
}

function saveRecentPaths(data: RecentPathsData): void {
    try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(data))
    } catch {
        // Ignore storage errors
    }
}

export function useRecentPaths() {
    const [data, setData] = useState<RecentPathsData>(loadRecentPaths)

    const getRecentPaths = useCallback((machineId: string | null): string[] => {
        if (!machineId) return []
        return data[machineId] ?? []
    }, [data])

    const addRecentPath = useCallback((machineId: string, path: string): void => {
        const trimmed = path.trim()
        if (!trimmed) return

        setData((prev) => {
            const existing = prev[machineId] ?? []
            // Remove if already exists, then add to front
            const filtered = existing.filter((p) => p !== trimmed)
            const updated = [trimmed, ...filtered].slice(0, MAX_PATHS_PER_MACHINE)

            const newData = { ...prev, [machineId]: updated }
            saveRecentPaths(newData)
            return newData
        })
    }, [])

    const removeRecentPath = useCallback((machineId: string, path: string): void => {
        const trimmed = path.trim()
        if (!trimmed) return

        setData((prev) => {
            const existing = prev[machineId] ?? []
            const updated = existing.filter((p) => p !== trimmed)
            const newData = { ...prev, [machineId]: updated }
            saveRecentPaths(newData)
            return newData
        })
    }, [])

    const clearRecentPaths = useCallback((machineId: string): void => {
        setData((prev) => {
            if (!prev[machineId] || prev[machineId].length === 0) {
                return prev
            }
            const newData = { ...prev, [machineId]: [] }
            saveRecentPaths(newData)
            return newData
        })
    }, [])

    const getLastUsedMachineId = useCallback((): string | null => {
        try {
            return localStorage.getItem(LAST_MACHINE_ID_KEY)
        } catch {
            return null
        }
    }, [])

    const setLastUsedMachineId = useCallback((machineId: string): void => {
        try {
            localStorage.setItem(LAST_MACHINE_ID_KEY, machineId)
        } catch {
            // Ignore storage errors
        }
    }, [])

    return useMemo(() => ({
        getRecentPaths,
        addRecentPath,
        removeRecentPath,
        clearRecentPaths,
        getLastUsedMachineId,
        setLastUsedMachineId,
    }), [
        getRecentPaths,
        addRecentPath,
        removeRecentPath,
        clearRecentPaths,
        getLastUsedMachineId,
        setLastUsedMachineId,
    ])
}
