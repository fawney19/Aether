import { describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import ProviderKeyIdentityBlock from '@/features/providers/components/ProviderKeyIdentityBlock.vue'
import type { EndpointAPIKey } from '@/api/endpoints'
import { createI18n } from '@/i18n'

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    Copy: Icon,
    Download: Icon,
    RefreshCw: Icon,
    ShieldX: Icon,
  }
})

function createProviderKey(overrides: Partial<EndpointAPIKey> = {}): EndpointAPIKey {
  return {
    id: 'provider-key-1',
    provider_id: 'provider-1',
    api_formats: ['openai:chat'],
    api_key_masked: 'sk-***',
    auth_type: 'oauth',
    name: 'Primary key',
    internal_priority: 10,
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
    ...overrides,
  }
}

function mount(props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup() {
      return () => h(ProviderKeyIdentityBlock, props)
    },
  }))
  app.use(createI18n())
  app.mount(root)

  return {
    root,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

describe('ProviderKeyIdentityBlock', () => {
  it('renders identity badges and emits copy/download/refresh actions', () => {
    const onCopyName = vi.fn()
    const onDownloadCredential = vi.fn()
    const onRefreshOAuth = vi.fn()

    const { root, unmount } = mount({
      apiKey: createProviderKey({
        oauth_temporary: true,
      }),
      maskedSecretLabel: 'oauth:***',
      oauthPlanLabel: 'Team',
      oauthPlanClass: 'text-purple-600',
      oauthOrgBadge: { label: 'org:demo', title: 'org_id: demo' },
      kiroSubscriptionLabel: 'Pro',
      kiroSubscriptionClass: 'text-blue-600',
      canExportCredential: true,
      showOAuthRefreshControl: true,
      oauthStatus: { text: '12m', isExpired: false, isExpiringSoon: true, isInvalid: false },
      oauthStatusTitle: 'OAuth expires soon',
      oauthRefreshButtonTitle: 'Refresh OAuth',
      canRefreshCredential: true,
      onCopyName,
      onDownloadCredential,
      onRefreshOAuth,
    })

    expect(root.querySelector('[data-testid="provider-key-name"]')?.textContent).toContain('Primary key')
    expect(root.querySelector('[data-testid="provider-key-oauth-plan"]')?.textContent).toContain('Team')
    expect(root.querySelector('[data-testid="provider-key-oauth-org"]')?.textContent).toContain('org:demo')
    expect(root.querySelector('[data-testid="provider-key-kiro-plan"]')?.textContent).toContain('Pro')
    expect(root.querySelector('[data-testid="provider-key-oauth-status"]')?.textContent).toContain('12m')
    expect(root.querySelector('[data-testid="provider-key-temporary"]')).toBeTruthy()

    ;(root.querySelector('[data-testid="provider-key-name"]') as HTMLElement).click()
    ;(root.querySelector('button[title="下载 OAuth 授权文件"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="Refresh OAuth"]') as HTMLButtonElement).click()

    expect(onCopyName).toHaveBeenCalledWith('Primary key')
    expect(onDownloadCredential).toHaveBeenCalledTimes(1)
    expect(onRefreshOAuth).toHaveBeenCalledTimes(1)

    unmount()
  })

  it('renders account block and inactive account states', () => {
    const onCopyFullKey = vi.fn()
    const onClearOAuthInvalid = vi.fn()

    const { root, unmount } = mount({
      apiKey: createProviderKey({ name: '' }),
      maskedSecretLabel: 'sk-***',
      canExportCredential: false,
      showOAuthRefreshControl: true,
      accountLevelBlock: true,
      oauthStatusTitle: 'Account blocked',
      antigravityInactive: true,
      onCopyFullKey,
      onClearOAuthInvalid,
    })

    expect(root.querySelector('[data-testid="provider-key-name"]')?.textContent).toContain('未命名密钥')
    expect(root.querySelector('[data-testid="provider-key-account-block"]')?.textContent).toContain('账号异常')
    expect(root.querySelector('[data-testid="provider-key-antigravity-inactive"]')?.textContent).toContain('账号未激活')

    ;(root.querySelector('button[title="复制密钥"]') as HTMLButtonElement).click()
    ;(root.querySelector('button[title="清除异常标记（确认账号已完成验证后使用）"]') as HTMLButtonElement).click()

    expect(onCopyFullKey).toHaveBeenCalledTimes(1)
    expect(onClearOAuthInvalid).toHaveBeenCalledTimes(1)

    unmount()
  })
})
