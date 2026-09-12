export const MAX_ROUTING_FAILOVER_RULES = 64
export const MAX_ROUTING_FAILOVER_PATTERN_BYTES = 4096

export interface RoutingFailoverRule {
  pattern: string
  status_codes: number[]
}

export interface RoutingFailoverRules {
  success_failover_patterns: RoutingFailoverRule[]
  error_stop_patterns: RoutingFailoverRule[]
}

export interface RoutingFailoverPolicy {
  max_transfer_count: number
  max_transfer_timeout_seconds: number
  failover_rules: RoutingFailoverRules
}

export function normalizeRoutingFailoverPolicy(value?: Partial<RoutingFailoverPolicy>): RoutingFailoverPolicy {
  const rules = value?.failover_rules
  const cloneRules = (entries?: RoutingFailoverRule[]): RoutingFailoverRule[] => (
    Array.isArray(entries) ? entries.map(rule => ({
      pattern: String(rule.pattern ?? ''),
      status_codes: Array.isArray(rule.status_codes) ? [...rule.status_codes] : [],
    })) : []
  )
  return {
    max_transfer_count: Number(value?.max_transfer_count ?? 0),
    max_transfer_timeout_seconds: Number(value?.max_transfer_timeout_seconds ?? 0),
    failover_rules: {
      success_failover_patterns: cloneRules(rules?.success_failover_patterns),
      error_stop_patterns: cloneRules(rules?.error_stop_patterns),
    },
  }
}

export function validateRoutingFailoverPolicy(policy: RoutingFailoverPolicy): string | null {
  for (const [name, value] of [
    ['全局最大转移次数', policy.max_transfer_count],
    ['全局最大转移时间', policy.max_transfer_timeout_seconds],
  ] as const) {
    if (!Number.isSafeInteger(value) || value < 0) return `${name}必须是非负整数`
  }
  for (const [name, entries, success] of [
    ['成功转移规则', policy.failover_rules.success_failover_patterns, true],
    ['错误终止规则', policy.failover_rules.error_stop_patterns, false],
  ] as const) {
    if (entries.length > MAX_ROUTING_FAILOVER_RULES) return `${name}最多 ${MAX_ROUTING_FAILOVER_RULES} 条`
    for (const [index, rule] of entries.entries()) {
      if (!rule.pattern.trim() && (success || rule.status_codes.length === 0)) {
        return `${name}第 ${index + 1} 条需要${success ? '正则表达式' : '状态码或正则表达式'}`
      }
      if (new TextEncoder().encode(rule.pattern.trim()).length > MAX_ROUTING_FAILOVER_PATTERN_BYTES) {
        return `${name}第 ${index + 1} 条正则过长`
      }
      if (rule.status_codes.some(status => !Number.isInteger(status) || (success ? status !== 200 : status < 400 || status > 599))) {
        return `${name}第 ${index + 1} 条状态码必须为${success ? ' 200' : ' 400–599'}`
      }
    }
  }
  return null
}
