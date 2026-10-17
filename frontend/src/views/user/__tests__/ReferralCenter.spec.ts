import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import type { ReferralDashboardResponse } from '@/api/referrals'
import { setI18nLocale } from '@/i18n'

const mocks = vi.hoisted(() => ({ getMyReferral: vi.fn(), copy: vi.fn(), error: vi.fn() }))
vi.mock('@/api/referrals', () => ({ referralApi: { getMyReferral: mocks.getMyReferral } }))
vi.mock('@/composables/useClipboard', () => ({ useClipboard: () => ({ copyToClipboard: mocks.copy }) }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ error: mocks.error }) }))

import ReferralCenter from '../ReferralCenter.vue'

const mounted: Array<{ app: App; root: HTMLElement }> = []
const data: ReferralDashboardResponse = {
  invite_code: 'TEST123',
  invitation_link: '/register?invite=TEST123',
  summary: { total_invites: 0, effective_invites: 0, paid_reward_usd: 0, pending_reward_usd: 0, reversed_reward_usd: 0 },
}

async function settle() {
  for (let index = 0; index < 6; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function mountPage() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(ReferralCenter)
  app.mount(root)
  mounted.push({ app, root })
  await settle()
  return root
}

function button(root: HTMLElement, text: string) {
  const result = [...root.querySelectorAll('button')].find(item => item.textContent?.includes(text))
  if (!result) throw new Error(`Missing button: ${text}`)
  return result
}

beforeEach(() => {
  vi.resetAllMocks()
  mocks.getMyReferral.mockResolvedValue(structuredClone(data))
  mocks.copy.mockResolvedValue(true)
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ReferralCenter', () => {
  it('displays and copies a complete shareable link, with a separate code action', async () => {
    const root = await mountPage()
    const expected = new URL(data.invitation_link, window.location.origin).href
    expect(root.querySelector('input')?.value).toBe(expected)
    const input = root.querySelector('input')!
    input.focus()
    expect(input.selectionStart).toBe(0)
    expect(input.selectionEnd).toBe(expected.length)
    button(root, '复制邀请链接').click()
    expect(mocks.copy).toHaveBeenLastCalledWith(expected)
    button(root, '复制邀请码').click()
    expect(mocks.copy).toHaveBeenLastCalledWith('TEST123')
    expect(root.textContent).toContain('还没有邀请记录')
  })

  it('preserves a configured absolute invite URL and reports issued and reversed amounts separately', async () => {
    mocks.getMyReferral.mockResolvedValue({ ...data,
      invitation_link: 'https://gateway.example.com/register?invite=TEST123',
      summary: { total_invites: 3, effective_invites: 1, paid_reward_usd: 20, pending_reward_usd: 5, reversed_reward_usd: 4 },
    })
    const root = await mountPage()
    expect(root.querySelector('input')?.value).toBe('https://gateway.example.com/register?invite=TEST123')
    expect(root.textContent).toContain('$20.00')
    expect(root.textContent).toContain('$5.00')
    expect(root.textContent).toContain('$4.00')
    expect(root.textContent).toContain('已完成首次付款的受邀用户')
    expect(root.textContent).not.toContain('还没有邀请记录')
  })

  it('allows retry after loading fails without showing fabricated zero statistics', async () => {
    mocks.getMyReferral.mockRejectedValueOnce(new Error('unavailable'))
    const root = await mountPage()
    expect(root.textContent).toContain('邀请数据暂不可用')
    expect(root.querySelector('input')).toBeNull()
    expect(root.textContent).not.toContain('$0.00')
    button(root, '重新加载').click()
    await settle()
    expect(mocks.getMyReferral).toHaveBeenCalledTimes(2)
    expect(root.querySelector('input')?.value).toContain('TEST123')
  })

  it('keeps share actions translated in English', async () => {
    setI18nLocale('en-US')
    const root = await mountPage()
    expect(button(root, 'Copy invite link')).toBeTruthy()
    expect(button(root, 'Copy invite code')).toBeTruthy()
    expect(root.textContent).toContain('Invite friends')
  })

  it('translates the retry action after an error', async () => {
    setI18nLocale('en-US')
    mocks.getMyReferral.mockRejectedValueOnce(new Error('unavailable'))
    const root = await mountPage()
    expect(button(root, 'Reload')).toBeTruthy()
    expect(root.textContent).toContain('Referral data is currently unavailable')
  })
})
