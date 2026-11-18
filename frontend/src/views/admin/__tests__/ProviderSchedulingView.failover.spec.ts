import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, reactive, type App, type ComponentPublicInstance } from 'vue'
import ProviderSchedulingView from '@/features/providers/components/ProviderSchedulingView.vue'
import { createEmptyModelPolicy, createEmptyRoutingGroupConfig, getModelScheduling, savePerModelRoutingConfig, type RoutingModelPolicy } from '@/features/routing/utils/routingPolicy'
import { createSchedulingPolicy, readSchedulingPolicies, writeSchedulingPolicies } from '@/features/routing/utils/schedulingPolicies'
import type { RoutingGroupRecord, RoutingGroupUpdateRequest } from '@/api/routing-profiles'

const routingApi = vi.hoisted(() => ({
  listRoutingGroups: vi.fn(),
  updateRoutingGroup: vi.fn(),
  createRoutingGroup: vi.fn(),
  deleteRoutingGroup: vi.fn(),
}))
const contextChange = vi.hoisted(() => vi.fn())
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))
const globalModelsApi = vi.hoisted(() => ({ getGlobalModels: vi.fn() }))
const route = reactive({ name: 'ProviderManagement', query: { group: 'strategy-a' } })

vi.mock('@/api/routing-profiles', () => routingApi)
vi.mock('@/api/global-models', () => globalModelsApi)
vi.mock('@/composables/useToast', () => ({ useToast: () => toast }))
vi.mock('vue-router', () => ({ useRoute: () => route, useRouter: () => ({ replace: vi.fn(), push: vi.fn() }), onBeforeRouteLeave: vi.fn(), onBeforeRouteUpdate: vi.fn() }))
vi.mock('@/features/providers/composables/useSchedulingProviderBalance', () => ({ provideSchedulingProviderBalance: vi.fn() }))
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn(), warn: vi.fn() } }))
vi.mock('@/features/routing/components/RoutingPriorityPolicyEditor.vue', () => ({ default: { render: () => null } }))

const mounted: Array<{ app: App, root: HTMLElement }> = []
let workspaceInstance: ComponentPublicInstance & { updatePriorityPolicy: (policy: RoutingModelPolicy) => void }

function group(id: string): RoutingGroupRecord {
  return {
    id,
    name: id,
    enabled: true,
    is_system_default: false,
    sort_order: 0,
    config_json: createEmptyRoutingGroupConfig(),
    version: 1,
    created_at: 1,
    updated_at: 1,
  }
}

async function flush() {
  await nextTick()
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
}

async function mountPage(groups = [group('strategy-a'), group('strategy-b')]) {
  routingApi.listRoutingGroups.mockResolvedValue({ items: groups, total: groups.length })
  routingApi.updateRoutingGroup.mockImplementation(async (id: string, payload: RoutingGroupUpdateRequest) => ({
    ...groups.find(entry => entry.id === id),
    ...payload,
    version: 2,
  }))
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(ProviderSchedulingView, { onContextChange: contextChange })
  workspaceInstance = app.mount(root) as typeof workspaceInstance
  mounted.push({ app, root })
  await flush()
  button(root, '故障转移').click()
  await flush()
  return root
}

function element<T extends HTMLElement>(root: HTMLElement, selector: string): T {
  const found = root.querySelector<T>(selector)
  if (!found) throw new Error(`Missing element: ${selector}`)
  return found
}

function button(root: HTMLElement, label: string): HTMLButtonElement {
  return element(root, `button[aria-label="${label}"]`)
}

async function input(root: HTMLElement, label: string, value: string) {
  const field = element<HTMLInputElement | HTMLTextAreaElement>(root, `[aria-label="${label}"]`)
  field.value = value
  field.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

async function editJson(root: HTMLElement, section: string, value: string) {
  button(root, `切到${section} JSON`).click()
  await nextTick()
  await input(root, `${section} JSON`, value)
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('ResizeObserver', class {
    observe() {}
    unobserve() {}
    disconnect() {}
  })
  globalModelsApi.getGlobalModels.mockResolvedValue({ models: [
    { id: 'id-a', name: 'model-a', display_name: '模型 A' },
    { id: 'id-b', name: 'model-b', display_name: '模型 B' },
  ] })
  route.name = 'ProviderManagement'
  route.query.group = 'strategy-a'
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.unstubAllGlobals()
})

describe('ProviderSchedulingView failover persistence', () => {
  it('enables Save for JSON-only edits and persists both sections together', async () => {
    const root = await mountPage()
    expect(button(root, '保存调度').disabled).toBe(true)
    await editJson(root, '成功转移规则', '[{"pattern":"(?i)capacity"}]')
    await editJson(root, '错误终止规则', '[{"status_codes":[400,413]}]')
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(1)
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules).toEqual({
      success_failover_patterns: [{ pattern: '(?i)capacity', status_codes: [] }],
      error_stop_patterns: [{ pattern: '', status_codes: [400, 413] }],
    })
    expect(toast.error).not.toHaveBeenCalled()
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('does not submit partial JSON drafts when either section is invalid', async () => {
    const root = await mountPage()
    await editJson(root, '成功转移规则', '[{"pattern":"capacity"}]')
    await editJson(root, '错误终止规则', '{')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    expect(root.querySelector('[role="alert"]')).not.toBeNull()
    await input(root, '错误终止规则 JSON', '[{"status_codes":[429]}]')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(1)
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules.success_failover_patterns).toHaveLength(1)
  })

  it('discards local rule drafts when navigating to another strategy', async () => {
    const root = await mountPage()
    await editJson(root, '成功转移规则', '[{"pattern":"only-strategy-a"}]')
    route.query.group = 'strategy-b'
    await flush()
    expect(root.querySelector('textarea[aria-label="成功转移规则 JSON"]')).toBeNull()
    expect(button(root, '保存调度').disabled).toBe(true)
    await input(root, '全局最大转移次数', '3')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup.mock.calls[0][0]).toBe('strategy-b')
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules.success_failover_patterns).toEqual([])
  })

  it('saves scoped scheduling and global failover edits together without a per-model save', async () => {
    const strategy = group('strategy-a')
    strategy.config_json = savePerModelRoutingConfig(strategy.config_json, 'model-a')
    const root = await mountPage([strategy])
    const loadBalance = [...root.querySelectorAll<HTMLButtonElement>('button')].find(control => control.textContent?.trim() === '负载均衡')
    if (!loadBalance) throw new Error('Missing model scheduling control')
    loadBalance.click()
    await nextTick()
    await input(root, '全局最大转移次数', '5')
    button(root, '添加错误终止规则').click()
    await nextTick()
    await input(root, '终止规则 1 状态码', '429')
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(1)
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(saved.default_policy.max_transfer_count).toBe(5)
    expect(saved.default_policy.failover_rules.error_stop_patterns).toEqual([{ pattern: '', status_codes: [429] }])
    expect(getModelScheduling(saved, 'model-a').scheduling_mode).toBe('load_balance')
    expect(toast.error).not.toHaveBeenCalled()
  })

  it('blocks saving an empty scope and persists all selected models in one strategy', async () => {
    const root = await mountPage()
    const byText = (text: string) => {
      const found = [...root.querySelectorAll<HTMLButtonElement>('button')].find(control => control.textContent?.trim() === text)
      if (!found) throw new Error(`Missing control: ${text}`)
      return found
    }
    button(root, '区分模型').click()
    await flush()
    expect(button(root, '保存调度').disabled).toBe(true)
    const modelPicker = root.querySelector<HTMLButtonElement>('[aria-label="选择适用模型"]')
      ?? root.querySelector<HTMLButtonElement>('[aria-label="编辑模型"]')
    if (!modelPicker) throw new Error('Missing model picker')
    modelPicker.click()
    await flush()
    element<HTMLInputElement>(document.body, 'input[aria-label="选择模型 model-a"]').click()
    await flush()
    element<HTMLInputElement>(document.body, 'input[aria-label="选择模型 model-a"]').click()
    await flush()
    expect(button(root, '保存调度').disabled).toBe(true)
    for (const model of ['model-a', 'model-b']) {
      element<HTMLInputElement>(document.body, `input[aria-label="选择模型 ${model}"]`).click()
      await nextTick()
    }
    button(document.body, '完成选择').click()
    await flush()
    byText('负载均衡').click()
    await nextTick()
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(saved.model_policies.map((policy: { model: string }) => policy.model)).toEqual(['model-a', 'model-b'])
    expect(saved.rules).toHaveLength(1)
    expect(getModelScheduling(saved, 'model-a').scheduling_mode).toBe('load_balance')
    expect(getModelScheduling(saved, 'model-b').scheduling_mode).toBe('load_balance')
    expect(getModelScheduling(saved, 'other-model').scheduling_mode).toBe('cache_affinity')
    expect(root.querySelectorAll('[aria-label^="选择调度配置 "]')).toHaveLength(1)
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('saves one all-model configuration after switching from multiple model-specific configurations', async () => {
    const strategy = group('strategy-a')
    const first = { ...createSchedulingPolicy(strategy.config_json), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const second = { ...createSchedulingPolicy(strategy.config_json), models: ['model-b'], schedulingMode: 'load_balance' as const }
    strategy.config_json = writeSchedulingPolicies(strategy.config_json, [first, second])
    const root = await mountPage([strategy])
    expect(button(root, '区分模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelectorAll('[aria-label^="选择调度配置 "]')).toHaveLength(2)
    button(root, '全部模型').click()
    await flush()
    expect(root.querySelectorAll('[aria-label^="选择调度配置 "]')).toHaveLength(0)
    expect(root.querySelector('[role="group"][aria-label="调度策略"]')).not.toBeNull()
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledOnce()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(readSchedulingPolicies(saved)).toHaveLength(1)
    expect(readSchedulingPolicies(saved)[0]).toMatchObject({ scope: 'all', models: [], schedulingMode: 'fixed_order' })
    expect(saved.rules).toEqual([])
    expect(saved.model_policies.map((policy: { model: string }) => policy.model)).toEqual(['*'])
    expect(getModelScheduling(saved, 'model-b').scheduling_mode).toBe('fixed_order')
    expect(getModelScheduling(saved, 'future-model').scheduling_mode).toBe('fixed_order')
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(toast.error).not.toHaveBeenCalled()
  })
})

describe('ProviderSchedulingView inline settings drafts', () => {
  it.each([undefined, true])('loads strict format conversion (%s) and saves the toggled group policy', async (initial) => {
    const strategy = group('strategy-a')
    if (initial === undefined) {
      Reflect.deleteProperty(strategy.config_json.default_policy, 'strict_format_conversion')
    } else {
      strategy.config_json.default_policy.strict_format_conversion = initial
    }
    const root = await mountPage([strategy])
    button(root, '高级设置').click()
    await flush()
    const toggle = element<HTMLButtonElement>(root, '[role="switch"][aria-label="格式转换严谨模式"]')
    expect(toggle.getAttribute('aria-checked')).toBe(String(initial === true))
    toggle.click()
    await flush()
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledOnce()
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.strict_format_conversion).toBe(!initial)
    expect(toggle.getAttribute('aria-checked')).toBe(String(!initial))
  })

  it('preserves JSON drafts while failover settings are collapsed and saves from the header', async () => {
    const root = await mountPage()
    await editJson(root, '成功转移规则', '[{"pattern":"keep-after-collapse"}]')
    button(root, '故障转移').click()
    await flush()
    expect(button(root, '故障转移').getAttribute('aria-expanded')).toBe('false')
    expect(root.querySelector('[role="dialog"]')).toBeNull()
    expect(element<HTMLTextAreaElement>(root, '[aria-label="成功转移规则 JSON"]').value).toContain('keep-after-collapse')
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules.success_failover_patterns[0].pattern).toBe('keep-after-collapse')
  })

  it('reveals invalid failover JSON on save without changing advanced settings or dropping the draft', async () => {
    const root = await mountPage()
    await editJson(root, '错误终止规则', '{')
    button(root, '故障转移').click()
    await flush()
    expect(button(root, '故障转移').getAttribute('aria-expanded')).toBe('false')
    expect(element<HTMLTextAreaElement>(root, '[aria-label="错误终止规则 JSON"]').value).toBe('{')
    button(root, '保存调度').click()
    await flush()
    expect(button(root, '故障转移').getAttribute('aria-expanded')).toBe('true')
    expect(button(root, '高级设置').getAttribute('aria-expanded')).toBe('false')
    expect(root.querySelector('[role="dialog"]')).toBeNull()
    expect(root.querySelector('[role="alert"]')).not.toBeNull()
    expect(element<HTMLTextAreaElement>(root, '[aria-label="错误终止规则 JSON"]').value).toBe('{')
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })
})

describe('ProviderSchedulingView top-level scheduling selection', () => {
  it.each(['selected', 'all'] as const)('keeps the second %s configuration selected after saving its directory priority', async (scope) => {
    const strategy = group('strategy-a')
    const first = { ...createSchedulingPolicy(strategy.config_json), models: ['model-a'] }
    const second = { ...createSchedulingPolicy(strategy.config_json, scope), models: scope === 'selected' ? ['model-b'] : [] }
    strategy.config_json = writeSchedulingPolicies(strategy.config_json, [first, second])
    const root = await mountPage([strategy])
    button(root, '选择调度配置 2').click()
    await flush()
    workspaceInstance.updatePriorityPolicy({ ...second.policy, provider_priority_overrides: { 'provider-b': 7 } })
    await flush()
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledOnce()
    expect(button(root, '选择调度配置 2').getAttribute('aria-pressed')).toBe('true')
    expect(contextChange.mock.lastCall?.[0].activePolicy).toMatchObject({
      scope,
      modelNames: second.models,
      policy: { provider_priority_overrides: { 'provider-b': 7 } },
    })
    expect(contextChange.mock.lastCall?.[0].providerModelIds).toEqual(scope === 'selected' ? ['id-b'] : undefined)
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('emits all selected model IDs and applies directory priorities to the whole selection', async () => {
    const strategy = group('strategy-a')
    strategy.config_json = writeSchedulingPolicies(strategy.config_json, [{ ...createSchedulingPolicy(strategy.config_json), models: ['model-a', 'model-b'] }])
    const root = await mountPage([strategy])
    const context = contextChange.mock.lastCall?.[0]
    expect(context.providerModelIds).toEqual(['id-a', 'id-b'])
    expect(context.activePolicy.modelNames).toEqual(['model-a', 'model-b'])
    expect(context.priorityMode).toBe('provider')
    workspaceInstance.updatePriorityPolicy({ ...createEmptyModelPolicy('*'), provider_priority_overrides: { 'provider-a': 8 } })
    await flush()
    button(root, '保存调度').click()
    await flush()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(saved.model_policies.map((policy: RoutingModelPolicy) => policy.provider_priority_overrides)).toEqual([{ 'provider-a': 8 }, { 'provider-a': 8 }])
  })

  it('loads and saves legacy Key configuration as provider scheduling without remotely saving on open', async () => {
    const strategy = group('strategy-a')
    const selected = { ...createSchedulingPolicy(strategy.config_json), models: ['model-a'], policy: { ...createEmptyModelPolicy('*'), key_priority_overrides: { legacyKey: 4 }, provider_priority_overrides: { 'provider-a': 2 } } }
    strategy.config_json = writeSchedulingPolicies(strategy.config_json, [selected])
    strategy.config_json.default_policy.priority_mode = 'global_key'
    for (const rule of strategy.config_json.rules) {
      for (const action of rule.actions) {
        if (action && typeof action === 'object' && 'type' in action && action.type === 'set_scheduling') Object.assign(action, { priority_mode: 'global_key' })
      }
    }
    const root = await mountPage([strategy])
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(contextChange.mock.lastCall?.[0].priorityMode).toBe('provider')
    expect(contextChange.mock.lastCall?.[0].config.default_policy.priority_mode).toBe('provider')
    expect(getModelScheduling(contextChange.mock.lastCall?.[0].config, 'model-a').priority_mode).toBe('provider')
    expect(root.querySelector('[aria-label="Key 优先级排序"]')).toBeNull()
    await input(root, '全局最大转移次数', '3')
    button(root, '保存调度').click()
    await flush()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(saved.default_policy.priority_mode).toBe('provider')
    expect(getModelScheduling(saved, 'model-a').priority_mode).toBe('provider')
    expect(saved.model_policies[0].key_priority_overrides).toEqual({ legacyKey: 4 })
    expect(saved.model_policies[0].provider_priority_overrides).toEqual({ 'provider-a': 2 })
  })

  it('keeps the expanded provider configuration without a Key sorting surface', async () => {
    const root = await mountPage()
    expect(root.querySelector('[aria-label="Key 优先级排序"]')).toBeNull()
    expect(contextChange.mock.lastCall?.[0].priorityMode).toBe('provider')
    expect(contextChange.mock.lastCall?.[0].activePolicy.priorityMode).toBe('provider')
    expect(contextChange.mock.lastCall?.[0].providerModelIds).toBeUndefined()
    button(root, '故障转移').click()
    await flush()
    expect(root.querySelector('[aria-label="策略调度配置"]')).not.toBeNull()
    const combined = element(root, '[aria-label="策略分组与调度配置"]')
    expect(combined.contains(element(root, 'button[role="combobox"][aria-label="当前调度策略"]'))).toBe(true)
    expect(combined.contains(element(root, '[aria-label="策略调度配置"]'))).toBe(true)
    expect(combined.querySelector('h3')?.textContent?.trim()).not.toBe('调度配置')
    expect(root.querySelector('[aria-label="Key 优先级排序"]')).toBeNull()
  })
})
