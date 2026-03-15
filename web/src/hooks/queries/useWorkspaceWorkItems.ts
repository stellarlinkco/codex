import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubWorkItemsSnapshot } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspaceWorkItems(api: ApiClient | null, workspaceId: string | null) {
    return useQuery<GithubWorkItemsSnapshot, Error>({
        queryKey: workspaceId ? queryKeys.workspaceWorkItems(workspaceId) : ['workspace-work-items', 'none'],
        enabled: Boolean(api) && Boolean(workspaceId),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workspaceId) {
                throw new Error('Missing workspace id')
            }
            return await api.getWorkspaceWorkItems(workspaceId)
        }
    })
}

