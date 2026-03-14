import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubJobLogResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubJobLog(
    api: ApiClient | null,
    jobId: string | null,
    options?: { refetchIntervalMs?: number | false }
) {
    return useQuery<GithubJobLogResponse, Error>({
        queryKey: jobId ? queryKeys.githubJobLog(jobId) : ['github-job-log', 'none'],
        enabled: Boolean(api) && Boolean(jobId),
        refetchInterval: options?.refetchIntervalMs ?? false,
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!jobId) {
                throw new Error('Missing job id')
            }
            return await api.getGithubJobLog(jobId)
        }
    })
}
