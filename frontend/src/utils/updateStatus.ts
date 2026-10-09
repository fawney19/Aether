import type { CheckUpdateResponse } from '@/api/admin'

export function describeUpdateStatus(status: CheckUpdateResponse | null): string {
  if (!status) return '检查中'
  if (status.has_update) return '有新版本'
  if (status.error) return '检查失败'
  return '已是最新'
}

export function buildUpdateErrorStatus(
  previousStatus: CheckUpdateResponse | null,
  error: unknown
): CheckUpdateResponse {
  return {
    current_version: previousStatus?.current_version || '',
    latest_version: null,
    has_update: false,
    updatable: false,
    update_blocker: null,
    release_url: null,
    release_notes: null,
    published_at: null,
    error: error instanceof Error ? error.message : '检查更新失败'
  }
}
