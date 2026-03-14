import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubWorkItemsSnapshot } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubWorkItems(api: ApiClient | null) {
    return useQuery<GithubWorkItemsSnapshot, Error>({
        queryKey: queryKeys.githubWorkItems,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getGithubWorkItems()
        }
    })
}

