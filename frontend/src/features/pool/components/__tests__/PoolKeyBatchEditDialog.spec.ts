import { createApp, nextTick } from 'vue'
import { describe, expect, it, vi } from 'vitest'

import PoolKeyBatchEditDialog from '../PoolKeyBatchEditDialog.vue'

const batchUpdatePoolKeys = vi.hoisted(() => vi.fn().mockResolvedValue({
  affected: 2,
  message: 'updated',
}))

vi.mock('@/api/endpoints/models', () => ({
  getProviderModels: vi.fn().mockResolvedValue([]),
}))

vi.mock('@/api/endpoints/pool', () => ({
  batchUpdatePoolKeys,
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
  it('opts in independently to a queue-only update through its label', async () => {
    batchUpdatePoolKeys.mockClear()
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(PoolKeyBatchEditDialog, {
      open: true,
      providerId: 'provider-1',
      keyIds: ['key-1', 'key-2'],
      availableApiFormats: ['openai:chat'],
    })

    try {
      app.mount(root)
      await nextTick()

      const queue = document.body.querySelector<HTMLButtonElement>('[role="switch"][aria-label="并发满时排队"]')
      const applyLabel = [...document.body.querySelectorAll('label')]
        .find(label => label.textContent?.includes('修改并发排队设置'))
      expect(queue?.getAttribute('aria-checked')).toBe('true')
      expect(queue?.matches(':disabled')).toBe(true)
      applyLabel?.click()
      await nextTick()
      expect(queue?.matches(':disabled')).toBe(false)
      queue?.click()
      await nextTick()
      expect(queue?.getAttribute('aria-checked')).toBe('false')

      const submit = [...document.body.querySelectorAll<HTMLButtonElement>('button')]
        .find(button => button.textContent?.includes('应用到 2 个密钥'))
      expect(submit).toBeDefined()
      submit?.click()
      await nextTick()
      await nextTick()
      expect(batchUpdatePoolKeys).toHaveBeenCalledWith('provider-1', {
        key_ids: ['key-1', 'key-2'],
        patch: { concurrency_queue_enabled: false },
      })
    } finally {
      app.unmount()
      root.remove()
    }
  })

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

    applyLabel?.parentElement?.querySelector<HTMLButtonElement>('[role="switch"]')?.click()
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
