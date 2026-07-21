import { afterEach, describe, expect, it, vi } from 'vitest'
import { observeNearViewportOnce } from '@/utils/nearViewport'

interface MockObserverInstance {
  callback: IntersectionObserverCallback
  options?: IntersectionObserverInit
  observed: Element[]
  disconnect: ReturnType<typeof vi.fn>
}

const instances: MockObserverInstance[] = []

class MockIntersectionObserver {
  readonly root = null
  readonly rootMargin = ''
  readonly thresholds = [0]
  readonly callback: IntersectionObserverCallback
  readonly options?: IntersectionObserverInit
  readonly observed: Element[] = []
  readonly disconnect = vi.fn()

  constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
    this.callback = callback
    this.options = options
    instances.push(this)
  }

  observe(target: Element) {
    this.observed.push(target)
  }

  unobserve() {}
  takeRecords(): IntersectionObserverEntry[] { return [] }
}

afterEach(() => {
  instances.length = 0
  vi.unstubAllGlobals()
})

describe('observeNearViewportOnce', () => {
  it('uses the provided scroll root and loads once on intersection', () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver)
    const target = document.createElement('section')
    const root = document.createElement('main')
    const onNearViewport = vi.fn()

    observeNearViewportOnce({
      target,
      root,
      rootMargin: '600px 0px',
      onNearViewport,
    })

    const observer = instances[0]
    expect(observer?.observed).toEqual([target])
    expect(observer?.options).toMatchObject({ root, rootMargin: '600px 0px', threshold: 0 })
    expect(onNearViewport).not.toHaveBeenCalled()

    observer?.callback([
      { isIntersecting: true, target } as IntersectionObserverEntry,
    ], observer as unknown as IntersectionObserver)
    observer?.callback([
      { isIntersecting: true, target } as IntersectionObserverEntry,
    ], observer as unknown as IntersectionObserver)

    expect(onNearViewport).toHaveBeenCalledTimes(1)
    expect(observer?.disconnect).toHaveBeenCalledTimes(1)
  })

  it('disconnects without loading when disposed before intersection', () => {
    vi.stubGlobal('IntersectionObserver', MockIntersectionObserver)
    const target = document.createElement('section')
    const onNearViewport = vi.fn()
    const stop = observeNearViewportOnce({ target, onNearViewport })
    const observer = instances[0]

    stop()
    observer?.callback([
      { isIntersecting: true, target } as IntersectionObserverEntry,
    ], observer as unknown as IntersectionObserver)

    expect(observer?.disconnect).toHaveBeenCalledTimes(1)
    expect(onNearViewport).not.toHaveBeenCalled()
  })

  it('falls back to immediate loading when IntersectionObserver is unavailable', () => {
    vi.stubGlobal('IntersectionObserver', undefined)
    const onNearViewport = vi.fn()

    const stop = observeNearViewportOnce({
      target: document.createElement('section'),
      onNearViewport,
    })

    expect(onNearViewport).toHaveBeenCalledTimes(1)
    expect(() => stop()).not.toThrow()
  })
})
