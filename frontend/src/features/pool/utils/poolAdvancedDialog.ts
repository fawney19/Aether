export type PoolHealthToggleKey =
  | 'probing_enabled'
  | 'account_self_check_enabled'
  | 'auto_remove_banned_keys'
  | 'auto_remove_quota_exhausted_keys'
  | 'skip_exhausted_accounts'

export interface PoolHealthToggleCard {
  key: PoolHealthToggleKey
  label: string
  description: string
}

export interface PoolCooldownFieldLayout {
  fields: string[]
  desktopColumnsClass: string
}

export function buildPoolHealthToggleCards(): PoolHealthToggleCard[] {
  return [
    {
      key: 'probing_enabled',
      label: '自适应热池',
      description: '自动维护热池，缺口时异步补位。',
    },
    {
      key: 'account_self_check_enabled',
      label: '账号自检',
      description: '定时确认账号状态，策略由提供商适配器内置。',
    },
    {
      key: 'auto_remove_banned_keys',
      label: '异常自动清除',
      description: '检测到不可恢复账号异常，或 RT 与 AT 均失效时自动从号池移除。',
    },
    {
      key: 'auto_remove_quota_exhausted_keys',
      label: '自动清理额度耗尽',
      description: '探测到黑色“额度耗尽”账号后自动从号池移除。',
    },
    {
      key: 'skip_exhausted_accounts',
      label: '跳过额度耗尽账号',
      description: '当 Codex / Kiro 账号额度已耗尽时，直接标记为不可调度并在请求侧跳过。',
    },
  ]
}

export function buildPoolCooldownFieldLayout(): PoolCooldownFieldLayout {
  return {
    fields: [
      'rate_limit_cooldown_seconds',
      'overload_cooldown_seconds',
    ],
    desktopColumnsClass: 'xl:grid-cols-2',
  }
}
