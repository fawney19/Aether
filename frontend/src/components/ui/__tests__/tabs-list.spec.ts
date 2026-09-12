import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'

import Tabs from '../tabs.vue'
import TabsList from '../tabs-list.vue'
import TabsTrigger from '../tabs-trigger.vue'
import { setI18nLocale, useI18n } from '@/i18n'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('TabsList', () => {
  it('repositions the indicator after translated labels change width', async () => {
    vi.useFakeTimers()
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return { width: this.textContent === '个人设置' ? 80 : 140 } as DOMRect
    })
    vi.spyOn(HTMLElement.prototype, 'offsetLeft', 'get').mockReturnValue(4)

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp({
      setup() {
        const { t } = useI18n()
        return () => h(Tabs, { modelValue: 'settings' }, {
          default: () => h(TabsList, {}, {
            default: () => h(TabsTrigger, { value: 'settings' }, () => t('common.settings')),
          }),
        })
      },
    })
    app.mount(root)
    mountedApps.push({ app, root })
    await nextTick()
    await vi.runAllTimersAsync()

    const indicator = root.querySelector<HTMLElement>('.tabs-indicator')
    expect(indicator?.style.width).toBe('80px')
    expect(indicator?.style.transform).toBe('translateX(4px)')

    setI18nLocale('en-US')
    await nextTick()
    await vi.runAllTimersAsync()

    expect(indicator?.style.width).toBe('140px')
  })
})
