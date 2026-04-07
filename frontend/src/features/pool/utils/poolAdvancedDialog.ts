export type PoolHealthToggleKey =
  | 'health_policy_enabled'
  | 'probing_enabled'
  | 'auto_remove_banned_keys'

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
