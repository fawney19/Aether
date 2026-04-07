export type PoolManagementStatus = 'all' | 'active' | 'cooldown' | 'inactive'

export interface PoolManagementViewState {
  providerId: string | null
  search: string
  status: PoolManagementStatus
  page: number
  pageSize: number
}

export interface PoolManagementStateSource {
  providerId?: string
  search?: string
  status?: string
  page?: string
  pageSize?: string
}

export interface StorageLike {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

export const POOL_MANAGEMENT_VIEW_STORAGE_KEY = 'aether:pool-management:view-state'

export const DEFAULT_POOL_MANAGEMENT_VIEW_STATE: PoolManagementViewState = {
  providerId: null,
  search: '',
  status: 'all',
  page: 1,
  pageSize: 50,
}

function normalizeProviderId(value: unknown): string | null {
  const normalized = String(value ?? '').trim()
  return normalized || null
}

function normalizeSearch(value: unknown): string {
  return String(value ?? '')
}

function normalizeStatus(value: unknown): PoolManagementStatus {
  if (value === 'active' || value === 'cooldown' || value === 'inactive') {
    return value
  }
  return 'all'
}

function normalizePositiveInteger(value: unknown, fallback: number): number {
  const normalized = Number.parseInt(String(value ?? ''), 10)
  if (!Number.isFinite(normalized) || normalized <= 0) {
    return fallback
  }
  return normalized
}

function normalizeViewState(input: Partial<PoolManagementViewState>): PoolManagementViewState {
  return {
    providerId: normalizeProviderId(input.providerId),
    search: normalizeSearch(input.search),
    status: normalizeStatus(input.status),
    page: normalizePositiveInteger(input.page, DEFAULT_POOL_MANAGEMENT_VIEW_STATE.page),
    pageSize: normalizePositiveInteger(input.pageSize, DEFAULT_POOL_MANAGEMENT_VIEW_STATE.pageSize),
  }
}

function readStoredState(storage?: StorageLike): Partial<PoolManagementViewState> {
  if (!storage) return {}

  try {
    const raw = storage.getItem(POOL_MANAGEMENT_VIEW_STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as Partial<PoolManagementViewState> | null
    return parsed && typeof parsed === 'object' ? parsed : {}
  } catch {
    return {}
  }
}

export function readPoolManagementViewState(
  source: PoolManagementStateSource,
  storage?: StorageLike,
): PoolManagementViewState {
  const stored = normalizeViewState(readStoredState(storage))

  return normalizeViewState({
    providerId: source.providerId ?? stored.providerId,
    search: source.search ?? stored.search,
    status: source.status ?? stored.status,
    page: source.page ?? stored.page,
    pageSize: source.pageSize ?? stored.pageSize,
  })
}

export function writePoolManagementViewState(
  state: PoolManagementViewState,
  storage?: StorageLike,
): void {
  if (!storage) return

  try {
    storage.setItem(
      POOL_MANAGEMENT_VIEW_STORAGE_KEY,
      JSON.stringify(normalizeViewState(state)),
    )
  } catch {
    // 忽略存储失败，避免影响主流程。
  }
}

export function buildPoolManagementQueryPatch(
  state: PoolManagementViewState,
): Record<string, string | undefined> {
  const normalized = normalizeViewState(state)
  const search = normalized.search.trim()

  return {
    providerId: normalized.providerId || undefined,
    search: search || undefined,
    status: normalized.status === 'all' ? undefined : normalized.status,
    page: normalized.page <= 1 ? undefined : String(normalized.page),
    pageSize:
      normalized.pageSize === DEFAULT_POOL_MANAGEMENT_VIEW_STATE.pageSize
        ? undefined
        : String(normalized.pageSize),
  }
}
