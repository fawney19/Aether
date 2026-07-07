import type { Component } from 'vue'
import {
  Rocket,
  Network,
  BookOpen,
  Target,
  Settings,
  Blocks,
  HelpCircle
} from 'lucide-vue-next'
import type { MessageKey } from '@/i18n'

// 导航配置
export interface GuideNavItem {
  id: string
  nameKey: MessageKey
  path: string
  icon: Component
  descriptionKey?: MessageKey
  subItems?: { nameKey: MessageKey; hash: string }[]
}

export const guideNavItems: GuideNavItem[] = [
  {
    id: 'overview',
    nameKey: 'guide.nav.overview',
    path: '/guide',
    icon: Rocket,
    descriptionKey: 'guide.nav.overview.description',
    subItems: [
      { nameKey: 'guide.nav.overview.production', hash: '#production' },
      { nameKey: 'guide.nav.overview.configSteps', hash: '#config-steps' },
      { nameKey: 'guide.nav.overview.reverseProxy', hash: '#reverse-proxy' },
      { nameKey: 'guide.nav.overview.asyncTasks', hash: '#async-tasks' },
      { nameKey: 'guide.nav.overview.proxyConfig', hash: '#proxy-config' }
    ]
  },
  {
    id: 'architecture',
    nameKey: 'guide.nav.architecture',
    path: '/guide/architecture',
    icon: Network,
    descriptionKey: 'guide.nav.architecture.description'
  },
  {
    id: 'concepts',
    nameKey: 'guide.nav.concepts',
    path: '/guide/concepts',
    icon: BookOpen,
    descriptionKey: 'guide.nav.concepts.description',
    subItems: [
      { nameKey: 'guide.nav.concepts.createModel', hash: '#create-model' },
      { nameKey: 'guide.nav.concepts.addProvider', hash: '#add-provider' },
      { nameKey: 'guide.nav.concepts.addEndpoint', hash: '#add-endpoint' },
      { nameKey: 'guide.nav.concepts.addKey', hash: '#add-key' },
      { nameKey: 'guide.nav.concepts.modelPermission', hash: '#model-permission' },
      { nameKey: 'guide.nav.concepts.linkModel', hash: '#link-model' },
      { nameKey: 'guide.nav.concepts.modelMapping', hash: '#model-mapping' },
      { nameKey: 'guide.nav.overview.reverseProxy', hash: '#reverse-proxy' },
      { nameKey: 'guide.nav.concepts.priorityManagement', hash: '#priority-management' }
    ]
  },
  {
    id: 'strategy',
    nameKey: 'guide.nav.strategy',
    path: '/guide/strategy',
    icon: Target,
    descriptionKey: 'guide.nav.strategy.description',
    subItems: [
      { nameKey: 'guide.nav.strategy.requestLogging', hash: '#request-logging' },
      { nameKey: 'guide.nav.strategy.scheduling', hash: '#scheduling' },
      { nameKey: 'guide.nav.strategy.rateLimit', hash: '#rate-limit' },
      { nameKey: 'guide.nav.strategy.payloadCleanup', hash: '#payload-cleanup' },
      { nameKey: 'guide.nav.strategy.cronTasks', hash: '#cron-tasks' }
    ]
  },
  {
    id: 'advanced',
    nameKey: 'guide.nav.advanced',
    path: '/guide/advanced',
    icon: Settings,
    descriptionKey: 'guide.nav.advanced.description',
    subItems: [
      { nameKey: 'guide.nav.advanced.formatConversion', hash: '#format-conversion' },
      { nameKey: 'guide.nav.advanced.streamPolicy', hash: '#stream-policy' },
      { nameKey: 'guide.nav.advanced.headerBodyEdit', hash: '#header-body-edit' },
      { nameKey: 'guide.nav.concepts.modelMapping', hash: '#model-mapping' },
      { nameKey: 'guide.nav.advanced.regexMapping', hash: '#regex-mapping' },
      { nameKey: 'guide.nav.advanced.balanceMonitor', hash: '#balance-monitor' },
      { nameKey: 'guide.nav.advanced.configExport', hash: '#config-export' },
      { nameKey: 'guide.nav.advanced.lockKey', hash: '#lock-key' }
    ]
  },
  {
    id: 'modules',
    nameKey: 'guide.nav.modules',
    path: '/guide/modules',
    icon: Blocks,
    descriptionKey: 'guide.nav.modules.description',
    subItems: [
      { nameKey: 'guide.nav.modules.managementTokens', hash: '#management-tokens' },
      { nameKey: 'guide.nav.modules.emailConfig', hash: '#email-config' },
      { nameKey: 'guide.nav.modules.oauthLogin', hash: '#oauth-login' },
      { nameKey: 'guide.nav.modules.ldapAuth', hash: '#ldap-auth' }
    ]
  },
  {
    id: 'faq',
    nameKey: 'guide.nav.faq',
    path: '/guide/faq',
    icon: HelpCircle,
    descriptionKey: 'guide.nav.faq.description'
  }
]

// 样式类常量 - 使用 Literary Tech 主题
export const panelClasses = {
  card: 'literary-card rounded-2xl backdrop-blur-sm transition-all duration-300',
  cardHover: 'hover:-translate-y-1 hover:shadow-lg dark:hover:shadow-[var(--book-cloth)]/10 shadow-[var(--book-cloth)]/10',
  section: 'literary-surface-inset bg-white/40 dark:bg-black/20 backdrop-blur-md rounded-xl md:rounded-2xl p-5 md:p-8 transition-colors',
  commandPanel: 'literary-surface-elevated rounded-xl overflow-hidden shadow-sm backdrop-blur-md',
  configPanel: 'literary-surface-elevated rounded-xl overflow-hidden',
  panelHeader: 'px-4 py-3 border-b literary-border bg-[var(--color-background-soft)]/50',
  codeBody: 'p-0',
  badge: 'literary-badge bg-[var(--color-background)] rounded-full px-3 py-1.5',
  badgeBlue: 'inline-flex items-center gap-1.5 rounded-full bg-blue-500/10 dark:bg-blue-500/20 border border-blue-500/20 dark:border-blue-500/40 px-2 py-0.5 text-xs font-medium text-blue-600 dark:text-blue-400',
  badgeGreen: 'inline-flex items-center gap-1.5 rounded-full bg-green-500/10 dark:bg-green-500/20 border border-green-500/20 dark:border-green-500/40 px-2 py-0.5 text-xs font-medium text-green-600 dark:text-green-400',
  badgeYellow: 'inline-flex items-center gap-1.5 rounded-full bg-yellow-500/10 dark:bg-yellow-500/20 border border-yellow-500/20 dark:border-yellow-500/40 px-2 py-0.5 text-xs font-medium text-yellow-600 dark:text-yellow-400',
  badgePurple: 'inline-flex items-center gap-1.5 rounded-full bg-purple-500/10 dark:bg-purple-500/20 border border-purple-500/20 dark:border-purple-500/40 px-2 py-0.5 text-xs font-medium text-purple-600 dark:text-purple-400',
  iconButtonSmall: [
    'flex items-center justify-center rounded-lg border h-8 w-8',
    'literary-border',
    'bg-transparent',
    'text-[var(--color-text)]',
    'transition hover:bg-[var(--color-background-soft)]'
  ].join(' ')
} as const
