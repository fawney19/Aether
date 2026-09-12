import { describe, expect, it } from 'vitest'
import { normalizeRoutingFailoverPolicy, validateRoutingFailoverPolicy } from '../utils/routingFailover'
import { createEmptyRoutingGroupConfig, getModelScheduling, normalizeRoutingGroupConfig, upsertModelSchedulingRule } from '../utils/routingPolicy'

describe('routing failover policy', () => {
  it('keeps legacy strategies unlimited with empty global rules', () => {
    const policy = normalizeRoutingGroupConfig({}).default_policy
    expect(policy.max_transfer_count).toBe(0)
    expect(policy.max_transfer_timeout_seconds).toBe(0)
    expect(policy.failover_rules).toEqual({ success_failover_patterns: [], error_stop_patterns: [] })
    expect(validateRoutingFailoverPolicy(policy)).toBeNull()
  })

  it('preserves global limits and rules across model edits without sharing mutable arrays', () => {
    const config = createEmptyRoutingGroupConfig()
    Object.assign(config.default_policy, {
      max_transfer_count: 3,
      max_transfer_timeout_seconds: 90,
      failover_rules: {
        success_failover_patterns: [{ pattern: '(?i)capacity', status_codes: [] }],
        error_stop_patterns: [{ pattern: '', status_codes: [400, 413] }],
      },
    })
    const updated = upsertModelSchedulingRule(config, 'model-a', { priority_mode: 'provider', scheduling_mode: 'fixed_order' })
    const policy = getModelScheduling(updated, 'model-a')
    expect(policy.max_transfer_count).toBe(3)
    expect(policy.max_transfer_timeout_seconds).toBe(90)
    expect(policy.failover_rules).toEqual(config.default_policy.failover_rules)
    expect(validateRoutingFailoverPolicy(policy)).toBeNull()
    policy.failover_rules.error_stop_patterns[0].status_codes.push(422)
    expect(config.default_policy.failover_rules.error_stop_patterns[0].status_codes).toEqual([400, 413])
  })

  it('rejects invalid budgets and ambiguous empty rules before saving', () => {
    const policy = normalizeRoutingFailoverPolicy()
    policy.max_transfer_count = -1
    expect(validateRoutingFailoverPolicy(policy)).toContain('非负整数')
    policy.max_transfer_count = 0
    policy.max_transfer_timeout_seconds = 0.5
    expect(validateRoutingFailoverPolicy(policy)).toContain('非负整数')
    policy.max_transfer_timeout_seconds = 0
    policy.failover_rules.error_stop_patterns.push({ pattern: '', status_codes: [] })
    expect(validateRoutingFailoverPolicy(policy)).toContain('状态码或正则')
    policy.failover_rules.error_stop_patterns[0].status_codes = [200]
    expect(validateRoutingFailoverPolicy(policy)).toContain('400–599')
    policy.failover_rules.error_stop_patterns[0].status_codes = [400]
    expect(validateRoutingFailoverPolicy(policy)).toBeNull()
  })
})
