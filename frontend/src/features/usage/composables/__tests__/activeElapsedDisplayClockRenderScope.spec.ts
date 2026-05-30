import { afterEach, describe, expect, it } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App, type Ref } from 'vue'
import {
  provideActiveElapsedDisplayClock,
  useActiveElapsedDisplayNowMs,
} from '../useActiveElapsedDisplayClock'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

function mountApp(component: ReturnType<typeof defineComponent>) {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(component)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

describe('active elapsed display clock render scope', () => {
  it('updates injected elapsed text without re-rendering the surrounding table', async () => {
    const displayNowMs = ref(1_000)
    const rows = Array.from({ length: 1_000 }, (_, index) => ({
      id: `row-${index}`,
      model: `model-${index}`,
    }))
    let tableRenderCount = 0
    let elapsedRenderCount = 0

    const ElapsedTextProbe = defineComponent({
      name: 'ElapsedTextProbe',
      setup() {
        const injectedDisplayNowMs = useActiveElapsedDisplayNowMs()
        return () => {
          elapsedRenderCount += 1
          return h('span', injectedDisplayNowMs?.value)
        }
      },
    })

    const TableProbe = defineComponent({
      name: 'TableProbe',
      setup() {
        return () => {
          tableRenderCount += 1
          return h('table', rows.map(row => h('tr', { key: row.id }, [
            h('td', row.model),
            row.id === 'row-0' ? h('td', h(ElapsedTextProbe)) : h('td', '-'),
          ])))
        }
      },
    })

    const Harness = defineComponent({
      name: 'Harness',
      setup() {
        provideActiveElapsedDisplayClock(displayNowMs as Ref<number>)
        return () => h(TableProbe)
      },
    })

    mountApp(Harness)
    expect(tableRenderCount).toBe(1)
    expect(elapsedRenderCount).toBe(1)

    displayNowMs.value = 1_250
    await nextTick()

    expect(tableRenderCount).toBe(1)
    expect(elapsedRenderCount).toBe(2)
  })
})
