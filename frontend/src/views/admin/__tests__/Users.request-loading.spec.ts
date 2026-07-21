import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/admin/Users.vue'),
  'utf8',
)

describe('Users request loading', () => {
  it('debounces search without delaying discrete filters', () => {
    const searchWatcher = source
      .split('watch(searchQuery, () => {')[1]
      ?.split('watch([filterRole, filterStatus, filterGroup, sortOption]')[0]
    expect(searchWatcher).toBeTruthy()
    expect(searchWatcher).toContain('setTimeout(')
    expect(searchWatcher).toContain('USERS_SEARCH_DEBOUNCE_MS')

    const discreteFilterWatcher = source
      .split('watch([filterRole, filterStatus, filterGroup, sortOption], () => {')[1]
      ?.split('watch(paginatedUsers')[0]
    expect(discreteFilterWatcher).toBeTruthy()
    expect(discreteFilterWatcher).not.toContain('setTimeout(')
    expect(discreteFilterWatcher).toContain('refreshUsers()')
  })

  it('does not reload invariant metadata for every list refresh', () => {
    const listRefresh = source
      .split('async function refreshUsers(')[1]
      ?.split('async function handleManualRefresh()')[0]
    expect(listRefresh).toBeTruthy()
    expect(listRefresh).not.toContain('loadUserGroups()')
    expect(listRefresh).not.toContain('loadUserWallets(')

    const manualRefresh = source
      .split('async function handleManualRefresh()')[1]
      ?.split('function handleTableSort')[0]
    expect(manualRefresh).toBeTruthy()
    expect(manualRefresh).toContain('refreshUsers()')
    expect(manualRefresh).toContain('loadUserGroups()')
    expect(manualRefresh).toContain('loadUserWallets()')
    expect(source).toContain('@refresh="handleManualRefresh"')
  })

  it('refreshes wallet state after user access-control mutations', () => {
    const batchCompleted = source
      .split('async function handleUserBatchCompleted')[1]
      ?.split('function invalidateUserOptions')[0]
    expect(batchCompleted).toContain('Promise.all([refreshUsers(), loadUserWallets()])')

    const formSubmit = source
      .split('async function handleUserFormSubmit')[1]
      ?.split('async function manageApiKeys')[0]
    expect(formSubmit).toContain('Promise.all([refreshUsers(), loadUserWallets()])')
  })

  it('seeds a new managed key from the selected target user feature settings', () => {
    const openCreateKey = source
      .split('function openCreateUserApiKeyDialog()')[1]
      ?.split('function openEditUserApiKeyDialog')[0]

    expect(openCreateKey).toBeTruthy()
    expect(openCreateKey).toContain('selectedUser.value?.feature_settings')
    expect(openCreateKey).not.toContain('authStore')
  })

  it('rejects a stale API key response after switching users or closing the dialog', () => {
    const manageKeys = source
      .split('async function manageApiKeys(user: User)')[1]
      ?.split('async function manageUserSessions')[0]
    expect(manageKeys).toContain('userApiKeys.value = []')
    expect(manageKeys).toContain('loadUserApiKeys(user.id)')

    const loadKeys = source
      .split('async function loadUserApiKeys(userId: string)')[1]
      ?.split('function openCreateUserApiKeyDialog')[0]
    expect(loadKeys).toContain('const requestId = ++userApiKeysRequestId')
    expect(loadKeys).toContain('requestId !== userApiKeysRequestId')
    expect(loadKeys).toContain('selectedUser.value?.id !== userId')
    expect(loadKeys).toContain('!showApiKeysDialog.value')

    const closeKeys = source
      .split('function closeApiKeysDialog()')[1]
      ?.split('async function manageUserSessions')[0]
    expect(closeKeys).toContain('userApiKeysRequestId += 1')
    expect(closeKeys).toContain('userApiKeys.value = []')
    expect(source).toContain('@close="closeApiKeysDialog"')
  })

  it('keeps an in-flight key mutation bound to its original target user', () => {
    const submitKey = source
      .split('async function submitUserApiKeyForm()')[1]
      ?.split('async function revokeSelectedUserSession')[0]

    expect(submitKey).toContain('const targetUserId = selectedUser.value.id')
    expect(submitKey).toContain('const mutationRequestId = ++userApiKeyMutationRequestId')
    expect(submitKey).toContain('selectedUser.value?.id === targetUserId')
    expect(submitKey).toContain('usersStore.createApiKey(targetUserId')
    expect(submitKey).toContain('usersStore.updateApiKey(targetUserId')
    expect(submitKey).toContain('if (!mutationIsCurrent()) return')
  })
})
