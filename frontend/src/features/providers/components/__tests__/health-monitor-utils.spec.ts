import { describe, expect, it } from 'vitest'
import {
  formatAvailability,
  formatTimelineTooltip,
  getHealthBadgeVariant,
  getHealthLabel,
  getTimelineColor,
  summarizeHealthMonitorItems,
} from '../health-monitor-utils'

describe('health monitor SLA outcome semantics', () => {
  it('does not mark user-error-only traffic as a service outage', () => {
    const item = {
      total_attempts: 4,
      sla_eligible_count: 0,
      success_count: 0,
      failed_count: 0,
      service_error_count: 0,
      user_error_count: 4,
      success_rate: 1,
    }

    expect(getHealthLabel(item)).toBe('暂无 SLA 样本')
    expect(getHealthBadgeVariant(item)).toBe('outline')
    expect(formatAvailability(item)).toBe('-')
    expect(summarizeHealthMonitorItems([item])).toMatchObject({
      unhealthy: 0,
      empty: 1,
      attempts: 4,
    })
  })

  it('shows service and user errors separately in timeline details', () => {
    const tooltip = formatTimelineTooltip({
      status: 'warning',
      timeRangeStart: '2026-08-13T00:00:00Z',
      timeRangeEnd: '2026-08-13T01:00:00Z',
      metrics: {
        total_attempts: 12,
        sla_eligible_count: 10,
        success_count: 9,
        service_error_count: 1,
        user_error_count: 2,
        success_rate: 0.9,
      },
    })

    expect(tooltip).toContain('总请求/SLA样本/成功/服务错误/用户错误/SLA可用率/状态')
    expect(tooltip).toContain('12 次/10 次/9 次/1 次/2 次/90.00%/波动')
  })

  it('does not show timeline availability when all attempts are user errors', () => {
    const tooltip = formatTimelineTooltip({
      status: 'user_error',
      timeRangeStart: '2026-08-13T00:00:00Z',
      timeRangeEnd: '2026-08-13T01:00:00Z',
      metrics: {
        total_attempts: 4,
        sla_eligible_count: 0,
        success_count: 0,
        service_error_count: 0,
        user_error_count: 4,
        success_rate: 1,
      },
    })

    expect(tooltip).toContain('4 次/0 次/0 次/0 次/4 次/-/')
  })

  it('uses a neutral timeline color for user errors', () => {
    expect(getTimelineColor('user_error')).toContain('bg-slate-400')
  })
})
