import type { RouteRecordRaw } from 'vue-router'
import { view } from './helpers'

export const adminRoutes: RouteRecordRaw[] = [
  {
    path: '/admin',
    component: view(() => import('@/layouts/MainLayout.vue')),
    meta: { requiresAuth: true, requiresAdmin: true },
    children: [
      {
        path: 'dashboard',
        name: 'AdminDashboard',
        component: view(() => import('@/views/shared/Dashboard.vue'))
      },
      {
        path: 'operations',
        name: 'AdminOperationsDashboard',
        component: view(() => import('@/views/admin/AdminOperationsDashboard.vue'))
      },
      {
        path: 'users',
        name: 'Users',
        component: view(() => import('@/views/admin/Users.vue'))
      },
      {
        path: 'keys',
        name: 'ApiKeys',
        component: view(() => import('@/views/admin/ApiKeys.vue'))
      },
      {
        path: 'wallets',
        name: 'WalletsManagement',
        component: view(() => import('@/views/admin/WalletsManagement.vue'))
      },
      {
        path: 'payment-gateways',
        name: 'PaymentGatewaySettings',
        component: view(() => import('@/views/admin/PaymentGatewaySettings.vue'))
      },
      {
        path: 'billing-plans',
        name: 'BillingPlansManagement',
        component: view(() => import('@/views/admin/BillingPlansManagement.vue'))
      },
      {
        path: 'referrals',
        name: 'ReferralManagement',
        component: view(() => import('@/views/admin/ReferralManagement.vue'))
      },
      {
        path: 'management-tokens',
        name: 'AdminManagementTokens',
        component: view(() => import('@/views/user/ManagementTokens.vue')),
        meta: { module: 'management_tokens' }
      },
      {
        path: 'providers',
        name: 'ProviderManagement',
        component: view(() => import('@/views/admin/ProviderManagement.vue'))
      },
      {
        path: 'pool',
        name: 'PoolManagement',
        component: view(() => import('@/views/admin/PoolManagement.vue'))
      },
      {
        path: 'models',
        name: 'ModelManagement',
        component: view(() => import('@/views/admin/ModelManagement.vue'))
      },
      {
        path: 'routing',
        name: 'RoutingProfiles',
        component: view(() => import('@/views/admin/RoutingProfiles.vue'))
      },
      {
        path: 'routing/new',
        name: 'RoutingProfileCreate',
        component: view(() => import('@/views/admin/RoutingProfiles.vue'))
      },
      {
        path: 'routing/:groupId',
        name: 'RoutingProfileDetail',
        component: view(() => import('@/views/admin/RoutingProfiles.vue'))
      },
      {
        path: 'health-monitor',
        name: 'HealthMonitor',
        component: view(() => import('@/views/shared/HealthMonitor.vue'))
      },
      {
        path: 'usage',
        name: 'Usage',
        component: view(() => import('@/views/shared/Usage.vue'))
      },
      {
        path: 'user-stats',
        name: 'UserStats',
        component: view(() => import('@/views/admin/UserStats.vue'))
      },
      {
        path: 'cost-analysis',
        name: 'CostAnalysis',
        component: view(() => import('@/views/admin/CostAnalysis.vue'))
      },
      {
        path: 'performance-analysis',
        name: 'PerformanceAnalysis',
        component: view(() => import('@/views/admin/PerformanceAnalysis.vue'))
      },
      {
        path: 'system',
        name: 'SystemSettings',
        component: view(() => import('@/views/admin/SystemSettings.vue'))
      },
      {
        path: 'modules',
        name: 'ModuleManagement',
        component: view(() => import('@/views/admin/ModuleManagement.vue'))
      },
      {
        path: 'model-directives',
        name: 'ModelDirectivesManagement',
        component: view(() => import('@/views/admin/ModelDirectivesManagement.vue')),
        meta: { module: 'model_directives' }
      },
      {
        path: 'modules/chat-pii-redaction',
        name: 'ChatPiiRedactionModule',
        component: view(() => import('@/views/admin/modules/ChatPiiRedaction.vue')),
        meta: { module: 'chat_pii_redaction' }
      },
      {
        path: 'modules/s3-backup',
        name: 'S3BackupSettings',
        component: view(() => import('@/views/admin/modules/S3BackupSettings.vue')),
        meta: { module: 's3_backup' }
      },
      {
        path: 'modules/important-notification',
        redirect: '/admin/notification-service'
      },
      {
        path: 'notification-service',
        name: 'ImportantNotificationModule',
        component: view(() => import('@/views/admin/modules/ImportantNotification.vue')),
        meta: { module: 'important_notification' }
      },
      {
        path: 'server-chan',
        redirect: '/admin/modules/server-chan'
      },
      {
        path: 'modules/server-chan',
        name: 'ServerChanSettings',
        component: view(() => import('@/views/admin/modules/ServerChanSettings.vue')),
        meta: { module: 'server_chan_push' }
      },
      {
        path: 'bark',
        redirect: '/admin/modules/bark'
      },
      {
        path: 'modules/bark',
        name: 'BarkSettings',
        component: view(() => import('@/views/admin/modules/BarkSettings.vue')),
        meta: { module: 'bark_push' }
      },
      {
        path: 'email',
        name: 'EmailSettings',
        component: view(() => import('@/views/admin/EmailSettings.vue'))
      },
      {
        path: 'ldap',
        name: 'LdapSettings',
        component: view(() => import('@/views/admin/LdapSettings.vue')),
        meta: { module: 'ldap' }
      },
      {
        path: 'oauth',
        name: 'OAuthSettings',
        component: view(() => import('@/views/admin/OAuthSettings.vue')),
        meta: { module: 'oauth' }
      },
      {
        path: 'audit-logs',
        name: 'AuditLogs',
        component: view(() => import('@/views/admin/AuditLogs.vue'))
      },
      {
        path: 'cache-monitoring',
        name: 'CacheMonitoring',
        component: view(() => import('@/views/admin/CacheMonitoring.vue'))
      },
      {
        path: 'ip-security',
        name: 'IPSecurity',
        component: view(() => import('@/views/admin/IPSecurity.vue'))
      },
      {
        path: 'announcements',
        name: 'AnnouncementManagement',
        component: view(() => import('@/views/user/Announcements.vue'))
      },
      {
        path: 'async-tasks',
        name: 'AsyncTasks',
        component: view(() => import('@/views/admin/AsyncTasks.vue'))
      },
      {
        path: 'proxy-nodes',
        name: 'ProxyNodes',
        component: view(() => import('@/views/admin/ProxyNodes.vue')),
        meta: { module: 'proxy_nodes' }
      },
      {
        path: 'gemini-files',
        name: 'GeminiFilesManagement',
        component: view(() => import('@/views/admin/GeminiFilesManagement.vue'))
      },
      {
        path: 'video-tasks',
        redirect: '/admin/async-tasks'
      }
    ]
  }
]
