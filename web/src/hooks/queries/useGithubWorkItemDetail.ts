import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { GithubWorkItemDetail } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useGithubWorkItemDetail(api: ApiClient | null, workItemKey: string | null) {
    return useQuery<GithubWorkItemDetail, Error>({
        queryKey: workItemKey ? queryKeys.githubWorkItemDetail(workItemKey) : ['github-work-item-detail', 'none'],
        enabled: Boolean(api) && Boolean(workItemKey),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            if (!workItemKey) {
                throw new Error('Missing work item key')
            }
            return await api.getGithubWorkItemDetail(workItemKey)
        }
    })
}

