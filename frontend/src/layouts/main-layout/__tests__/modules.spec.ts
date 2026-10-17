import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, ref, type App, type Ref } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import { useModuleStore } from '@/stores/modules'
import { buildNavigation } from '@/layouts/main-layout/navigation'
import { useNavigationModules } from '@/layouts/main-layout/modules'

const { getAllStatus, getUserStatus } = vi.hoisted(() => ({
  getAllStatus: vi.fn(),
  getUserStatus: vi.fn(),
}))

vi.mock('@/api/modules', () => ({ modulesApi: { getAllStatus, getUserStatus } }))

describe('user referral navigation module loading', () => {
  let app: App | undefined
  let container: HTMLDivElement

  beforeEach(() => {
    vi.resetAllMocks()
    setActivePinia(createPinia())
    container = document.createElement('div')
    document.body.append(container)
  })

  afterEach(() => {
    app?.unmount()
    app = undefined
    container.remove()
  })

  function mount(canAccessAdmin: boolean | Ref<boolean>) {
    app = createApp({
      setup() {
        useNavigationModules(() => typeof canAccessAdmin === 'boolean' ? canAccessAdmin : canAccessAdmin.value)
        const store = useModuleStore()
        return () => h('nav', buildNavigation({
          canAccessAdmin: false,
          modules: store.modules,
          isModuleActive: store.isUserActive,
        }).flatMap(group => group.items).map(item => h('a', { href: item.href }, item.name)))
      },
    })
    app.mount(container)
  }

  async function settle() {
    await vi.waitFor(() => expect(useModuleStore().userLoading).toBe(false))
    await nextTick()
  }

  it.each([true, false])('shows the referral entry only when the user status is active=%s', async active => {
    getUserStatus.mockResolvedValue({ referral: { name: 'referral', available: true, enabled: active, active } })
    mount(false)
    await settle()

    expect(!!container.querySelector('a[href="/dashboard/referral"]')).toBe(active)
    expect(getUserStatus).toHaveBeenCalledTimes(1)
    expect(getAllStatus).not.toHaveBeenCalled()
  })

  it('keeps the referral entry hidden when status loading fails', async () => {
    getUserStatus.mockRejectedValue(new Error('unavailable'))
    mount(false)
    await settle()
    expect(container.querySelector('a[href="/dashboard/referral"]')).toBeNull()
    expect(useModuleStore().userLoaded).toBe(false)
  })

  it('refreshes user status on remount after the administrator disables referrals', async () => {
    getUserStatus.mockResolvedValueOnce({ referral: { name: 'referral', available: true, enabled: true, active: true } })
    mount(false)
    await settle()
    expect(container.querySelector('a[href="/dashboard/referral"]')).not.toBeNull()
    app?.unmount()

    getUserStatus.mockResolvedValueOnce({ referral: { name: 'referral', available: true, enabled: false, active: false } })
    mount(false)
    await settle()
    expect(getUserStatus).toHaveBeenCalledTimes(2)
    expect(container.querySelector('a[href="/dashboard/referral"]')).toBeNull()
  })

  it('continues to load administrator status for the administrator layout', async () => {
    getAllStatus.mockResolvedValue({})
    mount(true)
    await vi.waitFor(() => expect(useModuleStore().loaded).toBe(true))
    expect(getAllStatus).toHaveBeenCalledTimes(1)
    expect(getUserStatus).not.toHaveBeenCalled()
  })

  it('loads the correct status when the role changes within the mounted layout', async () => {
    getUserStatus.mockResolvedValue({ referral: { name: 'referral', available: true, enabled: true, active: true } })
    getAllStatus.mockResolvedValue({})
    const canAccessAdmin = ref(true)
    mount(canAccessAdmin)
    await vi.waitFor(() => expect(useModuleStore().loaded).toBe(true))
    expect(getUserStatus).not.toHaveBeenCalled()

    canAccessAdmin.value = false
    await nextTick()
    await settle()
    expect(container.querySelector('a[href="/dashboard/referral"]')).not.toBeNull()

    canAccessAdmin.value = true
    await nextTick()
    expect(getAllStatus).toHaveBeenCalledTimes(1)
  })

  it('loads administrator status after a user becomes an administrator without remounting', async () => {
    getUserStatus.mockResolvedValue({})
    getAllStatus.mockResolvedValue({})
    const canAccessAdmin = ref(false)
    mount(canAccessAdmin)
    await settle()
    expect(getAllStatus).not.toHaveBeenCalled()
    canAccessAdmin.value = true
    await nextTick()
    await vi.waitFor(() => expect(useModuleStore().loaded).toBe(true))
    expect(getAllStatus).toHaveBeenCalledTimes(1)
  })

  it('shares an in-flight public status request between callers', async () => {
    let resolve!: (statuses: Record<string, never>) => void
    getUserStatus.mockImplementation(() => new Promise(done => { resolve = done }))
    const store = useModuleStore()
    const first = store.fetchUserModules()
    const second = store.fetchUserModules()
    expect(getUserStatus).toHaveBeenCalledTimes(1)
    resolve({})
    await Promise.all([first, second])
    expect(store.userLoaded).toBe(true)
  })

  it('clears a previously active entry on failure and can recover on retry', async () => {
    getUserStatus.mockResolvedValueOnce({ referral: { name: 'referral', available: true, enabled: true, active: true } })
    const store = useModuleStore()
    await store.fetchUserModules()
    expect(store.isUserActive('referral')).toBe(true)
    getUserStatus.mockRejectedValueOnce(new Error('unavailable'))
    await expect(store.fetchUserModules()).rejects.toThrow('unavailable')
    expect(store.isUserActive('referral')).toBe(false)
    expect(store.userLoaded).toBe(false)
    getUserStatus.mockResolvedValueOnce({ referral: { name: 'referral', available: true, enabled: true, active: true } })
    await store.fetchUserModules()
    expect(store.isUserActive('referral')).toBe(true)
  })
})
