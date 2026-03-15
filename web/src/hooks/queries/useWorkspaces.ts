import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { WorkspaceSummary } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspaces(api: ApiClient | null) {
    return useQuery<WorkspaceSummary[], Error>({
        queryKey: queryKeys.workspaces,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.listWorkspaces()
        }
    })
}

