import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, type App } from 'vue'

import ModelsTab from '../ModelsTab.vue'
import { createI18n } from '@/i18n'
import type { Model, ProviderWithEndpointsSummary } from '@/api/endpoints'

const modelMocks = vi.hoisted(() => ({
  deleteModel: vi.fn(),
  updateModel: vi.fn(),
}))

const confirmMocks = vi.hoisted(() => ({
  confirmDanger: vi.fn(),
}))

const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}))

vi.mock('@/api/endpoints/models', () => modelMocks)
vi.mock('@/api/endpoints/keys', () => ({
  getProviderKeys: vi.fn().mockResolvedValue([]),
}))
vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => confirmMocks,
}))
vi.mock('@/composables/useToast', () => ({
  useToast: () => toastMocks,
}))
vi.mock('@/composables/useClipboard', () => ({
  useClipboard: () => ({ copyToClipboard: vi.fn() }),
}))
vi.mock('@/composables/useModelTest', () => ({
  useModelTest: () => ({
    testing: { value: false },
    dialogOpen: { value: false },
    testResult: { value: null },
    testMode: { value: 'global' },
    testTrace: { value: null },
    requestId: { value: null },
    resetState: vi.fn(),
    startTest: vi.fn(),
    stopPolling: vi.fn(),
  }),
}))
vi.mock('../ModelTestDialog.vue', () => ({
  default: defineComponent({
    name: 'ModelTestDialogStub',
    setup: () => () => null,
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function createProvider(id = 'provider-1'): ProviderWithEndpointsSummary {
  return {
    id,
    name: 'OpenAI Responses',
    provider_type: 'custom',
    provider_priority: 1,
    keep_priority_on_conversion: false,
    enable_format_conversion: false,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 2,
    active_models: 2,
    global_model_ids: [],
    avg_health_score: 1,
    unhealthy_endpoints: 0,
    api_formats: ['openai:chat'],
    endpoint_health_details: [],
    ops_configured: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

function createModel(overrides: Partial<Model> = {}): Model {
  return {
    id: 'model-1',
    provider_id: 'provider-1',
    global_model_id: 'gm-1',
    provider_model_name: 'deepseek-v4-flash',
    global_model_name: 'deepseek-v4-flash',
    global_model_display_name: 'DeepSeek V4 Flash',
    is_active: true,
    is_available: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

const sampleModels = [
  createModel(),
  createModel({
    id: 'model-2',
    provider_model_name: 'kimi-k3',
    global_model_name: 'kimi-k3',
    global_model_display_name: 'Kimi K3',
  }),
]

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

function mountTab(options?: {
  models?: Model[]
  provider?: ProviderWithEndpointsSummary
}) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const state = reactive({
    models: options?.models ?? sampleModels,
    provider: options?.provider ?? createProvider(),
  })
  const onRefresh = vi.fn()
  const onBatchAssign = vi.fn()
  const app = createApp(defineComponent({
    setup() {
      return () => h(ModelsTab, {
        provider: state.provider,
        models: state.models,
        endpoints: [],
        onRefresh,
        onBatchAssign,
      })
    },
  }))
  app.use(createI18n())
  app.mount(root)
  mountedApps.push({ app, root })
  return { root, state, onRefresh }
}

beforeEach(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal('ResizeObserver', ResizeObserverStub)
  modelMocks.deleteModel.mockReset()
  modelMocks.deleteModel.mockResolvedValue({ message: 'ok' })
  modelMocks.updateModel.mockReset()
  confirmMocks.confirmDanger.mockReset()
  confirmMocks.confirmDanger.mockResolvedValue(true)
  toastMocks.success.mockReset()
  toastMocks.error.mockReset()
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.unstubAllGlobals()
})

describe('ModelsTab batch delete', () => {
  it('keeps delete selected hidden until a model is checked', async () => {
    const { root } = mountTab()
    await settle()

    expect(root.querySelector('[data-testid="models-tab-select-all"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="models-tab-row-checkbox-model-1"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="models-tab-row-checkbox-model-2"]')).toBeTruthy()
    expect(root.querySelector('[data-testid="models-tab-delete-selected"]')).toBeNull()
  })

  it('selects all, shows a partial state, and deletes after confirmation', async () => {
    const { root, onRefresh } = mountTab()
    await settle()

    const selectAll = root.querySelector('[data-testid="models-tab-select-all"]') as HTMLInputElement
    selectAll.click()
    await settle()

    expect(selectAll.checked).toBe(true)
    expect(selectAll.indeterminate).toBe(false)
    expect((root.querySelector('[data-testid="models-tab-row-checkbox-model-1"]') as HTMLInputElement).checked).toBe(true)
    expect((root.querySelector('[data-testid="models-tab-row-checkbox-model-2"]') as HTMLInputElement).checked).toBe(true)
    expect(root.textContent).toContain('已选 2 个')

    ;(root.querySelector('[data-testid="models-tab-row-checkbox-model-2"]') as HTMLInputElement).click()
    await settle()

    expect(selectAll.checked).toBe(false)
    expect(selectAll.indeterminate).toBe(true)
    expect(root.textContent).toContain('已选 1 个')

    const deleteButton = root.querySelector('[data-testid="models-tab-delete-selected"]') as HTMLButtonElement
    expect(deleteButton.textContent).toContain('删除选中')
    deleteButton.click()
    await settle()

    expect(confirmMocks.confirmDanger).toHaveBeenCalledWith(
      '确定删除选中的 1 个模型吗？\n\n此操作不可撤销。',
      '批量删除模型',
    )
    expect(modelMocks.deleteModel).toHaveBeenCalledTimes(1)
    expect(modelMocks.deleteModel).toHaveBeenCalledWith('provider-1', 'model-1')
    expect(toastMocks.success).toHaveBeenCalledWith('成功删除 1 个模型')
    expect(onRefresh).toHaveBeenCalledTimes(1)
    expect(root.querySelector('[data-testid="models-tab-delete-selected"]')).toBeNull()
    expect(selectAll.checked).toBe(false)
    expect(selectAll.indeterminate).toBe(false)
  })

  it('does not delete when confirmation is cancelled', async () => {
    confirmMocks.confirmDanger.mockResolvedValue(false)
    const { root, onRefresh } = mountTab()
    await settle()

    ;(root.querySelector('[data-testid="models-tab-row-checkbox-model-1"]') as HTMLInputElement).click()
    await settle()
    ;(root.querySelector('[data-testid="models-tab-delete-selected"]') as HTMLButtonElement).click()
    await settle()

    expect(modelMocks.deleteModel).not.toHaveBeenCalled()
    expect(onRefresh).not.toHaveBeenCalled()
    expect(root.querySelector('[data-testid="models-tab-delete-selected"]')).toBeTruthy()
  })

  it('drops stale selection when the current provider list refreshes', async () => {
    const { root, state } = mountTab()
    await settle()

    const selectAll = root.querySelector('[data-testid="models-tab-select-all"]') as HTMLInputElement
    selectAll.click()
    await settle()
    expect(root.textContent).toContain('已选 2 个')

    state.models = [sampleModels[1]]
    await settle()

    expect(root.textContent).toContain('已选 1 个')
    expect(root.querySelector('[data-testid="models-tab-row-checkbox-model-1"]')).toBeNull()
    expect((root.querySelector('[data-testid="models-tab-row-checkbox-model-2"]') as HTMLInputElement).checked).toBe(true)
  })

  it('clears selection when switching providers', async () => {
    const { root, state } = mountTab()
    await settle()

    ;(root.querySelector('[data-testid="models-tab-select-all"]') as HTMLInputElement).click()
    await settle()
    expect(root.textContent).toContain('已选 2 个')

    state.provider = createProvider('provider-2')
    await settle()

    expect(root.querySelector('[data-testid="models-tab-delete-selected"]')).toBeNull()
    expect((root.querySelector('[data-testid="models-tab-select-all"]') as HTMLInputElement).checked).toBe(false)
  })
})
