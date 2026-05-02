import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App, type Component } from 'vue'
import KeyFormDialog from '@/features/providers/components/KeyFormDialog.vue'
import OAuthKeyEditDialog from '@/features/providers/components/OAuthKeyEditDialog.vue'
import type { EndpointAPIKey } from '@/api/endpoints'

const endpointMocks = vi.hoisted(() => ({
  addProviderKey: vi.fn(),
  updateProviderKey: vi.fn(),
  getAllCapabilities: vi.fn(),
  sortApiFormats: vi.fn((formats: string[]) => [...formats].sort()),
}))

vi.mock('@/api/endpoints', () => ({
  addProviderKey: endpointMocks.addProviderKey,
  updateProviderKey: endpointMocks.updateProviderKey,
  getAllCapabilities: endpointMocks.getAllCapabilities,
  sortApiFormats: endpointMocks.sortApiFormats,
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h(tag, slots.default?.())
    },
  })

  const Dialog = defineComponent({
    name: 'DialogStub',
    props: {
      modelValue: Boolean,
    },
    setup(props, { slots }) {
      return () => props.modelValue
        ? h('section', [slots.default?.(), slots.footer?.()])
        : null
    },
  })

  const Input = defineComponent({
    name: 'InputStub',
    inheritAttrs: false,
    props: {
      modelValue: {
        type: [String, Number],
        default: '',
      },
      masked: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        value: props.modelValue ?? '',
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
      })
    },
  })

  const Label = defineComponent({
    name: 'LabelStub',
    inheritAttrs: false,
    props: {
      for: String,
    },
    setup(props, { attrs, slots }) {
      return () => h('label', { ...attrs, for: props.for }, slots.default?.())
    },
  })

  const Button = defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    props: {
      disabled: Boolean,
      variant: String,
    },
    setup(props, { attrs, slots }) {
      return () => h('button', {
        ...attrs,
        disabled: props.disabled,
        type: attrs.type ?? 'button',
      }, slots.default?.())
    },
  })

  const Switch = defineComponent({
    name: 'SwitchStub',
    inheritAttrs: false,
    props: {
      modelValue: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        type: 'checkbox',
        checked: props.modelValue,
        onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).checked),
      })
    },
  })

  return {
    Dialog,
    Button,
    Input,
    Label,
    Switch,
    Select: passthrough('SelectStub'),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub', 'span'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
  }
})

vi.mock('@/components/common/JsonImportInput.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'JsonImportInputStub',
      setup() {
        return () => h('textarea')
      },
    }),
  }
})

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirmWarning: vi.fn().mockResolvedValue(true),
  }),
}))

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    CircleHelp: Icon,
    Key: Icon,
    SquarePen: Icon,
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function createProviderKey(overrides: Partial<EndpointAPIKey> = {}): EndpointAPIKey {
  return {
    id: 'provider-key-1',
    provider_id: 'provider-1',
    api_formats: ['openai:chat'],
    api_key_masked: 'sk-***',
    auth_type: 'api_key',
    name: 'Primary key',
    rate_multipliers: null,
    internal_priority: 10,
    rpm_limit: 30,
    concurrent_limit: null,
    allowed_models: null,
    capabilities: null,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    health_score: 100,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 1,
    avg_response_time_ms: 0,
    is_active: true,
    note: '',
    created_at: '2026-04-27T00:00:00Z',
    updated_at: '2026-04-27T00:00:00Z',
    auto_fetch_models: false,
    model_include_patterns: [],
    model_exclude_patterns: [],
    ...overrides,
  }
}

function mountDialog(component: Component, props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(component, props)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

async function settle() {
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

function findInput(root: HTMLElement, id: string) {
  const input = root.querySelector<HTMLInputElement>(`#${id}`)
  expect(input).not.toBeNull()
  return input as HTMLInputElement
}

function updateInput(input: HTMLInputElement, value: string) {
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

async function submit(root: HTMLElement) {
  const form = root.querySelector('form')
  expect(form).not.toBeNull()
  form?.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
  await settle()
}

function lastUpdatePayload() {
  const calls = endpointMocks.updateProviderKey.mock.calls
  expect(calls.length).toBeGreaterThan(0)
  return calls[calls.length - 1][1] as Record<string, unknown>
}

beforeEach(() => {
  endpointMocks.addProviderKey.mockReset()
  endpointMocks.updateProviderKey.mockReset()
  endpointMocks.getAllCapabilities.mockReset()
  endpointMocks.sortApiFormats.mockClear()

  endpointMocks.addProviderKey.mockResolvedValue(createProviderKey())
  endpointMocks.updateProviderKey.mockResolvedValue(createProviderKey())
  endpointMocks.getAllCapabilities.mockResolvedValue([])
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('provider key concurrent_limit form behavior', () => {
  it('hydrates and serializes a positive concurrent_limit number from the normal key form', async () => {
    const root = mountDialog(KeyFormDialog, {
      open: true,
      endpoint: null,
      editingKey: createProviderKey({ rpm_limit: 42, concurrent_limit: 3 }),
      providerId: 'provider-1',
      providerType: 'openai',
      availableApiFormats: ['openai:chat'],
    })
    await settle()

    const concurrentLimitInput = findInput(root, 'concurrent_limit')
    expect(concurrentLimitInput.value).toBe('3')
    expect(findInput(root, 'rpm_limit').value).toBe('42')

    updateInput(concurrentLimitInput, '5')
    await submit(root)

    const payload = lastUpdatePayload()
    expect(payload.concurrent_limit).toBe(5)
    expect(typeof payload.concurrent_limit).toBe('number')
    expect(payload.concurrent_limit).not.toBe('')
    expect(payload.rpm_limit).toBe(42)
  })

  it('serializes cleared normal key concurrent_limit as null instead of an empty string', async () => {
    const root = mountDialog(KeyFormDialog, {
      open: true,
      endpoint: null,
      editingKey: createProviderKey({ rpm_limit: 24, concurrent_limit: 6 }),
      providerId: 'provider-1',
      providerType: 'openai',
      availableApiFormats: ['openai:chat'],
    })
    await settle()

    updateInput(findInput(root, 'concurrent_limit'), '')
    await submit(root)

    const payload = lastUpdatePayload()
    expect(payload).toHaveProperty('concurrent_limit', null)
    expect(payload.concurrent_limit).not.toBe('')
    expect(payload.rpm_limit).toBe(24)
  })

  it('hydrates and serializes a positive concurrent_limit number from the OAuth edit form', async () => {
    const root = mountDialog(OAuthKeyEditDialog, {
      open: true,
      editingKey: createProviderKey({
        id: 'oauth-key-1',
        auth_type: 'oauth',
        name: 'OAuth account',
        rpm_limit: 35,
        concurrent_limit: 3,
      }),
    })
    await settle()

    const concurrentLimitInput = findInput(root, 'concurrent_limit')
    expect(concurrentLimitInput.value).toBe('3')
    expect(findInput(root, 'rpm_limit').value).toBe('35')

    updateInput(concurrentLimitInput, '7')
    await submit(root)

    const payload = lastUpdatePayload()
    expect(endpointMocks.updateProviderKey).toHaveBeenCalledWith('oauth-key-1', expect.any(Object))
    expect(payload.concurrent_limit).toBe(7)
    expect(typeof payload.concurrent_limit).toBe('number')
    expect(payload.concurrent_limit).not.toBe('')
    expect(payload.rpm_limit).toBe(35)
  })

  it('serializes cleared OAuth concurrent_limit as null instead of an empty string', async () => {
    const root = mountDialog(OAuthKeyEditDialog, {
      open: true,
      editingKey: createProviderKey({
        id: 'oauth-key-2',
        auth_type: 'oauth',
        rpm_limit: 18,
        concurrent_limit: 4,
      }),
    })
    await settle()

    updateInput(findInput(root, 'concurrent_limit'), '')
    await submit(root)

    const payload = lastUpdatePayload()
    expect(payload).toHaveProperty('concurrent_limit', null)
    expect(payload.concurrent_limit).not.toBe('')
    expect(payload.rpm_limit).toBe(18)
  })

  it('keeps zero concurrent_limit as a numeric unlimited value', async () => {
    const root = mountDialog(OAuthKeyEditDialog, {
      open: true,
      editingKey: createProviderKey({
        id: 'oauth-key-zero',
        auth_type: 'oauth',
        rpm_limit: 11,
        concurrent_limit: 2,
      }),
    })
    await settle()

    updateInput(findInput(root, 'concurrent_limit'), '0')
    await submit(root)

    const payload = lastUpdatePayload()
    expect(payload.concurrent_limit).toBe(0)
    expect(typeof payload.concurrent_limit).toBe('number')
    expect(payload.rpm_limit).toBe(11)
  })
})
