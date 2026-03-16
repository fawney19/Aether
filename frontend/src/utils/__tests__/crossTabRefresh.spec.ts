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

describe('CrossTabRefreshCoordinator', () => {
  beforeEach(() => {
    localStorage.clear()
    channelRegistry.clear()
  })

  afterEach(() => {
    localStorage.clear()
    channelRegistry.clear()
  })

  it('deduplicates refresh requests across tabs', async () => {
    let resolveRefresh: ((token: string) => void) | null = null
    const firstExecutor = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          resolveRefresh = resolve
        }),
    )
    const secondExecutor = vi.fn(() => Promise.resolve('should-not-run'))

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

    resolveRefresh?.('access-from-first-tab')

    await expect(firstRun).resolves.toBe('access-from-first-tab')
    await expect(secondRun).resolves.toBe('access-from-first-tab')

    first.destroy()
    second.destroy()
  })

  it('propagates refresh failure to waiting tabs without second refresh call', async () => {
    const refreshError = new Error('refresh failed')
    let rejectRefresh: ((error: Error) => void) | null = null
    const firstExecutor = vi.fn(
      () =>
        new Promise<string>((_resolve, reject) => {
          rejectRefresh = reject
        }),
    )
    const secondExecutor = vi.fn(() => Promise.resolve('should-not-run'))

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

    rejectRefresh?.(refreshError)

    await expect(firstRun).rejects.toThrow('refresh failed')
    await expect(secondRun).rejects.toThrow('failed in another tab')
    expect(secondExecutor).not.toHaveBeenCalled()

    first.destroy()
    second.destroy()
  })
})
