import { importWithRetry } from '@/utils/importRetry'

type PrefetchableRouteLoader<T> = (() => Promise<T>) & {
  prefetch: () => Promise<T>
}

export const view = <T>(loader: () => Promise<T>): PrefetchableRouteLoader<T> => {
  const routeLoader = (() => importWithRetry(loader)) as PrefetchableRouteLoader<T>
  // 预取只做一次原始 import；失败时不得触发导航加载器的清缓存和强刷恢复逻辑。
  routeLoader.prefetch = loader
  return routeLoader
}
