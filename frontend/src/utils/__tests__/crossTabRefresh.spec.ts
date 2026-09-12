import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { BroadcastChannelLike } from '@/utils/crossTabRefresh'
import { CrossTabRefreshCoordinator } from '@/utils/crossTabRefresh'

type Listener = (event: { data: unknown }) => void

const channelRegistry = new Map<string, Set<FakeBroadcastChannel>>()

class FakeBroadcastChannel implements BroadcastChannelLike {
  private readonly listeners = new Set<Listener>()

  constructor(private readonly name: string) {
    const channels = channelRegistry.get(name) ?? new Set<FakeBroadcastChannel>()
    channels.add(this)
    channelRegistry.set(name, channels)
  }

  postMessage(data: unknown): void {
    const channels = channelRegistry.get(this.name) ?? new Set<FakeBroadcastChannel>()
    for (const channel of channels) {
      if (channel === this) continue
      for (const listener of channel.listeners) {
        listener({ data })
      }
    }
  }

  addEventListener(_type: 'message', listener: Listener): void {
    this.listeners.add(listener)
  }

  removeEventListener(_type: 'message', listener: Listener): void {
    this.listeners.delete(listener)
  }

  close(): void {
    channelRegistry.get(this.name)?.delete(this)
  }
}

function createChannel(name: string): BroadcastChannelLike {
  return new FakeBroadcastChannel(name)
}

function createDeferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve
    reject = promiseReject
  })
  return { promise, resolve, reject }
}

function createHttpError(status: number, message: string): Error & { response: { status: number } } {
  return Object.assign(new Error(message), { response: { status } })
}

function createRacyStorage(): Storage {
  let hiddenInitialLockReads = 2
  let ownLockWrite: string | null = null

  return {
    get length() {
      return localStorage.length
    },
    clear: () => localStorage.clear(),
    getItem: (key: string) => {
      if (key === 'aether_auth_refresh_lock') {
        if (hiddenInitialLockReads > 0) {
          hiddenInitialLockReads -= 1
          return null
        }
        if (ownLockWrite) {
          const value = ownLockWrite
          ownLockWrite = null
          return value
        }
      }
      return localStorage.getItem(key)
    },
    key: (index: number) => localStorage.key(index),
    removeItem: (key: string) => localStorage.removeItem(key),
    setItem: (key: string, value: string) => {
      localStorage.setItem(key, value)
      if (key === 'aether_auth_refresh_lock') {
        ownLockWrite = value
      }
    },
  }
}

describe('CrossTabRefreshCoordinator', () => {
  beforeEach(() => {
    localStorage.clear()
    channelRegistry.clear()
  })

  afterEach(() => {
    localStorage.clear()
    channelRegistry.clear()
  })

  it('serializes refresh requests without transferring access tokens between tabs', async () => {
    let resolveRefresh!: (token: string) => void
    const firstExecutor = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          resolveRefresh = resolve
        }),
    )
    const secondExecutor = vi.fn(() => Promise.resolve('access-from-second-tab'))

    const first = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
    })
    const second = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
    })

    const firstRun = first.run(firstExecutor)
    await Promise.resolve()
    const secondRun = second.run(secondExecutor)

    expect(firstExecutor).toHaveBeenCalledTimes(1)
    expect(secondExecutor).not.toHaveBeenCalled()

    resolveRefresh('access-from-first-tab')

    await expect(firstRun).resolves.toBe('access-from-first-tab')
    await expect(secondRun).resolves.toBe('access-from-second-tab')
    expect(secondExecutor).toHaveBeenCalledTimes(1)
    expect(localStorage.getItem('aether_auth_refresh_result')).not.toContain('access-from-first-tab')

    first.destroy()
    second.destroy()
  })

  it('treats peer failure as a retry hint and verifies the session locally', async () => {
    const refreshError = new Error('refresh failed')
    const firstAttempt = createDeferred<string>()
    const firstExecutor = vi
      .fn<() => Promise<string>>()
      .mockImplementationOnce(() => firstAttempt.promise)
      .mockRejectedValue(refreshError)
    const secondExecutor = vi.fn(() => Promise.resolve('verified-in-second-tab'))

    const first = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
    })
    const second = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
    })

    const firstRun = first.run(firstExecutor)
    await Promise.resolve()
    const secondRun = second.run(secondExecutor)

    firstAttempt.reject(refreshError)

    await expect(firstRun).rejects.toThrow('refresh failed')
    await expect(secondRun).resolves.toBe('verified-in-second-tab')
    expect(firstExecutor.mock.calls.length).toBeGreaterThanOrEqual(2)
    expect(firstExecutor.mock.calls.length).toBeLessThanOrEqual(3)
    expect(secondExecutor).toHaveBeenCalledTimes(1)

    first.destroy()
    second.destroy()
  })

  it('surfaces a genuine single-tab refresh failure without waiting for the request timeout', async () => {
    const refreshError = createHttpError(401, 'authoritative refresh rejection')
    const executor = vi.fn(() => Promise.reject(refreshError))
    const coordinator = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
      waitTimeoutMs: 500,
    })

    const nextTimerTick = Symbol('next-timer-tick')
    let timerId: ReturnType<typeof setTimeout> | undefined
    const outcome = await Promise.race([
      coordinator.run(executor).catch((error: unknown) => error),
      new Promise<symbol>((resolve) => {
        timerId = setTimeout(() => resolve(nextTimerTick), 0)
      }),
    ])
    if (timerId !== undefined) clearTimeout(timerId)

    expect(outcome).toBe(refreshError)
    expect(executor).toHaveBeenCalledTimes(1)

    coordinator.destroy()
  })

  it('does not wait on other deterministic client-side refresh rejections', async () => {
    const refreshError = createHttpError(400, 'invalid refresh request')
    const executor = vi.fn(() => Promise.reject(refreshError))
    const coordinator = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
      waitTimeoutMs: 500,
    })

    const outcome = await Promise.race([
      coordinator.run(executor).catch((error: unknown) => error),
      new Promise<symbol>((resolve) => setTimeout(() => resolve(Symbol('timeout')), 0)),
    ])

    expect(outcome).toBe(refreshError)
    expect(executor).toHaveBeenCalledTimes(1)

    coordinator.destroy()
  })

  it('keeps the coordination window for a refresh-token rotation conflict', async () => {
    const refreshError = createHttpError(409, 'refresh token was rotated concurrently')
    const executor = vi.fn(() => Promise.reject(refreshError))
    const coordinator = new CrossTabRefreshCoordinator({
      storage: localStorage,
      channelFactory: createChannel,
      waitTimeoutMs: 20,
    })

    let settled = false
    const outcome = coordinator.run(executor).catch((error: unknown) => error)
    void outcome.then(() => {
      settled = true
    })
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(settled).toBe(false)
    await expect(outcome).resolves.toBe(refreshError)
    expect(executor).toHaveBeenCalledTimes(1)

    coordinator.destroy()
  })

  it('recovers a lost-lock failure when the competing refresh succeeds later', async () => {
    const failedAttempt = createDeferred<string>()
    const successfulAttempt = createDeferred<string>()
    const firstExecutor = vi
      .fn<() => Promise<string>>()
      .mockImplementationOnce(() => failedAttempt.promise)
      .mockResolvedValue('verified-after-winner')
    const secondExecutor = vi.fn(() => successfulAttempt.promise)
    const first = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 100,
    })
    const second = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 100,
    })

    const firstRun = first.run(firstExecutor)
    const secondRun = second.run(secondExecutor)
    expect(firstExecutor).toHaveBeenCalledTimes(1)
    expect(secondExecutor).toHaveBeenCalledTimes(1)

    failedAttempt.reject(createHttpError(409, 'lost refresh-token rotation'))
    await Promise.resolve()
    successfulAttempt.resolve('access-from-winner')

    await expect(secondRun).resolves.toBe('access-from-winner')
    await expect(firstRun).resolves.toBe('verified-after-winner')
    expect(firstExecutor).toHaveBeenCalledTimes(2)

    first.destroy()
    second.destroy()
  })

  it('retries the final lock owner when a competing refresh succeeds first', async () => {
    const successfulAttempt = createDeferred<string>()
    const failedAttempt = createDeferred<string>()
    const firstExecutor = vi.fn(() => successfulAttempt.promise)
    const secondExecutor = vi
      .fn<() => Promise<string>>()
      .mockImplementationOnce(() => failedAttempt.promise)
      .mockResolvedValue('verified-after-failure-hint')
    const first = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 100,
    })
    const second = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 100,
    })

    const firstRun = first.run(firstExecutor)
    const secondRun = second.run(secondExecutor)
    successfulAttempt.resolve('access-from-first-winner')
    await expect(firstRun).resolves.toBe('access-from-first-winner')

    failedAttempt.reject(createHttpError(409, 'stale rotated cookie'))
    await expect(secondRun).resolves.toBe('verified-after-failure-hint')
    expect(secondExecutor).toHaveBeenCalledTimes(2)
    expect(localStorage.getItem('aether_auth_refresh_result')).not.toContain('access-from')

    first.destroy()
    second.destroy()
  })

  it('waits past the old short grace period for a delayed competing success', async () => {
    const successfulAttempt = createDeferred<string>()
    const failedAttempt = createDeferred<string>()
    const firstExecutor = vi.fn(() => successfulAttempt.promise)
    const secondExecutor = vi
      .fn<() => Promise<string>>()
      .mockImplementationOnce(() => failedAttempt.promise)
      .mockResolvedValue('verified-after-delayed-winner')
    const first = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 500,
    })
    const second = new CrossTabRefreshCoordinator({
      storage: createRacyStorage(),
      channelFactory: createChannel,
      waitTimeoutMs: 500,
    })

    const firstRun = first.run(firstExecutor)
    const secondRun = second.run(secondExecutor)
    failedAttempt.reject(createHttpError(409, 'stale rotated cookie'))
    await new Promise((resolve) => setTimeout(resolve, 300))
    successfulAttempt.resolve('access-from-delayed-winner')

    await expect(firstRun).resolves.toBe('access-from-delayed-winner')
    await expect(secondRun).resolves.toBe('verified-after-delayed-winner')
    expect(secondExecutor).toHaveBeenCalledTimes(2)

    first.destroy()
    second.destroy()
  })

  it('does not miss a refresh result published between lock observation and waiter registration', async () => {
    const storage = {
      ...localStorage,
      getItem: vi.fn((key: string) => {
        const value = localStorage.getItem(key)
        if (key === 'aether_auth_refresh_lock' && value) {
          queueMicrotask(() => {
            const lock = JSON.parse(value) as { requestId: string }
            window.dispatchEvent(new StorageEvent('storage', {
              key: 'aether_auth_refresh_result',
              newValue: JSON.stringify({
                requestId: lock.requestId,
                status: 'success',
                emittedAt: Date.now(),
              }),
            }))
            localStorage.removeItem('aether_auth_refresh_lock')
          })
        }
        return value
      }),
    } as Storage
    localStorage.setItem('aether_auth_refresh_lock', JSON.stringify({
      owner: 'other-tab',
      requestId: 'early-result',
      expiresAt: Date.now() + 1000,
    }))
    const executor = vi.fn(() => Promise.resolve('own-access-token'))
    const coordinator = new CrossTabRefreshCoordinator({
      storage,
      channelFactory: () => null,
      waitTimeoutMs: 100,
    })

    await expect(coordinator.run(executor)).resolves.toBe('own-access-token')
    expect(executor).toHaveBeenCalledTimes(1)
    coordinator.destroy()
  })
})
