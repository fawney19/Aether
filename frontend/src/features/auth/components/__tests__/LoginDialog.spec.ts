import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, nextTick, type App } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'

import LoginDialog from '../LoginDialog.vue'

const authStoreMock = vi.hoisted(() => ({
  loading: false,
  error: '',
  canAccessAdmin: false,
  login: vi.fn(),
}))

const routerPushMock = vi.hoisted(() => vi.fn())
const routeMock = vi.hoisted(() => ({
  query: {},
}))
const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
}))

const authApiMocks = vi.hoisted(() => ({
  getRegistrationSettings: vi.fn(),
  getAuthSettings: vi.fn(),
}))

const oauthApiMocks = vi.hoisted(() => ({
  getProviders: vi.fn(),
}))

vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return {
    ...actual,
    useRoute: () => routeMock,
    useRouter: () => ({
      push: routerPushMock,
    }),
  }
})

vi.mock('@/stores/auth', () => ({
  useAuthStore: () => authStoreMock,
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => toastMocks,
}))

vi.mock('@/composables/useSiteInfo', () => ({
  useSiteInfo: () => ({
    siteName: 'Aether',
  }),
}))

vi.mock('@/config/demo', () => ({
  isDemoMode: () => false,
  DEMO_ACCOUNTS: {
    admin: { email: 'admin@demo.aether.io', password: 'demo123' },
    user: { email: 'user@demo.aether.io', password: 'demo123' },
  },
}))

vi.mock('@/api/auth', () => ({
  authApi: authApiMocks,
}))

vi.mock('@/api/oauth', () => ({
  oauthApi: oauthApiMocks,
}))

vi.mock('@/utils/deviceId', () => ({
  getClientDeviceId: () => 'device-123',
}))

vi.mock('@/utils/url', () => ({
  getApiUrl: (path: string) => path,
}))

vi.mock('@/utils/oauth-icons', () => ({
  getOAuthIcon: () => '',
}))

vi.mock('../RegisterDialog.vue', () => ({
  default: defineComponent({
    name: 'RegisterDialogStub',
    setup() {
      return () => null
    },
  }),
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Dialog: defineComponent({
      name: 'DialogStub',
      props: {
        modelValue: { type: Boolean, default: false },
      },
      emits: ['update:modelValue'],
      setup(props, { slots }) {
        return () => props.modelValue ? h('div', { 'data-testid': 'dialog' }, slots.default?.()) : null
      },
    }),
  }
})

vi.mock('@/components/ui/button.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'ButtonStub',
      props: {
        disabled: { type: Boolean, default: false },
        type: { type: String, default: 'button' },
      },
      setup(props, { attrs, slots }) {
        return () => h('button', {
          ...attrs,
          type: props.type,
          disabled: props.disabled,
        }, slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/label.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'LabelStub',
      setup(_props, { attrs, slots }) {
        return () => h('label', attrs, slots.default?.())
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountLoginDialog() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(LoginDialog, {
    modelValue: true,
    'onUpdate:modelValue': vi.fn(),
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

async function settle() {
  for (let index = 0; index < 4; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function createDuplicatedNavigationFailure(path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path,
        component: defineComponent({ setup: () => () => null }),
      },
    ],
  })

  await router.push(path)
  return router.push(path)
}

beforeEach(() => {
  authStoreMock.loading = false
  authStoreMock.error = ''
  authStoreMock.canAccessAdmin = false
  authStoreMock.login.mockReset()
  routeMock.query = {}
  routerPushMock.mockReset()
  toastMocks.success.mockReset()
  toastMocks.warning.mockReset()
  toastMocks.error.mockReset()
  authApiMocks.getRegistrationSettings.mockResolvedValue({
    enable_registration: false,
    require_email_verification: false,
    email_configured: true,
    password_policy_level: 'weak',
    turnstile_enabled: false,
    turnstile_site_key: null,
  })
  authApiMocks.getAuthSettings.mockResolvedValue({
    local_enabled: true,
    ldap_enabled: false,
    ldap_exclusive: false,
  })
  oauthApiMocks.getProviders.mockResolvedValue([])
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
  sessionStorage.clear()
  localStorage.clear()
})

describe('LoginDialog password manager contract', () => {
  it('exposes standard login form and field autocomplete metadata', async () => {
    const root = mountLoginDialog()
    await settle()

    const form = root.querySelector('form')
    expect(form?.getAttribute('name')).toBe('login')
    expect(form?.getAttribute('action')).toBe('/api/auth/login')
    expect(form?.getAttribute('method')).toBe('post')
    expect(form?.getAttribute('autocomplete')).toBe('on')
    expect(form?.getAttribute('data-form-type')).toBe('login')

    const username = root.querySelector<HTMLInputElement>('input[name="username"]')
    const password = root.querySelector<HTMLInputElement>('input[name="password"]')

    expect(username?.id).toBe('username')
    expect(username?.getAttribute('autocomplete')).toBe('username')
    expect(username?.getAttribute('autocapitalize')).toBe('none')
    expect(username?.getAttribute('spellcheck')).toBe('false')
    expect(password?.id).toBe('password')
    expect(password?.type).toBe('password')
    expect(password?.getAttribute('autocomplete')).toBe('current-password')
  })

  it('submits DOM-filled credentials and awaits router navigation without timer delay', async () => {
    authStoreMock.login.mockResolvedValue(true)
    routerPushMock.mockResolvedValue(undefined)
    sessionStorage.setItem('redirectPath', '/admin/dashboard')
    const root = mountLoginDialog()
    await settle()

    const form = root.querySelector('form')
    const username = root.querySelector<HTMLInputElement>('input[name="username"]')
    const password = root.querySelector<HTMLInputElement>('input[name="password"]')
    expect(form).not.toBeNull()
    expect(username).not.toBeNull()
    expect(password).not.toBeNull()

    username!.value = ' admin@example.com '
    password!.value = 'secret-from-manager'
    form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await settle()

    expect(authStoreMock.login).toHaveBeenCalledWith('admin@example.com', 'secret-from-manager', 'local')
    expect(routerPushMock).toHaveBeenCalledWith('/admin/dashboard')
    expect(sessionStorage.getItem('redirectPath')).toBeNull()
    expect(toastMocks.success).toHaveBeenCalledWith('登录成功，正在跳转...')
  })

  it('treats duplicated router navigation after successful auth as a completed login', async () => {
    authStoreMock.login.mockResolvedValue(true)
    routerPushMock.mockResolvedValue(await createDuplicatedNavigationFailure('/dashboard'))
    const root = mountLoginDialog()
    await settle()

    const form = root.querySelector('form')
    const username = root.querySelector<HTMLInputElement>('input[name="username"]')
    const password = root.querySelector<HTMLInputElement>('input[name="password"]')

    username!.value = 'user@example.com'
    password!.value = 'secret-from-manager'
    form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await settle()

    expect(authStoreMock.login).toHaveBeenCalledWith('user@example.com', 'secret-from-manager', 'local')
    expect(routerPushMock).toHaveBeenCalledWith('/dashboard')
    expect(toastMocks.error).not.toHaveBeenCalled()
    expect(toastMocks.success).toHaveBeenCalledWith('登录成功，正在跳转...')
    expect(root.querySelector('[data-testid="dialog"]')).toBeNull()
  })
})
