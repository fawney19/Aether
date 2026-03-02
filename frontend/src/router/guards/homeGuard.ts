import type { RouteLocationNormalized } from 'vue-router'
import type { useAuthStore } from '@/stores/auth'

/**
 * 处理已认证用户访问首页时的重定向。
 * @returns 重定向路径，null 表示不需要重定向，空字符串表示放行（不跳转）
 */
export function resolveHomeRedirect(
  to: RouteLocationNormalized,
  from: RouteLocationNormalized,
  authStore: ReturnType<typeof useAuthStore>
): string | null {
  if (to.path !== '/') {
    return null
  }

  if (!authStore.isAuthenticated) {
    return null
  }

  // 已登录用户如果是从dashboard返回首页、刷新首页、或者有returnTo参数,允许访问首页
  const isFromApp =
    from.path.startsWith('/dashboard') ||
    from.path.startsWith('/admin') ||
    from.path.startsWith('/guide') ||
    from.path === '/' ||
    from.path === '/login' ||
    from.path === '/register'
  if (to.query.returnTo || isFromApp) {
    return ''
  }

  // 已登录用户首次访问首页(非返回/刷新场景),根据角色跳转到对应仪表盘
  const isAdmin = authStore.user?.role === 'admin'
  const redirectPath = sessionStorage.getItem('redirectPath')
  if (redirectPath && redirectPath !== '/') {
    sessionStorage.removeItem('redirectPath')
    return redirectPath
  }

  return isAdmin ? '/admin/dashboard' : '/dashboard'
}
