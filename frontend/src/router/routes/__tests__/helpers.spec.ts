import { describe, expect, it, vi } from 'vitest'

const { importWithRetryMock } = vi.hoisted(() => ({
  importWithRetryMock: vi.fn(async (loader: () => Promise<unknown>) => loader()),
}))

vi.mock('@/utils/importRetry', () => ({
  importWithRetry: importWithRetryMock,
}))

import { view } from '../helpers'

describe('route view loader', () => {
  it('exposes a side-effect-free raw loader for navigation prefetch', async () => {
    const component = { name: 'LazyPage' }
    const rawLoader = vi.fn(async () => component)
    const routeLoader = view(rawLoader)

    await expect(routeLoader.prefetch()).resolves.toBe(component)
    expect(rawLoader).toHaveBeenCalledTimes(1)
    expect(importWithRetryMock).not.toHaveBeenCalled()

    await expect(routeLoader()).resolves.toBe(component)
    expect(importWithRetryMock).toHaveBeenCalledTimes(1)
  })
})
