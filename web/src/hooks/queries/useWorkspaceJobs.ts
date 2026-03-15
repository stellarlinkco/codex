import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubJobsResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspaceJobs(api: ApiClient | null, workspaceId: string | null) {
    return useQuery<GithubJobsResponse, Error>({
        queryKey: workspaceId ? queryKeys.workspaceJobs(workspaceId) : ['workspace-jobs', 'none'],
        enabled: Boolean(api) && Boolean(workspaceId),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workspaceId) {
                throw new Error('Missing workspace id')
            }
            return await api.getWorkspaceJobs(workspaceId)
        }
    })
}

