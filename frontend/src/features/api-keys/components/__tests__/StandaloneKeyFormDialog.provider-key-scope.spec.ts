import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, type App } from 'vue'

import type { EndpointAPIKey } from '@/api/endpoints/keys'
import type { StandaloneKeyFormData } from '../StandaloneKeyFormDialog.vue'

const apiMocks = vi.hoisted(() => ({
  getProvidersSummary: vi.fn(),
  getGlobalModels: vi.fn(),
  getProviderKeys: vi.fn(),
  getApiFormats: vi.fn(),
  logError: vi.fn(),
}))

vi.mock('@/api/endpoints/providers', () => ({
  getProvidersSummary: apiMocks.getProvidersSummary,
}))
vi.mock('@/api/global-models', () => ({
  getGlobalModels: apiMocks.getGlobalModels,
}))
vi.mock('@/api/endpoints/keys', () => ({
  getProviderKeys: apiMocks.getProviderKeys,
}))
vi.mock('@/api/admin', () => ({
  adminApi: {
    getApiFormats: apiMocks.getApiFormats,
  },
}))
vi.mock('@/utils/logger', () => ({
  log: {
    error: apiMocks.logError,
  },
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent: define, h: createElement } = await import('vue')
  const passthrough = (name: string, tag = 'div') => define({
    name,
    inheritAttrs: false,
    props: {
      modelValue: { type: null, default: undefined },
      open: { type: Boolean, default: false },
    },
    setup(props, { attrs, slots }) {
      return () => createElement(tag, attrs, slots.default?.())
    },
  })

  return {
    Dialog: define({
      name: 'DialogStub',
      inheritAttrs: false,
      props: { modelValue: { type: Boolean, default: false } },
      setup(props, { attrs, slots }) {
        return () => props.modelValue
          ? createElement('section', attrs, [slots.default?.(), slots.footer?.()])
          : null
      },
    }),
    Collapsible: passthrough('CollapsibleStub'),
    CollapsibleContent: passthrough('CollapsibleContentStub'),
    CollapsibleTrigger: passthrough('CollapsibleTriggerStub'),
    Button: define({
      name: 'ButtonStub',
      inheritAttrs: false,
      props: {
        disabled: { type: Boolean, default: false },
        type: { type: String, default: 'button' },
      },
      setup(props, { attrs, slots }) {
        return () => createElement('button', {
          ...attrs,
          disabled: props.disabled,
          type: props.type,
        }, slots.default?.())
      },
    }),
    Input: passthrough('InputStub', 'input'),
    Label: passthrough('LabelStub', 'label'),
    Switch: passthrough('SwitchStub', 'button'),
  }
})

vi.mock('@/components/common', async () => {
  const { defineComponent: define, h: createElement } = await import('vue')
  return {
    MultiSelect: define({
      name: 'MultiSelectStub',
      inheritAttrs: false,
      props: {
        modelValue: { type: Array, default: () => [] },
      },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit, slots }) {
        return () => createElement('button', {
          ...attrs,
          type: 'button',
          'data-testid': 'provider-selector',
          onClick: () => emit(
            'update:modelValue',
            props.modelValue.length > 0 ? [] : ['provider-1'],
          ),
        }, slots.default?.())
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent: define, h: createElement } = await import('vue')
  const icon = (name: string) => define({
    name,
    setup: () => () => createElement('span'),
  })
  return {
    ChevronDown: icon('ChevronDownStub'),
    Plus: icon('PlusStub'),
    SquarePen: icon('SquarePenStub'),
    X: icon('XStub'),
  }
})

import StandaloneKeyFormDialog from '../StandaloneKeyFormDialog.vue'

interface MountedDialog {
  app: App
  root: HTMLElement
  state: { open: boolean }
  submitted: StandaloneKeyFormData[]
}

const mountedDialogs: MountedDialog[] = []

const provider = {
  id: 'provider-1',
  name: 'Provider 1',
}

const disabledSelectedKey: EndpointAPIKey = {
  id: 'key-disabled',
  provider_id: 'provider-1',
  api_formats: ['openai:chat'],
  api_key_masked: 'sk-disabled',
  auth_type: 'api_key',
  name: 'Disabled selected key',
  internal_priority: 0,
  allowed_models: null,
  cache_ttl_minutes: 0,
  max_probe_interval_minutes: 5,
  health_score: 1,
  consecutive_failures: 0,
  request_count: 0,
  success_count: 0,
  error_count: 0,
  success_rate: 1,
  avg_response_time_ms: 0,
  is_active: false,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

function standaloneKey(overrides: Partial<StandaloneKeyFormData> = {}): StandaloneKeyFormData {
  return {
    id: 'standalone-1',
    name: 'Standalone key',
    initial_balance_usd: 10,
    unlimited_balance: false,
    auto_delete_on_expiry: false,
    allowed_providers: ['provider-1'],
    allowed_provider_key_ids: { 'provider-1': ['key-disabled'] },
    allowed_api_formats: null,
    allowed_models: null,
    rate_limit: null,
    concurrent_limit: null,
    ip_rules: null,
    feature_settings: null,
    ...overrides,
  }
}

function mountDialog(apiKey: StandaloneKeyFormData): MountedDialog {
  const state = reactive({ open: false })
  const submitted: StandaloneKeyFormData[] = []
  const root = document.createElement('div')
  document.body.appendChild(root)
  const host = defineComponent({
    setup() {
      return () => h(StandaloneKeyFormDialog, {
        open: state.open,
        apiKey,
        onSubmit: (data: StandaloneKeyFormData) => submitted.push(data),
      })
    },
  })
  const app = createApp(host)
  app.mount(root)
  const mounted = { app, root, state, submitted }
  mountedDialogs.push(mounted)
  return mounted
}

async function settle() {
  for (let index = 0; index < 6; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

beforeEach(() => {
  apiMocks.getProvidersSummary.mockReset().mockResolvedValue({ items: [provider] })
  apiMocks.getGlobalModels.mockReset().mockResolvedValue({ models: [] })
  apiMocks.getProviderKeys.mockReset().mockResolvedValue([disabledSelectedKey])
  apiMocks.getApiFormats.mockReset().mockResolvedValue({ formats: [] })
  apiMocks.logError.mockReset()
})

afterEach(() => {
  for (const mounted of mountedDialogs.splice(0)) {
    mounted.app.unmount()
    mounted.root.remove()
  }
})

describe('StandaloneKeyFormDialog provider key scope', () => {
  it('reloads provider keys after cancelling and reselecting a provider', async () => {
    const mounted = mountDialog(standaloneKey())
    mounted.state.open = true
    await settle()

    const providerSelector = mounted.root.querySelector('[data-testid="provider-selector"]') as HTMLButtonElement
    providerSelector.click()
    await settle()
    providerSelector.click()
    await settle()

    expect(apiMocks.getProviderKeys).toHaveBeenCalledTimes(2)
  })

  it('allows a failed provider key request to retry after reselecting the provider', async () => {
    apiMocks.getProviderKeys
      .mockRejectedValueOnce(new Error('temporary provider key failure'))
      .mockResolvedValueOnce([disabledSelectedKey])
    const mounted = mountDialog(standaloneKey())
    mounted.state.open = true
    await settle()

    const providerSelector = mounted.root.querySelector('[data-testid="provider-selector"]') as HTMLButtonElement
    providerSelector.click()
    await settle()
    providerSelector.click()
    await settle()

    expect(apiMocks.getProviderKeys).toHaveBeenCalledTimes(2)
    expect(mounted.root.textContent).toContain('Disabled selected key')
  })

  it('allows selecting a disabled unselected key and round-trips the scope', async () => {
    const mounted = mountDialog(standaloneKey({ allowed_provider_key_ids: null }))
    mounted.state.open = true
    await settle()

    const keyLabel = [...mounted.root.querySelectorAll('label')]
      .find((label) => label.textContent?.includes('Disabled selected key'))
    const keyCheckbox = keyLabel?.querySelector('input[type="checkbox"]') as HTMLInputElement
    expect(keyCheckbox).toBeTruthy()
    expect(keyCheckbox.checked).toBe(false)
    expect(keyCheckbox.disabled).toBe(false)
    keyCheckbox.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()

    const updateButton = [...mounted.root.querySelectorAll('button')]
      .find((button) => button.textContent?.includes('更新')) as HTMLButtonElement
    updateButton.click()
    await settle()

    expect(mounted.submitted[0]?.allowed_provider_key_ids).toEqual({
      'provider-1': ['key-disabled'],
    })
  })

  it('keeps a disabled selected key cancellable and submits unrestricted scope as null', async () => {
    const mounted = mountDialog(standaloneKey())
    mounted.state.open = true
    await settle()

    const keyLabel = [...mounted.root.querySelectorAll('label')]
      .find((label) => label.textContent?.includes('Disabled selected key'))
    const keyCheckbox = keyLabel?.querySelector('input[type="checkbox"]') as HTMLInputElement
    expect(keyCheckbox).toBeTruthy()
    expect(keyCheckbox.checked).toBe(true)
    expect(keyCheckbox.disabled).toBe(false)
    keyCheckbox.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()

    const updateButton = [...mounted.root.querySelectorAll('button')]
      .find((button) => button.textContent?.includes('更新')) as HTMLButtonElement
    updateButton.click()
    await settle()

    expect(mounted.submitted[0]?.allowed_provider_key_ids).toBeNull()
  })

  it('submits null key scope when providers are unrestricted', async () => {
    const mounted = mountDialog(standaloneKey({
      allowed_providers: null,
      allowed_provider_key_ids: { 'provider-1': ['key-disabled'] },
    }))
    mounted.state.open = true
    await settle()

    const updateButton = [...mounted.root.querySelectorAll('button')]
      .find((button) => button.textContent?.includes('更新')) as HTMLButtonElement
    updateButton.click()
    await settle()

    expect(mounted.submitted[0]?.allowed_providers).toBeNull()
    expect(mounted.submitted[0]?.allowed_provider_key_ids).toBeNull()
  })
})
