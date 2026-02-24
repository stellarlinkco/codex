import type { Suggestion } from '@/hooks/useActiveSuggestions'

export async function getComposerAutocompleteSuggestions(
    query: string,
    handlers: {
        getSlashSuggestions: (query: string) => Promise<Suggestion[]>
        getSkillSuggestions: (query: string) => Promise<Suggestion[]>
    }
): Promise<Suggestion[]> {
    if (query.startsWith('@')) {
        return []
    }
    if (query.startsWith('$')) {
        return await handlers.getSkillSuggestions(query)
    }
    return await handlers.getSlashSuggestions(query)
}

