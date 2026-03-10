import { ref, onUnmounted } from 'vue'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import { batchQueryBalance, getArchitectures, type ActionResultResponse, type ArchitectureInfo } from '@/api/providerOps'
import { formatBalanceExtraFromSchema, type CredentialsSchema } from '@/features/providers/auth-templates/schema-utils'
import type { BalanceExtraItem } from '@/features/providers/auth-templates'
import { log } from '@/utils/logger'

const MAX_BALANCE_RETRIES = 3

export function useProviderBalance() {
  // 余额数据缓存 {providerId: ActionResultResponse}
  const balanceCache = ref<Record<string, ActionResultResponse>>({})
  // 余额加载请求版本计数器（用于防止竞态条件）
  let balanceLoadVersion = 0

  // 追踪待处理的定时器，用于组件卸载时清理
  const pendingTimers = new Set<ReturnType<typeof setTimeout>>()

  // 架构 schema 缓存（用于 balance extra 格式化）
  const architectureSchemas = ref<Record<string, CredentialsSchema>>({})
  const architectureSchemasLoaded = ref(false)

  // 用于触发倒计时更新的响应式计数器
  const tickCounter = ref(0)
  let tickInterval: ReturnType<typeof setInterval> | null = null

  function startTick() {
    if (tickInterval) return
    tickInterval = setInterval(() => {
      tickCounter.value++
    }, 1000)
  }

  function stopTick() {
    if (tickInterval) {
      clearInterval(tickInterval)
      tickInterval = null
    }
  }

  /** 加载架构 schema 缓存 */
  async function loadArchitectureSchemas() {
    if (architectureSchemasLoaded.value) return
    try {
      const archs: ArchitectureInfo[] = await getArchitectures()
      const schemas: Record<string, CredentialsSchema> = {}
      for (const arch of archs) {
        if (arch.credentials_schema) {
          schemas[arch.architecture_id] = arch.credentials_schema as CredentialsSchema
        }
      }
      architectureSchemas.value = schemas
      architectureSchemasLoaded.value = true
    } catch {
      // 加载失败不影响主流程
    }
  }

  // 异步加载余额数据（使用批量接口）
  async function loadBalances(providers: ProviderWithEndpointsSummary[], fullReload = true) {
    if (fullReload) {
      balanceCache.value = {}
    }
    const currentVersion = ++balanceLoadVersion
    try {
      const opsProviderIds = providers.filter(p => p.ops_configured).map(p => p.id)
      if (opsProviderIds.length === 0) return

      const results = await batchQueryBalance(opsProviderIds)

      // 检查是否有新的请求已经开始，如果有则丢弃当前结果
      if (currentVersion !== balanceLoadVersion) return

      // 收集需要重试的 provider IDs
      const pendingProviderIds: string[] = []

      // 将结果存入缓存（包括 pending 状态）
      for (const [providerId, result] of Object.entries(results)) {
        // 存入缓存：success, auth_expired (带有效数据), pending
        if (result.status === 'success' || result.status === 'auth_expired' || result.status === 'pending') {
          balanceCache.value[providerId] = result
        }
        // 收集 pending 状态的 provider，稍后重试
        if (result.status === 'pending') {
          pendingProviderIds.push(providerId)
        }
      }

      // 如果有 pending 状态的 provider，3秒后自动重试
      if (pendingProviderIds.length > 0) {
        const timerId = setTimeout(() => {
          pendingTimers.delete(timerId)
          // 检查版本号，确保没有新的加载请求
          if (currentVersion === balanceLoadVersion) {
            retryPendingBalances(pendingProviderIds, currentVersion, 0)
          }
        }, 3000)
        pendingTimers.add(timerId)
      }
    } catch (e) {
      log.warn('[loadBalances] 加载余额数据失败', e)
    }
  }

  // 重试加载 pending 状态的余额
  async function retryPendingBalances(providerIds: string[], loadVersion: number, retryCount: number) {
    try {
      const results = await batchQueryBalance(providerIds)
      const stillPending: string[] = []

      for (const [providerId, result] of Object.entries(results)) {
        if (result.status !== 'pending') {
          balanceCache.value[providerId] = result
        } else {
          stillPending.push(providerId)
        }
      }

      // 如果还有 pending 且未达到最大重试次数，继续重试（指数退避）
      if (stillPending.length > 0 && retryCount < MAX_BALANCE_RETRIES) {
        const delay = 3000 * Math.pow(1.5, retryCount) // 3s, 4.5s, 6.75s
        const timerId = setTimeout(() => {
          pendingTimers.delete(timerId)
          // 检查版本号，确保没有新的加载请求
          if (loadVersion === balanceLoadVersion) {
            retryPendingBalances(stillPending, loadVersion, retryCount + 1)
          }
        }, delay)
        pendingTimers.add(timerId)
      }
    } catch (e) {
      log.warn('[retryPendingBalances] 重试加载余额失败', e)
    }
  }

  /**
   * 类型守卫：检查是否为 BalanceInfo（简化版）
   */
  function isBalanceInfo(data: unknown): data is { total_available: number | null; currency: string } {
    if (typeof data !== 'object' || data === null) return false
    if (!('total_available' in data) || !('currency' in data)) return false
    const d = data as Record<string, unknown>
    if (d.total_available !== null && typeof d.total_available !== 'number') return false
    if (typeof d.currency !== 'string') return false
    return true
  }

  // 获取 provider 的余额显示
  function getProviderBalance(providerId: string): { available: number | null; currency: string } | null {
    const result = balanceCache.value[providerId]
    // auth_expired 时余额数据仍有效（只是签到 Cookie 失效）
    if (!result || (result.status !== 'success' && result.status !== 'auth_expired') || !result.data) {
      return null
    }
    if (!isBalanceInfo(result.data)) {
      return null
    }
    return {
      available: result.data.total_available,
      currency: result.data.currency || 'USD',
    }
  }

  // 获取 provider 余额明细（balance + points 分开显示）
  function getProviderBalanceBreakdown(providerId: string): { balance: number; points: number; currency: string } | null {
    const result = balanceCache.value[providerId]
    if (!result || (result.status !== 'success' && result.status !== 'auth_expired') || !result.data) {
      return null
    }
    const data = result.data as Record<string, unknown>
    const extra = data.extra
    if (!extra || extra.balance === undefined || extra.points === undefined) {
      return null
    }
    return {
      balance: extra.balance,
      points: extra.points,
      currency: data.currency || 'USD',
    }
  }

  // 获取 provider 余额查询的错误状态
  function getProviderBalanceError(providerId: string): { status: string; message: string } | null {
    const result = balanceCache.value[providerId]
    if (!result) {
      return null
    }
    // pending 状态不是错误，正在加载中
    if (result.status === 'pending') {
      return null
    }
    // 认证失败或过期
    if (result.status === 'auth_failed' || result.status === 'auth_expired') {
      return {
        status: result.status,
        message: result.message || '认证失败',
      }
    }
    // 其他错误
    if (result.status !== 'success') {
      return {
        status: result.status,
        message: result.message || '查询失败',
      }
    }
    return null
  }

  // 检查余额是否正在加载中
  function isBalanceLoading(providerId: string): boolean {
    const result = balanceCache.value[providerId]
    return result?.status === 'pending'
  }

  // 获取 provider 的签到信息（从 extra 字段）
  function getProviderCheckin(providerId: string): { success: boolean | null; message: string } | null {
    const result = balanceCache.value[providerId]
    if (!result || result.status !== 'success' || !result.data) {
      return null
    }
    const data = result.data as Record<string, unknown>
    const extra = data.extra
    if (!extra || extra.checkin_success === undefined) {
      return null
    }
    return {
      success: extra.checkin_success,
      message: extra.checkin_message || '',
    }
  }

  // 获取 provider 的 Cookie 失效状态（从 extra 字段）
  function getProviderCookieExpired(providerId: string): { expired: boolean; message: string } | null {
    const result = balanceCache.value[providerId]
    if (!result || !result.data) {
      return null
    }
    if (result.status !== 'success' && result.status !== 'auth_expired') {
      return null
    }
    const data = result.data as Record<string, unknown>
    const extra = data.extra
    if (!extra || !extra.cookie_expired) {
      return null
    }
    return {
      expired: true,
      message: extra.cookie_expired_message || 'Cookie 已失效',
    }
  }

  // 格式化余额显示
  function formatBalanceDisplay(balance: { available: number | null; currency: string } | null): string {
    if (!balance || balance.available == null) {
      return '-'
    }
    const symbol = balance.currency === 'USD' ? '$' : balance.currency
    return `${symbol}${balance.available.toFixed(2)}`
  }

  // 格式化重置倒计时（从 Unix 时间戳）
  function formatResetCountdown(resetsAt: number): string {
    // 依赖 tickCounter 触发响应式更新
    void tickCounter.value

    const now = Date.now() / 1000
    const diff = resetsAt - now

    if (diff <= 0) return '即将重置'

    const totalHours = Math.floor(diff / 3600)
    const minutes = Math.floor((diff % 3600) / 60)
    const seconds = Math.floor(diff % 60)

    const pad = (n: number) => n.toString().padStart(2, '0')

    if (totalHours > 0) {
      return `${totalHours}:${pad(minutes)}:${pad(seconds)}`
    }
    return `${minutes}:${pad(seconds)}`
  }

  // 获取 provider 余额的额外信息（如窗口限额）
  function getProviderBalanceExtra(providerId: string, architectureId?: string): BalanceExtraItem[] {
    if (!architectureId) return []

    const result = balanceCache.value[providerId]
    // auth_expired 时余额数据仍有效（只是签到 Cookie 失效）
    if (!result || (result.status !== 'success' && result.status !== 'auth_expired') || !result.data) {
      return []
    }

    const data = result.data as Record<string, unknown>
    const extra = data.extra
    if (!extra) return []

    // 从 schema 缓存中获取格式化配置
    const schema = architectureSchemas.value[architectureId]
    if (!schema) return []

    return formatBalanceExtraFromSchema(schema, extra)
  }

  // 配额已用颜色（根据使用比例）
  function getQuotaUsedColorClass(provider: ProviderWithEndpointsSummary): string {
    const used = provider.monthly_used_usd ?? 0
    const quota = provider.monthly_quota_usd ?? 0
    if (quota <= 0) return 'text-foreground'
    const ratio = used / quota
    if (ratio >= 0.9) return 'text-red-600 dark:text-red-400'
    if (ratio >= 0.7) return 'text-amber-600 dark:text-amber-400'
    return 'text-foreground'
  }

  // 组件卸载时清理
  function cleanup() {
    stopTick()
    pendingTimers.forEach(clearTimeout)
    pendingTimers.clear()
  }

  onUnmounted(cleanup)

  return {
    balanceCache,
    loadArchitectureSchemas,
    loadBalances,
    getProviderBalance,
    getProviderBalanceBreakdown,
    getProviderBalanceError,
    isBalanceLoading,
    getProviderCheckin,
    getProviderCookieExpired,
    formatBalanceDisplay,
    formatResetCountdown,
    getProviderBalanceExtra,
    getQuotaUsedColorClass,
    tickCounter,
    startTick,
    stopTick,
  }
}
