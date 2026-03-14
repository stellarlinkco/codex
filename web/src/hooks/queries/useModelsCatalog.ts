import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '@/api/client'
import type { ModelsCatalogResponse } from '@/types/api'
import { queryKeys } from '@/lib/query-keys'

export function useModelsCatalog(api: ApiClient | null) {
    return useQuery<ModelsCatalogResponse, Error>({
        queryKey: queryKeys.modelsCatalog,
        enabled: Boolean(api),
        staleTime: 5 * 60 * 1000,
        queryFn: async () => {
            if (!api) {
                throw new Error('No API client')
            }
            return await api.getModelsCatalog()
        }
    })
}

