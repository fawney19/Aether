import { normalizeRoutingFailoverPolicy, type RoutingFailoverPolicy } from './routingFailover'

export type RoutingPriorityMode = 'provider' | 'global_key'
export type RoutingSchedulingMode = 'fixed_order' | 'cache_affinity' | 'load_balance'
export type RoutingRulePhase = 'client_request' | 'provider_request'
export type RoutingSortingScope = 'unified' | 'per_model'

/** 首个候选（粘性 Key）的总尝试次数默认值：失败后同 Key 重试 1 次 */
export const DEFAULT_STICKY_KEY_ATTEMPTS = 2

export interface RoutingDefaultPolicy extends RoutingFailoverPolicy {
  priority_mode: RoutingPriorityMode
  scheduling_mode: RoutingSchedulingMode
  keep_priority_on_conversion: boolean
  enable_cf_heartbeat: boolean
  cyber_continue_failover: boolean
  cancel_on_client_disconnect: boolean
  /** 首个候选的总尝试次数；后续候选始终只尝试 1 次。0 或 1 表示不重试 */
  sticky_key_attempts: number
}

export interface RoutingPoolSchedulingPreset {
  preset: string
  enabled: boolean
  mode?: string | null
}

export interface RoutingPoolPolicyOverride {
  scheduling_presets: RoutingPoolSchedulingPreset[]
}

export interface RoutingModelPolicy {
  model: string
  allowed_providers: string[]
  allowed_keys: string[]
  provider_priority_overrides: Record<string, number>
  key_priority_overrides: Record<string, number>
  /** api_format -> key_id -> priority；同一 Key 在不同 API 格式下可独立排序 */
  key_priority_overrides_by_format: Record<string, Record<string, number>>
  pool_priority_overrides: Record<string, number>
  pool_policy_overrides: Record<string, RoutingPoolPolicyOverride>
}

export interface RoutingRule {
  id: string
  priority: number
  enabled: boolean
  phase: RoutingRulePhase
  conditions: unknown
  actions: unknown[]
  stop_processing: boolean
}

export interface RoutingPredicateCondition {
  field: string
  op: 'eq' | 'prefix'
  value: string
}

export interface RoutingSetSchedulingAction {
  type: 'set_scheduling'
  priority_mode: RoutingPriorityMode
  scheduling_mode: RoutingSchedulingMode
  sticky_key_attempts?: number
}

export interface RoutingGroupConfig {
  default_policy: RoutingDefaultPolicy
  model_policies: RoutingModelPolicy[]
  rules: RoutingRule[]
}

export const DEFAULT_ROUTING_POLICY_MODEL = '*'
export const MODEL_SCHEDULING_RULE_PREFIX = 'ui_model_scheduling:'
export const SCHEDULING_POLICY_RULE_PREFIX = 'ui_scheduling_policy:'

export function createEmptyRoutingGroupConfig(): RoutingGroupConfig {
  return {
    default_policy: {
      ...normalizeRoutingFailoverPolicy(),
      priority_mode: 'provider',
      scheduling_mode: 'cache_affinity',
      keep_priority_on_conversion: false,
      enable_cf_heartbeat: false,
      cyber_continue_failover: false,
      cancel_on_client_disconnect: false,
      sticky_key_attempts: DEFAULT_STICKY_KEY_ATTEMPTS,
    },
    model_policies: [],
    rules: [],
  }
}

export function normalizeStickyKeyAttempts(value: unknown): number {
  const parsed = Math.trunc(Number(value))
  if (!Number.isFinite(parsed) || parsed < 0) return DEFAULT_STICKY_KEY_ATTEMPTS
  return Math.min(parsed, 99)
}

export function createEmptyModelPolicy(model = ''): RoutingModelPolicy {
  return {
    model,
    allowed_providers: [],
    allowed_keys: [],
    provider_priority_overrides: {},
    key_priority_overrides: {},
    key_priority_overrides_by_format: {},
    pool_priority_overrides: {},
    pool_policy_overrides: {},
  }
}

export function normalizeRoutingGroupConfig(value: Partial<RoutingGroupConfig> | null | undefined): RoutingGroupConfig {
  const base = createEmptyRoutingGroupConfig()
  const rawDefaultPolicy = (value?.default_policy ?? {}) as Partial<RoutingDefaultPolicy> & {
    enable_openai_image_sync_heartbeat?: boolean
    enable_standard_text_sync_heartbeat?: boolean
  }
  const {
    enable_openai_image_sync_heartbeat: legacyImageHeartbeat,
    enable_standard_text_sync_heartbeat: legacyTextHeartbeat,
    ...defaultPolicyWithoutLegacyHeartbeat
  } = rawDefaultPolicy

  return {
    default_policy: {
      ...base.default_policy,
      ...defaultPolicyWithoutLegacyHeartbeat,
      ...normalizeRoutingFailoverPolicy(rawDefaultPolicy),
      enable_cf_heartbeat: Boolean(
        rawDefaultPolicy.enable_cf_heartbeat || legacyImageHeartbeat || legacyTextHeartbeat,
      ),
      sticky_key_attempts: normalizeStickyKeyAttempts(
        rawDefaultPolicy.sticky_key_attempts ?? DEFAULT_STICKY_KEY_ATTEMPTS,
      ),
    },
    model_policies: Array.isArray(value?.model_policies)
      ? value.model_policies.map(policy => ({
          ...createEmptyModelPolicy(policy.model),
          ...policy,
          allowed_providers: Array.isArray(policy.allowed_providers) ? [...policy.allowed_providers] : [],
          allowed_keys: Array.isArray(policy.allowed_keys) ? [...policy.allowed_keys] : [],
          provider_priority_overrides: { ...(policy.provider_priority_overrides ?? {}) },
          key_priority_overrides: { ...(policy.key_priority_overrides ?? {}) },
          key_priority_overrides_by_format: normalizeKeyPriorityOverridesByFormat(
            policy.key_priority_overrides_by_format,
          ),
          pool_priority_overrides: { ...(policy.pool_priority_overrides ?? {}) },
          pool_policy_overrides: { ...(policy.pool_policy_overrides ?? {}) },
        }))
      : base.model_policies,
    rules: Array.isArray(value?.rules) ? value.rules.map(rule => ({ ...rule })) : base.rules,
  }
}

export function upsertModelPolicy(config: RoutingGroupConfig, policy: RoutingModelPolicy): RoutingGroupConfig {
  const model = policy.model.trim()
  if (!model) {
    return normalizeRoutingGroupConfig(config)
  }

  const next = normalizeRoutingGroupConfig(config)
  const index = next.model_policies.findIndex(item => item.model === model)
  const normalizedPolicy = { ...createEmptyModelPolicy(model), ...policy, model }

  if (index >= 0) {
    next.model_policies[index] = normalizedPolicy
  } else {
    next.model_policies.push(normalizedPolicy)
  }

  return next
}

export function removeModelPolicy(config: RoutingGroupConfig, model: string): RoutingGroupConfig {
  const next = normalizeRoutingGroupConfig(config)
  next.model_policies = next.model_policies.filter(policy => policy.model !== model)
  return next
}

export function getDefaultModelPolicy(config: RoutingGroupConfig): RoutingModelPolicy {
  const normalized = normalizeRoutingGroupConfig(config)
  return normalized.model_policies.find(policy => policy.model === DEFAULT_ROUTING_POLICY_MODEL)
    ?? createEmptyModelPolicy(DEFAULT_ROUTING_POLICY_MODEL)
}

export function getModelPolicy(config: RoutingGroupConfig, model: string): RoutingModelPolicy {
  const normalizedModel = model.trim() || DEFAULT_ROUTING_POLICY_MODEL
  if (normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return getDefaultModelPolicy(config)
  }
  const normalized = normalizeRoutingGroupConfig(config)
  return normalized.model_policies.find(policy => policy.model === normalizedModel)
    ?? createEmptyModelPolicy(normalizedModel)
}

export function upsertDefaultModelPolicy(
  config: RoutingGroupConfig,
  patch: Partial<Omit<RoutingModelPolicy, 'model'>>,
): RoutingGroupConfig {
  const current = getDefaultModelPolicy(config)
  const next = upsertModelPolicy(config, {
    ...current,
    ...patch,
    model: DEFAULT_ROUTING_POLICY_MODEL,
  })
  next.model_policies = [
    ...next.model_policies.filter(policy => policy.model === DEFAULT_ROUTING_POLICY_MODEL),
    ...next.model_policies.filter(policy => policy.model !== DEFAULT_ROUTING_POLICY_MODEL),
  ]
  return next
}

export function setDefaultProviderPriorityOverrides(
  config: RoutingGroupConfig,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  return upsertDefaultModelPolicy(config, {
    provider_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function setDefaultKeyPriorityOverrides(
  config: RoutingGroupConfig,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  return upsertDefaultModelPolicy(config, {
    key_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function setDefaultPoolPriorityOverrides(
  config: RoutingGroupConfig,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  return upsertDefaultModelPolicy(config, {
    pool_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function setModelProviderPriorityOverrides(
  config: RoutingGroupConfig,
  model: string,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  const normalizedModel = model.trim() || DEFAULT_ROUTING_POLICY_MODEL
  if (normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return setDefaultProviderPriorityOverrides(config, overrides)
  }
  return upsertModelPolicy(config, {
    ...getModelPolicy(config, normalizedModel),
    model: normalizedModel,
    provider_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function setModelKeyPriorityOverrides(
  config: RoutingGroupConfig,
  model: string,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  const normalizedModel = model.trim() || DEFAULT_ROUTING_POLICY_MODEL
  if (normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return setDefaultKeyPriorityOverrides(config, overrides)
  }
  return upsertModelPolicy(config, {
    ...getModelPolicy(config, normalizedModel),
    model: normalizedModel,
    key_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function normalizeRoutingApiFormatKey(apiFormat: string): string {
  return apiFormat.trim().toLowerCase()
}

export function getModelKeyPriorityOverridesForFormat(
  config: RoutingGroupConfig,
  model: string,
  apiFormat: string,
): Record<string, number> {
  const policy = getModelPolicy(config, model)
  const format = normalizeRoutingApiFormatKey(apiFormat)
  return { ...(policy.key_priority_overrides_by_format[format] ?? {}) }
}

/**
 * 某个 Key 在指定 API 格式下的生效覆盖值：按格式覆盖优先，其次是不分格式的 Key 覆盖。
 */
export function resolveModelKeyPriorityOverride(
  config: RoutingGroupConfig,
  model: string,
  apiFormat: string,
  keyId: string,
): number | undefined {
  const policy = getModelPolicy(config, model)
  const format = normalizeRoutingApiFormatKey(apiFormat)
  return policy.key_priority_overrides_by_format[format]?.[keyId]
    ?? policy.key_priority_overrides[keyId]
}

export function setModelKeyPriorityOverridesForFormat(
  config: RoutingGroupConfig,
  model: string,
  apiFormat: string,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  const normalizedModel = model.trim() || DEFAULT_ROUTING_POLICY_MODEL
  const format = normalizeRoutingApiFormatKey(apiFormat)
  if (!format) return normalizeRoutingGroupConfig(config)

  const current = getModelPolicy(config, normalizedModel)
  const byFormat = { ...current.key_priority_overrides_by_format }
  const normalized = normalizePriorityOverrides(overrides)
  if (Object.keys(normalized).length > 0) {
    byFormat[format] = normalized
  } else {
    delete byFormat[format]
  }

  if (normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return upsertDefaultModelPolicy(config, { key_priority_overrides_by_format: byFormat })
  }
  return upsertModelPolicy(config, {
    ...current,
    model: normalizedModel,
    key_priority_overrides_by_format: byFormat,
  })
}

export function setModelPoolPriorityOverrides(
  config: RoutingGroupConfig,
  model: string,
  overrides: Record<string, number>,
): RoutingGroupConfig {
  const normalizedModel = model.trim() || DEFAULT_ROUTING_POLICY_MODEL
  if (normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return setDefaultPoolPriorityOverrides(config, overrides)
  }
  return upsertModelPolicy(config, {
    ...getModelPolicy(config, normalizedModel),
    model: normalizedModel,
    pool_priority_overrides: normalizePriorityOverrides(overrides),
  })
}

export function modelSchedulingRuleId(model: string): string {
  return `${MODEL_SCHEDULING_RULE_PREFIX}${encodeURIComponent(model.trim())}`
}

export function isGeneratedModelSchedulingRule(rule: RoutingRule): boolean {
  return rule.id.startsWith(MODEL_SCHEDULING_RULE_PREFIX)
}

export function isGeneratedSchedulingPolicyRule(rule: RoutingRule): boolean {
  return rule.id.startsWith(SCHEDULING_POLICY_RULE_PREFIX)
}

export function schedulingRuleModels(rule: RoutingRule): string[] {
  if (isGeneratedModelSchedulingRule(rule)) {
    try {
      return [decodeURIComponent(rule.id.slice(MODEL_SCHEDULING_RULE_PREFIX.length))]
    } catch {
      return []
    }
  }
  if (!isGeneratedSchedulingPolicyRule(rule)) return []
  const conditions = rule.conditions as { any?: RoutingPredicateCondition[] } | null
  if (!Array.isArray(conditions?.any)) return []
  return conditions.any.flatMap(condition => {
    if (condition?.field !== 'model' || typeof condition.value !== 'string') return []
    if (condition.op === 'eq') return [condition.value]
    if (condition.op === 'prefix') return [`${condition.value}*`]
    return []
  })
}

export function modelPatternCondition(model: string): RoutingPredicateCondition {
  const normalizedModel = model.trim()
  if (normalizedModel.endsWith('*')) {
    return {
      field: 'model',
      op: 'prefix',
      value: normalizedModel.slice(0, -1),
    }
  }
  return {
    field: 'model',
    op: 'eq',
    value: normalizedModel,
  }
}

export function getModelScheduling(
  config: RoutingGroupConfig,
  model: string,
): RoutingDefaultPolicy {
  const normalized = normalizeRoutingGroupConfig(config)
  const rules = normalized.rules
    .filter(rule => rule.enabled && rule.phase === 'client_request' && schedulingRuleModels(rule).some(pattern => (
      pattern.endsWith('*') ? model.startsWith(pattern.slice(0, -1)) : pattern === model
    )))
    .sort((left, right) => left.priority - right.priority || left.id.localeCompare(right.id))
  let action: RoutingSetSchedulingAction | undefined
  for (const rule of rules) {
    for (const candidate of rule.actions) {
      if (isSetSchedulingAction(candidate)) action = { ...action, ...candidate }
    }
    if (rule.stop_processing) break
  }
  return {
    ...normalized.default_policy,
    priority_mode: action?.priority_mode ?? normalized.default_policy.priority_mode,
    scheduling_mode: action?.scheduling_mode ?? normalized.default_policy.scheduling_mode,
    keep_priority_on_conversion: normalized.default_policy.keep_priority_on_conversion,
    enable_cf_heartbeat: normalized.default_policy.enable_cf_heartbeat,
    cyber_continue_failover: normalized.default_policy.cyber_continue_failover,
    cancel_on_client_disconnect: normalized.default_policy.cancel_on_client_disconnect,
    sticky_key_attempts: action?.sticky_key_attempts ?? normalized.default_policy.sticky_key_attempts,
  }
}

export function upsertModelSchedulingRule(
  config: RoutingGroupConfig,
  model: string,
  scheduling: Pick<RoutingDefaultPolicy, 'priority_mode' | 'scheduling_mode'>,
): RoutingGroupConfig {
  const normalizedModel = model.trim()
  if (!normalizedModel || normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) {
    return normalizeRoutingGroupConfig(config)
  }

  const next = normalizeRoutingGroupConfig(config)
  const rule: RoutingRule = {
    id: modelSchedulingRuleId(normalizedModel),
    priority: 10_000 + next.rules.filter(isGeneratedModelSchedulingRule).length,
    enabled: true,
    phase: 'client_request',
    conditions: modelPatternCondition(normalizedModel),
    actions: [{
      type: 'set_scheduling',
      priority_mode: scheduling.priority_mode,
      scheduling_mode: scheduling.scheduling_mode,
    } satisfies RoutingSetSchedulingAction],
    stop_processing: false,
  }

  const index = next.rules.findIndex(item => item.id === rule.id)
  if (index >= 0) {
    next.rules[index] = {
      ...next.rules[index],
      ...rule,
      priority: next.rules[index].priority,
    }
  } else {
    next.rules.push(rule)
  }
  return next
}

export function removeModelSchedulingRule(config: RoutingGroupConfig, model: string): RoutingGroupConfig {
  const ruleId = modelSchedulingRuleId(model)
  const next = normalizeRoutingGroupConfig(config)
  next.rules = next.rules.filter(rule => rule.id !== ruleId)
  return next
}

export function removeGeneratedModelSchedulingRules(config: RoutingGroupConfig): RoutingGroupConfig {
  const next = normalizeRoutingGroupConfig(config)
  next.rules = next.rules.filter(rule => !isGeneratedModelSchedulingRule(rule) && !isGeneratedSchedulingPolicyRule(rule))
  return next
}

export function setRoutingSortingScope(
  config: RoutingGroupConfig,
  scope: RoutingSortingScope,
): RoutingGroupConfig {
  if (scope === 'per_model') return normalizeRoutingGroupConfig(config)

  const next = removeGeneratedModelSchedulingRules(config)
  next.model_policies = next.model_policies
    .filter(policy => policy.model === DEFAULT_ROUTING_POLICY_MODEL)
  return next
}

export function removePerModelRoutingConfig(
  config: RoutingGroupConfig,
  model: string,
): RoutingGroupConfig {
  return removeModelSchedulingRule(removeModelPolicy(config, model), model)
}

export function copyPerModelRoutingConfig(
  config: RoutingGroupConfig,
  sourceConfig: RoutingGroupConfig,
  sourceModel: string,
  targetModel: string,
): RoutingGroupConfig {
  const source = sourceModel.trim()
  const target = targetModel.trim()
  if (!source || !target || source === target) return normalizeRoutingGroupConfig(config)

  const sourcePolicy = getModelPolicy(sourceConfig, source)
  const sourceScheduling = getModelScheduling(sourceConfig, source)
  const next = upsertModelPolicy(config, {
    ...sourcePolicy,
    model: target,
  })
  return upsertModelSchedulingRule(next, target, {
    priority_mode: sourceScheduling.priority_mode,
    scheduling_mode: sourceScheduling.scheduling_mode,
  })
}

export function savePerModelRoutingConfig(
  config: RoutingGroupConfig,
  model: string,
): RoutingGroupConfig {
  const normalizedModel = model.trim()
  const next = normalizeRoutingGroupConfig(config)
  if (!normalizedModel || normalizedModel === DEFAULT_ROUTING_POLICY_MODEL) return next
  if (next.model_policies.some(policy => policy.model === normalizedModel)) return next
  return upsertModelPolicy(next, createEmptyModelPolicy(normalizedModel))
}

export function normalizePriorityOverrides(overrides: Record<string, number>): Record<string, number> {
  const normalized: Record<string, number> = {}
  for (const [rawId, rawPriority] of Object.entries(overrides)) {
    const id = rawId.trim()
    const priority = Math.max(0, Math.trunc(Number(rawPriority)))
    if (!id || !Number.isFinite(priority)) continue
    normalized[id] = priority
  }
  return normalized
}

function normalizeKeyPriorityOverridesByFormat(
  value: Record<string, Record<string, number>> | null | undefined,
): Record<string, Record<string, number>> {
  const normalized: Record<string, Record<string, number>> = {}
  if (!value || typeof value !== 'object') return normalized
  for (const [rawFormat, overrides] of Object.entries(value)) {
    const format = normalizeRoutingApiFormatKey(rawFormat)
    if (!format || !overrides || typeof overrides !== 'object') continue
    const merged = normalizePriorityOverrides({
      ...(normalized[format] ?? {}),
      ...overrides,
    })
    if (Object.keys(merged).length > 0) {
      normalized[format] = merged
    }
  }
  return normalized
}

function isSetSchedulingAction(action: unknown): action is RoutingSetSchedulingAction {
  if (!action || typeof action !== 'object') return false
  const candidate = action as Partial<RoutingSetSchedulingAction>
  return candidate.type === 'set_scheduling'
}
