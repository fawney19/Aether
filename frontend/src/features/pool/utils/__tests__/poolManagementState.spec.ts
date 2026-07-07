import { beforeEach, describe, expect, it } from 'vitest'

import {
  buildPoolManagementQueryPatch,
  readPoolManagementViewState,
  resolvePoolManagementPageAfterLoad,
  writePoolManagementViewState,
} from '@/features/pool/utils/poolManagementState'

function createMemoryStorage() {
  const store = new Map<string, string>()
  return {
    getItem(key: string) {
      return store.get(key) ?? null
    },
    setItem(key: string, value: string) {
      store.set(key, value)
    },
    removeItem(key: string) {
      store.delete(key)
    },
  }
}

describe('poolManagementState', () => {
  let storage: ReturnType<typeof createMemoryStorage>

  beforeEach(() => {
    storage = createMemoryStorage()
  })

  it('restores provider, filters and paging from query first', () => {
    writePoolManagementViewState(
      {
        providerId: 'provider-a',
        search: 'stored search',
        status: 'cooldown',
        page: 5,
        pageSize: 20,
        sortBy: 'last_used_at',
        sortOrder: 'asc',
        statsMode: 'account_total',
      },
      storage,
    )

    const state = readPoolManagementViewState(
      {
        providerId: 'provider-b',
        search: 'query search',
        status: 'inactive',
        page: '3',
        pageSize: '100',
        sortBy: 'imported_at',
        sortOrder: 'desc',
        statsMode: 'current_cycle',
      },
      storage,
    )

    expect(state).toEqual({
      providerId: 'provider-b',
      search: 'query search',
      status: 'inactive',
      page: 3,
      pageSize: 100,
      sortBy: 'imported_at',
      sortOrder: 'desc',
      statsMode: 'current_cycle',
    })
  })

  it('falls back to storage when query is missing', () => {
    writePoolManagementViewState(
      {
        providerId: 'provider-c',
        search: 'stored only',
        status: 'active',
        page: 2,
        pageSize: 50,
        sortBy: 'last_used_at',
        sortOrder: 'asc',
        statsMode: 'account_total',
      },
      storage,
    )

    const state = readPoolManagementViewState({}, storage)

    expect(state).toEqual({
      providerId: 'provider-c',
      search: 'stored only',
      status: 'available',
      page: 2,
      pageSize: 50,
      sortBy: 'last_used_at',
      sortOrder: 'asc',
      statsMode: 'account_total',
    })
  })

  it('supports score sort in storage and query state', () => {
    writePoolManagementViewState(
      {
        providerId: 'provider-g',
        search: 'score search',
        status: 'cooldown',
        page: 3,
        pageSize: 25,
        sortBy: 'score',
        sortOrder: 'desc',
        statsMode: 'current_cycle',
      },
      storage,
    )

    expect(readPoolManagementViewState({}, storage)).toMatchObject({
      providerId: 'provider-g',
      search: 'score search',
      status: 'cooldown',
      page: 3,
      pageSize: 25,
      sortBy: 'score',
      sortOrder: 'desc',
      statsMode: 'current_cycle',
    })

    expect(
      buildPoolManagementQueryPatch({
        providerId: 'provider-g',
        search: 'score search',
        status: 'cooldown',
        page: 3,
        pageSize: 25,
        sortBy: 'score',
        sortOrder: 'desc',
        statsMode: 'current_cycle',
      }),
    ).toMatchObject({
      sortBy: 'score',
      sortOrder: 'desc',
    })
  })

  it('omits defaults when building query patch', () => {
    expect(
      buildPoolManagementQueryPatch({
        providerId: 'provider-d',
        search: '  ',
        status: 'all',
        page: 1,
        pageSize: 50,
        sortBy: 'imported_at',
        sortOrder: 'desc',
        statsMode: 'current_cycle',
      }),
    ).toEqual({
      providerId: 'provider-d',
      search: undefined,
      status: undefined,
      page: undefined,
      pageSize: undefined,
      sortBy: undefined,
      sortOrder: undefined,
      statsMode: undefined,
    })
  })

  it('keeps sortable column state in query patch', () => {
    expect(
      buildPoolManagementQueryPatch({
        providerId: 'provider-e',
        search: '',
        status: 'all',
        page: 1,
        pageSize: 50,
        sortBy: 'score',
        sortOrder: 'desc',
        statsMode: 'account_total',
      }),
    ).toMatchObject({
      sortBy: 'score',
      sortOrder: 'desc',
      statsMode: 'account_total',
    })
  })

  it('restores stats mode from storage and lets query override it', () => {
    writePoolManagementViewState(
      {
        providerId: 'provider-f',
        search: '',
        status: 'all',
        page: 1,
        pageSize: 50,
        sortBy: null,
        sortOrder: 'desc',
        statsMode: 'account_total',
      },
      storage,
    )

    expect(readPoolManagementViewState({}, storage).statsMode).toBe('account_total')
    expect(
      readPoolManagementViewState({ statsMode: 'current_cycle' }, storage).statsMode,
    ).toBe('current_cycle')
  })

  it('clamps a restored page to the last available page after load', () => {
    expect(
      resolvePoolManagementPageAfterLoad({
        requestedPage: 5,
        pageSize: 50,
        total: 120,
      }),
    ).toBe(3)
  })

  it('resets an out-of-range empty result page back to page 1', () => {
    expect(
      resolvePoolManagementPageAfterLoad({
        requestedPage: 4,
        pageSize: 50,
        total: 0,
      }),
    ).toBe(1)
  })
})
