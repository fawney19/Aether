import type { SystemUpdatePreflightResponse } from '@/api/admin'

export function isPreflightBlocking(
  data: SystemUpdatePreflightResponse | null | undefined
): boolean {
  if (!data) return false
  if (data.can_apply_update === false) return true
  return data.checks.some((item) => item.status === 'blocked')
}
