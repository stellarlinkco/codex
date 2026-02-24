import { describe, it, expect, vi } from 'vitest'
import { getComposerAutocompleteSuggestions } from './autocomplete'

describe('getComposerAutocompleteSuggestions', () => {
    it('returns empty list for @ prefix', async () => {
        const getSlashSuggestions = vi.fn(async () => [{ key: 'x', text: '/x', label: '/x' }])
        const getSkillSuggestions = vi.fn(async () => [{ key: 'y', text: '$y', label: '$y' }])

        await expect(getComposerAutocompleteSuggestions('@foo', { getSlashSuggestions, getSkillSuggestions }))
            .resolves
            .toEqual([])
        expect(getSlashSuggestions).not.toHaveBeenCalled()
        expect(getSkillSuggestions).not.toHaveBeenCalled()
    })

    it('routes $ prefix to skills handler', async () => {
        const getSlashSuggestions = vi.fn(async () => [])
        const getSkillSuggestions = vi.fn(async () => [{ key: 's', text: '$s', label: '$s' }])

        await expect(getComposerAutocompleteSuggestions('$s', { getSlashSuggestions, getSkillSuggestions }))
            .resolves
            .toEqual([{ key: 's', text: '$s', label: '$s' }])
        expect(getSkillSuggestions).toHaveBeenCalledWith('$s')
    })

    it('routes non-$ prefixes to slash handler', async () => {
        const getSlashSuggestions = vi.fn(async () => [{ key: 'c', text: '/c', label: '/c' }])
        const getSkillSuggestions = vi.fn(async () => [])

        await expect(getComposerAutocompleteSuggestions('/c', { getSlashSuggestions, getSkillSuggestions }))
            .resolves
            .toEqual([{ key: 'c', text: '/c', label: '/c' }])
        expect(getSlashSuggestions).toHaveBeenCalledWith('/c')
    })
})

