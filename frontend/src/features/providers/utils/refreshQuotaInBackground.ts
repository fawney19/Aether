import type { RefreshQuotaResult } from '@/api/endpoints/keys'

interface BackgroundQuotaRefreshOptions {
  refresh: () => Promise<RefreshQuotaResult>
  isCurrent: () => boolean
  retryInitialEmptyQuota: boolean
}

function shouldRetryEmptyQuota(result: RefreshQuotaResult): boolean {
  return result.success === 0
    && result.results.length > 0
    && result.results.every(item =>
      (item.status === 'no_metadata' || item.status === 'error')
      && item.status_code !== 401
      && item.status_code !== 403
      && !item.metadata
      && !item.quota_snapshot,
    )
}

export async function refreshQuotaInBackground({
  refresh,
  isCurrent,
  retryInitialEmptyQuota,
}: BackgroundQuotaRefreshOptions): Promise<RefreshQuotaResult | null> {
  if (!isCurrent()) return null
  const result = await refresh()
  if (!isCurrent()) return null
  if (!retryInitialEmptyQuota || !shouldRetryEmptyQuota(result)) return result

  await new Promise<void>(resolve => setTimeout(resolve, 1000))
  if (!isCurrent()) return null
  const retried = await refresh()
  return isCurrent() ? retried : null
}
