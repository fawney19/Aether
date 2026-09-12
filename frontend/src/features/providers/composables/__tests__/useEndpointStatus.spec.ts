import { describe, expect, it } from 'vitest'

import type { EndpointHealthDetail } from '@/api/endpoints'
import {
  getEndpointDotColor,
  getEndpointHealthLabel,
  getEndpointHealthBarWidth,
  getEndpointTooltip,
} from '../useEndpointStatus'

const availableEndpoint: EndpointHealthDetail = {
  api_format: 'openai:chat',
  health_score: 1,
  is_active: true,
  total_keys: 1,
  active_keys: 1,
}

describe('endpoint health display', () => {
  it.each([null, undefined, Number.NaN, Number.POSITIVE_INFINITY])(
    'renders missing or invalid health %s as unknown rather than a percentage',
    (healthScore) => {
      const endpoint = { ...availableEndpoint, health_score: healthScore } as EndpointHealthDetail

      expect(getEndpointHealthLabel(endpoint)).toBe('-')
      expect(getEndpointHealthBarWidth(endpoint)).toBe('100%')
      expect(getEndpointDotColor(endpoint)).toBe('bg-muted-foreground/40')
      expect(getEndpointTooltip(endpoint, 'zh-CN')).toContain('暂无健康数据')
    },
  )

  it.each([
    { is_active: false },
    { active_keys: 0 },
    { active_keys: 0, total_keys: 0 },
  ])('does not show a health percentage for unavailable endpoint %s', (overrides) => {
    const endpoint = { ...availableEndpoint, ...overrides }

    expect(getEndpointHealthLabel(endpoint)).toBe('-')
    expect(getEndpointHealthBarWidth(endpoint)).toBe('100%')
    expect(getEndpointDotColor(endpoint)).toBe('bg-muted-foreground/40')
  })

  it.each([
    [0, '0%', '5%', 'bg-red-500'],
    [0.2, '20%', '20%', 'bg-red-500'],
    [0.5, '50%', '50%', 'bg-amber-500'],
    [0.8, '80%', '80%', 'bg-green-500'],
    [1, '100%', '100%', 'bg-green-500'],
    [-0.2, '0%', '5%', 'bg-red-500'],
    [1.2, '100%', '100%', 'bg-green-500'],
  ] as const)('renders score %s consistently across label, bar and tooltip', (score, label, width, color) => {
    const endpoint = { ...availableEndpoint, health_score: score }

    expect(getEndpointHealthLabel(endpoint)).toBe(label)
    expect(getEndpointHealthBarWidth(endpoint)).toBe(width)
    expect(getEndpointDotColor(endpoint)).toBe(color)
    expect(getEndpointTooltip(endpoint, 'zh-CN')).toContain(`健康度 ${label}`)
  })
})

describe('getEndpointTooltip', () => {
  it('formats the internal Codex Live id at the user-visible tooltip boundary', () => {
    const tooltip = getEndpointTooltip({
      api_format: 'codex:live',
      health_score: 1,
      is_active: true,
      total_keys: 1,
      active_keys: 1,
    }, 'zh-CN')

    expect(tooltip).toBe('OpenAI Live: 健康度 100%')
    expect(tooltip).not.toContain('codex:live')
  })
})
