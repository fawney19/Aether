export interface ObserveNearViewportOnceOptions {
  target: Element
  root?: Element | null
  rootMargin?: string
  onNearViewport: () => void
}

/**
 * Run a callback once when an element reaches the viewport's preload margin.
 * Browsers without IntersectionObserver load eagerly so content never remains stuck.
 */
export function observeNearViewportOnce({
  target,
  root = null,
  rootMargin = '0px',
  onNearViewport,
}: ObserveNearViewportOnceOptions): () => void {
  if (typeof IntersectionObserver === 'undefined') {
    onNearViewport()
    return () => {}
  }

  let stopped = false
  let observer: IntersectionObserver | null = null

  observer = new IntersectionObserver((entries) => {
    if (stopped || !entries.some(entry => entry.isIntersecting)) return

    stopped = true
    observer?.disconnect()
    onNearViewport()
  }, {
    root,
    rootMargin,
    threshold: 0,
  })

  observer.observe(target)

  return () => {
    if (stopped) return
    stopped = true
    observer?.disconnect()
  }
}
