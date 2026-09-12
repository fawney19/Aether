import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, ref, type App, type ComponentPublicInstance } from 'vue'
import EndpointFormDialog from '../EndpointFormDialog.vue'
import type { ProviderEndpoint, ProviderWithEndpointsSummary } from '@/api/endpoints'

const api = vi.hoisted(() => ({
  createEndpoint: vi.fn(),
  getDefaultBodyRules: vi.fn().mockResolvedValue({ body_rules: [] }),
  updateEndpoint: vi.fn(),
  deleteEndpoint: vi.fn(),
}))
vi.mock('@/api/endpoints', () => api)
vi.mock('@/api/admin', () => ({ adminApi: { getApiFormats: vi.fn().mockResolvedValue({ formats: [] }) } }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ error: vi.fn(), success: vi.fn(), warning: vi.fn() }) }))
vi.mock('@/stores/proxy-nodes', () => ({ useProxyNodesStore: () => ({ nodes: [], ensureLoaded: vi.fn() }) }))
vi.mock('../ProxyNodeSelect.vue', () => ({ default: { render: () => null } }))
vi.mock('../EndpointConditionEditor.vue', () => ({ default: { render: () => null } }))
vi.mock('../EndpointRulesRevealDialog.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/common/AlertDialog.vue', () => ({ default: { render: () => null } }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = defineComponent({
    inheritAttrs: false,
    setup: (_props, { slots }) => () => h('div', [slots.default?.(), slots.footer?.()]),
  })
  return Object.fromEntries([
    'Dialog', 'Button', 'Input', 'Textarea', 'Label', 'Badge', 'Select', 'SelectTrigger',
    'SelectValue', 'SelectContent', 'SelectItem', 'Switch', 'Collapsible', 'CollapsibleTrigger',
    'CollapsibleContent', 'Popover', 'PopoverTrigger', 'PopoverContent',
  ].map(name => [name, passthrough]))
})

interface DialogState {
  localEndpoints: ProviderEndpoint[]
  endpointEditStates: Record<string, unknown>
  endpointRulesJsonDirty: Record<string, boolean>
  endpointRulesJsonDraft: Record<string, string>
  enterEndpointRulesJsonMode: (endpoint: ProviderEndpoint) => void
  updateEndpointRulesJsonDraft: (endpointId: string, value: string) => void
  getEndpointEditRules: (endpointId: string) => Array<{ value: string, retainValue: boolean }>
  updateEndpointRuleField: (endpointId: string, index: number, field: 'value', value: string) => void
  hasRulePanelChanges: (endpoint: ProviderEndpoint) => boolean
  saveEndpoint: (endpoint: ProviderEndpoint) => Promise<void>
}

const endpoint: ProviderEndpoint = {
  id: 'endpoint-1', provider_id: 'provider-1', provider_name: 'Custom', api_format: 'openai:chat',
  base_url: 'https://example.test', is_active: true,
  header_rules: [{ action: 'set', key: 'x-auth', value: '***', has_value: true }],
  body_rules: [], config: {}, max_retries: 0, total_keys: 0, active_keys: 0,
  created_at: '2026-09-07T00:00:00Z', updated_at: '2026-09-07T00:00:00Z',
}
const mounted: Array<{ app: App, root: HTMLElement }> = []

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function mountDialog() {
  const props = reactive({
    modelValue: true,
    provider: { id: 'provider-1', provider_type: 'custom', name: 'Custom' } as ProviderWithEndpointsSummary,
    endpoints: [structuredClone(endpoint)],
  })
  const component = ref<ComponentPublicInstance>()
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({ setup: () => () => h(EndpointFormDialog, { ...props, ref: component }) }))
  app.mount(root)
  mounted.push({ app, root })
  await settle()
  const { setupState: state } = component.value!.$ as unknown as { setupState: DialogState }
  return { props, state }
}

beforeEach(() => {
  api.updateEndpoint.mockReset().mockResolvedValue(structuredClone(endpoint))
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('endpoint rule saving', () => {
  it('resets dirty state from the saved projection and preserves the new secret marker', async () => {
    const { state } = await mountDialog()
    state.updateEndpointRuleField(endpoint.id, 0, 'value', 'replacement-secret')
    expect(state.hasRulePanelChanges(state.localEndpoints[0])).toBe(true)
    await state.saveEndpoint(state.localEndpoints[0])
    await settle()
    expect(api.updateEndpoint).toHaveBeenCalledWith(endpoint.id, {
      header_rules: [{ action: 'set', key: 'x-auth', value: 'replacement-secret' }],
    })
    expect(state.getEndpointEditRules(endpoint.id)[0]).toMatchObject({ value: '***', retainValue: true })
    expect(state.hasRulePanelChanges(state.localEndpoints[0])).toBe(false)
    expect(state.endpointRulesJsonDirty[endpoint.id]).toBe(false)
  })

  it('does not overwrite newer edits when a save finishes', async () => {
    let resolveSave!: (saved: ProviderEndpoint) => void
    api.updateEndpoint.mockImplementationOnce(() => new Promise(resolve => { resolveSave = resolve }))
    const { state } = await mountDialog()
    state.updateEndpointRuleField(endpoint.id, 0, 'value', 'first-edit')
    const saving = state.saveEndpoint(state.localEndpoints[0])
    state.updateEndpointRuleField(endpoint.id, 0, 'value', 'newer-edit')
    resolveSave(structuredClone(endpoint))
    await saving
    expect(state.getEndpointEditRules(endpoint.id)[0].value).toBe('newer-edit')
    expect(state.hasRulePanelChanges(state.localEndpoints[0])).toBe(true)
  })

  it('refreshes JSON mode with the saved projection instead of retaining submitted plaintext', async () => {
    const { state } = await mountDialog()
    state.enterEndpointRulesJsonMode(state.localEndpoints[0])
    state.updateEndpointRulesJsonDraft(endpoint.id, JSON.stringify({
      header_rules: [{ action: 'set', key: 'x-auth', value: 'json-secret' }],
      body_rules: [], response_header_rules: [],
    }))
    await state.saveEndpoint(state.localEndpoints[0])
    const savedDraft = state.endpointRulesJsonDraft[endpoint.id]
    expect(savedDraft).not.toContain('json-secret')
    expect(JSON.parse(savedDraft).header_rules[0]).toMatchObject({ value: '***', has_value: true })
    expect(state.endpointRulesJsonDirty[endpoint.id]).toBe(false)
    expect(state.hasRulePanelChanges(state.localEndpoints[0])).toBe(false)
  })

  it('clears secret drafts on close and ignores the stale save response', async () => {
    let resolveSave!: (saved: ProviderEndpoint) => void
    api.updateEndpoint.mockImplementationOnce(() => new Promise(resolve => { resolveSave = resolve }))
    const { props, state } = await mountDialog()
    state.updateEndpointRuleField(endpoint.id, 0, 'value', 'unsaved-secret')
    const saving = state.saveEndpoint(state.localEndpoints[0])
    props.modelValue = false
    await settle()
    resolveSave(structuredClone(endpoint))
    await saving
    expect(state.endpointEditStates).toEqual({})
    expect(state.localEndpoints).toEqual([])
  })
})
