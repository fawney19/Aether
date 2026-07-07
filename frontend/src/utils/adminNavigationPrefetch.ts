import { log } from '@/utils/logger'

const PREFETCH_COOLDOWN_MS = 5 * 1000

const lastPrefetchAt = new Map<string, number>()

const adminRouteWarmers: Record<string, () => Promise<void>> = {
  '/admin/users': async () => {
    await import('@/views/admin/Users.vue')
  },
  '/admin/providers': async () => {
    await import('@/views/admin/ProviderManagement.vue')
  },
  '/admin/models': async () => {
    await import('@/views/admin/ModelManagement.vue')
  },
  '/admin/routing': async () => {
    await import('@/views/admin/RoutingProfiles.vue')
  },
  '/admin/pool': async () => {
    await import('@/views/admin/PoolManagement.vue')
  },
  '/admin/payment-gateways': async () => {
    await import('@/views/admin/PaymentGatewaySettings.vue')
  },
  '/admin/billing-plans': async () => {
    await import('@/views/admin/BillingPlansManagement.vue')
  },
}

export function prefetchAdminNavigationTarget(href: string): void {
  const warmer = adminRouteWarmers[href]
  if (!warmer) return

  const now = Date.now()
  const lastRun = lastPrefetchAt.get(href) ?? 0
  if (now - lastRun < PREFETCH_COOLDOWN_MS) {
    return
  }
  lastPrefetchAt.set(href, now)

  void warmer().catch((err) => {
    log.debug('[adminNavigationPrefetch] ignore prefetch failure', err)
  })
}
