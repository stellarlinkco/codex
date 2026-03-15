import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { Workspace } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspace(api: ApiClient | null, workspaceId: string | null) {
    return useQuery<Workspace, Error>({
        queryKey: workspaceId ? queryKeys.workspace(workspaceId) : ['workspace', 'none'],
        enabled: Boolean(api) && Boolean(workspaceId),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workspaceId) {
                throw new Error('Missing workspace id')
            }
            return await api.getWorkspace(workspaceId)
        }
    })
}

