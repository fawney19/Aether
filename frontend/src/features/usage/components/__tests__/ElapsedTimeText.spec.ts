import { afterEach, describe, expect, it } from 'vitest'
import { createApp, nextTick, ref, type App } from 'vue'
import { ACTIVE_ELAPSED_DISPLAY_NOW_MS_KEY, type ActiveElapsedDisplayNowMsRef } from '../../composables/useActiveElapsedDisplayClock'
import ElapsedTimeText from '../ElapsedTimeText.vue'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountElapsedTimeText(
  props: Record<string, unknown>,
  options: { providedDisplayNowMs?: ActiveElapsedDisplayNowMsRef } = {}
) {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(ElapsedTimeText, props)
  if (options.providedDisplayNowMs) {
    app.provide(ACTIVE_ELAPSED_DISPLAY_NOW_MS_KEY, options.providedDisplayNowMs)
  }
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ElapsedTimeText', () => {
  it('uses the supplied display clock for active requests', () => {
    const root = mountElapsedTimeText({
      createdAt: '2026-05-28T12:00:00Z',
      status: 'streaming',
      responseTimeMs: 10_000,
      displayNowMs: Date.parse('2026-05-28T12:00:40Z'),
    })

    expect(root.textContent).toBe('40.00s')
  })

  it('uses the injected shared display clock when no prop is supplied', async () => {
    const displayNowMs = ref(Date.parse('2026-05-28T12:00:40Z'))
    const root = mountElapsedTimeText({
      createdAt: '2026-05-28T12:00:00Z',
      status: 'streaming',
      responseTimeMs: 10_000,
    }, { providedDisplayNowMs: displayNowMs })

    expect(root.textContent).toBe('40.00s')

    displayNowMs.value = Date.parse('2026-05-28T12:00:45Z')
    await nextTick()

    expect(root.textContent).toBe('45.00s')
  })

  it('keeps terminal requests pinned to the backend final duration', () => {
    const root = mountElapsedTimeText({
      createdAt: '2026-05-28T12:00:00Z',
      status: 'completed',
      responseTimeMs: 42_340,
      displayNowMs: Date.parse('2026-05-28T12:01:30Z'),
    })

    expect(root.textContent).toBe('42.34s')
  })

  it('clamps active elapsed time at zero when the display clock is behind', () => {
    const root = mountElapsedTimeText({
      createdAt: '2026-05-28T12:00:40Z',
      status: 'pending',
      displayNowMs: Date.parse('2026-05-28T12:00:00Z'),
    })

    expect(root.textContent).toBe('0.00s')
  })
})
