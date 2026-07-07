import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick } from 'vue'
import RegisterDialog from '../RegisterDialog.vue'

const { registerMock, toastErrorMock, toastSuccessMock } = vi.hoisted(() => ({
  registerMock: vi.fn(),
  toastErrorMock: vi.fn(),
  toastSuccessMock: vi.fn(),
}))

vi.mock('@/api/auth', () => ({
  authApi: {
    register: registerMock,
    sendVerificationCode: vi.fn(),
    verifyEmail: vi.fn(),
    getVerificationStatus: vi.fn(),
  },
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: toastSuccessMock,
    error: toastErrorMock,
  }),
}))

type TurnstileRenderOptions = {
  action?: string
  execution?: string
  callback?: (token: string) => void
  'error-callback'?: () => void
}

type TurnstileMock = {
  render: ReturnType<typeof vi.fn>
  execute: ReturnType<typeof vi.fn>
  reset: ReturnType<typeof vi.fn>
  remove: ReturnType<typeof vi.fn>
}

function flushPromises() {
  return new Promise((resolve) => window.setTimeout(resolve, 0))
}

function installTurnstileMock(): TurnstileMock & {
  succeed: (token?: string) => void
  fail: () => void
  lastOptions: () => TurnstileRenderOptions | null
} {
  let renderOptions: TurnstileRenderOptions | null = null
  const turnstile = {
    render: vi.fn((_container: HTMLElement, options: TurnstileRenderOptions) => {
      renderOptions = options
      return 'widget-id'
    }),
    execute: vi.fn(),
    reset: vi.fn(),
    remove: vi.fn(),
    succeed: (token = 'turnstile-token') => {
      renderOptions?.callback?.(token)
    },
    fail: () => {
      renderOptions?.['error-callback']?.()
    },
    lastOptions: () => renderOptions,
  }
  ;(window as unknown as { turnstile: TurnstileMock }).turnstile = turnstile
  return turnstile
}

async function settle() {
  for (let index = 0; index < 4; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function mountRegisterDialog() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(RegisterDialog, {
    open: true,
    emailConfigured: false,
    requireEmailVerification: false,
    passwordPolicyLevel: 'weak',
    turnstileEnabled: true,
    turnstileSiteKey: 'site-public-key',
  })
  app.mount(root)
  await nextTick()
  return {
    app,
    root,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

async function fillRegistrationForm() {
  const inputs = Array.from(document.body.querySelectorAll('input'))
  const usernameInput = inputs.find((input) => input.placeholder === '请输入用户名')
  const passwordInput = inputs.find((input) => input.placeholder.includes('至少'))
  const confirmInput = inputs.find((input) => input.placeholder === '再次输入密码')

  for (const [input, value] of [
    [usernameInput, 'alice'],
    [passwordInput, 'secret123'],
    [confirmInput, 'secret123'],
  ] as const) {
    expect(input).toBeTruthy()
    input!.value = value
    input!.dispatchEvent(new Event('input', { bubbles: true }))
  }
  await nextTick()
}

async function clickRegister() {
  const registerButton = Array.from(document.body.querySelectorAll('button')).find(
    (button) => button.textContent?.trim() === '注册'
  )
  expect(registerButton).toBeTruthy()
  expect(registerButton!.hasAttribute('disabled')).toBe(false)
  registerButton!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  await nextTick()
  await flushPromises()
  await flushPromises()
  await nextTick()
}

describe('RegisterDialog Turnstile flow', () => {
  let mounted: Awaited<ReturnType<typeof mountRegisterDialog>> | null = null

  beforeEach(() => {
    registerMock.mockReset()
    registerMock.mockResolvedValue({ message: '注册成功' })
    toastErrorMock.mockReset()
    toastSuccessMock.mockReset()
  })

  afterEach(() => {
    mounted?.unmount()
    mounted = null
    document.body.innerHTML = ''
    delete (window as unknown as { turnstile?: TurnstileMock }).turnstile
    delete (window as unknown as { __aetherTurnstileScriptPromise?: Promise<void> })
      .__aetherTurnstileScriptPromise
  })

  it('gets a Turnstile token before submitting registration', async () => {
    const turnstile = installTurnstileMock()
    mounted = await mountRegisterDialog()
    await fillRegistrationForm()
    await settle()

    expect(turnstile.render).toHaveBeenCalledWith(
      expect.any(HTMLElement),
      expect.objectContaining({
        sitekey: 'site-public-key',
        action: 'register',
      })
    )
    expect(turnstile.lastOptions()?.execution).toBeUndefined()
    turnstile.succeed('turnstile-token')
    await settle()

    await clickRegister()

    expect(turnstile.execute).not.toHaveBeenCalled()
    expect(registerMock).toHaveBeenCalledWith({
      username: 'alice',
      password: 'secret123',
      turnstile_token: 'turnstile-token',
    })
    expect(turnstile.reset).toHaveBeenCalledWith('widget-id')
  })

  it('resets Turnstile and blocks registration when verification fails', async () => {
    const turnstile = installTurnstileMock()
    mounted = await mountRegisterDialog()
    await fillRegistrationForm()
    await settle()

    turnstile.fail()
    await settle()

    expect(registerMock).not.toHaveBeenCalled()
    expect(toastErrorMock).toHaveBeenCalledWith('人机验证加载失败，请重试', '人机验证')
  })
})
