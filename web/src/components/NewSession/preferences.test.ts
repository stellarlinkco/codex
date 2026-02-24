import { beforeEach, describe, expect, it } from 'vitest'
import {
    loadPreferredAgent,
    loadPreferredReasoningEffort,
    loadPreferredYoloMode,
    savePreferredAgent,
    savePreferredReasoningEffort,
    savePreferredYoloMode,
} from './preferences'

describe('NewSession preferences', () => {
    beforeEach(() => {
        localStorage.clear()
    })

    it('loads defaults when storage is empty', () => {
        expect(loadPreferredAgent()).toBe('codex')
        expect(loadPreferredYoloMode()).toBe(false)
        expect(loadPreferredReasoningEffort()).toBe('auto')
    })

    it('loads saved values from storage', () => {
        localStorage.setItem('codex:newSession:agent', 'codex')
        localStorage.setItem('codex:newSession:yolo', 'true')
        localStorage.setItem('codex:newSession:reasoningEffort', 'high')

        expect(loadPreferredAgent()).toBe('codex')
        expect(loadPreferredYoloMode()).toBe(true)
        expect(loadPreferredReasoningEffort()).toBe('high')
    })

    it('falls back to default agent on invalid stored value', () => {
        localStorage.setItem('codex:newSession:agent', 'unknown-agent')

        expect(loadPreferredAgent()).toBe('codex')
    })

    it('falls back to auto reasoning effort on invalid stored value', () => {
        localStorage.setItem('codex:newSession:reasoningEffort', 'unknown-effort')

        expect(loadPreferredReasoningEffort()).toBe('auto')
    })

    it('persists new values to storage', () => {
        savePreferredAgent('codex')
        savePreferredYoloMode(true)
        savePreferredReasoningEffort('low')

        expect(localStorage.getItem('codex:newSession:agent')).toBe('codex')
        expect(localStorage.getItem('codex:newSession:yolo')).toBe('true')
        expect(localStorage.getItem('codex:newSession:reasoningEffort')).toBe('low')
    })
})
