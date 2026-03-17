import { NETWORK_CONFIG } from '@/config/constants'

const REFRESH_LOCK_KEY = 'aether_auth_refresh_lock'
const REFRESH_RESULT_KEY = 'aether_auth_refresh_result'
const REFRESH_CHANNEL_NAME = 'aether-auth-refresh'
const DEFAULT_WAIT_TIMEOUT_MS = NETWORK_CONFIG.API_TIMEOUT + 5000
const MAX_RETRIES = 2

type RefreshStatus = 'success' | 'failure'

type RefreshLock = {
  owner: string
  requestId: string
  expiresAt: number
}

type RefreshResult = {
  requestId: string
  status: RefreshStatus
  accessToken?: string
  emittedAt: number
}

type RefreshEventMessage = {
  type: 'refresh-result'
  payload: RefreshResult
}

type Waiter = {
  resolve: (result: RefreshResult) => void
  reject: (error: Error) => void
  timeoutId: ReturnType<typeof setTimeout>
}

type BroadcastMessageEvent = {
  data: unknown
}

export type BroadcastChannelLike = {
  postMessage(data: unknown): void
  addEventListener(type: 'message', listener: (event: BroadcastMessageEvent) => void): void
  removeEventListener(
    type: 'message',
    listener: (event: BroadcastMessageEvent) => void,
  ): void
  close?(): void
}

type CoordinatorOptions = {
  storage?: Storage | null
  waitTimeoutMs?: number
  channelFactory?: (name: string) => BroadcastChannelLike | null
}

class CrossTabRefreshTimeoutError extends Error {
  constructor(requestId: string) {
    super(`Timed out while waiting for refresh request ${requestId}`)
    this.name = 'CrossTabRefreshTimeoutError'
  }
}

function createId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `refresh-${Math.random().toString(36).slice(2, 10)}-${Date.now()}`
}

function parseJson<T>(raw: string | null): T | null {
  if (!raw) return null
  try {
    return JSON.parse(raw) as T
  } catch {
    return null
  }
}

function defaultChannelFactory(name: string): BroadcastChannelLike | null {
  if (typeof BroadcastChannel === 'undefined') {
    return null
  }
  return new BroadcastChannel(name)
}

export class CrossTabRefreshCoordinator {
  private readonly storage: Storage | null
  private readonly waitTimeoutMs: number
  private readonly tabId = createId()
  private readonly channel: BroadcastChannelLike | null
  private readonly waiters = new Map<string, Waiter>()

  private readonly onStorage = (event: StorageEvent): void => {
    if (event.key !== REFRESH_RESULT_KEY || !event.newValue) {
      return
    }
    const result = parseJson<RefreshResult>(event.newValue)
    if (result) {
      this.resolveWaiter(result)
    }
  }

  private readonly onBroadcastMessage = (event: BroadcastMessageEvent): void => {
    const message = event.data as RefreshEventMessage | null
    if (!message || message.type !== 'refresh-result') {
      return
    }
    this.resolveWaiter(message.payload)
  }

  constructor(options: CoordinatorOptions = {}) {
    this.storage = options.storage ?? (typeof window !== 'undefined' ? window.localStorage : null)
    this.waitTimeoutMs = options.waitTimeoutMs ?? DEFAULT_WAIT_TIMEOUT_MS
    this.channel = (options.channelFactory ?? defaultChannelFactory)(REFRESH_CHANNEL_NAME)

    if (typeof window !== 'undefined') {
      window.addEventListener('storage', this.onStorage)
    }
    this.channel?.addEventListener('message', this.onBroadcastMessage)
  }

  destroy(): void {
    if (typeof window !== 'undefined') {
      window.removeEventListener('storage', this.onStorage)
    }
    this.channel?.removeEventListener('message', this.onBroadcastMessage)
    this.channel?.close?.()
    for (const waiter of this.waiters.values()) {
      clearTimeout(waiter.timeoutId)
    }
    this.waiters.clear()
  }

  async run(executor: () => Promise<string>, retryCount = 0): Promise<string> {
    const activeLock = this.readActiveLock()
    if (activeLock && activeLock.owner !== this.tabId) {
      return this.waitForRefreshResult(activeLock.requestId, executor, retryCount)
    }

    const lock = this.tryAcquireLock()
    if (!lock) {
      const currentLock = this.readActiveLock()
      if (currentLock && currentLock.owner !== this.tabId) {
        return this.waitForRefreshResult(currentLock.requestId, executor, retryCount)
      }
      return executor()
    }

    try {
      const accessToken = await executor()
      this.publishRefreshResult({
        requestId: lock.requestId,
        status: 'success',
        accessToken,
        emittedAt: Date.now(),
      })
      return accessToken
    } catch (error) {
      this.publishRefreshResult({
        requestId: lock.requestId,
        status: 'failure',
        emittedAt: Date.now(),
      })
      throw error
    } finally {
      this.releaseLock(lock)
    }
  }

  private waitForRefreshResult(requestId: string, executor: () => Promise<string>, retryCount: number): Promise<string> {
    return new Promise((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        this.waiters.delete(requestId)
        reject(new CrossTabRefreshTimeoutError(requestId))
      }, this.waitTimeoutMs)

      this.waiters.set(requestId, {
        resolve: (result) => {
          if (result.status === 'success' && result.accessToken) {
            resolve(result.accessToken)
            return
          }
          reject(new Error(`Refresh request ${requestId} failed in another tab`))
        },
        reject,
        timeoutId,
      })
    }).catch((error: unknown) => {
      if (error instanceof CrossTabRefreshTimeoutError) {
        if (retryCount >= MAX_RETRIES) {
          return executor()
        }
        return this.run(executor, retryCount + 1)
      }
      throw error
    })
  }

  private tryAcquireLock(): RefreshLock | null {
    if (!this.storage) {
      return {
        owner: this.tabId,
        requestId: createId(),
        expiresAt: Date.now() + this.waitTimeoutMs,
      }
    }

    const existing = this.readActiveLock()
    if (existing && existing.owner !== this.tabId) {
      return null
    }

    const lock: RefreshLock = {
      owner: this.tabId,
      requestId: createId(),
      expiresAt: Date.now() + this.waitTimeoutMs,
    }

    try {
      // 这是一个 best-effort 跨标签页锁；写入后立刻回读，只认最终赢得竞态的 owner。
      this.storage.setItem(REFRESH_LOCK_KEY, JSON.stringify(lock))
      const current = this.readLock()
      if (current && current.owner === lock.owner && current.requestId === lock.requestId) {
        return current
      }
    } catch {
      return lock
    }

    return null
  }

  private releaseLock(lock: RefreshLock): void {
    if (!this.storage) {
      return
    }
    try {
      const current = this.readLock()
      if (current && current.owner === lock.owner && current.requestId === lock.requestId) {
        this.storage.removeItem(REFRESH_LOCK_KEY)
      }
    } catch {
      // ignore storage release failures and allow lock TTL to expire naturally
    }
  }

  private publishRefreshResult(result: RefreshResult): void {
    const message: RefreshEventMessage = {
      type: 'refresh-result',
      payload: result,
    }
    this.channel?.postMessage(message)
    if (!this.storage) {
      return
    }
    try {
      this.storage.setItem(REFRESH_RESULT_KEY, JSON.stringify(result))
      // 清理残留的 token 数据，仅依赖 BroadcastChannel 和 storage 事件的瞬时传播
      setTimeout(() => {
        try {
          this.storage?.removeItem(REFRESH_RESULT_KEY)
        } catch {
          // ignore
        }
      }, 2000)
    } catch {
      // ignore storage publish failures; BroadcastChannel already covers most browsers
    }
  }

  private resolveWaiter(result: RefreshResult): void {
    const waiter = this.waiters.get(result.requestId)
    if (!waiter) {
      return
    }
    clearTimeout(waiter.timeoutId)
    this.waiters.delete(result.requestId)
    waiter.resolve(result)
  }

  private readActiveLock(): RefreshLock | null {
    const lock = this.readLock()
    if (!lock) {
      return null
    }
    if (lock.expiresAt > Date.now()) {
      return lock
    }
    if (this.storage) {
      try {
        this.storage.removeItem(REFRESH_LOCK_KEY)
      } catch {
        // ignore storage cleanup failures; stale lock will age out on next write
      }
    }
    return null
  }

  private readLock(): RefreshLock | null {
    if (!this.storage) {
      return null
    }
    try {
      return parseJson<RefreshLock>(this.storage.getItem(REFRESH_LOCK_KEY))
    } catch {
      return null
    }
  }
}
