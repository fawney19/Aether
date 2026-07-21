import { afterEach, describe, expect, it } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'

import Checkbox from '../checkbox.vue'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('Checkbox', () => {
  it('applies and clears the native indeterminate state', async () => {
    const indeterminate = ref(true)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp({
      render: () => h(Checkbox, { indeterminate: indeterminate.value }),
    })

    app.mount(root)
    mountedApps.push({ app, root })

    const input = root.querySelector<HTMLInputElement>('input[type="checkbox"]')
    expect(input?.indeterminate).toBe(true)
    expect(input?.getAttribute('aria-checked')).toBe('mixed')

    indeterminate.value = false
    await nextTick()

    expect(input?.indeterminate).toBe(false)
    expect(input?.getAttribute('aria-checked')).toBe('false')
  })
})
