import {
  DEFAULT_ROUTING_POLICY_MODEL,
  SCHEDULING_POLICY_RULE_PREFIX,
  createEmptyModelPolicy,
  getModelPolicy,
  getModelScheduling,
  isGeneratedModelSchedulingRule,
  isGeneratedSchedulingPolicyRule,
  modelPatternCondition,
  modelSchedulingRuleId,
  normalizeRoutingGroupConfig,
  schedulingRuleModels,
  type RoutingGroupConfig,
  type RoutingModelPolicy,
  type RoutingPriorityMode,
  type RoutingRule,
  type RoutingSchedulingMode,
  type RoutingSetSchedulingAction,
} from './routingPolicy'

export interface SchedulingPolicy {
  id: string
  scope: 'all' | 'selected'
  models: string[]
  priorityMode: RoutingPriorityMode
  schedulingMode: RoutingSchedulingMode
  policy: RoutingModelPolicy
  rule?: RoutingRule
}

export function createSchedulingPolicy(config: RoutingGroupConfig, scope: SchedulingPolicy['scope'] = 'selected'): SchedulingPolicy {
  const id = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`
  return {
    id: `${SCHEDULING_POLICY_RULE_PREFIX}${id}`,
    scope,
    models: [],
    priorityMode: config.default_policy.priority_mode,
    schedulingMode: config.default_policy.scheduling_mode,
    policy: createEmptyModelPolicy(DEFAULT_ROUTING_POLICY_MODEL),
  }
}

function policySignature(policy: RoutingModelPolicy): string {
  return JSON.stringify(policy, (_key, value) => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return value
    return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left.localeCompare(right)))
  })
}

export function readSchedulingPolicies(config: RoutingGroupConfig): SchedulingPolicy[] {
  const normalized = normalizeRoutingGroupConfig(config)
  const entries: SchedulingPolicy[] = []
  const assignedModels = new Set<string>()
  const sharedRules = normalized.rules.filter(isGeneratedSchedulingPolicyRule)

  for (const rule of sharedRules) {
    const grouped = new Map<string, SchedulingPolicy>()
    for (const model of schedulingRuleModels(rule)) {
      if (assignedModels.has(model)) continue
      const policy = { ...getModelPolicy(normalized, model), model: DEFAULT_ROUTING_POLICY_MODEL }
      const signature = policySignature(policy)
      let entry = grouped.get(signature)
      if (!entry) {
        const scheduling = getModelScheduling(normalized, model)
        const created = createSchedulingPolicy(normalized)
        entry = {
          ...created,
          id: grouped.size === 0 ? rule.id : created.id,
          priorityMode: scheduling.priority_mode,
          schedulingMode: scheduling.scheduling_mode,
          policy,
          rule,
        }
        grouped.set(signature, entry)
        entries.push(entry)
      }
      entry.models.push(model)
      assignedModels.add(model)
    }
  }

  const legacyModels = new Set([
    ...normalized.model_policies.map(policy => policy.model),
    ...normalized.rules.filter(isGeneratedModelSchedulingRule).flatMap(schedulingRuleModels),
  ])
  const legacyGroups = new Map<string, SchedulingPolicy>()
  for (const model of legacyModels) {
    if (model === DEFAULT_ROUTING_POLICY_MODEL || assignedModels.has(model)) continue
    const scheduling = getModelScheduling(normalized, model)
    const policy = { ...getModelPolicy(normalized, model), model: DEFAULT_ROUTING_POLICY_MODEL }
    const rule = normalized.rules.find(rule => rule.id === modelSchedulingRuleId(model))
    const signature = JSON.stringify([
      policySignature(policy),
      scheduling.priority_mode,
      scheduling.scheduling_mode,
      rule?.actions,
      rule?.enabled,
      rule?.phase,
      rule?.stop_processing,
    ])
    let entry = legacyGroups.get(signature)
    if (!entry) {
      entry = {
        ...createSchedulingPolicy(normalized),
        priorityMode: scheduling.priority_mode,
        schedulingMode: scheduling.scheduling_mode,
        policy,
        rule,
      }
      legacyGroups.set(signature, entry)
      entries.push(entry)
    }
    entry.models.push(model)
  }

  const defaultPolicy = normalized.model_policies.find(policy => policy.model === DEFAULT_ROUTING_POLICY_MODEL)
  if (defaultPolicy || entries.length === 0) {
    entries.push({
      ...createSchedulingPolicy(normalized, 'all'),
      policy: defaultPolicy ?? createEmptyModelPolicy(DEFAULT_ROUTING_POLICY_MODEL),
    })
  }
  return entries
}

export function validateSchedulingPolicies(entries: SchedulingPolicy[]): string | null {
  if (entries.length === 0) return '请至少添加一条调度配置'
  const assignedModels = new Set<string>()
  let hasAllModels = false
  for (const [index, entry] of entries.entries()) {
    if (entry.scope === 'all') {
      if (hasAllModels) return '只能有一条适用于全部模型的配置'
      hasAllModels = true
      continue
    }
    if (entry.models.length === 0) return `请为配置 ${index + 1} 选择至少一个全局模型`
    for (const model of entry.models) {
      if (!model.trim() || model === DEFAULT_ROUTING_POLICY_MODEL) return `配置 ${index + 1} 的模型无效`
      if (assignedModels.has(model)) return `模型 ${model} 不能重复分配给多条配置`
      assignedModels.add(model)
    }
  }
  return null
}

export function writeSchedulingPolicies(config: RoutingGroupConfig, entries: SchedulingPolicy[]): RoutingGroupConfig {
  const next = normalizeRoutingGroupConfig(config)
  const defaultEntry = entries.find(entry => entry.scope === 'all')
  if (defaultEntry) {
    next.default_policy.priority_mode = defaultEntry.priorityMode
    next.default_policy.scheduling_mode = defaultEntry.schedulingMode
  }
  next.model_policies = defaultEntry
    ? [{ ...defaultEntry.policy, model: DEFAULT_ROUTING_POLICY_MODEL }]
    : []
  next.rules = next.rules.filter(rule => !isGeneratedModelSchedulingRule(rule) && !isGeneratedSchedulingPolicyRule(rule))
  for (const [index, entry] of entries.entries()) {
    if (entry.scope === 'all' || entry.models.length === 0) continue
    for (const model of entry.models) {
      next.model_policies.push({ ...entry.policy, model })
    }
    const actions = [...(entry.rule?.actions ?? [])]
    const schedulingIndex = actions.findIndex(action => (
      Boolean(action) && typeof action === 'object' && (action as { type?: string }).type === 'set_scheduling'
    ))
    const action: RoutingSetSchedulingAction = {
      ...(schedulingIndex >= 0 ? actions[schedulingIndex] as RoutingSetSchedulingAction : {}),
      type: 'set_scheduling',
      priority_mode: entry.priorityMode,
      scheduling_mode: entry.schedulingMode,
    }
    if (schedulingIndex >= 0) actions[schedulingIndex] = action
    else actions.push(action)
    next.rules.push({
      priority: 10_000 + index,
      enabled: true,
      phase: 'client_request',
      stop_processing: false,
      ...entry.rule,
      id: entry.id,
      conditions: { any: entry.models.map(modelPatternCondition) },
      actions,
    })
  }
  return normalizeRoutingGroupConfig(next)
}

export function schedulingPolicyEditorConfig(config: RoutingGroupConfig, entry: SchedulingPolicy): RoutingGroupConfig {
  return normalizeRoutingGroupConfig({
    default_policy: {
      ...config.default_policy,
      priority_mode: entry.priorityMode,
      scheduling_mode: entry.schedulingMode,
    },
    model_policies: [{ ...entry.policy, model: DEFAULT_ROUTING_POLICY_MODEL }],
    rules: [],
  })
}
