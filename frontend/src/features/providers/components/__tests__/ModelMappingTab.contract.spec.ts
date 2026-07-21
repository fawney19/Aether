import { describe, expect, it } from 'vitest'
import { createSSRApp, h } from 'vue'
import { renderToString } from '@vue/server-renderer'

import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import ModelMappingTab from '../provider-tabs/ModelMappingTab.vue'

const provider = {
  id: 'provider-demo',
  name: 'Demo Provider',
  provider_type: 'custom',
  is_active: true,
  active_keys: 0,
  api_formats: [],
} as ProviderWithEndpointsSummary

describe('ModelMappingTab response contracts', () => {
  it('keeps the module visible when a legacy or malformed preview reaches the component', async () => {
    const app = createSSRApp({
      render: () => h(ModelMappingTab, {
        provider,
        models: [],
        endpoints: [],
        providerKeys: [],
        mappingPreview: {
          message: '演示模式：该接口暂未模拟',
          demo_mode: true,
        },
        loading: false,
      }),
    })

    const html = await renderToString(app)

    expect(html).toContain('模型映射')
    expect(html).toContain('暂无模型映射')
  })
})
