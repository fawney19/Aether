import { describe, expect, it } from 'vitest'
import type { LocationQuery, RouteLocationNormalizedLoaded } from 'vue-router'

import { buildBreadcrumbs, buildNavigation } from '@/layouts/main-layout/navigation'
import type { MessageKey } from '@/i18n'
import type { ModuleStatus } from '@/api/modules'

const translate = (key: MessageKey) => `tx:${key}`

function route(path: string, name?: string, meta: Record<string, unknown> = {}, query: LocationQuery = {}): RouteLocationNormalizedLoaded {
  return {
    path,
    fullPath: path,
    query,
    hash: '',
    name,
    params: {},
    matched: [],
    meta,
    redirectedFrom: undefined,
  } as RouteLocationNormalizedLoaded
}

describe('main layout navigation builder', () => {
  it.each([true, false])('keeps one admin referral history entry with active=%s while user entry follows the switch', active => {
    const referral: ModuleStatus = {
      name: 'referral', available: active, enabled: active, active,
      config_validated: true, config_error: null, display_name: '邀请返利',
      description: '', category: 'integration', health: 'healthy',
      admin_route: '/admin/referrals', admin_menu_group: 'management',
      admin_menu_icon: 'Gift', admin_menu_order: 70,
    }
    const options = { modules: { referral }, isModuleActive: () => active, t: translate }
    const admin = buildNavigation({ ...options, canAccessAdmin: true }).flatMap(group => group.items)
    expect(admin.filter(item => item.href === '/admin/referrals')).toHaveLength(1)
    const user = buildNavigation({ ...options, canAccessAdmin: false }).flatMap(group => group.items)
    expect(user.some(item => item.href === '/dashboard/referral')).toBe(active)
    expect(user.some(item => item.href === '/admin/referrals')).toBe(false)
  })

  it('keeps administrator referral history reachable when module status has not loaded', () => {
    const items = buildNavigation({ canAccessAdmin: true, modules: {}, isModuleActive: () => false, t: translate }).flatMap(group => group.items)
    expect(items.filter(item => item.href === '/admin/referrals')).toHaveLength(1)
  })

  it('builds user navigation from translation keys and active modules', () => {
    const navigation = buildNavigation({
      canAccessAdmin: false,
      modules: {},
      isModuleActive: (name) => name === 'referral',
      t: translate,
    })

    expect(navigation.map(group => group.title)).toEqual([
      'tx:nav.group.overview',
      'tx:nav.group.resources',
      'tx:nav.group.account',
    ])
    expect(navigation.flatMap(group => group.items.map(item => item.name))).toContain('tx:nav.myReferral')
  })

  it('exposes remote control to users and active administrators', () => {
    const userNavigation = buildNavigation({
      canAccessAdmin: false,
      modules: {},
      isModuleActive: () => false,
      t: translate,
    })
    const adminNavigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        vscodex: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/dashboard/vscodex',
          admin_menu_group: 'overview',
          admin_menu_order: 80,
          admin_menu_icon: 'SquareTerminal',
          display_name: '远程控制',
        },
      },
      isModuleActive: () => false,
      t: translate,
    })

    const findVscodeControl = (navigation: ReturnType<typeof buildNavigation>) => (
      navigation
        .flatMap(group => group.items)
        .find(item => item.href === '/dashboard/vscodex')
    )

    expect(findVscodeControl(userNavigation)).toMatchObject({
      name: 'tx:nav.vscodex',
      href: '/dashboard/vscodex',
    })
    expect(findVscodeControl(adminNavigation)).toMatchObject({
      name: '远程控制',
      href: '/dashboard/vscodex',
    })

    const overviewItems = adminNavigation.find(group => group.title === 'tx:nav.group.overview')?.items ?? []
    expect(overviewItems.findIndex(item => item.name === '远程控制')).toBe(
      overviewItems.findIndex(item => item.name === 'tx:nav.healthMonitor') + 1,
    )
  })

  it('keeps the five fixed overview destinations in product order', () => {
    const navigation = buildNavigation({ canAccessAdmin: true, modules: {}, isModuleActive: () => false })
    expect(navigation[0]?.items.map(item => item.href)).toEqual([
      '/admin/dashboard', '/admin/operations', '/admin/user-stats', '/admin/cost-analysis', '/admin/health-monitor',
    ])
  })

  it('offers one provider destination for management and scheduling', () => {
    const navigation = buildNavigation({ canAccessAdmin: true, modules: {}, isModuleActive: () => false })
    const destinations = navigation.flatMap(group => group.items.map(item => item.href))

    expect(destinations.filter(href => href === '/admin/providers')).toHaveLength(1)
    expect(destinations).not.toContain('/admin/routing')
  })

  it('builds admin navigation with dynamic module menu items sorted by menu order', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        first: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/admin/first',
          admin_menu_group: 'management',
          admin_menu_order: 2,
          admin_menu_icon: 'Gift',
          display_name: 'First module',
        },
        second: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/admin/second',
          admin_menu_group: 'management',
          admin_menu_order: 1,
          admin_menu_icon: 'Key',
          display_name: 'Second module',
        },
      },
      isModuleActive: () => false,
      t: translate,
    })

    const managementItems = navigation.find(group => group.title === 'tx:nav.group.management')?.items ?? []
    expect(managementItems.map(item => item.name)).toEqual(expect.arrayContaining(['Second module', 'First module']))
    expect(managementItems.findIndex(item => item.name === 'Second module')).toBeLessThan(
      managementItems.findIndex(item => item.name === 'First module')
    )
  })

  it('builds translated breadcrumbs for settings and module pages', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {},
      isModuleActive: () => false,
      t: translate,
    })

    expect(buildBreadcrumbs({
      route: route('/dashboard/settings'),
      navigation,
      modules: {},
      isNavActive: () => false,
      t: translate,
    })).toEqual([
      { label: 'tx:nav.group.account' },
      { label: 'tx:breadcrumb.personalSettings' },
    ])

    expect(buildBreadcrumbs({
      route: route('/dashboard/vscodex'),
      navigation: buildNavigation({
        canAccessAdmin: true,
        modules: {
          vscodex: {
            active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
            admin_route: '/dashboard/vscodex',
            admin_menu_group: 'overview',
            admin_menu_order: 80,
            admin_menu_icon: 'SquareTerminal',
            display_name: '远程控制',
          },
        },
        isModuleActive: () => false,
        t: translate,
      }),
      modules: {
        vscodex: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/dashboard/vscodex',
          admin_menu_group: 'overview',
          admin_menu_order: 80,
          admin_menu_icon: 'SquareTerminal',
          display_name: '远程控制',
        },
      },
      isNavActive: href => href === '/dashboard/vscodex',
      t: translate,
    })).toEqual([
      expect.objectContaining({ label: expect.any(String) }),
      { label: '远程控制' },
    ])
  })

  it.each<LocationQuery>([{}, { group: 'strategy-a' }, { group: 'new' }])(
    'uses the provider directory breadcrumb for every group %o',
    (query) => {
      const navigation = buildNavigation({ canAccessAdmin: true, modules: {}, isModuleActive: () => false, t: translate })

      expect(buildBreadcrumbs({
        route: route('/admin/providers', 'ProviderManagement', {}, query),
        navigation,
        modules: {},
        isNavActive: href => href === '/admin/providers',
        t: translate,
      })).toEqual([
        { label: 'tx:nav.group.management' },
        { label: 'tx:nav.providers' },
      ])
    },
  )

  it('uses the same provider directory breadcrumb for the default group', () => {
    const navigation = buildNavigation({ canAccessAdmin: true, modules: {}, isModuleActive: () => false, t: translate })

    expect(buildBreadcrumbs({
      route: route('/admin/providers', 'ProviderManagement'),
      navigation,
      modules: {},
      isNavActive: href => href === '/admin/providers',
      t: translate,
    })).toEqual([
      { label: 'tx:nav.group.management' },
      { label: 'tx:nav.providers' },
    ])
  })
})
