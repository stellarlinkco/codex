import { useCallback, useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import type { ApiClient } from '@/api/client'
import type { Machine } from '@/types/api'
import { usePlatform } from '@/hooks/usePlatform'
import { useSpawnSession } from '@/hooks/mutations/useSpawnSession'
import { useSessions } from '@/hooks/queries/useSessions'
import { useActiveSuggestions, type Suggestion } from '@/hooks/useActiveSuggestions'
import { useDirectorySuggestions } from '@/hooks/useDirectorySuggestions'
import { useRecentPaths } from '@/hooks/useRecentPaths'
import type { AgentType } from './types'
import { ActionButtons } from './ActionButtons'
import { AgentSelector } from './AgentSelector'
import { DirectorySection } from './DirectorySection'
import { MachineSelector } from './MachineSelector'
import { ModelSelector } from './ModelSelector'
import { ReasoningEffortSelector, type ReasoningEffortPreference } from './ReasoningEffortSelector'
import {
    loadPreferredAgent,
    loadPreferredReasoningEffort,
    loadPreferredYoloMode,
    savePreferredAgent,
    savePreferredReasoningEffort,
    savePreferredYoloMode,
} from './preferences'
import { YoloToggle } from './YoloToggle'

export function NewSession(props: {
    api: ApiClient
    machines: Machine[]
    isLoading?: boolean
    onSuccess: (sessionId: string) => void
    onCancel: () => void
}) {
    const { haptic } = usePlatform()
    const { spawnSession, isPending, error: spawnError } = useSpawnSession(props.api)
    const { sessions } = useSessions(props.api)
    const isFormDisabled = Boolean(isPending || props.isLoading)
    const {
        getRecentPaths,
        addRecentPath,
        removeRecentPath,
        clearRecentPaths,
        getLastUsedMachineId,
        setLastUsedMachineId,
    } = useRecentPaths()

    const [machineId, setMachineId] = useState<string | null>(null)
    const [directory, setDirectory] = useState('')
    const [suppressSuggestions, setSuppressSuggestions] = useState(false)
    const [isDirectoryFocused, setIsDirectoryFocused] = useState(false)
    const [pathExistence, setPathExistence] = useState<Record<string, boolean>>({})
    const [agent, setAgent] = useState<AgentType>(loadPreferredAgent)
    const [model, setModel] = useState('auto')
    const [reasoningEffort, setReasoningEffort] = useState<ReasoningEffortPreference>(loadPreferredReasoningEffort)
    const [yoloMode, setYoloMode] = useState(loadPreferredYoloMode)
    const [error, setError] = useState<string | null>(null)

    useEffect(() => {
        setModel('auto')
    }, [agent])

    useEffect(() => {
        savePreferredAgent(agent)
    }, [agent])

    useEffect(() => {
        savePreferredYoloMode(yoloMode)
    }, [yoloMode])

    useEffect(() => {
        savePreferredReasoningEffort(reasoningEffort)
    }, [reasoningEffort])

    useEffect(() => {
        if (props.machines.length === 0) return
        if (machineId && props.machines.find((m) => m.id === machineId)) return

        const lastUsed = getLastUsedMachineId()
        const foundLast = lastUsed ? props.machines.find((m) => m.id === lastUsed) : null

        if (foundLast) {
            setMachineId(foundLast.id)
            const paths = getRecentPaths(foundLast.id)
            if (paths[0]) setDirectory(paths[0])
        } else if (props.machines[0]) {
            setMachineId(props.machines[0].id)
        }
    }, [props.machines, machineId, getLastUsedMachineId, getRecentPaths])

    const recentPaths = useMemo(
        () => getRecentPaths(machineId),
        [getRecentPaths, machineId]
    )

    const allPaths = useDirectorySuggestions(machineId, sessions, recentPaths)

    const pathsToCheck = useMemo(
        () => Array.from(new Set(allPaths)).slice(0, 1000),
        [allPaths]
    )

    useEffect(() => {
        let cancelled = false

        if (!machineId) {
            setPathExistence({})
            return () => { cancelled = true }
        }

        const unknownPaths = pathsToCheck.filter((path) => typeof pathExistence[path] !== 'boolean')
        if (unknownPaths.length === 0) {
            return () => {
                cancelled = true
            }
        }

        void props.api.checkMachinePathsExists(machineId, unknownPaths)
            .then((result) => {
                if (cancelled) return
                setPathExistence((prev) => ({ ...prev, ...(result.exists ?? {}) }))
            })
            .catch(() => {
                // Ignore errors: fall back to server-side validation on create.
            })

        return () => {
            cancelled = true
        }
    }, [machineId, pathsToCheck, pathExistence, props.api])

    const directoryToCheck = directory.trim()

    useEffect(() => {
        let cancelled = false
        if (!machineId || !directoryToCheck) {
            return () => { cancelled = true }
        }

        const handle = window.setTimeout(() => {
            void props.api.checkMachinePathsExists(machineId, [directoryToCheck])
                .then((result) => {
                    if (cancelled) return
                    const exists = result.exists?.[directoryToCheck]
                    if (typeof exists !== 'boolean') return
                    setPathExistence((prev) => ({ ...prev, [directoryToCheck]: exists }))
                })
                .catch(() => {
                    // Ignore errors: fall back to server-side validation on create.
                })
        }, 200)

        return () => {
            cancelled = true
            window.clearTimeout(handle)
        }
    }, [machineId, directoryToCheck, props.api])

    const verifiedPaths = useMemo(
        () => allPaths.filter((path) => pathExistence[path]),
        [allPaths, pathExistence]
    )

    const getSuggestions = useCallback(async (query: string): Promise<Suggestion[]> => {
        const lowered = query.toLowerCase()
        return verifiedPaths
            .filter((path) => path.toLowerCase().includes(lowered))
            .slice(0, 8)
            .map((path) => ({
                key: path,
                text: path,
                label: path
            }))
    }, [verifiedPaths])

    const activeQuery = (!isDirectoryFocused || suppressSuggestions) ? null : directory

    const [suggestions, selectedIndex, moveUp, moveDown, clearSuggestions] = useActiveSuggestions(
        activeQuery,
        getSuggestions,
        { allowEmptyQuery: true, autoSelectFirst: false }
    )

    const handleMachineChange = useCallback((newMachineId: string) => {
        setPathExistence({})
        setMachineId(newMachineId)
        const paths = getRecentPaths(newMachineId)
        if (paths[0]) {
            setDirectory(paths[0])
        } else {
            setDirectory('')
        }
    }, [getRecentPaths])

    const handlePathClick = useCallback((path: string) => {
        setDirectory(path)
    }, [])

    const handlePathRemove = useCallback((path: string) => {
        if (!machineId) return
        removeRecentPath(machineId, path)
    }, [machineId, removeRecentPath])

    const handleClearRecentPaths = useCallback(() => {
        if (!machineId) return
        clearRecentPaths(machineId)
    }, [machineId, clearRecentPaths])

    const handleSuggestionSelect = useCallback((index: number) => {
        const suggestion = suggestions[index]
        if (suggestion) {
            setDirectory(suggestion.text)
            clearSuggestions()
            setSuppressSuggestions(true)
        }
    }, [suggestions, clearSuggestions])

    const handleDirectoryChange = useCallback((value: string) => {
        setSuppressSuggestions(false)
        setDirectory(value)
    }, [])

    const handleDirectoryFocus = useCallback(() => {
        setSuppressSuggestions(false)
        setIsDirectoryFocused(true)
    }, [])

    const handleDirectoryBlur = useCallback(() => {
        setIsDirectoryFocused(false)
    }, [])

    const handleDirectoryKeyDown = useCallback((event: ReactKeyboardEvent<HTMLInputElement>) => {
        if (suggestions.length === 0) return

        if (event.key === 'ArrowUp') {
            event.preventDefault()
            moveUp()
        }

        if (event.key === 'ArrowDown') {
            event.preventDefault()
            moveDown()
        }

        if (event.key === 'Enter' || event.key === 'Tab') {
            if (selectedIndex >= 0) {
                event.preventDefault()
                handleSuggestionSelect(selectedIndex)
            }
        }

        if (event.key === 'Escape') {
            clearSuggestions()
        }
    }, [suggestions, selectedIndex, moveUp, moveDown, clearSuggestions, handleSuggestionSelect])

    async function handleCreate() {
        if (!machineId || !directory.trim()) return

        setError(null)
        try {
            const resolvedModel = model !== 'auto' && agent !== 'opencode' ? model : undefined
            const resolvedReasoningEffort = reasoningEffort !== 'auto' ? reasoningEffort : undefined
            const result = await spawnSession({
                machineId,
                directory: directory.trim(),
                agent,
                model: resolvedModel,
                reasoningEffort: resolvedReasoningEffort,
                yolo: yoloMode,
            })

            if (result.type === 'success') {
                haptic.notification('success')
                setLastUsedMachineId(machineId)
                addRecentPath(machineId, directory.trim())
                props.onSuccess(result.sessionId)
                return
            }

            haptic.notification('error')
            setError(result.message)
        } catch (e) {
            haptic.notification('error')
            setError(e instanceof Error ? e.message : 'Failed to create session')
        }
    }

    const canCreate = Boolean(machineId && directory.trim() && !isFormDisabled && pathExistence[directory.trim()] !== false)

    return (
        <div className="flex flex-col divide-y divide-[var(--app-divider)]">
            <MachineSelector
                machines={props.machines}
                machineId={machineId}
                isLoading={props.isLoading}
                isDisabled={isFormDisabled}
                onChange={handleMachineChange}
            />
            <DirectorySection
                directory={directory}
                directoryExists={directory.trim().length > 0 ? pathExistence[directory.trim()] : undefined}
                suggestions={suggestions}
                selectedIndex={selectedIndex}
                isDisabled={isFormDisabled}
                recentPaths={recentPaths}
                onDirectoryChange={handleDirectoryChange}
                onDirectoryFocus={handleDirectoryFocus}
                onDirectoryBlur={handleDirectoryBlur}
                onDirectoryKeyDown={handleDirectoryKeyDown}
                onSuggestionSelect={handleSuggestionSelect}
                onPathClick={handlePathClick}
                onPathRemove={handlePathRemove}
                onClearRecentPaths={handleClearRecentPaths}
            />
            <AgentSelector
                agent={agent}
                isDisabled={isFormDisabled}
                onAgentChange={setAgent}
            />
            <ModelSelector
                agent={agent}
                model={model}
                isDisabled={isFormDisabled}
                onModelChange={setModel}
            />
            <ReasoningEffortSelector
                reasoningEffort={reasoningEffort}
                isDisabled={isFormDisabled}
                onReasoningEffortChange={setReasoningEffort}
            />
            <YoloToggle
                yoloMode={yoloMode}
                isDisabled={isFormDisabled}
                onToggle={setYoloMode}
            />

            {(error ?? spawnError) ? (
                <div className="px-3 py-2 text-sm text-red-600">
                    {error ?? spawnError}
                </div>
            ) : null}

            <ActionButtons
                isPending={isPending}
                canCreate={canCreate}
                isDisabled={isFormDisabled}
                onCancel={props.onCancel}
                onCreate={handleCreate}
            />
        </div>
    )
}
