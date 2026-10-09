import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import RegisterDialog from '../RegisterDialog.vue'

const authApiMocks = vi.hoisted(() => ({
  sendVerificationCode: vi.fn(),
  getVerificationStatus: vi.fn(),
  verifyEmail: vi.fn(),
  register: vi.fn(),
  validateInviteCode: vi.fn(),
}))

vi.mock('@/api/auth', () => ({
  authApi: authApiMocks,
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/utils/errorParser', () => ({
  parseApiError: (_error: unknown, fallback: string) => fallback,
}))

vi.mock('../TurnstileWidget.vue', () => ({
  default: defineComponent({
    name: 'TurnstileWidgetStub',
    props: {
      modelValue: { type: String, default: '' },
      siteKey: { type: String, required: true },
    },
    emits: ['update:modelValue'],
    setup(_props, { emit, expose }) {
      expose({ reset: vi.fn() })
      return () =>
        h('button', {
          type: 'button',
          'data-testid': 'turnstile-widget',
          onClick: () => emit('update:modelValue', 'turnstile-token-123'),
        }, 'Turnstile')
    },
  }),
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Dialog: defineComponent({
      name: 'DialogStub',
      props: { open: { type: Boolean, default: false } },
      emits: ['update:open'],
      setup(props, { slots }) {
        return () => props.open
          ? h('div', [slots.default?.(), slots.footer?.()])
          : null
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
      emits: ['click'],
      setup(props, { attrs, emit, slots }) {
        return () => h('button', {
          ...attrs,
          type: props.type,
          disabled: props.disabled,
          onClick: (event: MouseEvent) => emit('click', event),
        }, slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/input.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'InputStub',
      props: {
        modelValue: { type: [String, Number], default: '' },
        disabled: { type: Boolean, default: false },
        type: { type: String, default: 'text' },
        id: { type: String, default: undefined },
      },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('input', {
          ...attrs,
          id: props.id,
          type: props.type,
          disabled: props.disabled,
          value: props.modelValue,
          onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
        })
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

function mountRegisterDialog(props: Record<string, unknown> = {}) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(RegisterDialog, {
    open: true,
    requireEmailVerification: true,
    emailConfigured: true,
    turnstileEnabled: true,
    turnstileSiteKey: 'site-key-123',
    'onUpdate:open': vi.fn(),
    ...props,
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

beforeEach(() => {
  localStorage.clear()
  window.history.replaceState({}, '', '/')
  authApiMocks.validateInviteCode.mockReset()
  authApiMocks.validateInviteCode.mockResolvedValue({ valid: true })
  authApiMocks.sendVerificationCode.mockReset()
  authApiMocks.getVerificationStatus.mockReset()
  authApiMocks.verifyEmail.mockReset()
  authApiMocks.register.mockReset()
  authApiMocks.getVerificationStatus.mockResolvedValue({
    has_pending_code: false,
    is_verified: false,
    cooldown_remaining: null,
    code_expires_in: null,
  })
  authApiMocks.sendVerificationCode.mockResolvedValue({
    success: true,
    message: 'ok',
    expire_minutes: 5,
    verification_token: 'verification-session-token',
  })
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('RegisterDialog Turnstile verification flow', () => {
  it('shows Turnstile before sending code and includes the token in the request', async () => {
    const root = mountRegisterDialog()
    await settle()

    expect(root.textContent).toContain('Turnstile')
    const emailInput = root.querySelector('#reg-email') as HTMLInputElement
    emailInput.value = 'alice@example.com'
    emailInput.dispatchEvent(new Event('input'))
    await settle()

    const buttons = Array.from(root.querySelectorAll('button')) as HTMLButtonElement[]
    const sendButtonBeforeToken = buttons.find((button) => button.type === 'button' && button.disabled && !button.dataset.testid)
    expect(sendButtonBeforeToken).toBeDefined()
    expect(sendButtonBeforeToken?.disabled).toBe(true)

    const turnstileButton = root.querySelector('[data-testid="turnstile-widget"]') as HTMLButtonElement
    turnstileButton.click()
    await settle()

    const sendButton = buttons.find((button) => button.disabled === false && button.type === 'button' && button !== turnstileButton) as HTMLButtonElement
    expect(sendButton).toBeDefined()
    expect(sendButton.disabled).toBe(false)
    sendButton.click()
    await settle()

    expect(authApiMocks.sendVerificationCode).toHaveBeenCalledWith(
      'alice@example.com',
      'turnstile-token-123'
    )
  })
})

async function fillRegistration(root: HTMLElement) {
  for (const [selector, value] of [['#reg-uname', 'new-user'], ['input[autocomplete="new-password"]', 'securePass123']]) {
    const fields = root.querySelectorAll<HTMLInputElement>(selector)
    for (const input of fields) {
      input.value = value
      input.dispatchEvent(new Event('input'))
    }
  }
  await settle()
}
function submit(root: HTMLElement) { root.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })) }

async function enterInviteCode(root: HTMLElement, value: string) {
  const input = root.querySelector<HTMLInputElement>('#reg-invite-code')!
  input.value = value
  input.dispatchEvent(new Event('input'))
  await settle()
}

describe('RegisterDialog invitation registration', () => {
  it('always shows an editable optional invite field and allows direct registration without a code', async () => {
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    const input = root.querySelector<HTMLInputElement>('#reg-invite-code')!
    expect(input.value).toBe('')
    expect(input.readOnly).toBe(false)
    expect(input.required).toBe(false)
    expect(root.querySelector('label[for="reg-invite-code"]')?.textContent).toContain('邀请码')
    expect(root.textContent).not.toContain('清除邀请码')
    await fillRegistration(root)
    submit(root)
    await settle()
    expect(authApiMocks.validateInviteCode).not.toHaveBeenCalled()
    expect(authApiMocks.register).toHaveBeenCalledWith({ username: 'new-user', password: 'securePass123' })
  })

  it('validates and submits a manually entered normalized invite code', async () => {
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    await enterInviteCode(root, ' valid123 ')
    const input = root.querySelector<HTMLInputElement>('#reg-invite-code')!
    expect(input.value).toBe('VALID123')
    expect(input.readOnly).toBe(false)
    input.dispatchEvent(new Event('blur'))
    await settle()
    await fillRegistration(root)
    submit(root)
    await settle()
    expect(authApiMocks.validateInviteCode).toHaveBeenCalledTimes(2)
    expect(authApiMocks.validateInviteCode).toHaveBeenLastCalledWith('VALID123')
    expect(authApiMocks.register).toHaveBeenCalledWith(expect.objectContaining({ invite_code: 'VALID123' }))
  })

  it('allows direct registrants to correct an invalid manual code before submitting', async () => {
    authApiMocks.validateInviteCode.mockResolvedValue({ valid: false, reason: '邀请码无效' })
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    await fillRegistration(root)
    await enterInviteCode(root, 'INVALID')
    submit(root)
    await settle()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('邀请码无效')
    expect(authApiMocks.register).not.toHaveBeenCalled()
    authApiMocks.validateInviteCode.mockResolvedValue({ valid: true })
    await enterInviteCode(root, 'valid123')
    expect(root.querySelector('[role="alert"]')).toBeNull()
    submit(root)
    await settle()
    expect(authApiMocks.register).toHaveBeenCalledWith(expect.objectContaining({ invite_code: 'VALID123' }))
  })

  it('keeps an invalid cached invitation read-only and prevents registration without a valid invitation', async () => {
    localStorage.setItem('aether_invite_code', 'STALE')
    authApiMocks.validateInviteCode.mockResolvedValue({ valid: false, reason: '邀请人已停用' })
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    const input = root.querySelector<HTMLInputElement>('#reg-invite-code')!
    expect(input.value).toBe('STALE')
    expect(input.readOnly).toBe(true)
    expect(root.textContent).toContain('邀请人已停用')
    expect(root.textContent).not.toContain('清除邀请码')
    await enterInviteCode(root, 'OTHER')
    await fillRegistration(root)
    submit(root)
    await settle()
    expect(authApiMocks.register).not.toHaveBeenCalled()
    expect(authApiMocks.validateInviteCode).toHaveBeenLastCalledWith('STALE')
    expect(localStorage.getItem('aether_invite_code')).toBe('STALE')
  })

  it('revalidates before registration and removes both query and cached invitation after success', async () => {
    window.history.replaceState({}, '', '/register?invite=valid123&other=keep')
    localStorage.setItem('aether_invite_code', 'OLDER')
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    const input = root.querySelector<HTMLInputElement>('#reg-invite-code')!
    expect(input.value).toBe('VALID123')
    expect(input.readOnly).toBe(true)
    await enterInviteCode(root, 'REPLACEMENT')
    await fillRegistration(root)
    submit(root)
    await settle()
    expect(authApiMocks.validateInviteCode).toHaveBeenCalledTimes(2)
    expect(authApiMocks.validateInviteCode).toHaveBeenLastCalledWith('VALID123')
    expect(authApiMocks.register).toHaveBeenCalledWith(expect.objectContaining({ invite_code: 'VALID123' }))
    expect(localStorage.getItem('aether_invite_code')).toBeNull()
    expect(window.location.search).toBe('?other=keep')
  })

  it('ignores outdated validation results after a manual code is changed', async () => {
    let resolveValidation!: (value: { valid: boolean; reason: string }) => void
    authApiMocks.validateInviteCode.mockImplementationOnce(() => new Promise(resolve => { resolveValidation = resolve }))
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    await enterInviteCode(root, 'OLD')
    root.querySelector<HTMLInputElement>('#reg-invite-code')!.dispatchEvent(new Event('blur'))
    await settle()
    await enterInviteCode(root, 'NEW')
    resolveValidation({ valid: false, reason: '旧邀请码无效' })
    await settle()
    expect(root.querySelector<HTMLInputElement>('#reg-invite-code')!.value).toBe('NEW')
    expect(root.querySelector('[role="alert"]')).toBeNull()
  })

  it('shows verification unavailability without treating the code as invalid or silently discarding it', async () => {
    localStorage.setItem('aether_invite_code', 'VALID123')
    authApiMocks.validateInviteCode.mockRejectedValue(new Error('offline'))
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false })
    await settle()
    await fillRegistration(root)
    submit(root)
    await settle()
    expect(root.textContent).toContain('暂时无法核实邀请码')
    expect(localStorage.getItem('aether_invite_code')).toBe('VALID123')
    expect(authApiMocks.register).not.toHaveBeenCalled()
  })
  it('keeps successful registration successful when browser storage cleanup is unavailable', async () => {
    window.history.replaceState({}, '', '/register?invite=VALID123')
    authApiMocks.register.mockResolvedValue({ message: '注册成功' })
    const registered = vi.fn()
    const root = mountRegisterDialog({ requireEmailVerification: false, turnstileEnabled: false, onSuccess: registered })
    await settle()
    await fillRegistration(root)
    vi.spyOn(localStorage, 'removeItem').mockImplementationOnce(() => { throw new Error('storage disabled') })
    submit(root)
    await settle()
    expect(registered).toHaveBeenCalledOnce()
    expect(window.location.search).toBe('')
  })

})
