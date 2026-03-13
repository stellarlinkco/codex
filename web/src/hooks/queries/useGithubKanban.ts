import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubKanbanConfig } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubKanban(api: ApiClient | null) {
    return useQuery<GithubKanbanConfig, Error>({
        queryKey: queryKeys.githubKanban,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getGithubKanban()
        }
    })
}
