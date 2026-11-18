import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import { createMemoryHistory, createRouter, RouterView, type LocationQueryRaw } from 'vue-router'
import ProviderSchedulingView from '../ProviderSchedulingView.vue'
import ProviderTableHeader from '../ProviderTableHeader.vue'
import { createEmptyRoutingGroupConfig, setDefaultProviderPriorityOverrides, type RoutingGroupConfig } from '@/features/routing/utils/routingPolicy'
import type { RoutingGroupRecord, RoutingGroupUpdateRequest } from '@/api/routing-profiles'

const routingApi = vi.hoisted(() => ({ listRoutingGroups: vi.fn(), updateRoutingGroup: vi.fn(), createRoutingGroup: vi.fn(), deleteRoutingGroup: vi.fn() }))
const confirm = vi.hoisted(() => vi.fn())
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))
const inspectProvider = vi.hoisted(() => vi.fn())
const contextChange = vi.hoisted(() => vi.fn())
const commitFailoverDrafts = vi.hoisted(() => vi.fn())
vi.mock('@/api/routing-profiles', () => routingApi)
vi.mock('@/api/global-models', () => ({ getGlobalModels: vi.fn().mockResolvedValue({ models: [] }) }))
vi.mock('@/composables/useToast', () => ({ useToast: () => toast }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => ({ confirm }) }))
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn(), warn: vi.fn() } }))
vi.mock('@/features/providers/composables/useSchedulingProviderBalance', () => ({ provideSchedulingProviderBalance: vi.fn() }))
vi.mock('../ProviderSchedulingStatus.vue', () => ({ default: { render: () => null } }))
vi.mock('@/features/routing/components/RoutingFailoverPolicyEditor.vue', async () => {
  const { defineComponent } = await import('vue')
  return { default: defineComponent({ setup(_, { expose }) { expose({ commitJsonDrafts: commitFailoverDrafts }); return () => null } }) }
})
vi.mock('@/features/routing/components/RoutingSchedulingPolicyEditor.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { refreshRevision: Number, layout: String, config: Object },
      emits: ['inspect-provider', 'selection-change'],
      setup(props, { emit, slots }) {
        return () => h('div', [
          slots['toolbar-leading']?.(),
          h('button', { 'aria-label': '查看示例提供商', 'data-revision': props.refreshRevision, 'data-layout': props.layout, onClick: () => emit('inspect-provider', 'provider-a') }, '示例提供商'),
          h('button', { 'aria-label': '选择示例调度配置', onClick: () => emit('selection-change', {
            id: 'selected-policy', policy: props.config?.default_policy, priorityMode: 'provider', schedulingMode: 'fixed_order', scope: 'selected', modelNames: ['example-model'],
          }) }, '示例配置'),
          slots['toolbar-actions']?.(),
        ])
      },
    }),
  }
})

const mounted: Array<{ app: App; root: HTMLElement }> = []
const collapsedStorageKey = 'aether-provider-policy-collapsed'
const originalScrollIntoView = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollIntoView')
function group(id: string, overrides: Partial<RoutingGroupRecord> = {}): RoutingGroupRecord {
  return { id, name: id, enabled: true, is_system_default: false, sort_order: 0, config_json: createEmptyRoutingGroupConfig(), version: 1, created_at: 1, updated_at: 1, ...overrides }
}
async function flush() {
  await nextTick()
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
  await vi.waitFor(() => {
    expect(document.querySelector('#provider-policy-editor.policy-panel-enter-active, #provider-policy-editor.policy-panel-leave-active')).toBeNull()
  }, { interval: 10 })
}
async function mountWorkspace(query: LocationQueryRaw = {}, items = [group('first'), group('default', { is_system_default: true }), group('last')]) {
  routingApi.listRoutingGroups.mockResolvedValue({ items, total: items.length })
  routingApi.updateRoutingGroup.mockImplementation(async (id: string, payload: RoutingGroupUpdateRequest) => ({ ...items.find(item => item.id === id), ...payload, version: 2 }))
  routingApi.createRoutingGroup.mockImplementation(async payload => group('created', payload))
  const providerRevision = ref(0)
  const workspace = ref<{ updateDraftConfig: (config: RoutingGroupConfig) => void; refreshGroups: () => Promise<void>; ensureSaved: () => Promise<boolean> } | null>(null)
  const Providers = defineComponent({
    setup() {
      return () => h(ProviderSchedulingView, { ref: workspace, providerRevision: providerRevision.value, onInspectProvider: inspectProvider, onContextChange: contextChange }, {
        default: ({ panelCollapsed, togglePanel }: { panelCollapsed: boolean; togglePanel: () => void }) => h('div', { 'data-testid': 'provider-directory' }, [
          h(ProviderTableHeader, {
            searchQuery: '', filterApiFormat: 'all', filterModel: 'all', apiFormatFilters: [], modelFilters: [], hasActiveFilters: false, loading: false,
            policyPanelCollapsed: panelCollapsed, onTogglePolicyPanel: togglePanel,
          }),
        ]),
      })
    },
  })
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: '/providers', name: 'ProviderManagement', component: Providers },
    { path: '/other', name: 'Other', component: { render: () => h('div', '其他页面') } },
  ] })
  await router.push({ name: 'ProviderManagement', query })
  await router.isReady()
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({ render: () => h(RouterView) })
  app.use(router)
  app.mount(root)
  mounted.push({ app, root })
  await flush()
  return { root, router, providerRevision, workspace }
}
function element<T extends HTMLElement>(root: HTMLElement, selector: string): T {
  const found = root.querySelector<T>(selector) ?? document.body.querySelector<T>(selector)
  if (!found) throw new Error(`Missing ${selector}`)
  return found
}
function button(root: HTMLElement, label: string) {
  return element<HTMLButtonElement>(root, `button[aria-label="${label}"]`)
}
function selector(root: HTMLElement) {
  return element<HTMLButtonElement>(root, 'button[role="combobox"][aria-label="当前调度策略"]')
}
async function openGroupOptions(root: HTMLElement) {
  selector(root).focus()
  selector(root).dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }))
  await flush()
  return [...document.querySelectorAll<HTMLElement>('[role="listbox"] [role="option"]')]
}
async function chooseGroup(root: HTMLElement, label: string) {
  const option = (await openGroupOptions(root)).find(item => item.textContent?.trim() === label)
  if (!option) throw new Error(`Missing group option: ${label}`)
  option.focus()
  option.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))
  await flush()
}
async function editName(root: HTMLElement, name: string) {
  const input = element<HTMLInputElement>(root, '[aria-label="策略名称"]')
  input.value = name
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}
async function editMultiplier(root: HTMLElement, value: string, label = '分组倍率') {
  const input = element<HTMLInputElement>(root, `[aria-label="${label}"]`)
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.removeItem(collapsedStorageKey)
  commitFailoverDrafts.mockReturnValue(true)
  confirm.mockResolvedValue(false)
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} })
  vi.stubGlobal('matchMedia', vi.fn((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })))
  Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', { value: vi.fn(), configurable: true })
})
afterEach(() => {
  for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() }
  localStorage.removeItem(collapsedStorageKey)
  vi.unstubAllGlobals()
  if (originalScrollIntoView) Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', originalScrollIntoView)
  else Reflect.deleteProperty(HTMLElement.prototype, 'scrollIntoView')
})

describe('ProviderSchedulingView workspace navigation', () => {
  it('keeps the draft, mounted editors and scheduling context when the policy panel is collapsed', async () => {
    const { root, router, providerRevision } = await mountWorkspace()
    await editName(root, '折叠后保留')
    button(root, '选择示例调度配置').click()
    await nextTick()
    const activePolicy = JSON.parse(JSON.stringify(contextChange.mock.lastCall?.[0].activePolicy))
    const editor = element(root, '#provider-policy-editor')
    const inspectButton = button(root, '查看示例提供商')
    const collapse = button(root, '收起策略分组')
    const directory = element(root, '[data-testid="provider-directory"]')
    const title = element(directory, 'h3')
    expect(directory.contains(collapse)).toBe(true)
    expect(collapse.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(editor.tagName).toBe('ASIDE')
    expect(editor.contains(collapse)).toBe(false)
    expect(collapse.getAttribute('aria-controls')).toBe(editor.id)
    expect(collapse.getAttribute('aria-expanded')).toBe('true')
    collapse.click()
    await flush()
    expect(editor.style.display).toBe('none')
    expect(button(root, '展开策略分组').getAttribute('aria-expanded')).toBe('false')
    expect(localStorage.getItem(collapsedStorageKey)).toBe('true')
    expect(button(root, '查看示例提供商')).toBe(inspectButton)
    expect(contextChange.mock.lastCall?.[0].activePolicy).toEqual(activePolicy)
    expect(contextChange.mock.lastCall?.[0].groupName).toBe('折叠后保留')
    expect(button(root, '保存调度').disabled).toBe(false)
    expect(router.currentRoute.value.query.group).toBeUndefined()
    expect(confirm).not.toHaveBeenCalled()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    providerRevision.value += 1
    await nextTick()
    expect(inspectButton.dataset.revision).toBe('1')
    button(root, '展开策略分组').click()
    await flush()
    expect(editor.style.display).not.toBe('none')
    expect(button(root, '查看示例提供商')).toBe(inspectButton)
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('折叠后保留')
    expect(localStorage.getItem(collapsedStorageKey)).toBe('false')
  })

  it('restores the collapsed panel after revisiting the provider page', async () => {
    const { root, router } = await mountWorkspace()
    button(root, '收起策略分组').click()
    await flush()
    await router.push({ name: 'Other' })
    await router.push({ name: 'ProviderManagement' })
    await flush()
    expect(element(root, '#provider-policy-editor').style.display).toBe('none')
    expect(button(root, '展开策略分组').getAttribute('aria-expanded')).toBe('false')
    expect(selector(root).textContent?.trim()).toBe('default · 默认')
    expect(root.querySelector('[data-testid="provider-directory"]')).not.toBeNull()
  })

  it.each([
    { field: '策略名称', value: '', error: '策略名称不能为空' },
    { field: '分组倍率', value: '', error: '分组倍率必须是大于或等于 0 的有效数字' },
  ])('expands the panel and focuses $field when saving finds invalid input', async ({ field, value, error }) => {
    const { root, workspace } = await mountWorkspace()
    if (field === '策略名称') await editName(root, value)
    else await editMultiplier(root, value)
    button(root, '收起策略分组').click()
    await flush()
    confirm.mockResolvedValueOnce(true)
    expect(await workspace.value?.ensureSaved()).toBe(false)
    await flush()
    expect(element(root, '#provider-policy-editor').style.display).not.toBe('none')
    expect(document.activeElement).toBe(element(root, `[aria-label="${field}"]`))
    expect(toast.error).toHaveBeenCalledWith(error)
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it('reveals the failover editor when its pending JSON draft prevents saving a collapsed panel', async () => {
    const { root, workspace } = await mountWorkspace()
    await editName(root, '待校验配置')
    commitFailoverDrafts.mockReturnValueOnce(false)
    button(root, '收起策略分组').click()
    await flush()
    confirm.mockResolvedValueOnce(true)
    expect(await workspace.value?.ensureSaved()).toBe(false)
    await flush()
    expect(commitFailoverDrafts).toHaveBeenCalledOnce()
    expect(element(root, '#provider-policy-editor').style.display).not.toBe('none')
    expect(button(root, '故障转移').getAttribute('aria-expanded')).toBe('true')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('待校验配置')
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it('defaults legacy groups to private and saves visibility without losing scheduling or rankings', async () => {
    const legacyConfig = createEmptyRoutingGroupConfig()
    Reflect.deleteProperty(legacyConfig, 'user_visible')
    const { root, workspace } = await mountWorkspace({}, [group('default', { is_system_default: true, config_json: legacyConfig })])
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('false')
    expect(button(root, '保存调度').disabled).toBe(true)
    button(root, '用户可见').click()
    await nextTick()
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('true')
    expect(button(root, '启用策略').getAttribute('aria-checked')).toBe('true')
    expect(button(root, '保存调度').disabled).toBe(false)
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    const config = setDefaultProviderPriorityOverrides(
      JSON.parse(JSON.stringify(contextChange.mock.lastCall?.[0].config)),
      { 'provider-a': 4, 'provider-b': 1 },
    )
    config.default_policy.scheduling_mode = 'fixed_order'
    config.disabled_providers = ['provider-c']
    workspace.value?.updateDraftConfig(config)
    await nextTick()
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('true')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({
      enabled: true,
      config_json: { ...config, user_visible: true },
    }))
    expect(button(root, '保存调度').disabled).toBe(true)
    button(root, '用户可见').click()
    await nextTick()
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({
      config_json: { ...config, user_visible: false },
    }))
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('false')
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('defaults old groups to a multiplier of one and saves decimal or zero values with scheduling changes', async () => {
    const legacyConfig = createEmptyRoutingGroupConfig()
    Reflect.deleteProperty(legacyConfig, 'billing_multiplier')
    const { root, workspace } = await mountWorkspace({}, [group('default', { is_system_default: true, config_json: legacyConfig })])
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1')
    await editMultiplier(root, '1.25')
    const config = JSON.parse(JSON.stringify(contextChange.mock.lastCall?.[0].config))
    config.disabled_providers = ['provider-a']
    workspace.value?.updateDraftConfig(config)
    await nextTick()
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({ config_json: expect.objectContaining({ billing_multiplier: 1.25, disabled_providers: ['provider-a'] }) }))
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1.25')
    expect(button(root, '保存调度').disabled).toBe(true)
    await editMultiplier(root, '0')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({ config_json: expect.objectContaining({ billing_multiplier: 0 }) }))
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('0')
  })

  it('keeps invalid multiplier input unsaved across scheduling changes and cancelled navigation', async () => {
    const { root, router, workspace } = await mountWorkspace()
    await editMultiplier(root, '2')
    await editMultiplier(root, '')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('')
    expect(element(root, '[aria-label="分组倍率"]').getAttribute('aria-invalid')).toBe('true')
    expect(button(root, '保存调度').disabled).toBe(true)
    const config = JSON.parse(JSON.stringify(contextChange.mock.lastCall?.[0].config))
    config.disabled_providers = ['provider-a']
    workspace.value?.updateDraftConfig(config)
    await nextTick()
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('')
    expect(contextChange.mock.lastCall?.[0].config.billing_multiplier).toBe(2)
    confirm.mockResolvedValueOnce(true)
    expect(await workspace.value?.ensureSaved()).toBe(false)
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    expect(toast.error).toHaveBeenCalledWith('分组倍率必须是大于或等于 0 的有效数字')
    await chooseGroup(root, 'last')
    expect(router.currentRoute.value.query.group).toBeUndefined()
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('')
    await editMultiplier(root, '-1')
    expect(button(root, '保存调度').disabled).toBe(true)
    confirm.mockResolvedValue(true)
    await chooseGroup(root, 'last')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1')
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('validates new group multipliers without replacing blank input and accepts zero', async () => {
    const { root, router } = await mountWorkspace({ group: 'new' })
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    await editName(root, '免费分组')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1')
    await editMultiplier(root, '')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('')
    expect(button(root, '保存调度').disabled).toBe(true)
    button(root, '保存调度').click()
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    await editMultiplier(root, '0')
    expect(button(root, '保存调度').disabled).toBe(false)
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.createRoutingGroup).toHaveBeenCalledWith(expect.objectContaining({ name: '免费分组', config_json: expect.objectContaining({ billing_multiplier: 0 }) }))
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('0')
    expect(router.currentRoute.value.query.group).toBe('created')
  })

  it('opens the system default immediately and forwards provider inspection and refreshes', async () => {
    const { root, providerRevision } = await mountWorkspace()
    expect(selector(root).textContent?.trim()).toBe('default · 默认')
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(root.querySelector('[data-testid="provider-directory"]')).not.toBeNull()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(button(root, '查看示例提供商').dataset.layout).toBe('config-only')
    const directory = element(root, '[data-testid="provider-directory"]')
    const combined = element(root, '[aria-label="策略分组与调度配置"]')
    expect(combined.contains(selector(root))).toBe(true)
    expect(combined.contains(button(root, '查看示例提供商'))).toBe(true)
    expect(combined.contains(directory)).toBe(false)
    const header = element(combined, 'h3').parentElement!
    expect(header.contains(selector(root))).toBe(true)
    expect(header.contains(button(root, '新建策略'))).toBe(true)
    expect(header.contains(button(root, '删除策略'))).toBe(true)
    expect(header.contains(button(root, '保存调度'))).toBe(true)
    expect(header.contains(button(root, '设为系统默认'))).toBe(true)
    const headerButtons = [...header.querySelectorAll<HTMLButtonElement>('button')]
    expect(headerButtons[headerButtons.indexOf(button(root, '保存调度')) - 1]).toBe(button(root, '设为系统默认'))
    expect(button(root, '设为系统默认').textContent?.trim()).toBe('')
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe('true')
    expect(button(root, '保存调度').textContent?.trim()).toBe('')
    expect(button(root, '新建策略').textContent?.trim()).toBe('')
    expect(button(root, '删除策略').textContent?.trim()).toBe('')
    expect(combined.querySelector('[role="status"]')).toBeNull()
    expect(combined.querySelector('[aria-label="撤销修改"]')).toBeNull()
    expect(button(root, '查看示例提供商').compareDocumentPosition(directory) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(button(root, '故障转移').getAttribute('aria-expanded')).toBe('false')
    expect(button(root, '高级设置').getAttribute('aria-expanded')).toBe('false')
    expect(combined.contains(button(root, '故障转移'))).toBe(true)
    expect(combined.contains(button(root, '高级设置'))).toBe(true)
    expect(root.querySelector('[aria-label="分组管理"]')).toBeNull()
    expect(root.querySelector('[aria-label="管理策略"]')).toBeNull()
    const controlOrder = [
      selector(root),
      element(root, '[aria-label="策略名称"]'),
      button(root, '启用策略'),
      button(root, '查看示例提供商'),
      button(root, '高级设置'),
      button(root, '故障转移'),
    ]
    for (let index = 1; index < controlOrder.length; index += 1) {
      expect(controlOrder[index - 1].compareDocumentPosition(controlOrder[index]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    }
    button(root, '查看示例提供商').click()
    expect(inspectProvider).toHaveBeenCalledWith('provider-a')
    await editName(root, '保持草稿')
    providerRevision.value += 1
    await nextTick()
    expect(button(root, '查看示例提供商').dataset.revision).toBe('1')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('保持草稿')
    expect(button(root, '保存调度').disabled).toBe(false)
  })

  it('keeps the route, dropdown and draft unchanged when a strategy switch is cancelled', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '未保存名称')
    await chooseGroup(root, 'last')
    expect(confirm).toHaveBeenCalledOnce()
    expect(router.currentRoute.value.query.group).toBeUndefined()
    expect(selector(root).textContent?.trim()).toBe('default · 默认')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('未保存名称')
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    const options = await openGroupOptions(root)
    expect(options.find(option => option.getAttribute('aria-selected') === 'true')?.textContent?.trim()).toBe('default · 默认')
    expect(options.find(option => option.textContent?.trim() === 'last')?.getAttribute('aria-selected')).toBe('false')
  })

  it('switches only after discard is confirmed and resets the saved state', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '未保存名称')
    confirm.mockResolvedValue(true)
    await chooseGroup(root, 'last')
    expect(router.currentRoute.value.query.group).toBe('last')
    expect(selector(root).textContent?.trim()).toBe('last')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('last')
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('protects leaving the provider page while unrelated query changes preserve the directory', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '需要保护')
    await router.push({ name: 'ProviderManagement', query: { search: 'sample' } })
    expect(confirm).not.toHaveBeenCalled()
    await router.push({ name: 'Other' })
    expect(router.currentRoute.value.name).toBe('ProviderManagement')
    expect(confirm).toHaveBeenCalledOnce()
    confirm.mockResolvedValue(true)
    await router.push({ name: 'Other' })
    expect(router.currentRoute.value.name).toBe('Other')
  })

  it('blocks navigation while saving and restores a clean snapshot after success', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '已保存名称')
    let resolveSave!: (value: RoutingGroupRecord) => void
    routingApi.updateRoutingGroup.mockReturnValue(new Promise<RoutingGroupRecord>(resolve => { resolveSave = resolve }))
    button(root, '保存调度').click()
    await nextTick()
    await router.push({ name: 'Other' })
    expect(router.currentRoute.value.name).toBe('ProviderManagement')
    expect(confirm).not.toHaveBeenCalled()
    expect(toast.error).toHaveBeenCalledWith('正在保存调度设置，请稍候再切换')
    resolveSave(group('default', { name: '已保存名称', is_system_default: true }))
    await flush()
    expect(button(root, '保存调度').disabled).toBe(true)
    await router.push({ name: 'Other' })
    expect(router.currentRoute.value.name).toBe('Other')
  })

  it('retains edits after a failed save and can retry without changing route', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '失败后保留')
    routingApi.updateRoutingGroup.mockRejectedValue(new Error('offline'))
    button(root, '保存调度').click()
    await flush()
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('失败后保留')
    expect(button(root, '保存调度').disabled).toBe(false)
    routingApi.updateRoutingGroup.mockResolvedValue(group('default', { name: '失败后保留', is_system_default: true }))
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledTimes(2)
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({ name: '失败后保留', expected_version: 1 }))
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('失败后保留')
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(router.currentRoute.value.query.group).toBeUndefined()
  })

  it('asks before replacing existing edits with a new inline group draft', async () => {
    const { root, router } = await mountWorkspace()
    await editName(root, '保留的分组草稿')
    button(root, '新建策略').click()
    await flush()
    expect(selector(root).textContent?.trim()).toBe('default · 默认')
    expect(router.currentRoute.value.query.group).toBeUndefined()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(button(root, '保存调度').disabled).toBe(false)
    expect(contextChange.mock.lastCall?.[0].groupName).toBe('保留的分组草稿')
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    expect(confirm).toHaveBeenCalledOnce()
    confirm.mockResolvedValue(true)
    button(root, '新建策略').click()
    await flush()
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('')
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it('starts an inline draft and creates the complete edited configuration only on header save', async () => {
    const { root, router, workspace } = await mountWorkspace()
    button(root, '收起策略分组').click()
    await flush()
    await router.push({ name: 'ProviderManagement', query: { group: 'new' } })
    await flush()
    expect(element(root, '#provider-policy-editor').style.display).not.toBe('none')
    expect(document.activeElement).toBe(element(root, '[aria-label="策略名称"]'))
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(root.querySelector('button[aria-label="删除策略"]')).toBeNull()
    expect(root.querySelector('[data-testid="provider-directory"]')).not.toBeNull()
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1')
    expect(button(root, '启用策略').getAttribute('aria-checked')).toBe('true')
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('false')
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe('false')
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    await editName(root, '新策略')
    await editMultiplier(root, '1.25')
    button(root, '启用策略').click()
    button(root, '用户可见').click()
    await nextTick()
    const config = JSON.parse(JSON.stringify(contextChange.mock.lastCall?.[0].config)) as RoutingGroupConfig
    config.disabled_providers = ['provider-a']
    config.default_policy.scheduling_mode = 'fixed_order'
    config.default_policy.sticky_key_attempts = 3
    workspace.value?.updateDraftConfig(config)
    await nextTick()
    button(root, '新建策略').click()
    await flush()
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('新策略')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1.25')
    expect(button(root, '启用策略').getAttribute('aria-checked')).toBe('false')
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('true')
    expect(config.user_visible).toBe(true)
    expect(contextChange.mock.lastCall?.[0].config).toEqual(config)
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    let finish!: (group: RoutingGroupRecord) => void
    routingApi.createRoutingGroup.mockReturnValue(new Promise(resolve => { finish = resolve }))
    button(root, '保存调度').click()
    await nextTick()
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(button(root, '保存调度').disabled).toBe(true)
    button(root, '保存调度').click()
    expect(routingApi.createRoutingGroup).toHaveBeenCalledOnce()
    expect(routingApi.createRoutingGroup).toHaveBeenCalledWith(expect.objectContaining({ name: '新策略', enabled: false, is_system_default: false, config_json: config }))
    finish(group('created', { name: '新策略', enabled: false, config_json: config }))
    await flush()
    expect(router.currentRoute.value.query.group).toBe('created')
    expect(selector(root).textContent?.trim()).toBe('新策略 · 停用')
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(confirm).not.toHaveBeenCalled()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it.each([false, true])('opens a new draft from its deep link with first-group default=%s', async firstGroup => {
    localStorage.setItem(collapsedStorageKey, 'true')
    const { root, router } = await mountWorkspace({ group: 'new' }, firstGroup ? [] : [group('existing')])
    expect(element(root, '#provider-policy-editor').style.display).not.toBe('none')
    expect(button(root, '收起策略分组').getAttribute('aria-expanded')).toBe('true')
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('1')
    expect(button(root, '启用策略').getAttribute('aria-checked')).toBe('true')
    expect(button(root, '用户可见').getAttribute('aria-checked')).toBe('false')
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe(String(firstGroup))
    expect(root.querySelector('button[aria-label="删除策略"]')).toBeNull()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    await editName(root, '深链新建')
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.createRoutingGroup).toHaveBeenCalledWith(expect.objectContaining({
      name: '深链新建',
      enabled: true,
      is_system_default: firstGroup,
      config_json: expect.objectContaining({ billing_multiplier: 1, user_visible: false }),
    }))
    expect(router.currentRoute.value.query.group).toBe('created')
  })

  it.each([500, 409])('keeps a failed new draft intact and retries the same create payload after status %s', async status => {
    const { root, router } = await mountWorkspace({ group: 'new' })
    await editName(root, '重试创建')
    await editMultiplier(root, '0.75')
    routingApi.createRoutingGroup.mockRejectedValueOnce({ response: { status } })
    button(root, '保存调度').click()
    await flush()
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('重试创建')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('0.75')
    expect(button(root, '保存调度').disabled).toBe(false)
    expect(toast.error).toHaveBeenCalled()
    expect(root.querySelector('button[aria-label="重新加载分组"]')).toBeNull()
    const failedPayload = routingApi.createRoutingGroup.mock.calls[0][0]
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.createRoutingGroup).toHaveBeenCalledTimes(2)
    expect(routingApi.createRoutingGroup).toHaveBeenLastCalledWith(failedPayload)
    expect(router.currentRoute.value.query.group).toBe('created')
    expect(button(root, '保存调度').disabled).toBe(true)
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it.each(['switch', 'leave'] as const)('confirms discarding a new draft before %s navigation', async navigation => {
    const { root, router } = await mountWorkspace({ group: 'new' })
    await editName(root, '保留新建草稿')
    await editMultiplier(root, '2')
    const navigate = () => navigation === 'switch'
      ? chooseGroup(root, 'last')
      : router.push({ name: 'Other' })
    await navigate()
    expect(confirm).toHaveBeenCalledOnce()
    expect(router.currentRoute.value.name).toBe('ProviderManagement')
    expect(router.currentRoute.value.query.group).toBe('new')
    expect(selector(root).textContent?.trim()).toBe('新建策略')
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('保留新建草稿')
    expect(element<HTMLInputElement>(root, '[aria-label="分组倍率"]').value).toBe('2')
    confirm.mockResolvedValue(true)
    await navigate()
    expect(router.currentRoute.value.name).toBe(navigation === 'switch' ? 'ProviderManagement' : 'Other')
    if (navigation === 'switch') {
      expect(router.currentRoute.value.query.group).toBe('last')
      expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('last')
    }
    expect(routingApi.createRoutingGroup).not.toHaveBeenCalled()
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
  })

  it('preserves API-provided retry budgets and policy fields when saving only group metadata', async () => {
    const config = createEmptyRoutingGroupConfig()
    config.default_policy.sticky_key_attempts = 120
    const { root } = await mountWorkspace({ group: 'last' }, [group('default', { is_system_default: true }), group('last', { config_json: config })])
    expect(contextChange.mock.lastCall?.[0].config.default_policy.sticky_key_attempts).toBe(120)
    await editName(root, '主要策略')
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe('false')
    button(root, '设为系统默认').click()
    await nextTick()
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe('true')
    expect(routingApi.updateRoutingGroup).not.toHaveBeenCalled()
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledWith('last', expect.objectContaining({ name: '主要策略', is_system_default: true }))
    expect(routingApi.updateRoutingGroup.mock.lastCall?.[1]).not.toHaveProperty('config_json')
    expect(contextChange.mock.lastCall?.[0].config.default_policy.sticky_key_attempts).toBe(120)
    expect(selector(root).textContent?.trim()).toBe('主要策略 · 默认')
    const options = await openGroupOptions(root)
    expect(options.find(option => option.getAttribute('aria-selected') === 'true')?.textContent?.trim()).toBe('主要策略 · 默认')
    expect(options.find(option => option.textContent?.trim() === 'default')?.getAttribute('aria-selected')).toBe('false')
    expect(button(root, '设为系统默认').getAttribute('aria-pressed')).toBe('true')
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('accepts directory changes and saves membership with the draft base version', async () => {
    const { root, workspace } = await mountWorkspace()
    const config = createEmptyRoutingGroupConfig()
    config.disabled_providers = ['provider-a']
    workspace.value?.updateDraftConfig(config)
    await nextTick()
    expect(contextChange.mock.lastCall?.[0].config.disabled_providers).toEqual(['provider-a'])
    confirm.mockResolvedValue(true)
    expect(await workspace.value?.ensureSaved()).toBe(true)
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledWith('default', expect.objectContaining({ expected_version: 1, config_json: expect.objectContaining({ disabled_providers: ['provider-a'] }) }))
    await flush()
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('preserves a stale draft version after refreshing groups and shows a save conflict', async () => {
    const { root, workspace } = await mountWorkspace()
    await editName(root, '我的修改')
    routingApi.listRoutingGroups.mockResolvedValue({ items: [group('default', { is_system_default: true, version: 9, name: '其他人的修改' })] })
    await workspace.value?.refreshGroups()
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('我的修改')
    routingApi.updateRoutingGroup.mockRejectedValue({ response: { status: 409 } })
    button(root, '收起策略分组').click()
    await flush()
    confirm.mockResolvedValueOnce(true)
    expect(await workspace.value?.ensureSaved()).toBe(false)
    await flush()
    expect(element(root, '#provider-policy-editor').style.display).not.toBe('none')
    expect(button(root, '收起策略分组').getAttribute('aria-expanded')).toBe('true')
    expect(routingApi.updateRoutingGroup).toHaveBeenCalledWith('default', expect.objectContaining({ expected_version: 1 }))
    expect(button(root, '保存调度').disabled).toBe(false)
    expect(toast.error).toHaveBeenCalledWith(expect.stringContaining('其他操作中更新'))
    confirm.mockResolvedValue(true)
    button(root, '重新加载分组').click()
    await flush()
    expect(element<HTMLInputElement>(root, '[aria-label="策略名称"]').value).toBe('其他人的修改')
    await editName(root, '基于新版修改')
    routingApi.updateRoutingGroup.mockResolvedValue(group('default', { version: 10, name: '基于新版修改', is_system_default: true }))
    button(root, '保存调度').click()
    await flush()
    expect(routingApi.updateRoutingGroup).toHaveBeenLastCalledWith('default', expect.objectContaining({ expected_version: 9 }))
    expect(button(root, '保存调度').disabled).toBe(true)
  })

  it('warns before a tab refresh only when there are unsaved changes', async () => {
    const { root } = await mountWorkspace()
    const clean = new Event('beforeunload', { cancelable: true })
    window.dispatchEvent(clean)
    expect(clean.defaultPrevented).toBe(false)
    await editName(root, '刷新保护')
    const dirty = new Event('beforeunload', { cancelable: true })
    window.dispatchEvent(dirty)
    expect(dirty.defaultPrevented).toBe(true)
  })
})
