import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'

import ModelMappingDialog, { type AliasGroup } from '../ModelMappingDialog.vue'
import type { Model, ProviderEndpoint } from '@/api/endpoints'
import { updateModel } from '@/api/endpoints/models'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h(tag, [slots.default?.(), slots.footer?.()])
    },
  })

  return {
    Button: defineComponent({
      name: 'ButtonStub',
      setup(_, { attrs, slots }) {
        return () => h('button', { ...attrs, type: 'button' }, slots.default?.())
      },
    }),
    Dialog: passthrough('DialogStub'),
    Input: defineComponent({
      name: 'InputStub',
      props: { modelValue: String },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('input', {
          ...attrs,
          value: props.modelValue ?? '',
          onInput: (event: Event) => emit(
            'update:modelValue',
            (event.target as HTMLInputElement).value,
          ),
        })
      },
    }),
    Label: passthrough('LabelStub', 'label'),
    Select: passthrough('SelectStub'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub', 'span'),
  }
})

vi.mock('@/components/common/MultiSelect.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'MultiSelectStub',
      props: {
        modelValue: { type: Array, default: () => [] },
        options: { type: Array, default: () => [] },
      },
      emits: ['update:modelValue'],
      setup(props, { emit }) {
        return () => h('div', (props.options as Array<{ value: string, label: string }>).map(option => h(
          'button',
          {
            type: 'button',
            'data-endpoint-id': option.value,
            onClick: () => emit('update:modelValue', [option.value]),
          },
          option.label,
        )))
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })
  return {
    Check: Icon,
    ChevronDown: Icon,
    Loader2: Icon,
    Plus: Icon,
    RefreshCw: Icon,
    Search: Icon,
    Tag: Icon,
    Zap: Icon,
  }
})

vi.mock('@/api/endpoints/models', () => ({
  updateModel: vi.fn().mockResolvedValue(undefined),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  }),
}))

vi.mock('../../composables/useUpstreamModelsCache', () => ({
  useUpstreamModelsCache: () => ({
    fetchModels: vi.fn(),
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  vi.mocked(updateModel).mockClear()
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ModelMappingDialog', () => {
  it('offers session compaction only for an explicitly selected Responses endpoint', async () => {
    const chatEndpoint = {
      id: 'endpoint-chat',
      api_format: 'openai:chat',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    } as ProviderEndpoint
    const responsesEndpoint = {
      id: 'endpoint-responses',
      api_format: 'openai:responses',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    } as ProviderEndpoint
    const model = {
      id: 'model-sol',
      provider_model_name: 'gpt-5.6-sol',
      global_model_display_name: 'GPT-5.6 Sol',
      provider_model_mappings: [],
    } as Model
    const open = ref(false)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(ModelMappingDialog, {
          open: open.value,
          providerId: 'provider-1',
          endpoints: [chatEndpoint, responsesEndpoint],
          models: [model],
          preselectedModelId: model.id,
          'onUpdate:open': (value: boolean) => { open.value = value },
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })

    open.value = true
    await nextTick()
    await nextTick()

    expect(root.textContent).toContain('所有请求')
    expect(root.textContent).not.toContain('仅会话压缩')

    root.querySelector<HTMLButtonElement>('[data-endpoint-id="endpoint-chat"]')?.click()
    await nextTick()
    expect(root.textContent).not.toContain('仅会话压缩')

    root.querySelector<HTMLButtonElement>('[data-endpoint-id="endpoint-responses"]')?.click()
    await nextTick()
    expect(root.textContent).toContain('仅会话压缩')
  })

  it('returns to all requests when a compact mapping switches away from Responses', async () => {
    const responsesEndpoint = {
      id: 'endpoint-responses',
      api_format: 'openai:responses',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    } as ProviderEndpoint
    const chatEndpoint = {
      id: 'endpoint-chat',
      api_format: 'openai:chat',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    } as ProviderEndpoint
    const model = {
      id: 'model-sol',
      provider_model_name: 'gpt-5.6-sol',
      global_model_display_name: 'GPT-5.6 Sol',
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: [responsesEndpoint.id],
        operations: ['compact'],
      }],
    } as Model
    const editingGroup: AliasGroup = {
      model,
      apiFormatsKey: '',
      apiFormats: [],
      endpointIdsKey: responsesEndpoint.id,
      endpointIds: [responsesEndpoint.id],
      operationsKey: 'compact',
      operations: ['compact'],
      aliases: model.provider_model_mappings ?? [],
    }
    const open = ref(false)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(ModelMappingDialog, {
          open: open.value,
          providerId: 'provider-1',
          endpoints: [responsesEndpoint, chatEndpoint],
          models: [model],
          editingGroup,
          'onUpdate:open': (value: boolean) => { open.value = value },
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })

    open.value = true
    await nextTick()
    await nextTick()
    expect(root.textContent).toContain('仅会话压缩')

    root.querySelector<HTMLButtonElement>('[data-endpoint-id="endpoint-chat"]')?.click()
    await nextTick()
    expect(root.textContent).not.toContain('仅会话压缩')
    expect(root.querySelector<HTMLButtonElement>('[role="radio"][aria-checked="true"]')?.textContent)
      .toContain('所有请求')

    const saveButton = [...root.querySelectorAll('button')]
      .find(button => button.textContent?.includes('保存映射'))
    saveButton?.click()
    await vi.waitFor(() => expect(updateModel).toHaveBeenCalledTimes(1))

    expect(updateModel).toHaveBeenCalledWith('provider-1', 'model-sol', {
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: [chatEndpoint.id],
      }],
    })
  })

  it('preserves an edited compact scope when endpoint capabilities are unavailable', async () => {
    const model = {
      id: 'model-sol',
      provider_model_name: 'gpt-5.6-sol',
      global_model_display_name: 'GPT-5.6 Sol',
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: ['endpoint-responses'],
        operations: ['compact'],
      }],
    } as Model
    const editingGroup: AliasGroup = {
      model,
      apiFormatsKey: '',
      apiFormats: [],
      endpointIdsKey: 'endpoint-responses',
      endpointIds: ['endpoint-responses'],
      operationsKey: 'compact',
      operations: ['compact'],
      aliases: model.provider_model_mappings ?? [],
    }
    const open = ref(false)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(ModelMappingDialog, {
          open: open.value,
          providerId: 'provider-1',
          endpoints: [],
          models: [model],
          editingGroup,
          'onUpdate:open': (value: boolean) => { open.value = value },
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })

    open.value = true
    await nextTick()
    await nextTick()

    const saveButton = [...root.querySelectorAll('button')]
      .find(button => button.textContent?.includes('保存映射'))
    saveButton?.click()
    await vi.waitFor(() => expect(updateModel).toHaveBeenCalledTimes(1))

    expect(updateModel).toHaveBeenCalledWith('provider-1', 'model-sol', {
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: ['endpoint-responses'],
        operations: ['compact'],
      }],
    })
  })

  it('normalizes and replaces an edited compact operation scope', async () => {
    const endpoint = {
      id: 'endpoint-responses',
      api_format: 'openai:responses',
      base_url: 'https://api.example.com/v1',
      is_active: true,
    } as ProviderEndpoint
    const model = {
      id: 'model-sol',
      provider_model_name: 'gpt-5.6-sol',
      global_model_display_name: 'GPT-5.6 Sol',
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: [endpoint.id],
        operations: ['Compact'],
      }],
    } as Model
    const editingGroup: AliasGroup = {
      model,
      apiFormatsKey: '',
      apiFormats: [],
      endpointIdsKey: endpoint.id,
      endpointIds: [endpoint.id],
      operationsKey: 'Compact',
      operations: ['Compact'],
      aliases: model.provider_model_mappings ?? [],
    }
    const open = ref(false)
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({
      setup() {
        return () => h(ModelMappingDialog, {
          open: open.value,
          providerId: 'provider-1',
          endpoints: [endpoint],
          models: [model],
          editingGroup,
          'onUpdate:open': (value: boolean) => { open.value = value },
        })
      },
    }))
    app.mount(root)
    mountedApps.push({ app, root })

    open.value = true
    await nextTick()
    await nextTick()
    expect(root.textContent).toContain('仅会话压缩')

    const scopeButtons = [...root.querySelectorAll('button')]
    scopeButtons.find(button => button.textContent?.includes('所有请求'))?.click()
    await nextTick()
    scopeButtons.find(button => button.textContent?.includes('仅会话压缩'))?.click()
    await nextTick()
    const saveButton = [...root.querySelectorAll('button')]
      .find(button => button.textContent?.includes('保存映射'))
    expect(saveButton).toBeDefined()
    saveButton?.click()
    await vi.waitFor(() => expect(updateModel).toHaveBeenCalledTimes(1))

    expect(updateModel).toHaveBeenCalledWith('provider-1', 'model-sol', {
      provider_model_mappings: [{
        name: 'gpt-5.6-luna',
        priority: 1,
        endpoint_ids: [endpoint.id],
        operations: ['compact'],
      }],
    })
  })
})
