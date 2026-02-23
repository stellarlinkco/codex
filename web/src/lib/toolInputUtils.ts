import { isObject } from '@codex/protocol'

export function getInputString(input: unknown, key: string): string | null {
    if (!isObject(input)) return null
    const value = input[key]
    return typeof value === 'string' ? value : null
}

export function getInputStringAny(input: unknown, keys: string[]): string | null {
    for (const key of keys) {
        const value = getInputString(input, key)
        if (value) return value
    }
    return null
}

export function truncate(text: string, maxLen: number): string {
    if (text.length <= maxLen) return text
    return text.slice(0, maxLen - 3) + '...'
}
