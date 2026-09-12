import { describe, expect, it } from 'vitest'

import {
  DEFAULT_ROUTING_POLICY_MODEL,
  createEmptyModelPolicy,
  createEmptyRoutingGroupConfig,
  getDefaultModelPolicy,
  getModelScheduling,
  modelSchedulingRuleId,
  normalizeRoutingGroupConfig,
  normalizeStickyKeyAttempts,
  resolveModelKeyPriorityOverride,
  setDefaultPoolPriorityOverrides,
  setDefaultProviderPriorityOverrides,
  setModelKeyPriorityOverridesForFormat,
  upsertModelSchedulingRule,
  upsertModelPolicy,
} from '../utils/routingPolicy'
import { sortCandidateTraces, summarizeRoutingTrace, type RoutingDecisionTrace } from '../utils/routingTrace'

describe('routingPolicy', () => {
  it('normalizes partial configs with stable defaults', () => {
    const config = normalizeRoutingGroupConfig({})

    expect(config.default_policy.priority_mode).toBe('provider')
    expect(config.default_policy.scheduling_mode).toBe('cache_affinity')
    expect(config.default_policy.cancel_on_client_disconnect).toBe(false)
  })

  it('preserves cancellation policy across model scheduling edits', () => {
    const config = createEmptyRoutingGroupConfig()
    config.default_policy.cancel_on_client_disconnect = true
    const updated = upsertModelSchedulingRule(config, 'gpt-5', {
      priority_mode: 'global_key',
      scheduling_mode: 'fixed_order',
    })
    expect(normalizeRoutingGroupConfig(updated).default_policy.cancel_on_client_disconnect).toBe(true)
    expect(getModelScheduling(updated, 'gpt-5').cancel_on_client_disconnect).toBe(true)
    expect(getModelScheduling(updated, 'other-model').cancel_on_client_disconnect).toBe(true)
    expect(createEmptyRoutingGroupConfig().default_policy.cancel_on_client_disconnect).toBe(false)
  })

  it('drops the legacy group model allowlist while normalizing config', () => {
    const config = normalizeRoutingGroupConfig({
      allowed_models: ['legacy-model'],
    } as unknown as Parameters<typeof normalizeRoutingGroupConfig>[0])

    expect(config).not.toHaveProperty('allowed_models')
  })

  it('upserts model policies by model name', () => {
    const config = createEmptyRoutingGroupConfig()
    const next = upsertModelPolicy(config, {
      ...createEmptyModelPolicy('gpt-5'),
      allowed_providers: ['provider-a'],
    })

    expect(next.model_policies).toHaveLength(1)
    expect(next.model_policies[0].allowed_providers).toEqual(['provider-a'])
  })

  it('stores default priority overrides on the wildcard model policy', () => {
    const config = upsertModelPolicy(createEmptyRoutingGroupConfig(), createEmptyModelPolicy('gpt-5'))
    const next = setDefaultProviderPriorityOverrides(config, {
      'provider-a': 0,
      'provider-b': 2,
    })

    const policy = getDefaultModelPolicy(next)
    expect(policy.model).toBe(DEFAULT_ROUTING_POLICY_MODEL)
    expect(next.model_policies.map(item => item.model)).toEqual([DEFAULT_ROUTING_POLICY_MODEL, 'gpt-5'])
    expect(policy.provider_priority_overrides).toEqual({
      'provider-a': 0,
      'provider-b': 2,
    })
  })

  it('stores pool priority overrides separately from key overrides', () => {
    const next = setDefaultPoolPriorityOverrides(createEmptyRoutingGroupConfig(), {
      'provider-pool': 3,
    })

    const policy = getDefaultModelPolicy(next)
    expect(policy.pool_priority_overrides).toEqual({
      'provider-pool': 3,
    })
    expect(policy.key_priority_overrides).toEqual({})
  })

  it('defaults sticky key attempts to 2 and normalizes invalid values', () => {
    expect(createEmptyRoutingGroupConfig().default_policy.sticky_key_attempts).toBe(2)
    expect(normalizeRoutingGroupConfig({}).default_policy.sticky_key_attempts).toBe(2)
    expect(normalizeRoutingGroupConfig({
      default_policy: { ...createEmptyRoutingGroupConfig().default_policy, priority_mode: 'provider', scheduling_mode: 'cache_affinity', keep_priority_on_conversion: false, sticky_key_attempts: 3, enable_cf_heartbeat: false, cyber_continue_failover: false, cancel_on_client_disconnect: false },
    }).default_policy.sticky_key_attempts).toBe(3)
    expect(normalizeStickyKeyAttempts('5')).toBe(5)
    expect(normalizeStickyKeyAttempts(-1)).toBe(2)
    expect(normalizeStickyKeyAttempts('abc')).toBe(2)
    expect(normalizeStickyKeyAttempts(500)).toBe(99)
    expect(getModelScheduling(createEmptyRoutingGroupConfig(), 'gpt-5').sticky_key_attempts).toBe(2)
  })

  it('keeps key priority overrides independent per api format', () => {
    let config = setModelKeyPriorityOverridesForFormat(
      createEmptyRoutingGroupConfig(),
      DEFAULT_ROUTING_POLICY_MODEL,
      'OpenAI:Chat',
      { 'key-a': 0, 'key-b': 1 },
    )
    config = setModelKeyPriorityOverridesForFormat(
      config,
      DEFAULT_ROUTING_POLICY_MODEL,
      'claude:messages',
      { 'key-a': 3 },
    )

    const policy = getDefaultModelPolicy(config)
    expect(policy.key_priority_overrides).toEqual({})
    expect(policy.key_priority_overrides_by_format).toEqual({
      'openai:chat': { 'key-a': 0, 'key-b': 1 },
      'claude:messages': { 'key-a': 3 },
    })
    expect(resolveModelKeyPriorityOverride(config, DEFAULT_ROUTING_POLICY_MODEL, 'openai:chat', 'key-a')).toBe(0)
    expect(resolveModelKeyPriorityOverride(config, DEFAULT_ROUTING_POLICY_MODEL, 'claude:messages', 'key-a')).toBe(3)
    expect(resolveModelKeyPriorityOverride(config, DEFAULT_ROUTING_POLICY_MODEL, 'claude:messages', 'key-b')).toBeUndefined()

    const cleared = setModelKeyPriorityOverridesForFormat(config, DEFAULT_ROUTING_POLICY_MODEL, 'claude:messages', {})
    expect(getDefaultModelPolicy(cleared).key_priority_overrides_by_format).toEqual({
      'openai:chat': { 'key-a': 0, 'key-b': 1 },
    })
  })

  it('falls back to format-agnostic key overrides and normalizes legacy configs', () => {
    const config = normalizeRoutingGroupConfig({
      model_policies: [{
        ...createEmptyModelPolicy('gpt-5'),
        key_priority_overrides: { 'key-a': 7 },
        key_priority_overrides_by_format: { ' OpenAI:Chat ': { 'key-a': 1 } },
      }],
    })

    expect(config.model_policies[0].key_priority_overrides_by_format).toEqual({
      'openai:chat': { 'key-a': 1 },
    })
    expect(resolveModelKeyPriorityOverride(config, 'gpt-5', 'openai:chat', 'key-a')).toBe(1)
    expect(resolveModelKeyPriorityOverride(config, 'gpt-5', 'gemini:generate_content', 'key-a')).toBe(7)
  })

  it('stores per-model scheduling as generated routing rules', () => {
    const next = upsertModelSchedulingRule(createEmptyRoutingGroupConfig(), 'gpt-5', {
      priority_mode: 'global_key',
      scheduling_mode: 'fixed_order',
    })

    expect(next.rules).toHaveLength(1)
    expect(next.rules[0].id).toBe(modelSchedulingRuleId('gpt-5'))
    expect(next.rules[0].conditions).toEqual({
      field: 'model',
      op: 'eq',
      value: 'gpt-5',
    })
    expect(getModelScheduling(next, 'gpt-5')).toMatchObject({
      priority_mode: 'global_key',
      scheduling_mode: 'fixed_order',
    })
  })
})

describe('routingTrace', () => {
  it('sorts candidate traces by selected order', () => {
    const sorted = sortCandidateTraces([
      candidate('provider-b', 2),
      candidate('provider-a', 1),
    ])

    expect(sorted.map(item => item.provider_id)).toEqual(['provider-a', 'provider-b'])
  })

  it('summarizes trace metadata', () => {
    const trace: RoutingDecisionTrace = {
      group_id: 'group-a',
      group_version: 3,
      selection_source: 'explicit',
      selected_rules: ['rule-a'],
      original_model: 'gpt-5',
      resolved_model: 'gpt-5',
      client_api_format: 'openai:chat',
      global_candidates: [candidate('provider-a', 0)],
      pool_expansion: [],
      runtime_facts: {},
    }

    expect(summarizeRoutingTrace(trace)).toContain('分组: group-a')
    expect(summarizeRoutingTrace(trace)).toContain('候选: 1')
  })
})

function candidate(providerId: string, selectedOrder: number) {
  return {
    candidate_kind: 'provider' as const,
    provider_id: providerId,
    endpoint_id: `${providerId}-endpoint`,
    model_id: 'model-a',
    key_id: `${providerId}-key`,
    selected_order: selectedOrder,
    ranking_vector: {
      provider_priority_before: selectedOrder,
      provider_priority_after: selectedOrder,
      key_priority_before: selectedOrder,
      key_priority_after: selectedOrder,
    },
  }
}
