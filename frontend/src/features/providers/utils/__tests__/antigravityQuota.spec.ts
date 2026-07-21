import { describe, expect, it } from 'vitest'

import {
  compareAntigravityQuotaItems,
  dedupeAntigravityQuotaItemsByLabel,
  resolveAntigravityQuotaLabel,
} from '@/features/providers/utils/antigravityQuota'

describe('antigravityQuota', () => {
  it('uses canonical labels from model ids before stale upstream labels', () => {
    const opaqueDisplayIndex = { value: 1 }

    expect(
      resolveAntigravityQuotaLabel(
        'gemini-3-flash-agent',
        'Gemini 3.5 Flash (High)',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.5 Flash (High)')
    expect(
      resolveAntigravityQuotaLabel(
        'gemini-3.5-flash-low',
        'Gemini 3.5 Flash (Medium)',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.5 Flash (Medium)')
    expect(
      resolveAntigravityQuotaLabel(
        'gemini-3.5-flash-extra-low',
        'Gemini 3.5 Flash (Low)',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.5 Flash (Low)')
    expect(
      resolveAntigravityQuotaLabel(
        'gemini-pro-agent',
        'gemini-pro-agent',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.1 Pro (High)')
    expect(
      resolveAntigravityQuotaLabel(
        'gemini-3.1-pro-high',
        'gemini-3.1-pro-high',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.1 Pro (High)')
    expect(
      resolveAntigravityQuotaLabel(
        'gemini-2.5-flash',
        'Gemini 3.1 Flash Lite',
        opaqueDisplayIndex,
      ),
    ).toBe('Gemini 3.1 Flash Lite')
    expect(
      resolveAntigravityQuotaLabel(
        'claude-sonnet-4-6',
        'Claude Sonnet 4.6 (Thinking)',
        opaqueDisplayIndex,
      ),
    ).toBe('Claude Sonnet 4.6 (Thinking)')
    expect(
      resolveAntigravityQuotaLabel(
        'RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb',
        'RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb',
        opaqueDisplayIndex,
      ),
    ).toBe('Key-1')
  })

  it('keeps latest Gemini and Claude quota buckets before low-priority buckets', () => {
    const items = [
      {
        model: 'tab_flash_lite_preview',
        label: 'Tab Flash Lite Preview',
        remainingPercent: 1,
        resetSeconds: 10,
      },
      {
        model: 'gemini-3.5-flash-extra-low',
        label: 'Gemini 3.5 Flash (Low)',
        remainingPercent: 95,
        resetSeconds: null,
      },
      {
        model: 'gemini-3.5-flash-low',
        label: 'Gemini 3.5 Flash (Medium)',
        remainingPercent: 90,
        resetSeconds: null,
      },
      {
        model: 'gemini-3-flash-agent',
        label: 'Gemini 3.5 Flash (High)',
        remainingPercent: 80,
        resetSeconds: null,
      },
      {
        model: 'gemini-pro-agent',
        label: 'Gemini 3.1 Pro (High)',
        remainingPercent: 70,
        resetSeconds: null,
      },
      {
        model: 'chat_20706',
        label: 'chat_20706',
        remainingPercent: 0,
        resetSeconds: 1,
      },
      {
        model: 'claude-opus-4-6-thinking',
        label: 'Claude Opus 4.6 (Thinking)',
        remainingPercent: 100,
        resetSeconds: null,
      },
      {
        model: 'gemini-2.5-flash-lite',
        label: 'Gemini 3.1 Flash Lite',
        remainingPercent: 5,
        resetSeconds: 5,
      },
    ].sort(compareAntigravityQuotaItems)

    expect(items.map(item => item.model)).toEqual([
      'claude-opus-4-6-thinking',
      'gemini-pro-agent',
      'gemini-3-flash-agent',
      'gemini-3.5-flash-low',
      'gemini-3.5-flash-extra-low',
      'gemini-2.5-flash-lite',
      'tab_flash_lite_preview',
      'chat_20706',
    ])
  })

  it('deduplicates display labels while keeping the current preferred quota bucket', () => {
    const items = dedupeAntigravityQuotaItemsByLabel([
      {
        model: 'gemini-3.1-pro-high',
        label: 'Gemini 3.1 Pro (High)',
        remainingPercent: 95,
        resetSeconds: null,
      },
      {
        model: 'gemini-pro-agent',
        label: 'Gemini 3.1 Pro (High)',
        remainingPercent: 40,
        resetSeconds: null,
      },
      {
        model: 'claude-sonnet-4-6-thinking',
        label: 'Claude Sonnet 4.6 (Thinking)',
        remainingPercent: 95,
        resetSeconds: null,
      },
      {
        model: 'claude-sonnet-4-6',
        label: 'Claude Sonnet 4.6 (Thinking)',
        remainingPercent: 40,
        resetSeconds: null,
      },
      {
        model: 'gemini-3-flash-agent',
        label: 'Gemini 3.5 Flash (High)',
        remainingPercent: 80,
        resetSeconds: null,
      },
    ])

    expect(items.map(item => item.model)).toEqual([
      'claude-sonnet-4-6',
      'gemini-pro-agent',
      'gemini-3-flash-agent',
    ])
  })
})
