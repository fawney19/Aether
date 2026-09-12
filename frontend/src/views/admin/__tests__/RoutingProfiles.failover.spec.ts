import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, reactive, type App } from 'vue'
import RoutingProfiles from '../RoutingProfiles.vue'
import { createEmptyRoutingGroupConfig, getModelScheduling, savePerModelRoutingConfig } from '@/features/routing/utils/routingPolicy'
import { createSchedulingPolicy, readSchedulingPolicies, writeSchedulingPolicies } from '@/features/routing/utils/schedulingPolicies'
import type { RoutingGroupRecord, RoutingGroupUpdateRequest } from '@/api/routing-profiles'

const routingApi = vi.hoisted(() => ({
  listRoutingGroups: vi.fn(),
  updateRoutingGroup: vi.fn(),
  createRoutingGroup: vi.fn(),
  deleteRoutingGroup: vi.fn(),
}))
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))
const globalModelsApi = vi.hoisted(() => ({ getGlobalModels: vi.fn() }))
const route = reactive({ name: 'RoutingProfileDetail', params: { groupId: 'strategy-a' } })

vi.mock('@/api/routing-profiles', () => routingApi)
vi.mock('@/api/global-models', () => globalModelsApi)
vi.mock('@/composables/useToast', () => ({ useToast: () => toast }))
vi.mock('vue-router', () => ({ useRoute: () => route, useRouter: () => ({ replace: vi.fn(), push: vi.fn() }) }))
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn(), warn: vi.fn() } }))
vi.mock('@/features/routing/components', async () => ({
  RoutingFailoverPolicyEditor: (await import('@/features/routing/components/RoutingFailoverPolicyEditor.vue')).default,
  RoutingSchedulingPolicyEditor: (await import('@/features/routing/components/RoutingSchedulingPolicyEditor.vue')).default,
}))
vi.mock('@/features/routing/components/RoutingPriorityPolicyEditor.vue', () => ({ default: { render: () => null } }))

const mounted: Array<{ app: App, root: HTMLElement }> = []

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
  const app = createApp(RoutingProfiles)
  app.mount(root)
  mounted.push({ app, root })
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
  route.name = 'RoutingProfileDetail'
  route.params.groupId = 'strategy-a'
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.unstubAllGlobals()
})

describe('RoutingProfiles failover persistence', () => {
  it('enables Save for JSON-only edits and persists both sections together', async () => {
    const root = await mountPage()
    expect(button(root, '保存').disabled).toBe(true)
    await editJson(root, '成功转移规则', '[{"pattern":"(?i)capacity"}]')
    await editJson(root, '错误终止规则', '[{"status_codes":[400,413]}]')
    expect(button(root, '保存').disabled).toBe(false)
    button(root, '保存').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(1)
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules).toEqual({
      success_failover_patterns: [{ pattern: '(?i)capacity', status_codes: [] }],
      error_stop_patterns: [{ pattern: '', status_codes: [400, 413] }],
    })
    expect(toast.error).not.toHaveBeenCalled()
    expect(button(root, '保存').disabled).toBe(true)
  })

  it('does not submit partial JSON drafts when either section is invalid', async () => {
    const root = await mountPage()
    await editJson(root, '成功转移规则', '[{"pattern":"capacity"}]')
    await editJson(root, '错误终止规则', '{')
    button(root, '保存').click()
    await flush()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    expect(root.querySelector('[role="alert"]')).not.toBeNull()
    await input(root, '错误终止规则 JSON', '[{"status_codes":[429]}]')
    button(root, '保存').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(1)
    expect(routingApi.updateRoutingGroup.mock.calls[0][1].config_json.default_policy.failover_rules.success_failover_patterns).toHaveLength(1)
  })

  it('discards local rule drafts when navigating to another strategy', async () => {
    const root = await mountPage()
    await editJson(root, '成功转移规则', '[{"pattern":"only-strategy-a"}]')
    route.params.groupId = 'strategy-b'
    await flush()
    expect(root.querySelector('textarea[aria-label="成功转移规则 JSON"]')).toBeNull()
    expect(button(root, '保存').disabled).toBe(true)
    await input(root, '全局最大转移次数', '3')
    button(root, '保存').click()
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
    expect(button(root, '保存').disabled).toBe(false)
    button(root, '保存').click()
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
    expect(button(root, '保存').disabled).toBe(true)
    element<HTMLInputElement>(document.body, 'input[aria-label="选择模型 model-a"]').click()
    await flush()
    button(document.body, '清空已选').click()
    await flush()
    expect(button(root, '保存').disabled).toBe(true)
    for (const model of ['model-a', 'model-b']) {
      element<HTMLInputElement>(document.body, `input[aria-label="选择模型 ${model}"]`).click()
      await nextTick()
    }
    const done = [...document.querySelectorAll<HTMLButtonElement>('[aria-label="全局模型选择列表"] button')]
      .find(control => control.textContent?.trim() === '完成选择')
    done?.click()
    await flush()
    byText('负载均衡').click()
    await nextTick()
    expect(button(root, '保存').disabled).toBe(false)
    button(root, '保存').click()
    await flush()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(saved.model_policies.map((policy: { model: string }) => policy.model)).toEqual(['model-a', 'model-b'])
    expect(saved.rules).toHaveLength(1)
    expect(getModelScheduling(saved, 'model-a').scheduling_mode).toBe('load_balance')
    expect(getModelScheduling(saved, 'model-b').scheduling_mode).toBe('load_balance')
    expect(getModelScheduling(saved, 'other-model').scheduling_mode).toBe('cache_affinity')
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(1)
    expect(button(root, '保存').disabled).toBe(true)
  })

  it('saves one all-model configuration after switching from multiple model-specific configurations', async () => {
    const strategy = group('strategy-a')
    const first = { ...createSchedulingPolicy(strategy.config_json), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const second = { ...createSchedulingPolicy(strategy.config_json), models: ['model-b'], schedulingMode: 'load_balance' as const }
    strategy.config_json = writeSchedulingPolicies(strategy.config_json, [first, second])
    const root = await mountPage([strategy])
    expect(button(root, '区分模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(2)
    button(root, '全部模型').click()
    await flush()
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(1)
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    expect(button(root, '保存').disabled).toBe(false)
    button(root, '保存').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledOnce()
    const saved = routingApi.updateRoutingGroup.mock.calls[0][1].config_json
    expect(readSchedulingPolicies(saved)).toHaveLength(1)
    expect(readSchedulingPolicies(saved)[0]).toMatchObject({ scope: 'all', models: [], schedulingMode: 'fixed_order' })
    expect(saved.rules).toEqual([])
    expect(saved.model_policies.map((policy: { model: string }) => policy.model)).toEqual(['*'])
    expect(getModelScheduling(saved, 'model-b').scheduling_mode).toBe('fixed_order')
    expect(getModelScheduling(saved, 'future-model').scheduling_mode).toBe('fixed_order')
    expect(button(root, '保存').disabled).toBe(true)
    expect(toast.error).not.toHaveBeenCalled()
  })
})
