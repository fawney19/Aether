import type { CheckUpdateResponse } from '@/api/admin'

export function normalizeUpdateBlockerForDisplay(reason: string | null | undefined): string {
  // 兼容服务端及缓存中的旧提示，页面不再展示仓库操作说明。
  if (!reason || /源码构建|source build|git pull|对应标签|check out the tag|手动下载 Release|download a release or use the install script/i.test(reason)) {
    return '当前安装方式不支持在线更新，请手动安装目标版本。'
  }
  return reason
}

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
