import { createRouter, createWebHistory } from 'vue-router'
import type { RouteRecordRaw } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useModuleStore } from '@/stores/modules'
import { importWithRetry } from '@/utils/importRetry'
import { log } from '@/utils/logger'
import {
  ensureUserLoaded,
  resolveHomeRedirect,
  checkAdminAccess,
  checkModuleAccess
} from './guards'

const routes: RouteRecordRaw[] = [
  {
    path: '/',
    name: 'Home',
    component: () => importWithRetry(() => import('@/views/public/Home.vue')),
    meta: { requiresAuth: false }
  },

  {
    path: '/guide',
    component: () => importWithRetry(() => import('@/views/public/guide/GuideLayout.vue')),
    meta: { requiresAuth: false },
    children: [
      {
        path: '',
        name: 'GuideOverview',
        component: () => importWithRetry(() => import('@/views/public/guide/Overview.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'architecture',
        name: 'GuideArchitecture',
        component: () => importWithRetry(() => import('@/views/public/guide/ArchitectureGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'concepts',
        name: 'GuideConcepts',
        component: () => importWithRetry(() => import('@/views/public/guide/ConceptsGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'strategy',
        name: 'GuideStrategy',
        component: () => importWithRetry(() => import('@/views/public/guide/StrategyGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'advanced',
        name: 'GuideAdvanced',
        component: () => importWithRetry(() => import('@/views/public/guide/AdvancedGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'faq',
        name: 'GuideFaq',
        component: () => importWithRetry(() => import('@/views/public/guide/GuideFaq.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'modules',
        name: 'GuideModules',
        component: () => importWithRetry(() => import('@/views/public/guide/ModulesGuide.vue')),
        meta: { requiresAuth: false }
      }
    ]
  },
  {
    path: '/logo-demo',
    name: 'LogoColorDemo',
    component: () => importWithRetry(() => import('@/views/public/LogoColorDemo.vue')),
    meta: { requiresAuth: false }
  },
  {
    path: '/auth/callback',
    name: 'AuthCallback',
    component: () => importWithRetry(() => import('@/views/public/AuthCallback.vue')),
    meta: { requiresAuth: false }
  },

  {
    path: '/dashboard',
    component: () => importWithRetry(() => import('@/layouts/MainLayout.vue')),
    meta: { requiresAuth: true },
    children: [
      {
        path: '',
        name: 'Dashboard',
        component: () => importWithRetry(() => import('@/views/shared/Dashboard.vue'))
      },
      {
        path: 'api-keys',
        name: 'MyApiKeys',
        component: () => importWithRetry(() => import('@/views/user/MyApiKeys.vue'))
      },
      {
        path: 'management-tokens',
        name: 'ManagementTokens',
        component: () => importWithRetry(() => import('@/views/user/ManagementTokens.vue')),
        meta: { module: 'management_tokens' }
      },
      {
        path: 'announcements',
        name: 'Announcements',
        component: () => importWithRetry(() => import('@/views/user/Announcements.vue'))
      },
      {
        path: 'usage',
        name: 'MyUsage',
        component: () => importWithRetry(() => import('@/views/shared/Usage.vue'))
      },
      {
        path: 'endpoint-status',
        name: 'EndpointStatus',
        component: () => importWithRetry(() => import('@/views/shared/HealthMonitor.vue'))
      },
      {
        path: 'settings',
        name: 'Settings',
        component: () => importWithRetry(() => import('@/views/user/Settings.vue'))
      },
      {
        path: 'wallet',
        name: 'WalletCenter',
        component: () => importWithRetry(() => import('@/views/user/WalletCenter.vue'))
      },
      {
        path: 'models',
        name: 'ModelCatalog',
        component: () => importWithRetry(() => import('@/views/user/ModelCatalog.vue'))
      },
      {
        path: 'async-tasks',
        name: 'UserAsyncTasks',
        component: () => importWithRetry(() => import('@/views/admin/AsyncTasks.vue'))
      }
    ]
  },
  {
    path: '/admin',
    component: () => importWithRetry(() => import('@/layouts/MainLayout.vue')),
    meta: { requiresAuth: true, requiresAdmin: true },
    children: [
      {
        path: 'dashboard',
        name: 'AdminDashboard',
        component: () => importWithRetry(() => import('@/views/shared/Dashboard.vue'))
      },
      {
        path: 'users',
        name: 'Users',
        component: () => importWithRetry(() => import('@/views/admin/Users.vue'))
      },
      {
        path: 'keys',
        name: 'ApiKeys',
        component: () => importWithRetry(() => import('@/views/admin/ApiKeys.vue'))
      },
      {
        path: 'wallets',
        name: 'WalletsManagement',
        component: () => importWithRetry(() => import('@/views/admin/WalletsManagement.vue'))
      },
      {
        path: 'management-tokens',
        name: 'AdminManagementTokens',
        component: () => importWithRetry(() => import('@/views/user/ManagementTokens.vue')),
        meta: { module: 'management_tokens' }
      },
      {
        path: 'providers',
        name: 'ProviderManagement',
        component: () => importWithRetry(() => import('@/views/admin/ProviderManagement.vue'))
      },
      {
        path: 'pool',
        name: 'PoolManagement',
        component: () => importWithRetry(() => import('@/views/admin/PoolManagement.vue'))
      },
      {
        path: 'models',
        name: 'ModelManagement',
        component: () => importWithRetry(() => import('@/views/admin/ModelManagement.vue'))
      },
      {
        path: 'health-monitor',
        name: 'HealthMonitor',
        component: () => importWithRetry(() => import('@/views/shared/HealthMonitor.vue'))
      },
      {
        path: 'usage',
        name: 'Usage',
        component: () => importWithRetry(() => import('@/views/shared/Usage.vue'))
      },
      {
        path: 'user-stats',
        name: 'UserStats',
        component: () => importWithRetry(() => import('@/views/admin/UserStats.vue'))
      },
      {
        path: 'cost-analysis',
        name: 'CostAnalysis',
        component: () => importWithRetry(() => import('@/views/admin/CostAnalysis.vue'))
      },
      {
        path: 'performance-analysis',
        name: 'PerformanceAnalysis',
        component: () => importWithRetry(() => import('@/views/admin/PerformanceAnalysis.vue'))
      },
      {
        path: 'system',
        name: 'SystemSettings',
        component: () => importWithRetry(() => import('@/views/admin/SystemSettings.vue'))
      },
      {
        path: 'modules',
        name: 'ModuleManagement',
        component: () => importWithRetry(() => import('@/views/admin/ModuleManagement.vue'))
      },
      {
        path: 'email',
        name: 'EmailSettings',
        component: () => importWithRetry(() => import('@/views/admin/EmailSettings.vue'))
      },
      {
        path: 'ldap',
        name: 'LdapSettings',
        component: () => importWithRetry(() => import('@/views/admin/LdapSettings.vue')),
        meta: { module: 'ldap' }
      },
      {
        path: 'oauth',
        name: 'OAuthSettings',
        component: () => importWithRetry(() => import('@/views/admin/OAuthSettings.vue')),
        meta: { module: 'oauth' }
      },
      {
        path: 'audit-logs',
        name: 'AuditLogs',
        component: () => importWithRetry(() => import('@/views/admin/AuditLogs.vue'))
      },
      {
        path: 'cache-monitoring',
        name: 'CacheMonitoring',
        component: () => importWithRetry(() => import('@/views/admin/CacheMonitoring.vue'))
      },
      {
        path: 'ip-security',
        name: 'IPSecurity',
        component: () => importWithRetry(() => import('@/views/admin/IPSecurity.vue'))
      },
      {
        path: 'announcements',
        name: 'AnnouncementManagement',
        component: () => importWithRetry(() => import('@/views/user/Announcements.vue'))
      },
      {
        path: 'async-tasks',
        name: 'AsyncTasks',
        component: () => importWithRetry(() => import('@/views/admin/AsyncTasks.vue'))
      },
      {
        path: 'proxy-nodes',
        name: 'ProxyNodes',
        component: () => importWithRetry(() => import('@/views/admin/ProxyNodes.vue')),
        meta: { module: 'proxy_nodes' }
      },
      {
        path: 'gemini-files',
        name: 'GeminiFilesManagement',
        component: () => importWithRetry(() => import('@/views/admin/GeminiFilesManagement.vue'))
      },
      // 保留旧路由兼容性
      {
        path: 'video-tasks',
        redirect: '/admin/async-tasks'
      }
    ]
  }
]

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
