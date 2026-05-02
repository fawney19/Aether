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
      },
      storage,
    )

    const state = readPoolManagementViewState({}, storage)

    expect(state).toEqual({
      providerId: 'provider-c',
      search: 'stored only',
      status: 'active',
      page: 2,
      pageSize: 50,
      sortBy: 'last_used_at',
      sortOrder: 'asc',
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
        sortBy: null,
        sortOrder: 'desc',
      }),
    ).toEqual({
      providerId: 'provider-d',
      search: undefined,
      status: undefined,
      page: undefined,
      pageSize: undefined,
      sortBy: undefined,
      sortOrder: undefined,
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
        sortBy: 'last_used_at',
        sortOrder: 'asc',
      }),
    ).toMatchObject({
      sortBy: 'last_used_at',
      sortOrder: 'asc',
    })
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
