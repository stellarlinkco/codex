import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { KanbanConfig } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useKanban(api: ApiClient | null) {
    return useQuery<KanbanConfig, Error>({
        queryKey: queryKeys.kanban,
        enabled: Boolean(api),
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getKanban()
        }
    })
}
