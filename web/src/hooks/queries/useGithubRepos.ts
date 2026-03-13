import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubReposResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubRepos(api: ApiClient | null) {
    return useQuery<GithubReposResponse, Error>({
        queryKey: queryKeys.githubRepos,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getGithubRepos()
        }
    })
}

