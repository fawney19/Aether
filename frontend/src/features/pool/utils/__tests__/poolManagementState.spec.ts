import { beforeEach, describe, expect, it } from 'vitest'

import {
  buildPoolManagementQueryPatch,
  readPoolManagementViewState,
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
      },
      storage,
    )

    expect(state).toEqual({
      providerId: 'provider-b',
      search: 'query search',
      status: 'inactive',
      page: 3,
      pageSize: 100,
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
      }),
    ).toEqual({
      providerId: 'provider-d',
      search: undefined,
      status: undefined,
      page: undefined,
      pageSize: undefined,
    })
  })
})
