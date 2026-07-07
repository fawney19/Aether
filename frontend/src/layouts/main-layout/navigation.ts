import type { RouteLocationNormalizedLoaded } from 'vue-router'
import type { LucideIcon } from 'lucide-vue-next'
import {
  Activity,
  BarChart3,
  Box,
  Cog,
  CreditCard,
  Database,
  FileUp,
  FolderTree,
  Gift,
  Gauge,
  Home,
  Key,
  KeyRound,
  Layers,
  Package,
  Puzzle,
  Send,
  Server,
  Shield,
  SlidersHorizontal,
  Users,
  Wallet,
  Zap,
  Megaphone,
} from 'lucide-vue-next'
import type { NavigationGroup } from '@/components/layout/SidebarNav.vue'
import type { ModuleStatus } from '@/api/modules'
import { BUILTIN_TOOL_BREADCRUMBS } from '@/config/builtin-tools'
import type { MessageKey } from '@/i18n'

type ModuleRecord = Record<string, ModuleStatus>
type Translate = (key: MessageKey) => string

export interface BreadcrumbItem {
  label: string
  href?: string
}

type NavItem = NavigationGroup['items'][number]

const moduleIconMap: Record<string, LucideIcon> = {
  Key,
  KeyRound,
  FileUp,
  Shield,
  Puzzle,
  Server,
  Send,
  SlidersHorizontal,
  CreditCard,
  Gift,
}

function activeModuleItems(modules: ModuleRecord, group: string): NavItem[] {
  return Object.values(modules)
    .filter(m => m.active && m.admin_route && m.admin_menu_group === group)
    .sort((a, b) => a.admin_menu_order - b.admin_menu_order)
    .map(m => ({
      name: m.display_name,
      href: m.admin_route ?? '',
      icon: moduleIconMap[m.admin_menu_icon || ''] || Puzzle
    }))
}

export function buildNavigation(options: {
  canAccessAdmin: boolean
  modules: ModuleRecord
  isModuleActive: (name: string) => boolean
  t?: Translate
}): NavigationGroup[] {
  const { canAccessAdmin, modules, isModuleActive } = options
  const t = options.t ?? ((key: MessageKey) => key)

  if (!canAccessAdmin) {
    return [
      {
        title: t('nav.group.overview'),
        items: [
          { name: t('nav.dashboard'), href: '/dashboard', icon: Home },
          { name: t('nav.healthMonitor'), href: '/dashboard/endpoint-status', icon: Activity },
        ]
      },
      {
        title: t('nav.group.resources'),
        items: [
          { name: t('nav.modelCatalog'), href: '/dashboard/models', icon: Box },
          { name: t('nav.apiKeys'), href: '/dashboard/api-keys', icon: Key },
        ]
      },
      {
        title: t('nav.group.account'),
        items: [
          { name: t('nav.walletCenter'), href: '/dashboard/wallet', icon: Wallet },
          { name: t('nav.billingCenter'), href: '/dashboard/billing', icon: Package },
          ...(isModuleActive('referral') ? [{ name: t('nav.myReferral'), href: '/dashboard/referral', icon: Gift }] : []),
          { name: t('nav.usageStats'), href: '/dashboard/usage', icon: BarChart3 },
        ]
      }
    ]
  }

  const systemItems: NavItem[] = [
    { name: t('nav.announcements'), href: '/admin/announcements', icon: Megaphone },
    { name: t('nav.cacheMonitoring'), href: '/admin/cache-monitoring', icon: Gauge },
    ...activeModuleItems(modules, 'system'),
    { name: t('nav.moduleManagement'), href: '/admin/modules', icon: Puzzle },
    { name: t('nav.systemSettings'), href: '/admin/system', icon: Cog },
  ]

  return [
    {
      title: t('nav.group.overview'),
      items: [
        { name: t('nav.dashboard'), href: '/admin/dashboard', icon: Home },
        { name: t('nav.operations'), href: '/admin/operations', icon: Activity },
        { name: t('nav.healthMonitor'), href: '/admin/health-monitor', icon: Activity },
        { name: t('nav.userStats'), href: '/admin/user-stats', icon: BarChart3 },
        { name: t('nav.costAnalysis'), href: '/admin/cost-analysis', icon: Gauge },
        { name: t('nav.performanceAnalysis'), href: '/admin/performance-analysis', icon: Activity },
      ]
    },
    {
      title: t('nav.group.management'),
      items: [
        { name: t('nav.userManagement'), href: '/admin/users', icon: Users },
        { name: t('nav.providers'), href: '/admin/providers', icon: FolderTree },
        { name: t('nav.modelManagement'), href: '/admin/models', icon: Layers },
        { name: t('nav.routing'), href: '/admin/routing', icon: SlidersHorizontal },
        { name: t('nav.pool'), href: '/admin/pool', icon: Database },
        { name: t('nav.standaloneKeys'), href: '/admin/keys', icon: Key },
        { name: t('nav.walletManagement'), href: '/admin/wallets', icon: Wallet },
        { name: t('nav.billingManagement'), href: '/admin/billing-plans', icon: Package },
        ...activeModuleItems(modules, 'management'),
        { name: t('nav.asyncTasks'), href: '/admin/async-tasks', icon: Zap },
        { name: t('nav.usageRecords'), href: '/admin/usage', icon: BarChart3 },
      ]
    },
    {
      title: t('nav.group.system'),
      items: systemItems
    }
  ]
}

export function buildBreadcrumbs(options: {
  route: RouteLocationNormalizedLoaded
  navigation: NavigationGroup[]
  modules: ModuleRecord
  isNavActive: (href: string) => boolean
  t?: Translate
}): BreadcrumbItem[] {
  const { route, navigation, modules, isNavActive } = options
  const t = options.t ?? ((key: MessageKey) => key)

  if (route.path === '/dashboard/settings') {
    return [
      { label: t('nav.group.account') },
      { label: t('breadcrumb.personalSettings') }
    ]
  }

  if (route.meta?.module) {
    const moduleName = route.meta.module as string
    const moduleStatus = modules[moduleName]
    const displayName = moduleStatus?.display_name || moduleName
    return [
      { label: t('nav.group.system') },
      { label: t('nav.moduleManagement'), href: '/admin/modules' },
      { label: displayName }
    ]
  }

  if (BUILTIN_TOOL_BREADCRUMBS[route.path]) {
    return [
      { label: t('nav.group.system') },
      { label: t('nav.moduleManagement'), href: '/admin/modules' },
      { label: BUILTIN_TOOL_BREADCRUMBS[route.path] }
    ]
  }

  if (route.path.startsWith('/admin/routing/') && route.path !== '/admin/routing') {
    return [
      { label: t('nav.group.management') },
      { label: t('nav.routing'), href: '/admin/routing' },
      {
        label: route.name === 'RoutingProfileCreate'
          ? t('breadcrumb.routingCreate')
          : t('breadcrumb.routingConfig')
      }
    ]
  }

  for (const group of navigation) {
    const activeItem = group.items.find(item => isNavActive(item.href))
    if (activeItem) {
      return [
        { label: group.title || '' },
        { label: activeItem.name }
      ]
    }
  }

  const currentModule = Object.values(modules).find(
    m => m.admin_route && route.path === m.admin_route
  )
  if (currentModule) {
    return [
      { label: t('nav.moduleManagement'), href: '/admin/modules' },
      { label: currentModule.display_name }
    ]
  }

  return [{ label: t('nav.dashboard') }]
}
