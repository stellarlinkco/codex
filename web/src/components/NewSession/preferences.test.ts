import { beforeEach, describe, expect, it } from 'vitest'
import {
    loadPreferredAgent,
    loadPreferredYoloMode,
    savePreferredAgent,
    savePreferredYoloMode,
} from './preferences'

describe('NewSession preferences', () => {
    beforeEach(() => {
        localStorage.clear()
    })

    it('loads defaults when storage is empty', () => {
        expect(loadPreferredAgent()).toBe('codex')
        expect(loadPreferredYoloMode()).toBe(false)
    })

    it('loads saved values from storage', () => {
        localStorage.setItem('codex:newSession:agent', 'codex')
        localStorage.setItem('codex:newSession:yolo', 'true')

        expect(loadPreferredAgent()).toBe('codex')
        expect(loadPreferredYoloMode()).toBe(true)
    })

    it('falls back to default agent on invalid stored value', () => {
        localStorage.setItem('codex:newSession:agent', 'unknown-agent')

        expect(loadPreferredAgent()).toBe('codex')
    })

    it('persists new values to storage', () => {
        savePreferredAgent('codex')
        savePreferredYoloMode(true)

        expect(localStorage.getItem('codex:newSession:agent')).toBe('codex')
        expect(localStorage.getItem('codex:newSession:yolo')).toBe('true')
    })
})
