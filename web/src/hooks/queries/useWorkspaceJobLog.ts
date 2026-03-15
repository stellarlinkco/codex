import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubJobLogResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useWorkspaceJobLog(
    api: ApiClient | null,
    workspaceId: string | null,
    jobId: string | null,
    options?: { refetchIntervalMs?: number | false }
) {
    return useQuery<GithubJobLogResponse, Error>({
        queryKey: workspaceId && jobId ? queryKeys.workspaceJobLog(workspaceId, jobId) : ['workspace-job-log', 'none'],
        enabled: Boolean(api) && Boolean(workspaceId) && Boolean(jobId),
        refetchInterval: options?.refetchIntervalMs ?? false,
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workspaceId) {
                throw new Error('Missing workspace id')
            }
            if (!jobId) {
                throw new Error('Missing job id')
            }
            return await api.getWorkspaceJobLog(workspaceId, jobId)
        }
    })
}

