import { describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import { navigateAfterLogin } from '../loginRedirect'

function createRouterMock(push: Router['push']): Router {
  return { push } as Router
}

async function createDuplicatedNavigationFailure(path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path,
        component: {},
      },
    ],
  })

  await router.push(path)
  return router.push(path)
}

async function createAbortedNavigationFailure(path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path,
        component: {},
      },
    ],
  })
  router.beforeEach(() => false)

  return router.push(path)
}

describe('navigateAfterLogin', () => {
  it('never passes a cross-origin target to router or document navigation', async () => {
    const push = vi.fn<Router['push']>().mockResolvedValue(undefined)
    const documentNavigate = vi.fn()

    await navigateAfterLogin(createRouterMock(push), '//attacker.example/steal', documentNavigate)

    expect(push).toHaveBeenCalledWith('/')
    expect(documentNavigate).not.toHaveBeenCalled()
  })

  it('treats duplicated Vue Router navigation as a completed login navigation', async () => {
    const push = vi.fn<Router['push']>().mockResolvedValue(await createDuplicatedNavigationFailure('/dashboard'))
    const documentNavigate = vi.fn()

    const result = await navigateAfterLogin(createRouterMock(push), '/dashboard', documentNavigate)

    expect(push).toHaveBeenCalledWith('/dashboard')
    expect(documentNavigate).not.toHaveBeenCalled()
    expect(result).toBe('already-there')
  })

  it('falls back to document navigation when route chunk loading rejects during SPA navigation', async () => {
    const push = vi.fn<Router['push']>().mockRejectedValue(new Error('Failed to fetch dynamically imported module'))
    const documentNavigate = vi.fn()

    const result = await navigateAfterLogin(createRouterMock(push), '/admin/dashboard', documentNavigate)

    expect(push).toHaveBeenCalledWith('/admin/dashboard')
    expect(documentNavigate).toHaveBeenCalledWith('/admin/dashboard')
    expect(result).toBe('document')
  })

  it('falls back to document navigation when the router reports a real navigation failure', async () => {
    const push = vi.fn<Router['push']>().mockResolvedValue(await createAbortedNavigationFailure('/dashboard'))
    const documentNavigate = vi.fn()

    const result = await navigateAfterLogin(createRouterMock(push), '/dashboard', documentNavigate)

    expect(push).toHaveBeenCalledWith('/dashboard')
    expect(documentNavigate).toHaveBeenCalledWith('/dashboard')
    expect(result).toBe('document')
  })
})
