import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import type { ProviderEndpoint, ProviderWithEndpointsSummary } from '@/api/endpoints'
import EndpointFormDialog from '../EndpointFormDialog.vue'

const endpointMocks = vi.hoisted(() => ({
  createEndpoint: vi.fn(),
  deleteEndpoint: vi.fn(),
  getDefaultBodyRules: vi.fn(),
  updateEndpoint: vi.fn(),
}))

vi.mock('@/api/endpoints', () => endpointMocks)

vi.mock('@/api/admin', () => ({
  adminApi: {
    getApiFormats: vi.fn().mockResolvedValue({ formats: [] }),
  },
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h(tag, [slots.default?.(), slots.footer?.()])
    },
  })

  return {
    Badge: passthrough('BadgeStub', 'span'),
    Button: defineComponent({
      name: 'ButtonStub',
      setup(_, { attrs, slots }) {
        return () => h('button', { ...attrs, type: 'button' }, slots.default?.())
      },
    }),
    Collapsible: passthrough('CollapsibleStub'),
    CollapsibleContent: passthrough('CollapsibleContentStub'),
    CollapsibleTrigger: passthrough('CollapsibleTriggerStub'),
    Dialog: passthrough('DialogStub'),
    Input: defineComponent({
      name: 'InputStub',
      props: {
        disabled: Boolean,
        modelValue: { type: String, default: '' },
      },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('input', {
          ...attrs,
          disabled: props.disabled,
          value: props.modelValue,
          onInput: (event: Event) => emit(
            'update:modelValue',
            (event.target as HTMLInputElement).value,
          ),
        })
      },
    }),
    Label: passthrough('LabelStub', 'label'),
    Popover: passthrough('PopoverStub'),
    PopoverContent: passthrough('PopoverContentStub'),
    PopoverTrigger: passthrough('PopoverTriggerStub'),
    Select: passthrough('SelectStub'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub', 'span'),
    Switch: passthrough('SwitchStub', 'button'),
    Textarea: passthrough('TextareaStub', 'textarea'),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })
  return {
    AlignLeft: Icon,
    Check: Icon,
    CheckCircle: Icon,
    ChevronRight: Icon,
    Code2: Icon,
    Filter: Icon,
    Globe: Icon,
    GripVertical: Icon,
    HelpCircle: Icon,
    Plus: Icon,
    Power: Icon,
    Radio: Icon,
    RotateCcw: Icon,
    Save: Icon,
    Settings: Icon,
    Shuffle: Icon,
    Trash2: Icon,
    X: Icon,
  }
})

vi.mock('@/components/common/AlertDialog.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('div') }) }
})

vi.mock('../EndpointConditionEditor.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('div') }) }
})

vi.mock('../ProxyNodeSelect.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('div') }) }
})

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({ error: vi.fn(), success: vi.fn() }),
}))

vi.mock('@/i18n', async () => {
  const { ref } = await import('vue')
  return {
    setI18nLocale: vi.fn(),
    useI18n: () => ({ legacyT: (value: string) => value, locale: ref('zh-CN') }),
  }
})

vi.mock('@/stores/proxy-nodes', () => ({
  useProxyNodesStore: () => ({ ensureLoaded: vi.fn(), nodes: [] }),
}))

vi.mock('@/utils/logger', () => ({
  log: { error: vi.fn(), warn: vi.fn() },
}))

interface MountedDialog {
  readonly app: App
  readonly root: HTMLElement
}

const mountedDialogs: MountedDialog[] = []

function sampleEndpoint(): ProviderEndpoint {
  return {
    id: 'endpoint-responses',
    provider_id: 'provider-fixed',
    provider_name: 'Fixed provider',
    api_format: 'openai:responses',
    base_url: 'https://fixed.example.com',
    custom_path: '/old/responses',
    max_retries: 2,
    is_active: true,
    total_keys: 1,
    active_keys: 1,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

function sampleProvider(providerType: 'codex' | 'gemini_cli' | 'glm_coding_plan'): ProviderWithEndpointsSummary {
  return {
    id: 'provider-fixed',
    name: 'Fixed provider',
    provider_type: providerType,
    provider_priority: 0,
    keep_priority_on_conversion: false,
    enable_format_conversion: false,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 1,
    active_models: 1,
    global_model_ids: [],
    avg_health_score: 1,
    unhealthy_endpoints: 0,
    api_formats: ['openai:responses'],
    endpoint_health_details: [],
    ops_configured: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

function mountDialog(providerType: 'codex' | 'gemini_cli' | 'glm_coding_plan', endpoint = sampleEndpoint()): MountedDialog {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup() {
      return () => h(EndpointFormDialog, {
        modelValue: true,
        provider: sampleProvider(providerType),
        endpoints: [endpoint],
      })
    },
  }))
  app.mount(root)
  const mounted = { app, root }
  mountedDialogs.push(mounted)
  return mounted
}

async function settle(): Promise<void> {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

function customPathInput(root: HTMLElement): HTMLInputElement {
  const input = [...root.querySelectorAll('input')]
    .find(candidate => candidate.value === '/old/responses')
  if (!(input instanceof HTMLInputElement)) throw new Error('Missing custom path input')
  return input
}

beforeEach(() => {
  endpointMocks.createEndpoint.mockReset()
  endpointMocks.deleteEndpoint.mockReset()
  endpointMocks.getDefaultBodyRules.mockReset()
  endpointMocks.getDefaultBodyRules.mockResolvedValue({ body_rules: [] })
  endpointMocks.updateEndpoint.mockReset()
  endpointMocks.updateEndpoint.mockResolvedValue(undefined)
})

afterEach(() => {
  for (const { app, root } of mountedDialogs.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('EndpointFormDialog fixed-provider custom path', () => {
  it('keeps a fixed provider custom path editable', async () => {
    // Given
    const { root } = mountDialog('codex')

    // When
    await settle()

    // Then
    expect(customPathInput(root).disabled).toBe(false)
  })

  it('submits a changed fixed provider custom path', async () => {
    // Given
    const { root } = mountDialog('codex')
    await settle()

    // When
    const input = customPathInput(root)
    input.value = '/relay/responses'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    root.querySelector<HTMLButtonElement>('button[title="保存"]')?.click()
    await settle()

    // Then
    expect(endpointMocks.updateEndpoint).toHaveBeenCalledWith(
      'endpoint-responses',
      { custom_path: '/relay/responses' },
    )
  })

  it('keeps the read-only Gemini CLI custom path disabled', async () => {
    // Given
    const { root } = mountDialog('gemini_cli')

    // When
    await settle()

    // Then
    expect(customPathInput(root).disabled).toBe(true)
  })

  it('renders the GLM Coding Plan base URL presets', async () => {
    // Given
    const endpoint = { ...sampleEndpoint(), api_format: 'openai:chat' }
    const { root } = mountDialog('glm_coding_plan', endpoint)

    // When
    await settle()

    // Then
    expect(root.textContent).toContain('Zhipu')
    expect(root.textContent).toContain('Z.AI')
  })
})
