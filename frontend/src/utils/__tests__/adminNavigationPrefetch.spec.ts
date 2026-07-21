import { describe, expect, it, vi } from 'vitest'
import { createMemoryHistory, createRouter } from 'vue-router'

import { view } from '@/router/routes/helpers'
import { prefetchNavigationTarget } from '@/utils/adminNavigationPrefetch'

describe('navigation prefetch', () => {
  it('warms the async component resolved from the target route', async () => {
    const component = { render: () => null }
    const rawLoader = vi.fn(async () => component)
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/settings', component: view(rawLoader) }],
    })

    prefetchNavigationTarget(router, '/settings')

    await vi.waitFor(() => {
      expect(rawLoader).toHaveBeenCalledTimes(1)
    })
  })
})
