import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import RegisterDialog from '../RegisterDialog.vue'

const authApiMocks = vi.hoisted(() => ({
  sendVerificationCode: vi.fn(),
  getVerificationStatus: vi.fn(),
  verifyEmail: vi.fn(),
  register: vi.fn(),
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

function mountRegisterDialog() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(RegisterDialog, {
    open: true,
    requireEmailVerification: true,
    emailConfigured: true,
    turnstileEnabled: true,
    turnstileSiteKey: 'site-key-123',
    'onUpdate:open': vi.fn(),
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
