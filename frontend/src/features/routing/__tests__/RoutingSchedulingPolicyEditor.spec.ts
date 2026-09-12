import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'
import type { GlobalModelResponse } from '@/api/global-models'
import RoutingSchedulingPolicyEditor from '../components/RoutingSchedulingPolicyEditor.vue'
import {
  createEmptyRoutingGroupConfig,
  getDefaultModelPolicy,
  getModelPolicy,
  getModelScheduling,
  setDefaultProviderPriorityOverrides,
  type RoutingGroupConfig,
} from '../utils/routingPolicy'
import { createSchedulingPolicy, readSchedulingPolicies, writeSchedulingPolicies } from '../utils/schedulingPolicies'

vi.mock('../components/RoutingPriorityPolicyEditor.vue', () => ({
  default: {
    props: ['config'],
    emits: ['update:config'],
    setup: (props: { config: RoutingGroupConfig }, { emit }: { emit: (event: string, config: RoutingGroupConfig) => void }) => () => h('button', {
      'aria-label': '调整排序',
      onClick: () => emit('update:config', setDefaultProviderPriorityOverrides(props.config, { provider: 7 })),
    }, '调整排序'),
  },
}))

const mounted: Array<{ app: App, root: HTMLElement }> = []

function mountEditor(initial = createEmptyRoutingGroupConfig()) {
  const config = ref(initial)
  const valid = ref(true)
  const disabled = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)
  const reload = vi.fn()
  const models = ref(['a', 'b', 'c'].map(name => ({
    id: `id-${name}`, name: `model-${name}`, display_name: `模型 ${name.toUpperCase()}`,
  })) as GlobalModelResponse[])
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({
    setup: () => () => h(RoutingSchedulingPolicyEditor, {
      config: config.value,
      disabled: disabled.value,
      globalModels: models.value,
      loadingModels: loading.value,
      modelsError: error.value,
      'onUpdate:config': (value: RoutingGroupConfig) => { config.value = value },
      onValidityChange: (value: boolean) => { valid.value = value },
      onReloadModels: reload,
    }),
  })
  app.mount(root)
  mounted.push({ app, root })
  return { root, config, valid, disabled, loading, error, models, reload }
}

function control<T extends HTMLElement>(root: HTMLElement, label: string): T {
  const element = root.querySelector<T>(`[aria-label="${label}"]`)
  if (!element) throw new Error(`Missing control: ${label}`)
  return element
}

async function clickText(root: HTMLElement, text: string) {
  const element = [...root.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent?.trim() === text)
  if (!element) throw new Error(`Missing button: ${text}`)
  element.click()
  await flush()
}

async function select(root: HTMLElement, model: string) {
  await openModels(root)
  control<HTMLInputElement>(root, `选择模型 ${model}`).click()
  await flush()
}

async function flush() {
  await nextTick()
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
}

async function openModels(root: HTMLElement) {
  if (control(root, '全部模型').getAttribute('aria-pressed') === 'true') {
    await clickText(root, '区分模型')
  }
  if (root.querySelector('[aria-label="全局模型选择列表"]')) return
  control<HTMLButtonElement>(root, '选择适用模型').click()
  await flush()
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class {
    observe() {}
    unobserve() {}
    disconnect() {}
  })
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.unstubAllGlobals()
})

describe('RoutingSchedulingPolicyEditor', () => {
  it('starts with one all-model configuration and no model picker or add control', async () => {
    const { root, config, valid } = mountEditor()
    expect(control(root, '调度范围').getAttribute('role')).toBe('group')
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('true')
    expect(control(root, '区分模型').getAttribute('aria-pressed')).toBe('false')
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(1)
    expect(root.querySelector('[aria-label="选择适用模型"]')).toBeNull()
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    expect(valid.value).toBe(true)
    await clickText(root, '负载均衡')
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    expect(readSchedulingPolicies(config.value)[0]).toMatchObject({ scope: 'all', models: [] })
    expect(getModelScheduling(config.value, 'future-model').scheduling_mode).toBe('load_balance')
  })

  it('selects models first and then configures their shared scheduling and ranking', async () => {
    const { root, config, valid } = mountEditor()
    const focusedControl = control<HTMLButtonElement>(root, '区分模型')
    focusedControl.focus()
    await clickText(root, '区分模型')
    const picker = control<HTMLButtonElement>(root, '选择适用模型')
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(true)
    expect(valid.value).toBe(false)
    const list = control(root, '全局模型选择列表')
    expect(list.getAttribute('role')).toBe('region')
    expect(list.id).toBeTruthy()
    expect(picker.getAttribute('aria-controls')).toBe(list.id)
    expect(picker.getAttribute('aria-expanded')).toBe('true')
    expect(document.querySelector('[role="dialog"][aria-label="选择适用模型"]')).toBeNull()
    expect(document.activeElement).toBe(focusedControl)
    expect(document.querySelector('button[aria-label="指定全局模型"]')).toBeNull()
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('false')
    expect(list.querySelector('[aria-label="全部模型"]')).toBeNull()
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(false)
    await select(root, 'model-a')
    await select(root, 'model-b')
    await clickText(root, '完成选择')
    expect(picker.getAttribute('aria-expanded')).toBe('false')
    expect(root.querySelector('[aria-label="全局模型选择列表"]')).toBeNull()
    expect(document.activeElement).toBe(picker)
    await clickText(root, 'Key')
    await clickText(root, '固定顺序')
    control<HTMLButtonElement>(root, '调整排序').click()
    await nextTick()
    expect(valid.value).toBe(true)
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    for (const model of ['model-a', 'model-b']) {
      expect(getModelScheduling(config.value, model)).toMatchObject({ priority_mode: 'global_key', scheduling_mode: 'fixed_order' })
      expect(getModelPolicy(config.value, model).provider_priority_overrides).toEqual({ provider: 7 })
    }
    expect(getModelScheduling(config.value, 'model-c')).toMatchObject({ priority_mode: 'provider', scheduling_mode: 'cache_affinity' })
    expect(getModelPolicy(config.value, '*').provider_priority_overrides).toEqual({})
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(false)
    expect([...root.querySelectorAll('h4')].map(heading => heading.textContent?.trim())).toEqual(['适用模型', '调度设置'])
  })

  it('keeps the inline picker open while editing scheduling outside it', async () => {
    const { root, config } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    const list = control(root, '全局模型选择列表')
    const schedulingButton = [...root.querySelectorAll<HTMLButtonElement>('button')]
      .find(button => button.textContent?.trim() === '负载均衡')!
    schedulingButton.focus()
    schedulingButton.click()
    await flush()
    expect(control(root, '全局模型选择列表')).toBe(list)
    expect(control(root, '选择适用模型').getAttribute('aria-expanded')).toBe('true')
    expect(document.activeElement).toBe(schedulingButton)
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('load_balance')
  })

  it('expands a newly mounted empty configuration without moving focus', async () => {
    const { root, valid } = mountEditor()
    await select(root, 'model-a')
    await clickText(root, '完成选择')
    const focusedControl = root.appendChild(document.createElement('button'))
    focusedControl.focus()
    control<HTMLButtonElement>(root, '添加调度配置').click()
    await flush()
    expect(valid.value).toBe(false)
    expect(control(root, '选择适用模型').getAttribute('aria-expanded')).toBe('true')
    expect(control(root, '全局模型选择列表').getAttribute('role')).toBe('region')
    expect(document.activeElement).toBe(focusedControl)
  })

  it('selects or clears only matching search results, keeping hidden selections intact', async () => {
    const { root, config } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    const search = control<HTMLInputElement>(root, '搜索全局模型')
    search.value = '模型 B'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await flush()
    control<HTMLButtonElement>(root, '全选搜索结果').click()
    await flush()
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a', 'model-b'])
    control<HTMLButtonElement>(root, '全选搜索结果').click()
    await flush()
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a'])
  })

  it('keeps list order stable while selecting models and displays their names when collapsed', async () => {
    const { root } = mountEditor()
    await openModels(root)
    const names = () => [...document.querySelectorAll<HTMLInputElement>('[aria-label="全局模型选择列表"] input[aria-label^="选择模型 "]')]
      .map(input => input.getAttribute('aria-label'))
    const originalOrder = names()
    await select(root, 'model-c')
    await select(root, 'model-a')
    expect(names()).toEqual(originalOrder)
    expect(control<HTMLInputElement>(root, '选择模型 model-c').checked).toBe(true)
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(true)
    await clickText(root, '完成选择')
    control<HTMLButtonElement>(root, '收起调度配置 1').click()
    await nextTick()
    expect(control<HTMLButtonElement>(root, '展开调度配置 1').textContent).toContain('模型 C、模型 A')
  })

  it('shows the selected value in the form field and keeps model checkboxes in sync', async () => {
    const { root } = mountEditor()
    await openModels(root)
    const picker = control<HTMLButtonElement>(root, '选择适用模型')
    expect(picker.textContent).toContain('请选择全局模型')
    await select(root, 'model-a')
    expect(picker.textContent).toContain('模型 A')
    await select(root, 'model-b')
    expect(picker.textContent).toContain('模型 A、模型 B')
    await select(root, 'model-c')
    expect(picker.textContent).toContain('已选择 3 个模型')
    await clickText(root, '完成选择')
    await select(root, 'model-b')
    expect(picker.textContent).toContain('模型 A、模型 C')
    expect(control<HTMLInputElement>(root, '选择模型 model-b').checked).toBe(false)
    await clickText(root, '清空已选')
    expect(picker.textContent).toContain('请选择全局模型')
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(false)
    expect(control<HTMLInputElement>(root, '选择模型 model-c').checked).toBe(false)
  })

  it('closes with Escape without losing selections and restores focus to the picker', async () => {
    const { root, config } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    const search = control<HTMLInputElement>(root, '搜索全局模型')
    search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await flush()
    expect(document.querySelector('[aria-label="全局模型选择列表"]')).toBeNull()
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a'])
    await vi.waitFor(() => expect(document.activeElement).toBe(control(root, '选择适用模型')))
  })

  it('clears selections without discarding scheduling settings', async () => {
    const { root, config, valid } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    await clickText(root, '完成选择')
    await clickText(root, '固定顺序')
    await openModels(root)
    await clickText(root, '清空已选')
    expect(valid.value).toBe(false)
    expect(control(root, '选择适用模型').textContent).toContain('请选择全局模型')
    await select(root, 'model-b')
    expect(valid.value).toBe(true)
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('fixed_order')
  })

  it('closes the inline picker while saving and does not modify config on a no-op click', async () => {
    const { root, config, disabled } = mountEditor()
    const original = JSON.stringify(config.value)
    await clickText(root, '全部模型')
    await clickText(root, '缓存亲和')
    expect(JSON.stringify(config.value)).toBe(original)
    await openModels(root)
    await select(root, 'model-a')
    disabled.value = true
    await flush()
    expect(document.querySelector('[aria-label="全局模型选择列表"]')).toBeNull()
    expect(control<HTMLButtonElement>(root, '选择适用模型').disabled).toBe(true)
  })

  it('adds another strategy for remaining models and prevents duplicate assignment', async () => {
    const { root, config, valid } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    control<HTMLButtonElement>(root, '添加调度配置').click()
    await flush()
    expect(valid.value).toBe(false)
    expect(document.querySelector('input[aria-label="选择模型 model-a"]')).toBeNull()
    const search = control<HTMLInputElement>(root, '搜索全局模型')
    search.value = 'model-a'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(control<HTMLInputElement>(root, '选择模型 model-a').disabled).toBe(true)
    search.value = ''
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    control<HTMLButtonElement>(root, '选择当前列表').click()
    await flush()
    await clickText(root, '完成选择')
    await clickText(root, '负载均衡')
    expect(valid.value).toBe(true)
    const entries = readSchedulingPolicies(config.value)
    expect(entries.map(entry => entry.models)).toEqual([['model-a'], ['model-b', 'model-c']])
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('cache_affinity')
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('load_balance')
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(true)
  })

  it('inherits all-model settings on first entering model-specific mode and restores both drafts', async () => {
    const { root, config, valid } = mountEditor()
    await clickText(root, '负载均衡')
    await clickText(root, 'Key')
    control<HTMLButtonElement>(root, '调整排序').click()
    await flush()
    await clickText(root, '区分模型')
    expect(valid.value).toBe(false)
    expect(control(root, '选择适用模型').textContent).toContain('请选择全局模型')
    await select(root, 'model-a')
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    expect(getModelScheduling(config.value, 'model-a')).toMatchObject({ priority_mode: 'global_key', scheduling_mode: 'load_balance' })
    expect(getModelPolicy(config.value, 'model-a').provider_priority_overrides).toEqual({ provider: 7 })
    expect(getModelScheduling(config.value, 'new-model').scheduling_mode).toBe('cache_affinity')
    await clickText(root, '固定顺序')
    await clickText(root, '全部模型')
    expect(valid.value).toBe(true)
    expect(config.value.rules).toEqual([])
    expect(config.value.model_policies.map(policy => policy.model)).toEqual(['*'])
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    expect(getModelScheduling(config.value, 'new-model').scheduling_mode).toBe('load_balance')
    await clickText(root, '区分模型')
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a'])
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('fixed_order')
    expect(getModelScheduling(config.value, 'new-model').scheduling_mode).toBe('cache_affinity')
    await openModels(root)
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(true)
  })

  it('uses the first model-specific configuration when first switching multiple entries to all models', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const first = { ...createSchedulingPolicy(initial), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const second = { ...createSchedulingPolicy(initial), models: ['model-b'], schedulingMode: 'load_balance' as const }
    const { root, config } = mountEditor(writeSchedulingPolicies(initial, [first, second]))
    await clickText(root, '全部模型')
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    expect(readSchedulingPolicies(config.value)[0]).toMatchObject({ scope: 'all', models: [], schedulingMode: 'fixed_order' })
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(1)
    expect(config.value.rules).toEqual([])
    expect(config.value.model_policies.map(policy => policy.model)).toEqual(['*'])
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('fixed_order')
    await clickText(root, '区分模型')
    expect(readSchedulingPolicies(config.value).map(entry => entry.models)).toEqual([['model-a'], ['model-b']])
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('load_balance')
  })

  it('restores an unfinished model-specific draft after temporarily using all models', async () => {
    const { root, config, valid } = mountEditor()
    await select(root, 'model-a')
    await clickText(root, '固定顺序')
    control<HTMLButtonElement>(root, '添加调度配置').click()
    await flush()
    expect(valid.value).toBe(false)
    await clickText(root, '全部模型')
    expect(valid.value).toBe(true)
    await clickText(root, '区分模型')
    expect(valid.value).toBe(false)
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(2)
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(true)
    control<HTMLButtonElement>(root, '展开调度配置 2').click()
    await flush()
    expect(control(root, '选择适用模型').textContent).toContain('请选择全局模型')
    await select(root, 'model-b')
    expect(valid.value).toBe(true)
    expect(readSchedulingPolicies(config.value).map(entry => entry.models)).toEqual([['model-a'], ['model-b']])
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('fixed_order')
  })

  it('offers all models independently of the catalog and automatically covers future models', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const entry = { ...createSchedulingPolicy(initial), models: ['model-a'], schedulingMode: 'load_balance' as const }
    const { root, config, models, loading, error, valid } = mountEditor(writeSchedulingPolicies(initial, [entry]))
    control<HTMLButtonElement>(root, '调整排序').click()
    await nextTick()
    models.value = []
    loading.value = true
    await flush()
    expect(control<HTMLButtonElement>(root, '全部模型').disabled).toBe(false)
    loading.value = false
    error.value = '模型加载失败'
    await flush()
    await clickText(root, '全部模型')
    expect(valid.value).toBe(true)
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelector('[aria-label="选择适用模型"]')).toBeNull()
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    const saved = JSON.stringify(config.value)
    error.value = null
    models.value = [{ id: 'id-new', name: 'new-model', display_name: '新增模型' }] as GlobalModelResponse[]
    await flush()
    expect(JSON.stringify(config.value)).toBe(saved)
    expect(readSchedulingPolicies(JSON.parse(saved))[0]).toMatchObject({ scope: 'all', models: [] })
    expect(config.value.rules).toEqual([])
    expect(config.value.model_policies.map(policy => policy.model)).toEqual(['*'])
    expect(getModelScheduling(config.value, 'new-model').scheduling_mode).toBe('load_balance')
    expect(getDefaultModelPolicy(config.value).provider_priority_overrides).toEqual({ provider: 7 })
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelector('[aria-label="全局模型选择列表"]')).toBeNull()
  })

  it('keeps selecting the current list distinct from the all-model scope', async () => {
    const { root, config, models } = mountEditor()
    await openModels(root)
    control<HTMLButtonElement>(root, '选择当前列表').click()
    await flush()
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('false')
    expect(readSchedulingPolicies(config.value)[0]).toMatchObject({ scope: 'selected', models: ['model-a', 'model-b', 'model-c'] })
    await clickText(root, '完成选择')
    await clickText(root, '负载均衡')
    const saved = JSON.stringify(config.value)
    models.value.push({ id: 'id-new', name: 'new-model', display_name: '新增模型' } as GlobalModelResponse)
    await flush()
    expect(JSON.stringify(config.value)).toBe(saved)
    expect(getModelScheduling(config.value, 'new-model').scheduling_mode).toBe('cache_affinity')
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('load_balance')
    await openModels(root)
    expect(control<HTMLInputElement>(root, '选择模型 new-model').checked).toBe(false)
  })

  it('preserves a legacy all-model fallback and allows more model-specific configurations', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const selected = { ...createSchedulingPolicy(initial), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const fallback = { ...createSchedulingPolicy(initial, 'all'), schedulingMode: 'load_balance' as const }
    const { root, config } = mountEditor(writeSchedulingPolicies(initial, [selected, fallback]))
    const saved = JSON.stringify(config.value)
    expect(control(root, '区分模型').getAttribute('aria-pressed')).toBe('true')
    expect(control(root, '展开调度配置 2').textContent).toContain('默认配置')
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(false)
    control<HTMLButtonElement>(root, '展开调度配置 2').click()
    await flush()
    expect(root.querySelector('[aria-label="选择适用模型"]')).toBeNull()
    expect(JSON.stringify(config.value)).toBe(saved)
    control<HTMLButtonElement>(root, '添加调度配置').click()
    await flush()
    await select(root, 'model-b')
    await clickText(root, '缓存亲和')
    expect(readSchedulingPolicies(config.value)).toHaveLength(3)
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('fixed_order')
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('cache_affinity')
    expect(getModelScheduling(config.value, 'future-model').scheduling_mode).toBe('load_balance')
  })

  it('uses the legacy fallback when switching mixed configurations to all models', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const selected = { ...createSchedulingPolicy(initial), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const fallback = { ...createSchedulingPolicy(initial, 'all'), schedulingMode: 'load_balance' as const }
    const { root, config } = mountEditor(writeSchedulingPolicies(initial, [selected, fallback]))
    await clickText(root, '全部模型')
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    expect(config.value.rules).toEqual([])
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('load_balance')
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('load_balance')
    await clickText(root, '区分模型')
    expect(readSchedulingPolicies(config.value)).toHaveLength(2)
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('fixed_order')
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('load_balance')
  })

  it('keeps scope and ranking edits when moving between strategy cards', async () => {
    const { root, config } = mountEditor()
    await clickText(root, '固定顺序')
    await openModels(root)
    await select(root, 'model-a')
    control<HTMLButtonElement>(root, '添加调度配置').click()
    await nextTick()
    await select(root, 'model-b')
    control<HTMLButtonElement>(root, '展开调度配置 1').click()
    await nextTick()
    await select(root, 'model-c')
    control<HTMLButtonElement>(root, '调整排序').click()
    await nextTick()
    const entries = readSchedulingPolicies(config.value)
    expect(entries[0]).toMatchObject({ models: ['model-a', 'model-c'], schedulingMode: 'fixed_order' })
    expect(getModelPolicy(config.value, 'model-c').provider_priority_overrides).toEqual({ provider: 7 })
    expect(getModelPolicy(config.value, 'model-b').provider_priority_overrides).toEqual({})
  })

  it('releases models and removes rules when a strategy is deleted', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const first = { ...createSchedulingPolicy(initial), models: ['model-a'] }
    const second = { ...createSchedulingPolicy(initial), models: ['model-b'] }
    const { root, config } = mountEditor(writeSchedulingPolicies(initial, [first, second]))
    control<HTMLButtonElement>(root, '删除调度配置 2').click()
    await nextTick()
    expect(config.value.rules).toHaveLength(1)
    expect(config.value.model_policies.map(policy => policy.model)).toEqual(['model-a'])
    await openModels(root)
    expect(control<HTMLInputElement>(root, '选择模型 model-b').disabled).toBe(false)
    await select(root, 'model-b')
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a', 'model-b'])
  })

  it('returns to all-model mode when deleting the last selected entry beside a legacy fallback', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const selected = { ...createSchedulingPolicy(initial), models: ['model-a'], schedulingMode: 'fixed_order' as const }
    const fallback = { ...createSchedulingPolicy(initial, 'all'), schedulingMode: 'load_balance' as const }
    const { root, config, valid } = mountEditor(writeSchedulingPolicies(initial, [selected, fallback]))
    control<HTMLButtonElement>(root, '删除调度配置 1').click()
    await flush()
    expect(valid.value).toBe(true)
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelector('[aria-label="添加调度配置"]')).toBeNull()
    expect(readSchedulingPolicies(config.value)).toHaveLength(1)
    expect(config.value.rules).toEqual([])
    expect(getModelScheduling(config.value, 'model-a').scheduling_mode).toBe('load_balance')
    await clickText(root, '区分模型')
    expect(valid.value).toBe(false)
    expect(root.querySelectorAll('section[aria-label^="调度配置 "]')).toHaveLength(1)
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(false)
    await select(root, 'model-b')
    expect(getModelScheduling(config.value, 'model-b').scheduling_mode).toBe('load_balance')
  })

  it('searches global model names and display names without losing selected models', async () => {
    const { root, config } = mountEditor()
    await openModels(root)
    await select(root, 'model-a')
    const search = control<HTMLInputElement>(root, '搜索全局模型')
    search.value = '模型 B'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(root.querySelector('input[aria-label="选择模型 model-a"]')).toBeNull()
    await select(root, 'model-b')
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-a', 'model-b'])
    search.value = ''
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(control<HTMLInputElement>(root, '选择模型 model-a').checked).toBe(true)
    await select(root, 'model-a')
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['model-b'])
  })

  it('retains group-wide failover changes when the shared ranking is edited', async () => {
    const { root, config } = mountEditor()
    config.value.default_policy.max_transfer_count = 9
    config.value.default_policy.cancel_on_client_disconnect = true
    await nextTick()
    await openModels(root)
    await select(root, 'model-a')
    control<HTMLButtonElement>(root, '调整排序').click()
    await nextTick()
    expect(config.value.default_policy.max_transfer_count).toBe(9)
    expect(config.value.default_policy.cancel_on_client_disconnect).toBe(true)
  })

  it('reports loading failures without clearing previously selected models', async () => {
    const initial = createEmptyRoutingGroupConfig()
    const entry = { ...createSchedulingPolicy(initial), models: ['removed-model'] }
    const { root, config, models, loading, error, reload, valid } = mountEditor(writeSchedulingPolicies(initial, [entry]))
    models.value = []
    loading.value = true
    await nextTick()
    await openModels(root)
    expect(document.body.textContent).toContain('正在加载全局模型')
    loading.value = false
    error.value = '模型加载失败'
    await nextTick()
    expect(document.body.textContent).toContain('模型加载失败')
    await clickText(root, '重试')
    expect(reload).toHaveBeenCalledOnce()
    expect(readSchedulingPolicies(config.value)[0].models).toEqual(['removed-model'])
    expect(valid.value).toBe(true)
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(true)
  })

  it('disables configuration controls while saving', async () => {
    const { root, config, disabled } = mountEditor()
    const previous = JSON.stringify(config.value)
    disabled.value = true
    await nextTick()
    await clickText(root, '负载均衡')
    await clickText(root, '区分模型')
    await flush()
    expect(control<HTMLButtonElement>(root, '全部模型').disabled).toBe(true)
    expect(control<HTMLButtonElement>(root, '区分模型').disabled).toBe(true)
    expect(control(root, '全部模型').getAttribute('aria-pressed')).toBe('true')
    expect(root.querySelector('[aria-label="选择适用模型"]')).toBeNull()
    expect(document.querySelector('[aria-label="全局模型选择列表"]')).toBeNull()
    expect(root.querySelector('fieldset')?.disabled).toBe(true)
    expect(JSON.stringify(config.value)).toBe(previous)
  })

  it('keeps model-specific drafts unchanged when scope switching is disabled', async () => {
    const { root, config, disabled } = mountEditor()
    await select(root, 'model-a')
    const previous = JSON.stringify(config.value)
    disabled.value = true
    await flush()
    await clickText(root, '全部模型')
    expect(control(root, '区分模型').getAttribute('aria-pressed')).toBe('true')
    expect(control<HTMLButtonElement>(root, '添加调度配置').disabled).toBe(true)
    expect(JSON.stringify(config.value)).toBe(previous)
  })
})
