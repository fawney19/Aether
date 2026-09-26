import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { UserBatchActionResponse, UserBatchBalanceActionRequest } from '@/api/users'
import {
  createUserBatchWalletRetryCoordinator,
  UnresolvedWalletRequestMismatchError,
  WalletIdempotencyPersistenceUnavailableError,
  WalletIdempotencyUnavailableError,
} from '../userBatchWalletIdempotency'

function createStorage() {
  const values = new Map<string, string>()
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  }
}

const walletRequest = {
  selection: { user_ids: ['user-1', 'user-2'], group_ids: ['group-1'] },
  action: 'adjust_wallet_balance' as const,
  payload: { operation: 'deduct' as const, amount: 17.25 },
}
const defaultStorageKey = 'admin.users.batch.wallet-adjustment.pending.v1:default'
let testFallback: Map<string, string>

function response(interrupted = false): UserBatchActionResponse {
  return { total: 2, success: 1, failed: 0, failures: [], interrupted }
}

describe('user batch wallet idempotency', () => {
  beforeEach(() => {
    sessionStorage.clear()
    localStorage.clear()
    testFallback = new Map()
  })

  it('serializes the key with the exact top-level wallet request before sending', async () => {
    const storage = createStorage()
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => 'wallet-key-1',
    })
    let storedDuringSend: string | null = null
    let sentRequest: UserBatchBalanceActionRequest | undefined

    await coordinator.execute(walletRequest, async (request) => {
      sentRequest = request
      storedDuringSend = storage.getItem(defaultStorageKey)
      return response(true)
    })

    expect(sentRequest).toEqual({ ...walletRequest, idempotency_key: 'wallet-key-1' })
    expect(JSON.parse(storedDuringSend ?? 'null')).toEqual({
      idempotency_key: 'wallet-key-1',
      request: sentRequest,
    })
  })

  it('retains a transport failure and reopens with the exact request for retry', async () => {
    const storage = createStorage()
    const first = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => 'wallet-key-2',
    })
    const sendFailure = new Error('connection lost')
    await expect(first.execute(walletRequest, async () => { throw sendFailure })).rejects.toBe(sendFailure)

    const reopened = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => 'must-not-be-used',
    })
    const pending = reopened.getPending()
    expect(pending).toEqual({
      idempotency_key: 'wallet-key-2',
      request: { ...walletRequest, idempotency_key: 'wallet-key-2' },
    })

    const send = vi.fn(async () => response())
    await expect(reopened.retry(send)).resolves.toEqual(response())
    expect(send).toHaveBeenCalledWith(pending?.request)
    expect(storage.getItem(defaultStorageKey)).toBeNull()
  })

  it('keeps unresolved requests in persistent browser storage across coordinators', async () => {
    const first = createUserBatchWalletRetryCoordinator({
      createKey: () => 'wallet-key-persistent',
      scope: () => 'admin-1',
    })
    await expect(first.execute(walletRequest, async () => {
      throw new Error('connection lost')
    })).rejects.toThrow('connection lost')

    const reopened = createUserBatchWalletRetryCoordinator({ scope: () => 'admin-1' })
    const pending = reopened.getPending()
    expect(pending?.request).toEqual({
      ...walletRequest,
      idempotency_key: 'wallet-key-persistent',
    })

    const send = vi.fn(async () => response())
    await reopened.retry(send)
    expect(send).toHaveBeenCalledWith(pending?.request)
    expect(localStorage.getItem(
      'admin.users.batch.wallet-adjustment.pending.v1:admin-1',
    )).toBeNull()
  })

  it('keeps unresolved requests isolated by authenticated administrator', async () => {
    const storage = createStorage()
    const adminA = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      scope: () => 'admin-a',
      createKey: () => 'wallet-key-admin-a',
    })
    await expect(adminA.execute(walletRequest, async () => { throw new Error('connection lost') }))
      .rejects.toThrow('connection lost')

    const adminB = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      scope: () => 'admin-b',
      createKey: () => 'wallet-key-admin-b',
    })
    expect(adminB.getPending()).toBeNull()
    await adminB.execute(walletRequest, async () => response(true))

    expect(adminA.getPending()?.idempotency_key).toBe('wallet-key-admin-a')
    expect(adminB.getPending()?.idempotency_key).toBe('wallet-key-admin-b')
  })

  it('does not send if the authenticated administrator changes before dispatch', async () => {
    const storage = createStorage()
    let scopeReads = 0
    const send = vi.fn(async () => response())
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      scope: () => (++scopeReads === 1 ? 'admin-a' : 'admin-b'),
      createKey: () => 'wallet-key-scope-change',
    })

    await expect(coordinator.execute(walletRequest, send)).rejects.toThrow(
      'authenticated administrator changed',
    )
    expect(send).not.toHaveBeenCalled()
    expect(storage.getItem('admin.users.batch.wallet-adjustment.pending.v1:admin-a')).not.toBeNull()
  })

  it('retains interrupted requests and reuses their key until a terminal response', async () => {
    const storage = createStorage()
    const first = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => 'wallet-key-3',
    })
    await first.execute(walletRequest, async () => response(true))
    const reopened = createUserBatchWalletRetryCoordinator({ storage, fallback: testFallback })
    const pending = reopened.getPending()
    const send = vi.fn(async () => response(true))

    await reopened.retry(send)

    expect(send).toHaveBeenCalledWith(pending?.request)
    expect(reopened.getPending()).toEqual(pending)
    await reopened.retry(async (request) => {
      expect(request).toEqual(pending?.request)
      return response()
    })
    expect(reopened.getPending()).toBeNull()
  })

  it('matches the same serialized request when optional filter fields are omitted', async () => {
    const storage = createStorage()
    const requestWithUndefinedField = {
      ...walletRequest,
      selection: { filters: { search: 'active', is_active: undefined } },
    }
    const first = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => 'wallet-key-filter',
    })
    await first.execute(requestWithUndefinedField, async () => response(true))

    const reopened = createUserBatchWalletRetryCoordinator({ storage, fallback: testFallback })
    const send = vi.fn(async () => response())
    await reopened.execute({
      ...walletRequest,
      selection: { filters: { search: 'active' } },
    }, send)

    expect(send).toHaveBeenCalledWith({
      ...walletRequest,
      selection: { filters: { search: 'active' } },
      idempotency_key: 'wallet-key-filter',
    })
  })

  it('blocks changed payloads while unresolved and gives a later adjustment a new key', async () => {
    const storage = createStorage()
    let nextKey = 0
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => `wallet-key-${++nextKey}`,
    })
    await expect(coordinator.execute(walletRequest, async () => { throw new Error('connection lost') }))
      .rejects.toThrow('connection lost')
    const changedRequest = {
      ...walletRequest,
      payload: { operation: 'add' as const, amount: 20 },
    }
    const send = vi.fn(async () => response())

    await expect(coordinator.execute(changedRequest, send)).rejects.toBeInstanceOf(
      UnresolvedWalletRequestMismatchError,
    )
    expect(send).not.toHaveBeenCalled()
    expect(coordinator.getPending()?.idempotency_key).toBe('wallet-key-1')

    await coordinator.retry(async () => response())
    let newRequest
    await coordinator.execute(changedRequest, async (request) => {
      newRequest = request
      return response()
    })

    expect(newRequest).toEqual({ ...changedRequest, idempotency_key: 'wallet-key-2' })
  })

  it('fails closed when persistent storage cannot save the request', async () => {
    const unavailableStorage = {
      getItem: () => null,
      setItem: () => { throw new Error('storage unavailable') },
      removeItem: () => undefined,
    }
    const send = vi.fn(async () => response(true))
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage: unavailableStorage,
      fallback: testFallback,
      createKey: () => 'wallet-key-fallback',
    })

    await expect(coordinator.execute(walletRequest, send)).rejects.toBeInstanceOf(
      WalletIdempotencyPersistenceUnavailableError,
    )
    expect(send).not.toHaveBeenCalled()
    expect(testFallback.size).toBe(0)
  })

  it('does not send when persistent storage readback does not match', async () => {
    let wasWritten = false
    const mismatchedStorage = {
      getItem: () => wasWritten ? 'different request' : null,
      setItem: () => { wasWritten = true },
      removeItem: () => undefined,
    }
    const send = vi.fn(async () => response())
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage: mismatchedStorage,
      fallback: testFallback,
      createKey: () => 'wallet-key-readback',
    })

    await expect(coordinator.execute(walletRequest, send)).rejects.toBeInstanceOf(
      WalletIdempotencyPersistenceUnavailableError,
    )
    expect(send).not.toHaveBeenCalled()
    expect(testFallback.size).toBe(0)
  })

  it('does not create a new request when persistent storage cannot be read', async () => {
    const unavailableStorage = {
      getItem: () => { throw new Error('storage unavailable') },
      setItem: () => undefined,
      removeItem: () => undefined,
    }
    const send = vi.fn(async () => response())
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage: unavailableStorage,
      fallback: testFallback,
      createKey: () => 'must-not-be-used',
    })

    await expect(coordinator.execute(walletRequest, send)).rejects.toBeInstanceOf(
      WalletIdempotencyPersistenceUnavailableError,
    )
    expect(send).not.toHaveBeenCalled()
    expect(testFallback.size).toBe(0)
  })

  it('fails closed when secure UUID generation is unavailable', async () => {
    const storage = createStorage()
    const send = vi.fn(async () => response())
    const coordinator = createUserBatchWalletRetryCoordinator({
      storage,
      fallback: testFallback,
      createKey: () => { throw new WalletIdempotencyUnavailableError() },
    })

    await expect(coordinator.execute(walletRequest, send)).rejects.toBeInstanceOf(
      WalletIdempotencyUnavailableError,
    )
    expect(send).not.toHaveBeenCalled()
    expect(storage.getItem(defaultStorageKey)).toBeNull()
  })
})
