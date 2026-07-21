import { afterEach, describe, expect, it } from 'vitest'
import { createApp, h, type App } from 'vue'

import Badge from '../badge.vue'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('Badge', () => {
  it('renders the transparent outline variant without the card background', () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp({
      render: () => h(Badge, { variant: 'outline-transparent' }, () => 'Fast'),
    })

    app.mount(root)
    mountedApps.push({ app, root })

    const badge = root.firstElementChild
    expect(badge?.classList.contains('border-border')).toBe(true)
    expect(badge?.classList.contains('bg-transparent')).toBe(true)
    expect(badge?.classList.contains('bg-card/50')).toBe(false)
  })
})
