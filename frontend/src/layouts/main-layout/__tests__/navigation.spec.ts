import { describe, expect, it } from 'vitest'
import type { RouteLocationNormalizedLoaded } from 'vue-router'

import { buildBreadcrumbs, buildNavigation } from '@/layouts/main-layout/navigation'
import type { MessageKey } from '@/i18n'

const translate = (key: MessageKey) => `tx:${key}`

function route(path: string, name?: string, meta: Record<string, unknown> = {}): RouteLocationNormalizedLoaded {
  return {
    path,
    fullPath: path,
    query: {},
    hash: '',
    name,
    params: {},
    matched: [],
    meta,
    redirectedFrom: undefined,
  } as RouteLocationNormalizedLoaded
}

describe('main layout navigation builder', () => {
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

  it('builds admin navigation with dynamic module menu items sorted by menu order', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        first: {
          active: true,
          admin_route: '/admin/first',
          admin_menu_group: 'management',
          admin_menu_order: 2,
          admin_menu_icon: 'Gift',
          display_name: 'First module',
        },
        second: {
          active: true,
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

  it('builds translated breadcrumbs for settings and routing detail pages', () => {
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
      route: route('/admin/routing/new', 'RoutingProfileCreate'),
      navigation,
      modules: {},
      isNavActive: href => href === '/admin/routing',
      t: translate,
    })).toEqual([
      { label: 'tx:nav.group.management' },
      { label: 'tx:nav.routing', href: '/admin/routing' },
      { label: 'tx:breadcrumb.routingCreate' },
    ])
  })
})
