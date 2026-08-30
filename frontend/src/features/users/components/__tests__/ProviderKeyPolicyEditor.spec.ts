import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref } from 'vue'

import { getProviderKeys } from '@/api/endpoints/keys'
import type { EndpointAPIKey } from '@/api/endpoints/types'
import { createI18n } from '@/i18n'
import ProviderKeyPolicyEditor from '../ProviderKeyPolicyEditor.vue'

vi.mock('@/api/endpoints/keys', () => ({
  getProviderKeys: vi.fn(),
}))

function sampleKey(id: string, name: string, isActive: boolean): EndpointAPIKey {
  return {
    id,
    provider_id: 'provider-tiered',
    api_formats: ['claude:messages'],
    api_key_masked: `sk-***${id.slice(-4)}`,
    auth_type: 'api_key',
    name,
    internal_priority: 1,
    cache_ttl_minutes: 0,
    max_probe_interval_minutes: 5,
    health_score: 1,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 1,
    avg_response_time_ms: 0,
    is_active: isActive,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  }
}

async function flushAsyncState(): Promise<void> {
  await Promise.resolve()
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

function mountEditor(initialPolicies: Record<string, string[]>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const policies = ref(initialPolicies)
  const updates: Array<Record<string, string[]>> = []
  const Probe = defineComponent({
    setup() {
      return () => h(ProviderKeyPolicyEditor, {
        selectedProviderIds: ['provider-tiered'],
        keyPolicies: policies.value,
        providerOptions: [{ value: 'provider-tiered', label: 'tiered-provider' }],
        'onUpdate:keyPolicies': (value: Record<string, string[]>) => {
          updates.push(value)
          policies.value = value
        },
      })
    },
  })
  const app = createApp(Probe)
  app.use(createI18n())
  app.mount(root)

  return {
    root,
    updates,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

describe('ProviderKeyPolicyEditor', () => {
  beforeEach(() => {
    vi.mocked(getProviderKeys).mockReset()
    vi.mocked(getProviderKeys).mockResolvedValue([
      sampleKey('key-basic', '基础套餐', true),
      sampleKey('key-premium', '高级套餐', false),
    ])
  })

  it('keeps an empty allowlist explicit and lets inactive keys be selected', async () => {
    const mounted = mountEditor({ 'provider-tiered': [] })
    await flushAsyncState()

    expect(mounted.root.textContent).toContain('未允许任何 Key')
    expect(mounted.root.textContent).toContain('停用中，启用后生效')
    const inactiveLabel = [...mounted.root.querySelectorAll('label')]
      .find((label) => label.textContent?.includes('高级套餐'))
    const checkbox = inactiveLabel?.querySelector<HTMLInputElement>('input[type="checkbox"]')
    expect(checkbox?.disabled).toBe(false)

    checkbox?.click()
    await nextTick()

    expect(mounted.updates.at(-1)).toEqual({
      'provider-tiered': ['key-premium'],
    })
    mounted.unmount()
  })

  it('restores all keys only when the policy card is removed', async () => {
    const mounted = mountEditor({
      'provider-tiered': ['key-basic', 'key-premium'],
    })
    await flushAsyncState()

    expect(mounted.root.textContent).toContain('仅允许 2 个 Key')
    const removeButton = mounted.root.querySelector<HTMLButtonElement>(
      'button[aria-label^="删除限制并恢复全部 Key"]',
    )
    expect(removeButton).toBeTruthy()

    removeButton?.click()
    await nextTick()

    expect(mounted.updates.at(-1)).toEqual({})
    mounted.unmount()
  })
})
