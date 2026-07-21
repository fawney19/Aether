import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, reactive, type App } from 'vue'
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

function mountReactiveElapsedTimeText(initialProps: Record<string, unknown>) {
  const props = reactive(initialProps)
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({
    render: () => h(ElapsedTimeText, { ...props }),
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return { props, root }
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

  it('does not pause or move total time backwards when the first-byte clock arrives', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-07-17T12:00:06.250Z'))

    const { props, root } = mountReactiveElapsedTimeText({
      status: 'pending',
      createdAt: '2026-07-17T12:00:00Z',
      responseTimeUpdatedAt: null,
      responseTimeMs: null,
    })
    await nextTick()
    expect(root.textContent).toBe('6.25s')

    // The first-byte snapshot implies 5.85s at the same instant because its
    // timestamp is truncated to seconds. The visible clock must stay continuous.
    props.status = 'streaming'
    props.responseTimeUpdatedAt = '2026-07-17T12:00:06Z'
    props.responseTimeMs = 5600
    await nextTick()
    expect(root.textContent).toBe('6.25s')

    vi.advanceTimersByTime(500)
    await nextTick()
    expect(Number.parseFloat(root.textContent ?? '')).toBeGreaterThanOrEqual(6.74)
  })
})
