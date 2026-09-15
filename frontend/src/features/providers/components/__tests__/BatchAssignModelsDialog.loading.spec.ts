import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import BatchAssignModelsDialog from '../BatchAssignModelsDialog.vue'

const globalModelMocks = vi.hoisted(() => ({
  getGlobalModels: vi.fn(),
}))

const endpointMocks = vi.hoisted(() => ({
  getProviderModels: vi.fn(),
  getProviderKeys: vi.fn(),
  batchAssignModelsToProvider: vi.fn(),
  deleteModel: vi.fn(),
}))

vi.mock('@/api/endpoints/global-models', () => globalModelMocks)
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
    fetchModels: vi.fn(),
  }),
}))
vi.mock('@/components/ui/dialog/Dialog.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'DialogStub',
      setup: (_props, { slots }) => () => h('section', [slots.default?.(), slots.footer?.()]),
    }),
  }
})
vi.mock('@/components/ui', async () => {
  const { defineComponent } = await import('vue')
  const passthrough = (name: string) => defineComponent({
    name,
    inheritAttrs: false,
    setup: (_props, { slots }) => () => slots.default?.(),
  })
  return {
    DropdownMenu: passthrough('DropdownMenuStub'),
    DropdownMenuTrigger: passthrough('DropdownMenuTriggerStub'),
    DropdownMenuContent: passthrough('DropdownMenuContentStub'),
    DropdownMenuItem: passthrough('DropdownMenuItemStub'),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

beforeEach(() => {
  globalModelMocks.getGlobalModels.mockReset()
  globalModelMocks.getGlobalModels.mockResolvedValue({ models: [], total: 0 })
  endpointMocks.getProviderModels.mockReset()
  endpointMocks.getProviderModels.mockResolvedValue([])
  endpointMocks.getProviderKeys.mockReset()
  endpointMocks.getProviderKeys.mockResolvedValue([])
  endpointMocks.batchAssignModelsToProvider.mockReset()
  endpointMocks.deleteModel.mockReset()
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

function createGlobalModel(id: string, name: string, displayName = name) {
  return {
    id,
    name,
    display_name: displayName,
    is_active: true,
    default_tiered_pricing: { tiers: [] },
    created_at: '2026-01-01T00:00:00Z',
  }
}

function createProviderModel(id: string, globalModelId: string) {
  return {
    id,
    provider_id: 'provider-1',
    global_model_id: globalModelId,
    provider_model_name: globalModelId,
    is_active: true,
    is_available: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

function visibleModelIds(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll('[data-testid^="batch-assign-model-"]'))
    .map(node => node.getAttribute('data-testid')?.replace('batch-assign-model-', '') ?? '')
    .filter(Boolean)
}

describe('BatchAssignModelsDialog loading', () => {
  it('loads model choices when lazily mounted in the open state', async () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(BatchAssignModelsDialog, {
          open: true,
          providerId: 'provider-1',
          providerName: 'Provider One',
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })

    await settle()

    expect(globalModelMocks.getGlobalModels).toHaveBeenCalledOnce()
    expect(globalModelMocks.getGlobalModels).toHaveBeenCalledWith({ limit: 1000 })
    expect(endpointMocks.getProviderModels).toHaveBeenCalledWith('provider-1')
    expect(endpointMocks.getProviderKeys).toHaveBeenCalledWith('provider-1')
  })

  it('pins already associated models to the top of the list', async () => {
    globalModelMocks.getGlobalModels.mockResolvedValue({
      models: [
        createGlobalModel('gm-zeta', 'zeta-model', 'Zeta'),
        createGlobalModel('gm-alpha', 'alpha-model', 'Alpha'),
        createGlobalModel('gm-mu', 'mu-model', 'Mu'),
      ],
      total: 3,
    })
    endpointMocks.getProviderModels.mockResolvedValue([
      createProviderModel('pm-mu', 'gm-mu'),
    ])

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(BatchAssignModelsDialog, {
          open: true,
          providerId: 'provider-1',
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })
    await settle()

    expect(visibleModelIds(root)).toEqual(['gm-mu', 'gm-alpha', 'gm-zeta'])
  })

  it('keeps selected matches pinned above other search results', async () => {
    globalModelMocks.getGlobalModels.mockResolvedValue({
      models: [
        createGlobalModel('gm-beta', 'beta-flash', 'Beta Flash'),
        createGlobalModel('gm-alpha', 'alpha-flash', 'Alpha Flash'),
        createGlobalModel('gm-other', 'other-model', 'Other'),
      ],
      total: 3,
    })
    endpointMocks.getProviderModels.mockResolvedValue([
      createProviderModel('pm-beta', 'gm-beta'),
    ])

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(BatchAssignModelsDialog, {
          open: true,
          providerId: 'provider-1',
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })
    await settle()

    const search = root.querySelector('input') as HTMLInputElement
    search.value = 'flash'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await settle()

    expect(visibleModelIds(root)).toEqual(['gm-beta', 'gm-alpha'])
  })
})
