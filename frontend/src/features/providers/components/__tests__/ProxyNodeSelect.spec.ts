import { describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import ProxyNodeSelect from '@/features/providers/components/ProxyNodeSelect.vue'
import { createI18n } from '@/i18n'

const proxyNodesStore = vi.hoisted(() => ({
  loading: false,
  nodes: [],
  onlineNodes: [],
  ensureLoaded: vi.fn(() => Promise.resolve()),
}))

vi.mock('@/stores/proxy-nodes', () => ({
  useProxyNodesStore: () => proxyNodesStore,
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = (name: string) => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h('div', slots.default?.())
    },
  })

  return {
    Select: passthrough('SelectStub'),
    SelectTrigger: passthrough('SelectTriggerStub'),
    SelectValue: passthrough('SelectValueStub'),
    SelectContent: passthrough('SelectContentStub'),
    SelectItem: passthrough('SelectItemStub'),
  }
})

describe('ProxyNodeSelect', () => {
  it('loads proxy nodes when mounted', () => {
    const root = document.createElement('div')
    const app = createApp(defineComponent({
      setup() {
        return () => h(ProxyNodeSelect, { modelValue: '' })
      },
    }))

    app.use(createI18n())
    app.mount(root)

    expect(proxyNodesStore.ensureLoaded).toHaveBeenCalledTimes(1)

    app.unmount()
  })
})