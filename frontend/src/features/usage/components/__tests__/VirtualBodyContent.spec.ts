import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import VirtualBodyContent from '../RequestDetailDrawer/VirtualBodyContent.vue'

type Chunk = { hasNext: boolean, text: string }
const apps: Array<{ app: App, root: HTMLElement }> = []
afterEach(() => {
  for (const { app, root } of apps.splice(0)) { app.unmount(); root.remove() }
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

function mountChunks(loadChunk: (index: number) => Chunk | Promise<Chunk>) {
  const errors: unknown[] = []
  const viewer = ref<{ refresh: (index: number, resetTail?: boolean) => void } | null>(null)
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({ setup: () => () => h(VirtualBodyContent<Chunk>, {
    ref: viewer,
    loadChunk,
    onLoadError: (error: unknown) => errors.push(error),
  }, { default: ({ chunk }: { chunk: Chunk }) => h('div', { class: 'chunk-text' }, chunk.text) }) }))
  app.mount(root)
  apps.push({ app, root })
  const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
  return { app, root, viewport, viewer, errors }
}

function scroll(viewport: HTMLElement, top: number) {
  viewport.scrollTop = top
  viewport.dispatchEvent(new Event('scroll'))
}

describe('virtual body scrolling', () => {
  it('keeps neighboring chunks for smooth boundaries, evicts distant ones and reloads on return', async () => {
    const loadChunk = vi.fn((index: number) => ({ text: `chunk-${index}`, hasNext: index < 20 }))
    const { root, viewport } = mountChunks(loadChunk)
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-0'))
    scroll(viewport, 800)
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-1'))
    expect(root.textContent).toContain('chunk-0')
    scroll(viewport, 0)
    await nextTick()
    expect(loadChunk.mock.calls.filter(([index]) => index === 0)).toHaveLength(1)
    for (let index = 1; index <= 12; index += 1) {
      scroll(viewport, index * 1000)
      await vi.waitFor(() => expect(root.textContent).toContain(`chunk-${index}`))
      expect(root.querySelectorAll('[data-body-chunk]').length).toBeLessThanOrEqual(4)
    }
    expect(root.textContent).not.toContain('chunk-0')
    scroll(viewport, 0)
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-0'))
    expect(loadChunk.mock.calls.filter(([index]) => index === 0)).toHaveLength(2)
  })

  it('deduplicates rapid scrolling, ignores obsolete refresh results and stops after the end', async () => {
    const pending: Array<(value: Chunk) => void> = []
    const loadChunk = vi.fn(() => new Promise<Chunk>(resolve => pending.push(resolve)))
    const { root, viewport, viewer } = mountChunks(loadChunk)
    for (let count = 0; count < 10; count += 1) scroll(viewport, 100)
    await nextTick()
    expect(loadChunk).toHaveBeenCalledTimes(1)
    viewer.value!.refresh(0)
    pending[0]({ text: 'obsolete', hasNext: true })
    await vi.waitFor(() => expect(loadChunk).toHaveBeenCalledTimes(2))
    expect(root.textContent).not.toContain('obsolete')
    pending[1]({ text: 'complete', hasNext: false })
    await vi.waitFor(() => expect(root.textContent).toContain('complete'))
    scroll(viewport, 5000)
    await nextTick()
    expect(loadChunk).toHaveBeenCalledTimes(2)
  })

  it('ignores errors after unmount and disconnects its resize observer', async () => {
    const disconnect = vi.fn()
    vi.stubGlobal('ResizeObserver', class {
      observe = vi.fn()
      unobserve = vi.fn()
      disconnect = disconnect
    })
    let reject!: (reason: unknown) => void
    const { app, errors } = mountChunks(() => new Promise((_resolve, rejectPromise) => { reject = rejectPromise }))
    app.unmount()
    apps.pop()!.root.remove()
    reject(new Error('late failure'))
    await nextTick()
    expect(errors).toEqual([])
    expect(disconnect).toHaveBeenCalledTimes(1)
  })

  it('updates variable chunk heights while preserving an already-visible scroll anchor', async () => {
    let notifyResize!: () => void
    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: () => void) { notifyResize = callback }
      observe = vi.fn()
      unobserve = vi.fn()
      disconnect = vi.fn()
    })
    const measured = new Map([[0, 1000], [1, 1000], [2, 1000]])
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return { height: measured.get(Number(this.dataset.bodyChunk)) ?? 0 } as DOMRect
    })
    const { root, viewport } = mountChunks(index => ({ text: `chunk-${index}`, hasNext: index < 3 }))
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-0'))
    scroll(viewport, 1100)
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-1'))
    measured.set(0, 1600)
    notifyResize()
    await nextTick()
    expect(viewport.scrollTop).toBe(1700)
    expect(root.textContent).toContain('chunk-1')
  })

  it('fills short conversation chunks without leaving an empty visible window', async () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return { height: this.hasAttribute('data-body-chunk') ? 250 : 0 } as DOMRect
    })
    const { root, viewport } = mountChunks(index => ({ text: `chunk-${index}`, hasNext: index < 20 }))
    await vi.waitFor(() => expect(root.textContent).toContain('chunk-2'))
    for (let position = 0; position < 2500; position += 100) {
      scroll(viewport, position)
      const expected = Math.floor(position / 250)
      await vi.waitFor(() => expect(root.textContent).toContain(`chunk-${expected}`))
      expect(root.querySelectorAll('.chunk-text').length).toBeLessThanOrEqual(4)
    }
  })
})
