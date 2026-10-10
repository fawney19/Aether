import type { RouteLocationNormalized } from 'vue-router'
import type { useModuleStore } from '@/stores/modules'
import { log } from '@/utils/logger'

/**
 * 检查非管理端路由的模块激活状态。
 * @returns 重定向路径，或 null 表示通过
 */
export async function checkModuleAccess(
  to: RouteLocationNormalized,
  moduleStore: ReturnType<typeof useModuleStore>
): Promise<string | null> {
  // 检查路由链中是否有模块要求
  const moduleName = to.matched.find(record => record.meta.module)?.meta.module as
    | string
    | undefined
  if (!moduleName) {
    return null
  }

  // 确保模块状态已加载
  if (!moduleStore.userLoaded) {
    try {
      await moduleStore.fetchUserModules()
    } catch (error) {
      // fail-close: 获取模块状态失败时拒绝访问
      log.warn('Failed to fetch modules status, denying access', { error })
      return '/dashboard'
    }
  }

  // 用户侧需要检查模块是否激活（active），而不仅仅是可用（available）
  if (!moduleStore.isUserActive(moduleName)) {
    log.warn(`Module ${moduleName} is not active, redirecting to user dashboard`)
    return '/dashboard'
  }

  return null
}
