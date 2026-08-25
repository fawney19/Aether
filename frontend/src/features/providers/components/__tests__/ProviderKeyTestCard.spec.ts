import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import ProviderKeyTestCard from '@/features/providers/components/ProviderKeyTestCard.vue'
import type { EndpointAPIKey } from '@/api/endpoints/types'
import type { Model } from '@/api/endpoints/types/model'

const modelTestMocks = vi.hoisted(() => ({
  startTest: vi.fn(),
  testing: { value: false },
  testResult: { value: null },
}))

vi.mock('@/composables/useModelTest', () => ({
  useModelTest: () => modelTestMocks,
}))

vi.mock('@/api/endpoints/models', () => ({
  getProviderModels: vi.fn().mockResolvedValue([]),
}))

vi.mock('@/components/ui/select.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'SelectStub',
      props: { modelValue: { type: String, default: '' } },
      emits: ['update:modelValue'],
      setup(props, { slots, emit }) {
        return () => h('div', {
          'data-testid': 'provider-key-test-model-select',
          'data-value': props.modelValue,
          onClick: () => emit('update:modelValue', 'gpt-4.1'),
        }, slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/select-trigger.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ name: 'SelectTriggerStub', setup: (_, { slots }) => () => h('button', slots.default?.()) }) }
})

vi.mock('@/components/ui/select-value.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ name: 'SelectValueStub', setup: () => () => h('span', '选择测试模型') }) }
})

vi.mock('@/components/ui/select-content.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ name: 'SelectContentStub', setup: (_, { slots }) => () => h('div', slots.default?.()) }) }
})

vi.mock('@/components/ui/select-item.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'SelectItemStub',
      props: { value: { type: String, required: true } },
      setup(props, { slots }) {
        return () => h('div', { 'data-model-option': props.value }, slots.default?.())
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function sampleKey(): EndpointAPIKey {
  return {
    id: 'key-1',
    provider_id: 'provider-1',
    name: 'codex-1',
  } as EndpointAPIKey
}

function sampleModel(name: string, extras: Partial<Model> = {}): Model {
  return {
    id: `model-${name}`,
    provider_id: 'provider-1',
    global_model_id: `global-${name}`,
    provider_model_name: name,
    is_active: true,
    is_available: true,
    created_at: '2026-08-24T00:00:00Z',
    updated_at: '2026-08-24T00:00:00Z',
    global_model_name: name,
    global_model_display_name: name,
    ...extras,
  }
}

function mountCard(models: Model[]) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const Host = defineComponent({
    setup() {
      return () => h(ProviderKeyTestCard, {
        providerId: 'provider-1',
        apiKey: sampleKey(),
        models,
      })
    },
  })
  const app = createApp(Host)
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
  modelTestMocks.startTest.mockReset()
})

describe('ProviderKeyTestCard model dropdown', () => {
  it('renders provider models in a select instead of a free-text input', async () => {
    const root = mountCard([
      sampleModel('gpt-4.1'),
      sampleModel('gpt-5.4'),
      sampleModel('o3'),
    ])
    await nextTick()

    expect(root.querySelector('[data-testid="provider-key-test-model-select"]')).not.toBeNull()
    expect(root.querySelector('[data-testid="provider-key-test-model-input"]')).toBeNull()
    expect([...root.querySelectorAll('[data-model-option]')].map((node) => node.getAttribute('data-model-option')))
      .toEqual(['gpt-4.1', 'gpt-5.4', 'o3'])
  })

  it('prefers gpt-5.4 as the default test model', async () => {
    const root = mountCard([
      sampleModel('gpt-4.1'),
      sampleModel('gpt-5.4'),
    ])
    await nextTick()

    expect(root.querySelector('[data-testid="provider-key-test-model-select"]')?.getAttribute('data-value'))
      .toBe('gpt-5.4')
  })
})
