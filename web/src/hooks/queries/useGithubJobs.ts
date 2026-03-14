import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubJobsResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubJobs(api: ApiClient | null) {
    return useQuery<GithubJobsResponse, Error>({
        queryKey: queryKeys.githubJobs,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getGithubJobs()
        }
    })
}

