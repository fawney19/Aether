import type { RouteRecordRaw } from 'vue-router'
import { view } from './helpers'

export const dashboardRoutes: RouteRecordRaw[] = [
  {
    path: '/dashboard',
    component: view(() => import('@/layouts/MainLayout.vue')),
    meta: { requiresAuth: true },
    children: [
      {
        path: '',
        name: 'Dashboard',
        component: view(() => import('@/views/shared/Dashboard.vue'))
      },
      {
        path: 'api-keys',
        name: 'MyApiKeys',
        component: view(() => import('@/views/user/MyApiKeys.vue'))
      },
      {
        path: 'management-tokens',
        name: 'ManagementTokens',
        component: view(() => import('@/views/user/ManagementTokens.vue')),
        meta: { module: 'management_tokens' }
      },
      {
        path: 'announcements',
        name: 'Announcements',
        component: view(() => import('@/views/user/Announcements.vue'))
      },
      {
        path: 'usage',
        name: 'MyUsage',
        component: view(() => import('@/views/shared/Usage.vue'))
      },
      {
        path: 'endpoint-status',
        name: 'EndpointStatus',
        component: view(() => import('@/views/shared/HealthMonitor.vue'))
      },
      {
        path: 'settings',
        name: 'Settings',
        component: view(() => import('@/views/user/Settings.vue'))
      },
      {
        path: 'wallet',
        name: 'WalletCenter',
        component: view(() => import('@/views/user/WalletCenter.vue'))
      },
      {
        path: 'billing',
        name: 'BillingPlans',
        component: view(() => import('@/views/user/BillingPlans.vue'))
      },
      {
        path: 'referral',
        name: 'ReferralCenter',
        component: view(() => import('@/views/user/ReferralCenter.vue'))
      },
      {
        path: 'models',
        name: 'ModelCatalog',
        component: view(() => import('@/views/user/ModelCatalog.vue'))
      },
      {
        path: 'async-tasks',
        name: 'UserAsyncTasks',
        component: view(() => import('@/views/admin/AsyncTasks.vue'))
      }
    ]
  }
]
