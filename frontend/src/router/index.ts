import { createRouter, createWebHistory } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useModuleStore } from '@/stores/modules'
import { log } from '@/utils/logger'
import {
  ensureUserLoaded,
  resolveHomeRedirect,
  checkAdminAccess,
  checkModuleAccess
} from './guards'
import { routes } from './routes'

const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes
})

router.beforeEach(async (to, from, next) => {
  const authStore = useAuthStore()
  const moduleStore = useModuleStore()

  try {
    const isAuthenticated = await ensureUserLoaded(authStore)

    // 首页重定向
    const homeRedirect = resolveHomeRedirect(to, from, authStore)
    if (homeRedirect !== null) return next(homeRedirect === '' ? undefined : homeRedirect)

    // 需要认证但未认证
    const requiresAuth = to.matched.some(record => record.meta.requiresAuth !== false)
    if (requiresAuth && !isAuthenticated) {
      sessionStorage.setItem('redirectPath', to.fullPath)
      log.debug('No valid token found, redirecting to home')
      return next('/')
    }

    // 管理端检查
    const requiresAdmin = to.matched.some(record => record.meta.requiresAdmin)
    if (requiresAdmin) {
      const adminRedirect = await checkAdminAccess(to, authStore, moduleStore)
      if (adminRedirect) return next(adminRedirect)
    }

    // 非管理端的模块检查
    if (!requiresAdmin) {
      const moduleRedirect = await checkModuleAccess(to, moduleStore)
      if (moduleRedirect) return next(moduleRedirect)
    }

    next()
  } catch (error) {
    log.error('Router guard error', error)
    // 发生错误时,直接放行,不要乱跳转
    next()
  }
})

export default router
