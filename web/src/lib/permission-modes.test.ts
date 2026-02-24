import { describe, it, expect } from 'vitest'
import { getPermissionModesForFlavor } from '@codex/protocol'

describe('permission modes', () => {
    it('includes plan mode for codex flavor', () => {
        expect(getPermissionModesForFlavor('codex')).toContain('plan')
    })
})

