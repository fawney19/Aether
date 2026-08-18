import { describe, expect, it } from 'vitest'
import {
  resolveServiceErrorCount,
  resolveServiceErrorRate,
  resolveSlaEligibleCount,
  resolveSlaSuccessRate,
  resolveUserErrorRate,
} from '../outcomeMetrics'

describe('request outcome metrics', () => {
  it('uses explicit service and user outcome fields without counting user errors in SLA', () => {
    const metrics = {
      total_requests: 110,
      sla_eligible_count: 100,
      success_count: 98,
      service_error_count: 2,
      user_error_count: 10,
      success_rate: 98,
      service_error_rate: 2,
      user_error_rate: 9.09,
    }

    expect(resolveSlaEligibleCount(metrics)).toBe(100)
    expect(resolveSlaSuccessRate(metrics)).toBe(98)
    expect(resolveServiceErrorCount(metrics)).toBe(2)
    expect(resolveServiceErrorRate(metrics)).toBe(2)
    expect(resolveUserErrorRate(metrics)).toBe(9.09)
  })

  it('derives rates from counts using different SLA and request denominators', () => {
    const metrics = {
      request_count: 110,
      success_count: 98,
      service_error_count: 2,
      user_error_count: 10,
    }

    expect(resolveSlaEligibleCount(metrics)).toBe(100)
    expect(resolveSlaSuccessRate(metrics)).toBe(98)
    expect(resolveServiceErrorRate(metrics)).toBe(2)
    expect(resolveUserErrorRate(metrics)).toBeCloseTo(9.0909)
  })

  it('supports legacy error fields as service-error compatibility aliases', () => {
    expect(resolveServiceErrorCount({ error_requests: 4 })).toBe(4)
    expect(resolveServiceErrorRate({ error_rate: 2.5 })).toBe(2.5)
    expect(resolveServiceErrorRate({ success_rate: 97.5 })).toBe(2.5)
  })

  it('treats an explicit zero SLA denominator as no sample before compatibility rates', () => {
    const userErrorOnly = {
      total_requests: 4,
      sla_eligible_count: 0,
      success_count: 0,
      service_error_count: 0,
      user_error_count: 4,
      success_rate: 100,
      service_error_rate: 0,
      error_rate: 0,
    }

    expect(resolveSlaSuccessRate(userErrorOnly)).toBeNull()
    expect(resolveServiceErrorRate(userErrorOnly)).toBeNull()
  })

  it('supports ratio-based health metrics', () => {
    const metrics = {
      total_attempts: 12,
      sla_eligible_count: 10,
      success_count: 9,
      service_error_count: 1,
      user_error_count: 2,
    }
    expect(resolveSlaSuccessRate(metrics, 1)).toBe(0.9)
    expect(resolveServiceErrorRate(metrics, 1)).toBe(0.1)
    expect(resolveUserErrorRate(metrics, 1)).toBeCloseTo(1 / 6)
  })
})
