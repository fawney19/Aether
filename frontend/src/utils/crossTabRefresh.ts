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

function isDefinitiveRefreshRejection(error: unknown): boolean {
  if (!error || typeof error !== 'object') {
    return false
  }
  const response = (error as { response?: unknown }).response
  if (!response || typeof response !== 'object') {
    return false
  }
  const status = (response as { status?: unknown }).status
  // Refresh uses 409 exclusively for a previous-token rotation race. Every
  // other client-side rejection is deterministic and cannot be repaired by
  // waiting for another tab.
  return typeof status === 'number' && status >= 400 && status < 500 && status !== 409
}

export class CrossTabRefreshCoordinator {
  private readonly storage: Storage | null
  private readonly waitTimeoutMs: number
  private readonly tabId = createId()
  private readonly channel: BroadcastChannelLike | null
  private readonly waiters = new Map<string, Waiter>()
  private readonly earlyResults = new Map<string, RefreshResult>()
  private readonly recentSuccesses = new Map<string, number>()
  private readonly successObservers = new Set<() => void>()

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
    this.earlyResults.clear()
    this.recentSuccesses.clear()
    this.successObservers.clear()
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

    const attemptStartedAt = Date.now()
    try {
      const accessToken = await executor()
      this.publishRefreshResult({
        requestId: lock.requestId,
        status: 'success',
        emittedAt: Date.now(),
      })
      return accessToken
    } catch (error) {
      // localStorage has no compare-and-swap primitive. Another tab may have
      // won the same refresh rotation even when this tab observed its own
      // lock, so a failure is only a retry hint. Never make a peer treat it as
      // an authoritative logout/final failure.
      const currentLock = this.readActiveLock()
      if (currentLock && currentLock.owner !== this.tabId) {
        // We can prove that this attempt lost the best-effort lock race. Do
        // not emit a failure for a request that another tab superseded; wait
        // for that request's outcome after the finally block releases ours.
        return Promise.resolve().then(() => this.waitForRefreshResult(
          currentLock.requestId,
          executor,
          retryCount + 1,
        ))
      }

      if (this.ownsLock(lock)) {
        this.publishRefreshResult({
          requestId: lock.requestId,
          status: 'failure',
          emittedAt: Date.now(),
        })
      }

      // The refresh endpoint reserves 409 for a concurrent token rotation.
      // Other 4xx responses are authoritative, so waiting for the HTTP
      // timeout cannot recover the session and would block initial navigation.
      if (isDefinitiveRefreshRejection(error)) {
        throw error
      }

      // A failure is a hint, never a cross-tab verdict. Give a concurrent
      // winner a bounded opportunity to publish success before returning the
      // local error. If one does, retry using this tab's shared HttpOnly cookie.
      return Promise.resolve().then(async () => {
        const concurrentSuccess = await this.waitForConcurrentSuccess(
          attemptStartedAt,
          retryCount,
        )
        if (concurrentSuccess && retryCount < MAX_RETRIES) {
          return this.run(executor, retryCount + 1)
        }
        throw error
      })
    } finally {
      this.releaseLock(lock)
    }
  }

  private waitForRefreshResult(requestId: string, executor: () => Promise<string>, retryCount: number): Promise<string> {
    return new Promise<RefreshResult>((resolve, reject) => {
      const earlyResult = this.earlyResults.get(requestId)
      if (earlyResult) {
        this.earlyResults.delete(requestId)
        resolve(earlyResult)
        return
      }

      const timeoutId = setTimeout(() => {
        this.waiters.delete(requestId)
        reject(new CrossTabRefreshTimeoutError(requestId))
      }, this.waitTimeoutMs)

      this.waiters.set(requestId, {
        resolve,
        reject,
        timeoutId,
      })
    }).then((result) => {
      if (result.status === 'success') {
        // Tokens are never transferred between tabs. The previous request
        // rotated the shared HttpOnly cookie; wait for its lock release, then
        // obtain a token directly from the server in this tab.
        return this.waitForLockRelease(executor, retryCount)
      }

      // A failure from another tab is deliberately non-authoritative. It can
      // be the loser of a refresh-token rotation, so wait for that attempt to
      // release its lock and verify the shared session from this tab.
      if (retryCount >= MAX_RETRIES) {
        return this.run(executor, retryCount)
      }
      return this.waitForLockRelease(executor, retryCount + 1)
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

  private waitForLockRelease(executor: () => Promise<string>, retryCount: number): Promise<string> {
    return new Promise<string>((resolve) => {
      const deadline = Date.now() + this.waitTimeoutMs
      const wait = () => {
        const activeLock = this.readActiveLock()
        if (!activeLock || activeLock.owner === this.tabId || Date.now() >= deadline) {
          resolve(this.run(executor, retryCount))
          return
        }
        setTimeout(wait, 0)
      }
      wait()
    })
  }

  private waitForConcurrentSuccess(attemptStartedAt: number, retryCount: number): Promise<boolean> {
    if (retryCount >= MAX_RETRIES) {
      return Promise.resolve(false)
    }

    const hasSuccess = (): boolean => {
      for (const [requestId, emittedAt] of this.recentSuccesses) {
        if (emittedAt >= attemptStartedAt && requestId) {
          return true
        }
      }
      return false
    }

    if (hasSuccess()) {
      return Promise.resolve(true)
    }

    return new Promise<boolean>((resolve) => {
      let settled = false
      const finish = (value: boolean): void => {
        if (settled) return
        settled = true
        this.successObservers.delete(onSuccess)
        clearTimeout(timeoutId)
        resolve(value)
      }
      const onSuccess = (): void => {
        if (hasSuccess()) {
          finish(true)
        }
      }
      this.successObservers.add(onSuccess)
      // A competing request can legitimately run until the HTTP client timeout.
      // The coordinator timeout includes that budget for retryable failures.
      const timeoutId = setTimeout(() => finish(hasSuccess()), this.waitTimeoutMs)
      // A result may have arrived between the initial check and observer
      // registration, so check once more synchronously.
      onSuccess()
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

  private ownsLock(lock: RefreshLock): boolean {
    // Without storage the synthetic lock is local to this coordinator. A
    // BroadcastChannel peer cannot atomically replace it, so treat it as ours
    // for the bounded failure/retry policy.
    if (!this.storage) {
      return true
    }
    const current = this.readLock()
    return Boolean(
      current
      && current.owner === lock.owner
      && current.requestId === lock.requestId,
    )
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
      // Result metadata is transient; access tokens never enter storage.
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
    if (result.status === 'success') {
      this.recentSuccesses.set(result.requestId, result.emittedAt)
      setTimeout(() => {
        if (this.recentSuccesses.get(result.requestId) === result.emittedAt) {
          this.recentSuccesses.delete(result.requestId)
        }
      }, this.waitTimeoutMs)
      for (const observer of this.successObservers) {
        observer()
      }
    }
    const waiter = this.waiters.get(result.requestId)
    if (!waiter) {
      this.earlyResults.set(result.requestId, result)
      setTimeout(() => {
        if (this.earlyResults.get(result.requestId) === result) {
          this.earlyResults.delete(result.requestId)
        }
      }, this.waitTimeoutMs)
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
