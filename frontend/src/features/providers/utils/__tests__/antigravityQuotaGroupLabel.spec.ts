import { describe, expect, it } from 'vitest'

import { messages } from '@/i18n/messages'
import { resolveAntigravityQuotaGroupLabel } from '@/features/providers/utils/antigravityQuota'

describe('resolveAntigravityQuotaGroupLabel', () => {
  it('uses the localized compact weekly label instead of the upstream display name', () => {
    const window = {
      code: 'group:0:gemini-weekly',
      label: 'Gemini Models · Weekly Limit Remaining',
      quota_group_label: 'Gemini models',
      bucket_id: 'gemini-weekly',
      window: 'weekly',
    }

    expect(resolveAntigravityQuotaGroupLabel(
      window,
      key => messages['zh-CN'][key],
    )).toBe('Gemini Models · 周')
    expect(resolveAntigravityQuotaGroupLabel(
      window,
      key => messages['en-US'][key],
    )).toBe('Gemini Models · Weekly')
  })

  it('normalizes the legacy Claude family name and detects a five-hour bucket', () => {
    const window = {
      code: 'group:1:3p-5h',
      label: 'Claude & ChatGPT · 5 hour',
      quota_group_label: 'Claude & ChatGPT',
      bucket_id: '3p-5h',
    }

    expect(resolveAntigravityQuotaGroupLabel(
      window,
      key => messages['zh-CN'][key],
    )).toBe('Claude and GPT models · 5小时')
    expect(resolveAntigravityQuotaGroupLabel(
      window,
      key => messages['en-US'][key],
    )).toBe('Claude and GPT models · 5 Hours')
  })
})
