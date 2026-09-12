import type { PoolKeyDetail } from '@/api/endpoints/pool'
import type { RefreshQuotaResult } from '@/api/endpoints/keys'

export function mergePoolKeyQuotaSnapshots(
  keys: PoolKeyDetail[],
  results: RefreshQuotaResult['results'],
): PoolKeyDetail[] {
  const resultByKeyId = new Map<string, RefreshQuotaResult['results'][number]>()
  for (const result of results) {
    if (result.quota_snapshot || result.metadata) {
      resultByKeyId.set(result.key_id, result)
    }
  }
  if (resultByKeyId.size === 0) return keys

  return keys.map((key) => {
    const result = resultByKeyId.get(key.key_id)
    if (!result) return key
    const quotaSnapshot = result.quota_snapshot
    const providerType = String(quotaSnapshot?.provider_type || key.provider_type || '').trim().toLowerCase()
    return {
      ...key,
      ...(quotaSnapshot ? {
        quota_updated_at: quotaSnapshot.updated_at ?? quotaSnapshot.observed_at ?? key.quota_updated_at ?? null,
        status_snapshot: {
          oauth: key.status_snapshot?.oauth ?? { code: 'none' },
          account: key.status_snapshot?.account ?? { code: 'unknown', blocked: false },
          quota: quotaSnapshot,
        },
      } : {}),
      ...(result.metadata && providerType ? {
        upstream_metadata: {
          ...(key.upstream_metadata ?? {}),
          [providerType]: result.metadata,
        },
      } : {}),
    }
  })
}
