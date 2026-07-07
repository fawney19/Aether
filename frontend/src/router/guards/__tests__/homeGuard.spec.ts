import { beforeEach, describe, expect, it } from 'vitest'
import type { RouteLocationNormalized } from 'vue-router'

import { resolveHomeRedirect } from '@/router/guards/homeGuard'

function route(path: string, query: Record<string, string> = {}): RouteLocationNormalized {
  return {
    path,
    fullPath: path,
    query,
    hash: '',
    name: undefined,
    params: {},
    matched: [],
    meta: {},
    redirectedFrom: undefined,
  } as RouteLocationNormalized
}

function authStore(options: { isAuthenticated: boolean; canAccessAdmin?: boolean }) {
  return {
    isAuthenticated: options.isAuthenticated,
    canAccessAdmin: options.canAccessAdmin ?? false,
  } as never
}

describe('resolveHomeRedirect', () => {
  beforeEach(() => {
    sessionStorage.clear()
  })

  it('ignores non-home routes and unauthenticated home visits', () => {
    expect(resolveHomeRedirect(route('/dashboard'), route('/'), authStore({ isAuthenticated: true }))).toBeNull()
    expect(resolveHomeRedirect(route('/'), route('/login'), authStore({ isAuthenticated: false }))).toBeNull()
  })

  it('allows authenticated users to return to the public home from the app shell', () => {
    expect(resolveHomeRedirect(route('/'), route('/dashboard'), authStore({ isAuthenticated: true }))).toBe('')
    expect(resolveHomeRedirect(route('/', { returnTo: '/guide' }), route('/external'), authStore({ isAuthenticated: true }))).toBe('')
  })

  it('consumes stored redirect path before falling back to dashboard defaults', () => {
    sessionStorage.setItem('redirectPath', '/dashboard/api-keys')

    expect(resolveHomeRedirect(route('/'), route('/external'), authStore({ isAuthenticated: true }))).toBe('/dashboard/api-keys')
    expect(sessionStorage.getItem('redirectPath')).toBeNull()
  })

  it('routes authenticated users to the correct dashboard by role', () => {
    expect(resolveHomeRedirect(route('/'), route('/external'), authStore({ isAuthenticated: true }))).toBe('/dashboard')
    expect(resolveHomeRedirect(route('/'), route('/external'), authStore({ isAuthenticated: true, canAccessAdmin: true }))).toBe('/admin/dashboard')
  })
})
