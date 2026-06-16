import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import ElapsedTimeText from '../ElapsedTimeText.vue'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountElapsedTimeText(props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({
    render: () => h(ElapsedTimeText, props),
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

afterEach(() => {
  vi.useRealTimers()
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ElapsedTimeText', () => {
  it('uses active response timing from response_time_updated_at instead of stale created_at', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-06-08T12:00:10.000Z'))

    const root = mountElapsedTimeText({
      status: 'streaming',
      createdAt: '2026-06-08T11:59:00Z',
      responseTimeUpdatedAt: '2026-06-08T12:00:06Z',
      responseTimeMs: 1500,
    })
    await nextTick()

    expect(root.textContent).toBe('5.50s')
  })

  it('falls back to created_at when active timing has not reached the backend yet', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-06-08T12:00:10.000Z'))

    const root = mountElapsedTimeText({
      status: 'pending',
      createdAt: '2026-06-08T12:00:06Z',
      responseTimeUpdatedAt: null,
      responseTimeMs: null,
    })
    await nextTick()

    expect(root.textContent).toBe('4.00s')
  })
})
