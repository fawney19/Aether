import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App, type ComputedRef } from 'vue'

import { getI18nLocale, setI18nLocale } from '@/i18n'
import Settings from '../Settings.vue'

const meApiMock = vi.hoisted(() => ({
  getProfile: vi.fn(),
  getPreferences: vi.fn(),
  listSessions: vi.fn(),
  updatePreferences: vi.fn(),
}))
const toastMock = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))

vi.mock('@/api/me', () => ({ meApi: meApiMock }))
vi.mock('@/api/auth', () => ({
  authApi: { getRegistrationSettings: vi.fn().mockResolvedValue({ email_configured: false }) },
}))
vi.mock('@/api/oauth', () => ({ oauthApi: {} }))
vi.mock('@/stores/auth', () => ({
  useAuthStore: () => ({ fetchCurrentUser: vi.fn(), logout: vi.fn() }),
}))
vi.mock('vue-router', () => ({
  useRoute: () => ({ fullPath: '/dashboard/settings' }),
  useRouter: () => ({ replace: vi.fn() }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => toastMock }))
vi.mock('@/composables/useDarkMode', async () => {
  const { ref } = await import('vue')
  return { useDarkMode: () => ({ themeMode: ref('light'), setThemeMode: vi.fn() }) }
})
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn(), warn: vi.fn(), info: vi.fn() } }))

interface SelectContext {
  value: ComputedRef<string | undefined>
  select: (value: string) => void
}

// Keep the Select model/event contract while making its options directly
// accessible without Radix's portal and pointer-event requirements in jsdom.
vi.mock('@/components/ui/select.vue', async () => {
  const { computed, defineComponent, h, provide } = await import('vue')
  return {
    default: defineComponent({
      props: { modelValue: String, open: Boolean },
      emits: ['update:modelValue', 'update:open'],
      setup(props, { emit, slots }) {
        provide<SelectContext>('settings-test-select', {
          value: computed(() => props.modelValue),
          select: value => emit('update:modelValue', value),
        })
        return () => h('div', { 'data-select-value': props.modelValue }, slots.default?.())
      },
    }),
  }
})
vi.mock('@/components/ui/select-trigger.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: (_, { slots }) => () => h('button', { type: 'button' }, slots.default?.()) }) }
})
vi.mock('@/components/ui/select-value.vue', async () => {
  const { defineComponent, h, inject } = await import('vue')
  return {
    default: defineComponent({
      setup() {
        const select = inject<SelectContext>('settings-test-select')
        return () => h('span', select?.value.value)
      },
    }),
  }
})
vi.mock('@/components/ui/select-content.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) }) }
})
vi.mock('@/components/ui/select-item.vue', async () => {
  const { defineComponent, h, inject } = await import('vue')
  return {
    default: defineComponent({
      props: { value: { type: String, required: true } },
      setup(props, { slots }) {
        const select = inject<SelectContext>('settings-test-select')
        return () => h('button', {
          type: 'button',
          role: 'option',
          'data-option-value': props.value,
          'aria-selected': select?.value.value === props.value,
          onClick: () => select?.select(props.value),
        }, slots.default?.())
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function serverPreferences() {
  return {
    theme: 'light',
    language: 'zh-CN',
    timezone: 'Asia/Shanghai',
    notifications: { email: true, usage_alerts: true, announcements: true },
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(complete => { resolve = complete })
  return { promise, resolve }
}

async function flushPromises() {
  await nextTick()
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
}

function mountSettings() {
  const root = document.createElement('div')
  document.body.append(root)
  const app = createApp(Settings)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

function languageSelect(root: HTMLElement): HTMLElement {
  const select = root.querySelector('#language')?.closest<HTMLElement>('[data-select-value]')
  if (!select) throw new Error('The settings language select was not rendered')
  return select
}

function chooseEnglish(root: HTMLElement) {
  const option = languageSelect(root).querySelector<HTMLButtonElement>('[data-option-value="en-US"]')
  if (!option) throw new Error('The English option was not rendered')
  expect(option.textContent?.trim()).toBe('English')
  option.click()
}

beforeEach(() => {
  vi.clearAllMocks()
  setI18nLocale('zh-CN')
  meApiMock.getProfile.mockResolvedValue({
    id: 'settings-user', username: 'User', role: 'user', is_active: true,
    auth_source: 'ldap', feature_settings: {},
  })
  meApiMock.listSessions.mockResolvedValue([])
  meApiMock.getPreferences.mockResolvedValue(serverPreferences())
  meApiMock.updatePreferences.mockResolvedValue(undefined)
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('Settings language preferences', () => {
  it('initializes from the active locale and preserves it when old server preferences arrive', async () => {
    const preferences = deferred<ReturnType<typeof serverPreferences>>()
    meApiMock.getPreferences.mockReturnValueOnce(preferences.promise)
    setI18nLocale('en-US')
    const root = mountSettings()

    expect(languageSelect(root).dataset.selectValue).toBe('en-US')
    preferences.resolve(serverPreferences())
    await flushPromises()

    expect(languageSelect(root).dataset.selectValue).toBe('en-US')
    expect(languageSelect(root).querySelector('[data-option-value="en-US"]')?.getAttribute('aria-selected')).toBe('true')
    expect(getI18nLocale()).toBe('en-US')
    expect(localStorage.getItem('aether_locale')).toBe('en-US')
  })

  it('switches locale and persists en-US immediately on the Select update event', async () => {
    const save = deferred<void>()
    meApiMock.updatePreferences.mockReturnValueOnce(save.promise)
    const root = mountSettings()
    await flushPromises()

    expect(languageSelect(root).dataset.selectValue).toBe('zh-CN')
    chooseEnglish(root)

    expect(getI18nLocale()).toBe('en-US')
    expect(document.documentElement.lang).toBe('en-US')
    expect(localStorage.getItem('aether_locale')).toBe('en-US')
    expect(meApiMock.updatePreferences).toHaveBeenCalledWith(expect.objectContaining({ language: 'en-US' }))
    expect(toastMock.success).not.toHaveBeenCalled()
    await nextTick()
    expect(languageSelect(root).dataset.selectValue).toBe('en-US')

    save.resolve()
    await flushPromises()
    expect(toastMock.success).toHaveBeenCalled()
    expect(toastMock.error).not.toHaveBeenCalled()
  })

  it('does not undo a new selection when the initial preference request completes late', async () => {
    const preferences = deferred<ReturnType<typeof serverPreferences>>()
    meApiMock.getPreferences.mockReturnValueOnce(preferences.promise)
    const root = mountSettings()
    chooseEnglish(root)

    preferences.resolve(serverPreferences())
    await flushPromises()

    expect(getI18nLocale()).toBe('en-US')
    expect(languageSelect(root).dataset.selectValue).toBe('en-US')
    expect(meApiMock.updatePreferences).toHaveBeenCalledTimes(1)
    expect(meApiMock.updatePreferences).toHaveBeenCalledWith(expect.objectContaining({ language: 'en-US' }))
  })

  it('keeps its selected option synchronized with language changes from the top bar', async () => {
    const root = mountSettings()
    await flushPromises()

    setI18nLocale('en-US')
    await nextTick()
    expect(languageSelect(root).dataset.selectValue).toBe('en-US')
    expect(languageSelect(root).querySelector('[data-option-value="en-US"]')?.getAttribute('aria-selected')).toBe('true')

    setI18nLocale('zh-CN')
    await nextTick()
    expect(languageSelect(root).dataset.selectValue).toBe('zh-CN')
    expect(languageSelect(root).querySelector('[data-option-value="zh-CN"]')?.getAttribute('aria-selected')).toBe('true')
    expect(meApiMock.updatePreferences).not.toHaveBeenCalled()
  })
})
