import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubKanbanConfig } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspaceKanban(api: ApiClient | null, workspaceId: string | null) {
    return useQuery<GithubKanbanConfig, Error>({
        queryKey: workspaceId ? queryKeys.workspaceKanban(workspaceId) : ['workspace-kanban', 'none'],
        enabled: Boolean(api) && Boolean(workspaceId),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workspaceId) {
                throw new Error('Missing workspace id')
            }
            return await api.getWorkspaceKanban(workspaceId)
        }
    })
}

