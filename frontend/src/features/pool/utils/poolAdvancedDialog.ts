import type { PoolPreProbeConfig } from '@/api/endpoints/types/provider'

export const POOL_PRE_PROBE_HELP_TEXT = '提前探测号池里排在后面的候选，主请求失败时秒切活号；号池改动立即生效'
export const POOL_PRE_PROBE_DISABLED_TOOLTIP = '仅支持 OAuth 号池（Codex/Kiro/Antigravity/ChatGPT Web）'

const OAUTH_POOL_PROVIDER_TYPES = ['codex', 'kiro', 'antigravity', 'chatgpt_web'] as const

export type PoolHealthToggleKey =
  | 'health_policy_enabled'
  | 'probing_enabled'
  | 'auto_remove_banned_keys'
  | 'skip_exhausted_accounts'
  | 'pre_probe_enabled'

export type PoolPreProbeNumberKey = Exclude<keyof PoolPreProbeConfig, 'enabled'>

export type PoolPreProbeFormState = Record<PoolPreProbeNumberKey, number | null | undefined>

export const POOL_PRE_PROBE_NUMBER_KEYS = [
  'top_n',
  'required_healthy',
  'dedup_window_secs',
  'cache_ttl_seconds',
  'cache_max_entries',
  'probe_timeout_seconds',
  'per_provider_rate_limit_per_minute',
  'group_lock_ttl_seconds',
  'circuit_failure_rate_threshold',
  'circuit_sample_window_seconds',
  'circuit_suspend_seconds',
  '5xx_streak_threshold',
] as const satisfies readonly PoolPreProbeNumberKey[]

export const POOL_PRE_PROBE_DEFAULTS: Record<PoolPreProbeNumberKey, number> = {
  top_n: 8,
  required_healthy: 8,
  dedup_window_secs: 300,
  cache_ttl_seconds: 300,
  cache_max_entries: 10000,
  probe_timeout_seconds: 10,
  per_provider_rate_limit_per_minute: 60,
  group_lock_ttl_seconds: 10,
  circuit_failure_rate_threshold: 50,
  circuit_sample_window_seconds: 300,
  circuit_suspend_seconds: 600,
  '5xx_streak_threshold': 5,
}

export interface PoolHealthToggleCard {
  key: PoolHealthToggleKey
  label: string
  description: string
}

export interface PoolCooldownFieldLayout {
  fields: string[]
  desktopColumnsClass: string
}

export interface PoolSecondarySectionLayout {
  wrapperClass: string
}

export interface PoolCostFieldLayout {
  fields: string[]
  desktopColumnsClass: string
}

export interface PoolPreProbeFieldDefinition {
  key: PoolPreProbeNumberKey
  label: string
  unit?: string
  min: number
  max?: number
  placeholder: string
  description: string
}

export interface PoolPreProbeFieldLayout {
  fields: PoolPreProbeFieldDefinition[]
  desktopColumnsClass: string
}

export function isOAuthPoolProviderType(value: string | null | undefined): boolean {
  const normalized = (value || '').trim().toLowerCase()
  return OAUTH_POOL_PROVIDER_TYPES.includes(normalized as typeof OAUTH_POOL_PROVIDER_TYPES[number])
}

export function buildPoolHealthToggleCards(): PoolHealthToggleCard[] {
  return [
    {
      key: 'health_policy_enabled',
      label: '健康策略',
      description: '按上游错误自动冷却并跳过异常账号。',
    },
    {
      key: 'probing_enabled',
      label: '主动探测',
      description: '按固定间隔刷新 Key 的状态与额度，减少号池状态滞后。',
    },
    {
      key: 'auto_remove_banned_keys',
      label: '异常自动清除',
      description: '仅在检测到不可恢复的账号异常时自动从号池移除，不处理纯 Token 失效。',
    },
    {
      key: 'skip_exhausted_accounts',
      label: '跳过额度耗尽账号',
      description: '当 Codex / Kiro 账号额度已耗尽时，直接标记为不可调度并在请求侧跳过。',
    },
    {
      key: 'pre_probe_enabled',
      label: '候选预热',
      description: POOL_PRE_PROBE_HELP_TEXT,
    },
  ]
}

export function buildPoolCooldownFieldLayout(): PoolCooldownFieldLayout {
  return {
    fields: [
      'rate_limit_cooldown_seconds',
      'overload_cooldown_seconds',
      'sticky_session_ttl_seconds',
      'global_priority',
    ],
    desktopColumnsClass: 'xl:grid-cols-4',
  }
}

export function buildPoolSecondarySectionLayout(): PoolSecondarySectionLayout {
  return {
    wrapperClass: 'space-y-4',
  }
}

export function buildPoolCostFieldLayout(): PoolCostFieldLayout {
  return {
    fields: [
      'cost_window_seconds',
      'cost_limit_per_key_tokens',
      'cost_soft_threshold_percent',
    ],
    desktopColumnsClass: 'xl:grid-cols-3',
  }
}

export function buildPoolPreProbeFieldLayout(): PoolPreProbeFieldLayout {
  return {
    fields: [
      {
        key: 'top_n',
        label: '候选数量',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.top_n),
        description: '每次预热最多探测的候选 Key 数。',
      },
      {
        key: 'required_healthy',
        label: '目标活号数',
        min: 1,
        max: 8,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.required_healthy),
        description: '达到该健康候选数后停止继续探测。',
      },
      {
        key: 'dedup_window_secs',
        label: '去重窗口',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.dedup_window_secs),
        description: '同一 Key 在窗口期内不重复探测。',
      },
      {
        key: 'cache_ttl_seconds',
        label: '候选缓存 TTL',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.cache_ttl_seconds),
        description: '预热候选缓存的有效时间。',
      },
      {
        key: 'cache_max_entries',
        label: '缓存上限',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.cache_max_entries),
        description: '预热候选缓存最多保留的条目数。',
      },
      {
        key: 'probe_timeout_seconds',
        label: '探测超时',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.probe_timeout_seconds),
        description: '单个候选探测的最长等待时间。',
      },
      {
        key: 'per_provider_rate_limit_per_minute',
        label: '提供商速率',
        unit: '次/分钟',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.per_provider_rate_limit_per_minute),
        description: '每个提供商的预热探测速率上限。',
      },
      {
        key: 'group_lock_ttl_seconds',
        label: '分组锁 TTL',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.group_lock_ttl_seconds),
        description: '同一候选组跨节点并发预热的锁定时间。',
      },
      {
        key: 'circuit_failure_rate_threshold',
        label: '熔断失败率',
        unit: '%',
        min: 1,
        max: 100,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.circuit_failure_rate_threshold),
        description: '触发预热熔断的失败率阈值。',
      },
      {
        key: 'circuit_sample_window_seconds',
        label: '熔断采样窗口',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.circuit_sample_window_seconds),
        description: '统计预热失败率的时间窗口。',
      },
      {
        key: 'circuit_suspend_seconds',
        label: '熔断暂停',
        unit: '秒',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS.circuit_suspend_seconds),
        description: '触发熔断后暂停预热的时间。',
      },
      {
        key: '5xx_streak_threshold',
        label: '5xx 连续阈值',
        min: 1,
        placeholder: String(POOL_PRE_PROBE_DEFAULTS['5xx_streak_threshold']),
        description: '连续服务端错误达到该次数后进入预热冷却。',
      },
    ],
    desktopColumnsClass: 'xl:grid-cols-3',
  }
}

export function buildPoolPreProbeForm(config?: PoolPreProbeConfig | null): PoolPreProbeFormState {
  return POOL_PRE_PROBE_NUMBER_KEYS.reduce((acc, key) => {
    acc[key] = config?.[key] ?? POOL_PRE_PROBE_DEFAULTS[key]
    return acc
  }, {} as PoolPreProbeFormState)
}

export function buildPoolPreProbePayload(
  enabled: boolean,
  form: PoolPreProbeFormState,
): PoolPreProbeConfig {
  const payload: PoolPreProbeConfig = { enabled }
  for (const key of POOL_PRE_PROBE_NUMBER_KEYS) {
    const value = form[key]
    if (value !== null && value !== undefined) {
      ;(payload as Record<string, number | boolean>)[key] = value
    }
  }
  return payload
}
