import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, type App } from 'vue'
import PoolAdvancedDialog from '@/features/pool/components/PoolAdvancedDialog.vue'
import type { PoolAdvancedConfig, ProviderWithEndpointsSummary } from '@/api/endpoints/types/provider'

const REQUIRED_PRE_PROBE_DISABLED_TOOLTIP = '仅支持 OAuth 号池（Codex/Kiro/Antigravity/ChatGPT Web）'

const endpointMocks = vi.hoisted(() => ({
  updateProvider: vi.fn(),
}))

vi.mock('@/api/endpoints', () => ({
  updateProvider: endpointMocks.updateProvider,
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { attrs, slots }) {
      return () => h(tag, attrs, slots.default?.())
    },
  })

  const Dialog = defineComponent({
    name: 'DialogStub',
    props: {
      modelValue: Boolean,
    },
    setup(props, { slots }) {
      return () => props.modelValue
        ? h('section', [slots.default?.(), h('footer', slots.footer?.())])
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
    setup(_, { attrs, slots }) {
      return () => h('label', attrs, slots.default?.())
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
      disabled: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        type: 'checkbox',
        role: 'switch',
        checked: props.modelValue,
        disabled: props.disabled,
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
    Tooltip: passthrough('TooltipStub'),
    TooltipContent: passthrough('TooltipContentStub'),
    TooltipProvider: passthrough('TooltipProviderStub'),
    TooltipTrigger: passthrough('TooltipTriggerStub'),
  }
})

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/utils/errorParser', () => ({
  parseApiError: (error: unknown) => String(error),
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
  }
})

interface MountedDialog {
  root: HTMLElement
  app: App
  state: {
    modelValue: boolean
    providerId: string
    providerType?: string
    currentConfig: PoolAdvancedConfig | null
  }
}

const mountedDialogs: MountedDialog[] = []

function providerSummary(overrides: Partial<ProviderWithEndpointsSummary> = {}): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Provider 1',
    provider_type: 'codex',
    provider_priority: 100,
    keep_priority_on_conversion: false,
    enable_format_conversion: false,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 0,
    active_models: 0,
    global_model_ids: [],
    avg_health_score: 100,
    unhealthy_endpoints: 0,
    api_formats: ['openai:chat'],
    endpoint_health_details: [],
    pool_advanced: null,
    failover_rules: null,
    ops_configured: false,
    created_at: '2026-05-10T00:00:00Z',
    updated_at: '2026-05-10T00:00:00Z',
    ...overrides,
  }
}

function mountDialog(options: {
  providerType: string
  currentConfig: PoolAdvancedConfig | null
}) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const state = reactive({
    modelValue: false,
    providerId: 'provider-1',
    providerType: options.providerType,
    currentConfig: options.currentConfig,
  })

  const app = createApp(defineComponent({
    setup() {
      return () => h(PoolAdvancedDialog, {
        modelValue: state.modelValue,
        providerId: state.providerId,
        providerType: state.providerType,
        currentConfig: state.currentConfig,
        currentClaudeConfig: null,
        'onUpdate:modelValue': (value: boolean) => {
          state.modelValue = value
        },
      })
    },
  }))

  app.mount(root)
  const mounted = { root, app, state }
  mountedDialogs.push(mounted)
  return mounted
}

async function settle() {
  for (let index = 0; index < 4; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function openDialog(dialog: MountedDialog) {
  dialog.state.modelValue = true
  await settle()
}

function candidatePreheatSwitch(root: HTMLElement) {
  const input = root.querySelector<HTMLInputElement>('input[aria-label="候选预热"]')
  expect(input).not.toBeNull()
  return input as HTMLInputElement
}

function inputByLabel(root: HTMLElement, label: string) {
  const input = root.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)
  expect(input).not.toBeNull()
  return input as HTMLInputElement
}

function changeCheckbox(input: HTMLInputElement, checked: boolean) {
  input.checked = checked
  input.dispatchEvent(new Event('change', { bubbles: true }))
}

function updateInput(input: HTMLInputElement, value: string) {
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

async function save(root: HTMLElement) {
  const button = Array.from(root.querySelectorAll<HTMLButtonElement>('button'))
    .find(current => current.textContent?.includes('保存'))
  expect(button).not.toBeUndefined()
  button?.click()
  await settle()
}

function lastPoolAdvancedPayload() {
  const calls = endpointMocks.updateProvider.mock.calls
  expect(calls.length).toBeGreaterThan(0)
  return calls[calls.length - 1][1].pool_advanced as PoolAdvancedConfig
}

beforeEach(() => {
  endpointMocks.updateProvider.mockReset()
  endpointMocks.updateProvider.mockResolvedValue(providerSummary())
})

afterEach(() => {
  for (const dialog of mountedDialogs.splice(0)) {
    dialog.app.unmount()
    dialog.root.remove()
  }
})

describe('PoolAdvancedDialog candidate preheat', () => {
  it('enables candidate preheat for OAuth pool providers and reveals T13 subfields', async () => {
    const dialog = mountDialog({ providerType: 'codex', currentConfig: null })
    await openDialog(dialog)

    expect(dialog.root.textContent).toContain('提前探测号池里排在后面的候选，主请求失败时秒切活号；号池改动立即生效')
    const switchInput = candidatePreheatSwitch(dialog.root)
    expect(switchInput.disabled).toBe(false)
    expect(dialog.root.querySelector('input[aria-label="候选数量"]')).toBeNull()

    changeCheckbox(switchInput, true)
    await settle()

    expect(inputByLabel(dialog.root, '候选数量').value).toBe('8')
    expect(inputByLabel(dialog.root, '5xx 连续阈值').value).toBe('5')
  })

  it('disables candidate preheat for non-OAuth providers with the required tooltip', async () => {
    const dialog = mountDialog({ providerType: 'custom', currentConfig: null })
    await openDialog(dialog)

    const switchInput = candidatePreheatSwitch(dialog.root)
    expect(switchInput.disabled).toBe(true)
    expect(dialog.root.querySelector('[data-testid="pre-probe-switch-wrapper"]')?.getAttribute('title'))
      .toBe(REQUIRED_PRE_PROBE_DISABLED_TOOLTIP)

    changeCheckbox(switchInput, true)
    await save(dialog.root)

    expect(lastPoolAdvancedPayload().pre_probe).toBeUndefined()
  })

  it('serializes the OAuth switch to pool_advanced.pre_probe.enabled', async () => {
    const dialog = mountDialog({ providerType: 'kiro', currentConfig: null })
    await openDialog(dialog)

    changeCheckbox(candidatePreheatSwitch(dialog.root), true)
    await save(dialog.root)

    expect(lastPoolAdvancedPayload().pre_probe).toMatchObject({
      enabled: true,
      top_n: 8,
      required_healthy: 8,
      '5xx_streak_threshold': 5,
    })
  })

  it('preserves existing pre-probe subfields while editing one value', async () => {
    const dialog = mountDialog({
      providerType: 'chatgpt_web',
      currentConfig: {
        scheduling_presets: [{ preset: 'cache_affinity', enabled: true }],
        pre_probe: {
          enabled: true,
          top_n: 12,
          required_healthy: 6,
          dedup_window_secs: 240,
          cache_ttl_seconds: 180,
          cache_max_entries: 2048,
          probe_timeout_seconds: 11,
          per_provider_rate_limit_per_minute: 45,
          group_lock_ttl_seconds: 12,
          circuit_failure_rate_threshold: 40,
          circuit_sample_window_seconds: 240,
          circuit_suspend_seconds: 480,
          '5xx_streak_threshold': 4,
        },
      },
    })
    await openDialog(dialog)

    expect(candidatePreheatSwitch(dialog.root).checked).toBe(true)
    expect(inputByLabel(dialog.root, '候选数量').value).toBe('12')

    updateInput(inputByLabel(dialog.root, '候选数量'), '10')
    await save(dialog.root)

    expect(lastPoolAdvancedPayload().scheduling_presets).toEqual([{ preset: 'cache_affinity', enabled: true }])
    expect(lastPoolAdvancedPayload().pre_probe).toMatchObject({
      enabled: true,
      top_n: 10,
      required_healthy: 6,
      dedup_window_secs: 240,
      '5xx_streak_threshold': 4,
    })
  })
})
