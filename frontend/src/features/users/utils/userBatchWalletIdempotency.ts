import type {
  UserBatchActionResponse,
  UserBatchBalanceActionRequest,
} from '@/api/users'

export type UserBatchWalletAdjustmentRequest = Omit<
  UserBatchBalanceActionRequest,
  'idempotency_key'
>

export interface PendingUserBatchWalletRequest {
  idempotency_key: string
  request: UserBatchBalanceActionRequest
}

interface StringStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

interface CoordinatorOptions {
  storage?: StringStorage | null
  createKey?: () => string
  fallback?: Map<string, string>
  scope?: () => string | null
}

const STORAGE_KEY = 'admin.users.batch.wallet-adjustment.pending.v1'
const inMemoryFallback = new Map<string, string>()

export class UnresolvedWalletRequestMismatchError extends Error {
  constructor(readonly pending: PendingUserBatchWalletRequest) {
    super('A different wallet batch request is still unresolved')
    this.name = 'UnresolvedWalletRequestMismatchError'
  }
}

export class WalletIdempotencyUnavailableError extends Error {
  constructor() {
    super('crypto.randomUUID is unavailable')
    this.name = 'WalletIdempotencyUnavailableError'
  }
}

export class WalletIdempotencyPersistenceUnavailableError extends Error {
  constructor() {
    super('Persistent browser storage is unavailable')
    this.name = 'WalletIdempotencyPersistenceUnavailableError'
  }
}

export class WalletIdempotencyScopeUnavailableError extends Error {
  constructor() {
    super('The authenticated administrator identity is unavailable')
    this.name = 'WalletIdempotencyScopeUnavailableError'
  }
}

export class WalletIdempotencyScopeChangedError extends Error {
  constructor() {
    super('The authenticated administrator changed before the request was sent')
    this.name = 'WalletIdempotencyScopeChangedError'
  }
}

export class InvalidPendingWalletRequestError extends Error {
  constructor() {
    super('The stored wallet batch request is invalid')
    this.name = 'InvalidPendingWalletRequestError'
  }
}

function browserPersistentStorage(): StringStorage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

function secureRandomUUID(): string {
  try {
    const cryptoApi = globalThis.crypto
    if (typeof cryptoApi?.randomUUID === 'function') {
      return cryptoApi.randomUUID()
    }
  } catch {
    // Treat unavailable secure randomness as a hard failure.
  }
  throw new WalletIdempotencyUnavailableError()
}

function isPendingRequest(value: unknown): value is PendingUserBatchWalletRequest {
  if (typeof value !== 'object' || value === null) return false
  const record = value as Partial<PendingUserBatchWalletRequest>
  const request = record.request
  return typeof record.idempotency_key === 'string'
    && record.idempotency_key.length > 0
    && typeof request === 'object'
    && request !== null
    && request.action === 'adjust_wallet_balance'
    && request.idempotency_key === record.idempotency_key
    && typeof request.selection === 'object'
    && request.selection !== null
    && typeof request.payload === 'object'
    && request.payload !== null
    && (request.payload.operation === 'add' || request.payload.operation === 'deduct')
    && Number.isFinite(request.payload.amount)
    && request.payload.amount > 0
}

function parsePendingRequest(serialized: string): PendingUserBatchWalletRequest {
  try {
    const value: unknown = JSON.parse(serialized)
    if (isPendingRequest(value)) return value
  } catch {
    // Invalid persisted state must not allow a fresh adjustment to be sent.
  }
  throw new InvalidPendingWalletRequestError()
}

function stableSerialize(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => item === undefined ? 'null' : stableSerialize(item)).join(',')}]`
  }
  if (typeof value === 'object' && value !== null) {
    const fields = Object.entries(value as Record<string, unknown>)
      .filter(([, item]) => item !== undefined)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    return `{${fields.map(([key, item]) => `${JSON.stringify(key)}:${stableSerialize(item)}`).join(',')}}`
  }
  return JSON.stringify(value) ?? 'null'
}

export function matchesPendingWalletRequest(
  pending: PendingUserBatchWalletRequest,
  request: UserBatchWalletAdjustmentRequest,
): boolean {
  const { idempotency_key: _key, ...pendingPayload } = pending.request
  return stableSerialize(pendingPayload) === stableSerialize(request)
}

export function createUserBatchWalletRetryCoordinator(options: CoordinatorOptions = {}) {
  const storage = 'storage' in options ? options.storage ?? null : browserPersistentStorage()
  const fallback = options.fallback ?? inMemoryFallback
  const createKey = options.createKey ?? secureRandomUUID
  const getScope = options.scope ?? (() => 'default')

  function getStorageKey(): string {
    const scope = getScope()
    if (!scope) throw new WalletIdempotencyScopeUnavailableError()
    return `${STORAGE_KEY}:${encodeURIComponent(scope)}`
  }

  function readPending(storageKey: string): PendingUserBatchWalletRequest | null {
    let serialized: string | null = null
    let storageReadFailed = false
    try {
      serialized = storage?.getItem(storageKey) ?? null
    } catch {
      storageReadFailed = true
      serialized = null
    }
    serialized ??= fallback.get(storageKey) ?? null
    if (serialized === null && (!storage || storageReadFailed)) {
      throw new WalletIdempotencyPersistenceUnavailableError()
    }
    return serialized === null ? null : parsePendingRequest(serialized)
  }

  function persist(storageKey: string, pending: PendingUserBatchWalletRequest): void {
    const serialized = JSON.stringify(pending)
    if (!storage) throw new WalletIdempotencyPersistenceUnavailableError()
    try {
      storage.setItem(storageKey, serialized)
      if (storage.getItem(storageKey) !== serialized) {
        throw new Error('Stored wallet batch request could not be verified')
      }
    } catch {
      throw new WalletIdempotencyPersistenceUnavailableError()
    }
    fallback.set(storageKey, serialized)
  }

  function clear(storageKey: string): void {
    fallback.delete(storageKey)
    try {
      storage?.removeItem(storageKey)
    } catch {
      // A stale persisted request is safe to replay and will fail closed on mismatch.
    }
  }

  function getOrCreate(
    storageKey: string,
    request: UserBatchWalletAdjustmentRequest,
  ): UserBatchBalanceActionRequest {
    const pending = readPending(storageKey)
    if (pending) {
      if (!matchesPendingWalletRequest(pending, request)) {
        throw new UnresolvedWalletRequestMismatchError(pending)
      }
      persist(storageKey, pending)
      return pending.request
    }

    const idempotencyKey = createKey()
    if (!idempotencyKey) throw new WalletIdempotencyUnavailableError()
    const keyedRequest: UserBatchBalanceActionRequest = {
      ...request,
      idempotency_key: idempotencyKey,
    }
    persist(storageKey, { idempotency_key: idempotencyKey, request: keyedRequest })
    return keyedRequest
  }

  async function sendAndResolve(
    request: UserBatchBalanceActionRequest,
    send: (request: UserBatchBalanceActionRequest) => Promise<UserBatchActionResponse>,
    storageKey: string,
  ): Promise<UserBatchActionResponse> {
    const response = await send(request)
    if (!response.interrupted) clear(storageKey)
    return response
  }

  return {
    getPending() {
      return readPending(getStorageKey())
    },
    async execute(
      request: UserBatchWalletAdjustmentRequest,
      send: (request: UserBatchBalanceActionRequest) => Promise<UserBatchActionResponse>,
    ) {
      const storageKey = getStorageKey()
      const keyedRequest = getOrCreate(storageKey, request)
      if (getStorageKey() !== storageKey) throw new WalletIdempotencyScopeChangedError()
      return sendAndResolve(keyedRequest, send, storageKey)
    },
    async retry(
      send: (request: UserBatchBalanceActionRequest) => Promise<UserBatchActionResponse>,
    ): Promise<UserBatchActionResponse | null> {
      const storageKey = getStorageKey()
      const pending = readPending(storageKey)
      if (!pending) return null
      persist(storageKey, pending)
      if (getStorageKey() !== storageKey) throw new WalletIdempotencyScopeChangedError()
      return sendAndResolve(pending.request, send, storageKey)
    },
  }
}
