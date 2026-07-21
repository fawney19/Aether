import { createApp, nextTick } from 'vue'
import { describe, expect, it, vi } from 'vitest'

import PoolKeyBatchEditDialog from '../PoolKeyBatchEditDialog.vue'

vi.mock('@/api/endpoints/models', () => ({
  getProviderModels: vi.fn().mockResolvedValue([]),
}))

vi.mock('@/api/endpoints/pool', () => ({
  batchUpdatePoolKeys: vi.fn(),
}))

vi.mock('@/features/providers/composables/useUpstreamModelsCache', () => ({
  useUpstreamModelsCache: () => ({
    fetchModelsForKeys: vi.fn().mockResolvedValue({ models: [] }),
  }),
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({ confirm: vi.fn().mockResolvedValue(true) }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  }),
}))

describe('PoolKeyBatchEditDialog', () => {
  it('uses the same automatic model discovery language as the single-key editor', async () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(PoolKeyBatchEditDialog, {
      open: true,
      providerId: 'provider-1',
      providerName: 'Google API',
      keyIds: ['key-1', 'key-2'],
      availableApiFormats: ['gemini:generate_content'],
    })

    app.mount(root)
    await nextTick()

    const applyLabel = [...document.body.querySelectorAll('label')]
      .find(label => label.textContent?.includes('应用自动获取设置'))
    const applyCheckbox = applyLabel?.querySelector<HTMLInputElement>('input[type="checkbox"]')
    expect(applyCheckbox).toBeTruthy()
    if (applyCheckbox) {
      applyCheckbox.checked = true
      applyCheckbox.dispatchEvent(new Event('change', { bubbles: true }))
    }
    await nextTick()

    document.body.querySelector<HTMLButtonElement>('[role="switch"]')?.click()
    await nextTick()

    const text = document.body.textContent || ''
    expect(text).toContain('自动获取上游可用模型')
    expect(text).toContain('包含规则')
    expect(text).toContain('排除规则')
    expect(text).toContain('可用模型范围')
    expect(text).not.toContain('模型权限')
    expect(text).not.toContain('自动发现')

    app.unmount()
    root.remove()
  })
})
