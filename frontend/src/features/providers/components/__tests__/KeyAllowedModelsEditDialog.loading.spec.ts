import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'

import KeyAllowedModelsEditDialog from '../KeyAllowedModelsEditDialog.vue'
import type { EndpointAPIKey } from '@/api/endpoints'

const endpointMocks = vi.hoisted(() => ({
  getProviderModels: vi.fn(),
  updateProviderKey: vi.fn(),
  fetchModels: vi.fn(),
}))

vi.mock('@/api/endpoints', () => endpointMocks)
vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  }),
}))
vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirmWarning: vi.fn().mockResolvedValue(true),
  }),
}))
vi.mock('@/features/providers/composables/useUpstreamModelsCache', () => ({
  useUpstreamModelsCache: () => ({
    fetchModels: endpointMocks.fetchModels,
  }),
}))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const Dialog = defineComponent({
    name: 'DialogStub',
    setup: (_props, { slots }) => () => h('section', [slots.default?.(), slots.footer?.()]),
  })
  const passthrough = (name: string) => defineComponent({
    name,
    inheritAttrs: false,
    setup: (_props, { attrs, slots }) => () => h(name === 'ButtonStub' ? 'button' : 'div', attrs, slots.default?.()),
  })
  return {
    Dialog,
    Button: passthrough('ButtonStub'),
    Input: passthrough('InputStub'),
    Checkbox: (await import('@/components/ui/checkbox.vue')).default,
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

const apiKey: EndpointAPIKey = {
  id: 'key-1',
  provider_id: 'provider-1',
  api_formats: ['openai:chat'],
  api_key_masked: 'sk-***',
  auth_type: 'api_key',
  name: 'Primary key',
  internal_priority: 0,
  allowed_models: ['gpt-5'],
  cache_ttl_minutes: 0,
  max_probe_interval_minutes: 5,
  health_score: 1,
  consecutive_failures: 0,
  request_count: 0,
  success_count: 0,
  error_count: 0,
  success_rate: 1,
  avg_response_time_ms: 0,
  is_active: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

beforeEach(() => {
  endpointMocks.getProviderModels.mockReset()
  endpointMocks.getProviderModels.mockResolvedValue([{
    provider_model_name: 'gpt-5',
    global_model_name: 'gpt-5',
    global_model_display_name: 'GPT-5',
  }])
  endpointMocks.updateProviderKey.mockReset()
  endpointMocks.fetchModels.mockResolvedValue({ models: [{ id: 'gpt-5', api_formats: ['openai:chat'] }] })
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('KeyAllowedModelsEditDialog loading', () => {
  async function mountKey(key: EndpointAPIKey) {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(KeyAllowedModelsEditDialog, { open: true, apiKey: key, providerId: 'provider-1' })
    app.mount(root)
    mountedApps.push({ app, root })
    await settle()
    return root
  }

  function selectAll(root: HTMLElement, checked: boolean) {
    const input = root.querySelector<HTMLInputElement>('input[type="checkbox"]')!
    input.checked = checked
    input.dispatchEvent(new Event('change', { bubbles: true }))
  }

  async function save(root: HTMLElement) {
    const button = [...root.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.trim() === '保存')!
    button.click()
    await settle()
  }

  it.each([false, true])('shows unrestricted permissions as all checked with auto-fetch=%s', async autoFetch => {
    const root = await mountKey({ ...apiKey, allowed_models: null, auto_fetch_models: autoFetch })
    expect(root.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(true)
    expect(root.textContent).toContain('全部模型')
    expect(root.textContent).not.toContain('未选择模型')
    expect(root.querySelectorAll('.bg-primary.border-primary').length).toBeGreaterThan(0)
    expect(endpointMocks.updateProviderKey).not.toHaveBeenCalled()
  })

  it('saves the global selection as unrestricted and deselection as an empty permission list', async () => {
    const root = await mountKey(apiKey)
    selectAll(root, true)
    await settle()
    await save(root)
    expect(endpointMocks.updateProviderKey).toHaveBeenLastCalledWith('key-1', { allowed_models: null, locked_models: [] })

    selectAll(root, false)
    await settle()
    await save(root)
    expect(endpointMocks.updateProviderKey).toHaveBeenLastCalledWith('key-1', { allowed_models: [], locked_models: [] })
  })

  it('turns off unrestricted access when a model is unchecked', async () => {
    endpointMocks.getProviderModels.mockResolvedValue([
      { provider_model_name: 'model-a' }, { provider_model_name: 'model-b' },
    ])
    const root = await mountKey({ ...apiKey, allowed_models: null })
    const row = [...root.querySelectorAll<HTMLElement>('.cursor-pointer')].find(el => el.querySelector('p')?.textContent?.trim() === 'model-a')!
    row.click()
    await settle()
    expect(root.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(false)
    await save(root)
    expect(endpointMocks.updateProviderKey).toHaveBeenLastCalledWith('key-1', { allowed_models: ['model-b'], locked_models: [] })
  })

  it('loads saved permissions when lazily mounted in the open state', async () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(KeyAllowedModelsEditDialog, {
      open: true,
      apiKey,
      providerId: 'provider-1',
    })
    app.mount(root)
    mountedApps.push({ app, root })

    await settle()

    expect(endpointMocks.getProviderModels).toHaveBeenCalledOnce()
    expect(endpointMocks.getProviderModels).toHaveBeenCalledWith('provider-1', { limit: 1000 })
    expect(root.textContent).toContain('提供商模型')
    expect(root.textContent).toContain('已选 1 个')
    expect(root.querySelector('.bg-primary.border-primary')).toBeTruthy()
  })
})
