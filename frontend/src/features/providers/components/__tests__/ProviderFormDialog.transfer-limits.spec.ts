import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'

import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import ProviderFormDialog from '../ProviderFormDialog.vue'

const endpointMocks = vi.hoisted(() => ({
  createProvider: vi.fn(),
  updateProvider: vi.fn(),
}))

vi.mock('@/api/endpoints', () => ({
  createProvider: endpointMocks.createProvider,
  updateProvider: endpointMocks.updateProvider,
  normalizePoolAdvancedConfig: (value: unknown) => {
    if (value == null || value === false) return null
    if (value === true) return {}
    if (typeof value !== 'object' || Array.isArray(value)) return null
    return { ...value }
  },
}))

vi.mock('@/components/ui', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/components/ui')>()
  const { defineComponent, h } = await import('vue')
  const passthrough = (name: string) => defineComponent({
    name,
    setup: (_props, { slots }) => () => slots.default?.(),
  })

  return {
    ...actual,
    Select: defineComponent({
      name: 'SelectStub',
      props: {
        modelValue: String,
        disabled: Boolean,
      },
      emits: ['update:modelValue'],
      setup: (props, { emit, slots }) => () => h('select', {
        value: props.modelValue,
        disabled: props.disabled,
        onChange: (event: Event) => emit(
          'update:modelValue',
          (event.target as HTMLSelectElement).value,
        ),
      }, slots.default?.()),
    }),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: defineComponent({
      name: 'SelectItemStub',
      props: {
        value: { type: String, required: true },
        disabled: Boolean,
      },
      setup: (props, { slots }) => () => h('option', {
        value: props.value,
        disabled: props.disabled,
      }, slots.default?.()),
    }),
  }
})

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function makeProvider(
  overrides: Partial<ProviderWithEndpointsSummary> = {},
): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Provider One',
    provider_type: 'custom',
    provider_priority: 100,
    keep_priority_on_conversion: false,
    enable_format_conversion: true,
    max_transfer_count: 0,
    max_transfer_timeout_seconds: 0,
    is_active: true,
    total_endpoints: 0,
    active_endpoints: 0,
    total_keys: 0,
    active_keys: 0,
    total_models: 0,
    active_models: 0,
    global_model_ids: [],
    avg_health_score: 1,
    unhealthy_endpoints: 0,
    api_formats: [],
    endpoint_health_details: [],
    ops_configured: false,
    created_at: '2026-07-26T00:00:00Z',
    updated_at: '2026-07-26T00:00:00Z',
    ...overrides,
  }
}

function mountDialog(provider?: ProviderWithEndpointsSummary | null) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(ProviderFormDialog, {
    modelValue: true,
    provider,
    'onUpdate:modelValue': vi.fn(),
  })
  app.mount(root)
  mountedApps.push({ app, root })
}

async function settle() {
  for (let index = 0; index < 4; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function setInput(selector: string, value: string) {
  const input = document.body.querySelector<HTMLInputElement>(selector)
  if (!input) throw new Error(`Missing input: ${selector}`)
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

function clickButton(text: string) {
  const button = [...document.body.querySelectorAll<HTMLButtonElement>('button')]
    .find(candidate => candidate.textContent?.trim() === text)
  if (!button) throw new Error(`Missing button: ${text}`)
  button.click()
}

const billingFieldNames = [
  'billing_type',
  'monthly_quota_usd',
  'quota_reset_day',
  'quota_last_reset_at',
  'quota_expires_at',
] as const

function expectBillingConfigurationHidden() {
  for (const text of [
    '计费类型',
    '月卡额度',
    '按量付费',
    '免费套餐',
    '周期额度 (USD)',
    '重置周期 (天)',
    '周期开始时间',
    '过期时间',
  ]) {
    expect(document.body.textContent).not.toContain(text)
  }
}

function expectBillingFieldsOmitted(payload: unknown) {
  expect(payload).toEqual(expect.any(Object))
  for (const field of billingFieldNames) {
    expect(payload).not.toHaveProperty(field)
  }
}

beforeEach(() => {
  endpointMocks.createProvider.mockReset()
  endpointMocks.createProvider.mockResolvedValue({ id: 'provider-new', name: 'New Provider' })
  endpointMocks.updateProvider.mockReset()
  endpointMocks.updateProvider.mockResolvedValue(makeProvider())
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('ProviderFormDialog transfer limits', () => {
  it('loads and submits configured limits in edit mode', async () => {
    mountDialog(makeProvider({
      max_transfer_count: 10,
      max_transfer_timeout_seconds: 60,
    }))
    await settle()

    expect(document.body.querySelector<HTMLInputElement>('#max-transfer-count')?.value).toBe('10')
    expect(document.body.querySelector<HTMLInputElement>('#max-transfer-timeout-seconds')?.value).toBe('60')

    await setInput('#max-transfer-count', '12')
    await setInput('#max-transfer-timeout-seconds', '45')
    clickButton('保存')
    await settle()

    expect(endpointMocks.updateProvider).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        max_transfer_count: 12,
        max_transfer_timeout_seconds: 45,
      }),
    )
  })

  it('shows zero limits as unlimited placeholders while submitting explicit zero', async () => {
    mountDialog(makeProvider({
      max_transfer_count: undefined,
      max_transfer_timeout_seconds: undefined,
    }))
    await settle()

    const countInput = document.body.querySelector<HTMLInputElement>('#max-transfer-count')
    const timeoutInput = document.body.querySelector<HTMLInputElement>('#max-transfer-timeout-seconds')

    expect(countInput?.value).toBe('')
    expect(countInput?.placeholder).toBe('0 (不限制)')
    expect(timeoutInput?.value).toBe('')
    expect(timeoutInput?.placeholder).toBe('0 (不限制)')

    clickButton('保存')
    await settle()

    expect(endpointMocks.updateProvider).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        max_transfer_count: 0,
        max_transfer_timeout_seconds: 0,
      }),
    )
  })

  it('allows configuring transfer limits when creating', async () => {
    mountDialog(null)
    await settle()

    expect(document.body.querySelector<HTMLInputElement>('#max-transfer-count')?.value).toBe('')
    expect(document.body.querySelector<HTMLInputElement>('#max-transfer-timeout-seconds')?.value).toBe('')

    await setInput('#name', 'New Provider')
    await setInput('#max-transfer-count', '8')
    await setInput('#max-transfer-timeout-seconds', '30')
    clickButton('创建')
    await settle()

    expect(endpointMocks.createProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        max_transfer_count: 8,
        max_transfer_timeout_seconds: 30,
      }),
    )
  })
})

describe('ProviderFormDialog billing configuration', () => {
  it('hides billing configuration and omits billing fields when creating', async () => {
    mountDialog(null)
    await settle()

    expectBillingConfigurationHidden()

    await setInput('#name', 'New Provider')
    clickButton('创建')
    await settle()

    expect(endpointMocks.createProvider).toHaveBeenCalledTimes(1)
    expectBillingFieldsOmitted(endpointMocks.createProvider.mock.calls[0]?.[0])
  })

  it('hides existing monthly quota configuration and preserves it when editing', async () => {
    mountDialog(makeProvider({
      billing_type: 'monthly_quota',
      monthly_quota_usd: 200,
      quota_reset_day: 30,
      quota_last_reset_at: '2026-08-01T00:00:00Z',
      quota_expires_at: '2026-09-01T00:00:00Z',
    }))
    await settle()

    expectBillingConfigurationHidden()

    clickButton('保存')
    await settle()

    expect(endpointMocks.updateProvider).toHaveBeenCalledTimes(1)
    expectBillingFieldsOmitted(endpointMocks.updateProvider.mock.calls[0]?.[1])
  })
})

describe('ProviderFormDialog provider types', () => {
  it('creates an experimental Claude Code provider from the add dialog', async () => {
    mountDialog(null)
    await settle()

    const providerTypeSelect = [...document.body.querySelectorAll<HTMLSelectElement>('select')]
      .find(select => select.querySelector('option[value="claude_code"]'))
    const claudeCodeOption = providerTypeSelect?.querySelector<HTMLOptionElement>(
      'option[value="claude_code"]',
    )

    expect(claudeCodeOption?.disabled).toBe(false)
    expect(claudeCodeOption?.textContent?.trim()).toBe('Claude Code（实验性功能）')

    await setInput('#name', 'Claude Code Provider')
    if (!providerTypeSelect) throw new Error('Missing provider type select')
    providerTypeSelect.value = 'claude_code'
    providerTypeSelect.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()

    clickButton('创建')
    await settle()

    expect(endpointMocks.createProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'Claude Code Provider',
        provider_type: 'claude_code',
      }),
    )
  })

  it('shows fingerprint convergence only for Codex and submits the enabled setting', async () => {
    mountDialog(null)
    await settle()

    expect(document.body.querySelector('[data-testid="codex-fingerprint-convergence-setting"]')).toBeNull()

    const providerTypeSelect = [...document.body.querySelectorAll<HTMLSelectElement>('select')]
      .find(select => select.querySelector('option[value="codex"]'))
    if (!providerTypeSelect) throw new Error('Missing provider type select')

    providerTypeSelect.value = 'codex'
    providerTypeSelect.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()

    const convergenceSwitch = document.body.querySelector<HTMLButtonElement>(
      '#codex-fingerprint-convergence',
    )
    expect(convergenceSwitch).not.toBeNull()
    expect(document.body.textContent).toContain('Codex 指纹收敛')
    expect(convergenceSwitch?.getAttribute('aria-checked')).toBe('false')

    convergenceSwitch?.click()
    await nextTick()
    expect(convergenceSwitch?.getAttribute('aria-checked')).toBe('true')

    await setInput('#name', 'Codex Provider')
    clickButton('创建')
    await settle()

    expect(endpointMocks.createProvider).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'Codex Provider',
        provider_type: 'codex',
        codex_fingerprint_convergence_enabled: true,
      }),
    )
  })

  it('loads and can disable fingerprint convergence for an existing Codex provider', async () => {
    mountDialog(makeProvider({
      provider_type: 'codex',
      codex_fingerprint_convergence_enabled: true,
    }))
    await settle()

    const convergenceSwitch = document.body.querySelector<HTMLButtonElement>(
      '#codex-fingerprint-convergence',
    )
    expect(convergenceSwitch?.getAttribute('aria-checked')).toBe('true')

    convergenceSwitch?.click()
    clickButton('保存')
    await settle()

    expect(endpointMocks.updateProvider).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        provider_type: 'codex',
        codex_fingerprint_convergence_enabled: false,
      }),
    )
  })

  it('clears and does not submit the setting after switching away from Codex', async () => {
    mountDialog(null)
    await settle()

    const providerTypeSelect = [...document.body.querySelectorAll<HTMLSelectElement>('select')]
      .find(select => select.querySelector('option[value="codex"]'))
    if (!providerTypeSelect) throw new Error('Missing provider type select')

    providerTypeSelect.value = 'codex'
    providerTypeSelect.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()
    document.body.querySelector<HTMLButtonElement>('#codex-fingerprint-convergence')?.click()
    await nextTick()

    providerTypeSelect.value = 'custom'
    providerTypeSelect.dispatchEvent(new Event('change', { bubbles: true }))
    await nextTick()
    expect(document.body.querySelector('[data-testid="codex-fingerprint-convergence-setting"]')).toBeNull()

    await setInput('#name', 'Custom Provider')
    clickButton('创建')
    await settle()

    const payload = endpointMocks.createProvider.mock.calls[0]?.[0]
    expect(payload).not.toHaveProperty('codex_fingerprint_convergence_enabled')
  })
})
