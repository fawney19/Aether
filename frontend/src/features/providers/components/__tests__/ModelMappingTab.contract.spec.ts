import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, createSSRApp, h, nextTick, type App } from 'vue'
import { renderToString } from '@vue/server-renderer'

import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import type { EndpointAPIKey } from '@/api/endpoints/keys'
import ModelMappingTab from '../provider-tabs/ModelMappingTab.vue'

const keyMocks = vi.hoisted(() => ({ getProviderKeys: vi.fn() }))
vi.mock('@/api/endpoints/keys', () => keyMocks)

const testMocks = vi.hoisted(() => ({
  testModel: vi.fn(),
  getRequestTrace: vi.fn(),
  showError: vi.fn(),
  showSuccess: vi.fn(),
}))

vi.mock('@/api/endpoints/providers', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/endpoints/providers')>(),
  testModel: testMocks.testModel,
}))
vi.mock('@/api/requestTrace', () => ({
  requestTraceApi: { getRequestTrace: testMocks.getRequestTrace },
}))
vi.mock('@/composables/useToast', () => ({
  useToast: () => ({ error: testMocks.showError, success: testMocks.showSuccess }),
}))

const provider: ProviderWithEndpointsSummary = {
  id: 'provider-demo',
  name: 'Demo Provider',
  provider_type: 'custom',
  is_active: true,
  active_keys: 0,
  api_formats: [],
  provider_priority: 0,
  keep_priority_on_conversion: false,
  enable_format_conversion: true,
  total_endpoints: 0,
  active_endpoints: 0,
  total_keys: 0,
  total_models: 0,
  active_models: 0,
  global_model_ids: [],
  avg_health_score: null,
  unhealthy_endpoints: 0,
  endpoint_health_details: [],
  ops_configured: false,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

type MappingTabProps = InstanceType<typeof ModelMappingTab>['$props']
type MappingTestState = {
  runMappingTest: (key: string, model: string) => void
  handleSelectTestEndpoint: (id: string) => void
  selectedTestKeyIds: string[]
  testKeyOptions: Array<{ value: string; label: string }>
  loadingTestKeys: boolean
  handleTestDialogClose: () => void
  handleStartMappingTest: () => Promise<void>
}

const endpoints = [
  { id: 'chat', api_format: 'openai:chat', base_url: 'https://example.com', is_active: true, active_keys: 1 },
  { id: 'claude', api_format: 'claude:messages', base_url: 'https://example.com', is_active: true, active_keys: 1 },
] as MappingTabProps['endpoints']

function createTestKey(overrides: Partial<EndpointAPIKey>): EndpointAPIKey {
  return {
    id: 'test-key',
    provider_id: provider.id,
    name: 'Test Key',
    api_formats: [],
    api_key_masked: '',
    auth_type: 'api_key',
    internal_priority: 0,
    cache_ttl_minutes: 0,
    max_probe_interval_minutes: 1,
    health_score: 1,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 0,
    avg_response_time_ms: 0,
    is_active: true,
    created_at: provider.created_at,
    updated_at: provider.updated_at,
    ...overrides,
  }
}

const testKeys = [
  createTestKey({ id: 'chat-key', name: 'Chat Key', api_key_masked: 'sk-****chat', api_formats: ['openai:chat'], internal_priority: 0 }),
  createTestKey({ id: 'claude-key', name: 'Claude Key', api_formats: ['claude:messages'], internal_priority: 1 }),
  createTestKey({ id: 'disabled-key', is_active: false, internal_priority: 2 }),
]
const mounted: Array<{ app: App; root: HTMLElement }> = []

function mountMappingTab(overrides: Partial<MappingTabProps> = {}) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(ModelMappingTab, { provider, endpoints, models: [], ...overrides })
  const instance = app.mount(root)
  const state = (instance.$ as unknown as { setupState: MappingTestState }).setupState
  mounted.push({ app, root })
  return state
}

function buttonWithText(text: string): HTMLButtonElement {
  const button = [...document.querySelectorAll('button')]
    .find(element => element.textContent?.trim() === text)
  if (!button) throw new Error(`Missing button: ${text}`)
  return button
}

async function openMappingTest(state: MappingTestState) {
  state.runMappingTest('mapping', 'test-model')
  await Promise.resolve()
  await nextTick()
}

beforeEach(() => {
  vi.resetAllMocks()
  keyMocks.getProviderKeys.mockResolvedValue(testKeys)
  testMocks.testModel.mockResolvedValue({ success: true, model: 'test-model' })
  testMocks.getRequestTrace.mockResolvedValue(null)
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ModelMappingTab response contracts', () => {
  it('loads test keys and removes incompatible selections when switching endpoints', async () => {
    const state = mountMappingTab()
    await openMappingTest(state)

    expect(keyMocks.getProviderKeys).toHaveBeenCalledWith(provider.id)
    expect(state.testKeyOptions).toEqual([
      { value: 'chat-key', label: 'Chat Key · sk-****chat · api_key' },
    ])
    expect(document.body.textContent).toContain('测试 Key')
    buttonWithText('默认调度（不指定 Key）').click()
    await nextTick()
    const option = [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')]
      .find(element => element.parentElement?.textContent?.includes('Chat Key'))
    if (!option) throw new Error('Missing Chat Key option')
    option.click()
    await nextTick()
    expect(state.selectedTestKeyIds).toEqual(['chat-key'])

    state.handleSelectTestEndpoint('claude')
    expect(state.selectedTestKeyIds).toEqual([])
    expect(state.testKeyOptions.map(option => option.value)).toEqual(['claude-key'])
    state.selectedTestKeyIds = ['claude-key']
    state.handleTestDialogClose()
    expect(state.selectedTestKeyIds).toEqual([])
  })

  it.each([
    { selectedKeyIds: ['chat-key'] },
    { selectedKeyIds: ['chat-key', 'shared-key'] },
  ])('passes the selected keys to the test request: $selectedKeyIds', async ({ selectedKeyIds }) => {
    keyMocks.getProviderKeys.mockResolvedValue([
      ...testKeys,
      createTestKey({ id: 'shared-key', name: 'Shared Key', internal_priority: 3 }),
    ])
    const state = mountMappingTab()
    await openMappingTest(state)
    state.selectedTestKeyIds = [...selectedKeyIds, selectedKeyIds[0], 'disabled-key', 'claude-key']

    await state.handleStartMappingTest()

    expect(testMocks.testModel).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      provider_id: provider.id,
      mode: 'direct',
      model_name: 'test-model',
      endpoint_id: 'chat',
      api_format: 'openai:chat',
      api_key_ids: selectedKeyIds,
    }), expect.objectContaining({ signal: expect.any(AbortSignal) }))
  })

  it('keeps default scheduling when no key is selected', async () => {
    const state = mountMappingTab()
    await openMappingTest(state)

    await state.handleStartMappingTest()

    expect(testMocks.testModel).toHaveBeenCalledOnce()
    expect(testMocks.testModel.mock.calls[0][0]).not.toHaveProperty('api_key_ids')
  })

  it('waits for keys to load before allowing a test', async () => {
    let resolveKeys!: (keys: EndpointAPIKey[]) => void
    keyMocks.getProviderKeys.mockReturnValue(new Promise<EndpointAPIKey[]>(resolve => {
      resolveKeys = resolve
    }))
    const state = mountMappingTab()
    await openMappingTest(state)

    expect(buttonWithText('正在加载 Key').disabled).toBe(true)
    expect(buttonWithText('开始测试').disabled).toBe(true)
    await state.handleStartMappingTest()
    expect(testMocks.testModel).not.toHaveBeenCalled()

    resolveKeys(testKeys)
    await Promise.resolve()
    await nextTick()
    expect(buttonWithText('开始测试').disabled).toBe(false)
  })

  it('keeps the selector visible when no compatible keys are available', async () => {
    keyMocks.getProviderKeys.mockResolvedValue([testKeys[1], testKeys[2]])
    const state = mountMappingTab()
    await openMappingTest(state)

    expect(document.body.textContent).toContain('测试 Key')
    buttonWithText('默认调度（不指定 Key）').click()
    await nextTick()
    expect(document.body.textContent).toContain('暂无可选 Key')
  })

  it('keeps provided keys usable after a loading failure', async () => {
    keyMocks.getProviderKeys.mockRejectedValue(new Error('Key service unavailable'))
    const state = mountMappingTab({ providerKeys: testKeys })
    await openMappingTest(state)

    expect(testMocks.showError).toHaveBeenCalledOnce()
    expect(state.loadingTestKeys).toBe(false)
    expect(state.testKeyOptions.map(option => option.value)).toEqual(['chat-key'])
    expect(buttonWithText('开始测试').disabled).toBe(false)
  })

  it('ignores key responses from a closed dialog', async () => {
    let resolveKeys!: (keys: EndpointAPIKey[]) => void
    keyMocks.getProviderKeys.mockReturnValueOnce(new Promise<EndpointAPIKey[]>(resolve => {
      resolveKeys = resolve
    }))
    const state = mountMappingTab()
    await openMappingTest(state)
    state.handleTestDialogClose()
    expect(state.loadingTestKeys).toBe(false)

    keyMocks.getProviderKeys.mockResolvedValue([])
    await openMappingTest(state)
    resolveKeys(testKeys)
    await Promise.resolve()
    await nextTick()

    expect(state.testKeyOptions).toEqual([])
  })

  it('keeps the module visible when a legacy or malformed preview reaches the component', async () => {
    const props: InstanceType<typeof ModelMappingTab>['$props'] = {
      provider,
      models: [],
      endpoints: [],
      providerKeys: [],
      loading: false,
    }
    Reflect.set(props, 'mappingPreview', {
      message: '演示模式：该接口暂未模拟',
      demo_mode: true,
    })
    const app = createSSRApp({
      render: () => h(ModelMappingTab, props),
    })

    const html = await renderToString(app)

    expect(html).toContain('模型映射')
    expect(html).toContain('暂无模型映射')
  })
})
