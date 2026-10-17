import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
const mocks = vi.hoisted(() => ({ settings: vi.fn() }))
vi.mock('@/api/auth', () => ({ authApi: { getRegistrationSettings: mocks.settings } }))
import BasicConfigSection from '../BasicConfigSection.vue'
const mounted: Array<{ app: App; root: HTMLElement }> = []
const props = {
  defaultUserInitialGiftUsd: 0, rateLimitPerMinute: 60, enableRegistration: true, passwordPolicyLevel: 'weak',
  turnstileEnabled: false, turnstileSiteKey: null, turnstileSecretKey: '', turnstileSecretConfigured: false, turnstileAllowedHostnamesStr: '',
  referralEnabled: true, referralRewardMode: 'headcount', referralRechargePercent: 5, referralHeadcountAmountUsd: 10, referralHeadcountTrigger: 'email_verified',
  registrationPrivacyPolicyEnabled: false, registrationPrivacyPolicyFormat: 'markdown', registrationPrivacyPolicyContent: '', registrationPrivacyPolicyVersion: '1',
  autoDeleteExpiredKeys: false, enableFormatConversion: false, loading: false, hasChanges: false,
}
async function settle() { for (let i = 0; i < 6; i++) { await Promise.resolve(); await nextTick() } }
async function mount() { const root = document.createElement('div'); document.body.appendChild(root); const app = createApp(BasicConfigSection, props); app.mount(root); mounted.push({ app, root }); await settle(); return root }
beforeEach(() => vi.resetAllMocks())
afterEach(() => { for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() } })
describe('BasicConfigSection referral email dependency', () => {
  it('clearly explains disabled registration verification without automatically enabling it', async () => {
    mocks.settings.mockResolvedValue({ require_email_verification: false })
    const root = await mount()
    expect(root.textContent).toContain('注册邮箱验证未启用')
    expect(root.textContent).toContain('不会自动开启')
    expect(mocks.settings).toHaveBeenCalledOnce()
    expect(root.querySelector('[role="alert"]')).not.toBeNull()
  })
  it('does not report the dependency as unmet after verification is enabled', async () => {
    mocks.settings.mockResolvedValue({ require_email_verification: true })
    const root = await mount()
    expect(root.querySelector('[role="alert"]')).toBeNull()
    expect(root.textContent).toContain('先在“邮件设置”中启用注册邮箱验证')
  })
  it('reports an unavailable configuration instead of claiming verification is enabled', async () => {
    mocks.settings.mockRejectedValue(new Error('offline'))
    const root = await mount()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('暂时无法核实邮箱验证配置')
  })
})
