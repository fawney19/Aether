import { log } from '@/utils/logger'
import type { Router } from 'vue-router'

const PREFETCH_COOLDOWN_MS = 5 * 1000

const lastPrefetchAt = new Map<string, number>()

type PrefetchableRouteLoader = (() => unknown) & {
  prefetch?: () => Promise<unknown>
}

/**
 * 预取目标路由链中的异步组件。
 *
 * 直接从 Router 解析组件，避免维护一份容易遗漏用户页和新增管理页的硬编码表。
 */
export function prefetchNavigationTarget(router: Router, href: string): void {
  const now = Date.now()
  const lastRun = lastPrefetchAt.get(href) ?? 0
  if (now - lastRun < PREFETCH_COOLDOWN_MS) {
    return
  }

  const loaders = router.resolve(href).matched.flatMap((record) =>
    Object.values(record.components ?? {})
      .filter((component): component is PrefetchableRouteLoader =>
        typeof component === 'function'
        && typeof (component as PrefetchableRouteLoader).prefetch === 'function'
      )
      .map(component => component.prefetch as () => Promise<unknown>)
  )
  if (loaders.length === 0) return

  lastPrefetchAt.set(href, now)

  for (const loader of new Set(loaders)) {
    void Promise.resolve()
      .then(() => loader())
      .catch((err) => {
        log.debug('[navigationPrefetch] ignore prefetch failure', err)
      })
  }
}
