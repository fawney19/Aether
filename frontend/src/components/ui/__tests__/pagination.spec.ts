import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'

import Pagination from '../pagination.vue'
import { setI18nLocale } from '@/i18n'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('Pagination', () => {
  it('updates the summary and accessible page controls when the locale changes', async () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const updateCurrent = vi.fn()
    const app = createApp({
      render: () => h(Pagination, {
        current: 1,
        total: 1250,
        pageSize: 20,
        showPageSizeSelector: false,
        'onUpdate:current': updateCurrent,
      }),
    })
    app.mount(root)
    mountedApps.push({ app, root })

    expect(root.querySelector('[aria-live]')?.textContent).toContain('共 1,250 条')
    expect(root.querySelector('[aria-current="page"]')?.getAttribute('aria-label')).toBe('第 1 页')

    setI18nLocale('en-US')
    await nextTick()

    expect(root.querySelector('[aria-live]')?.textContent).toContain('Showing 1-20 of 1,250 items')
    expect(root.querySelector('[aria-current="page"]')?.getAttribute('aria-label')).toBe('Page 1')
    expect(root.querySelector('input')?.getAttribute('aria-label')).toBe('Go to page')

    root.querySelector<HTMLButtonElement>('[aria-label="Page 2"]')?.click()
    expect(updateCurrent).toHaveBeenCalledWith(2)
  })

  it('shows a zero-based empty range after the final record is removed', async () => {
    const total = ref(1)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp({
      render: () => h(Pagination, {
        current: 1,
        total: total.value,
        showPageSizeSelector: false,
      }),
    })
    app.mount(root)
    mountedApps.push({ app, root })

    total.value = 0
    setI18nLocale('en-US')
    await nextTick()

    expect(root.querySelector('[aria-live]')?.textContent).toContain('Showing 0-0 of 0 items')
    expect(root.querySelectorAll('button')).toHaveLength(0)
  })
})
