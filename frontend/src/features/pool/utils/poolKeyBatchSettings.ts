import type { PoolKeySettingsPatch } from '@/api/endpoints/pool'

export type PoolKeyBatchSettingField =
  | 'internal_priority'
  | 'rpm_limit'
  | 'concurrent_limit'
  | 'cache_ttl_minutes'
  | 'max_probe_interval_minutes'
  | 'is_active'
  | 'note'
  | 'proxy_node_id'

export type PoolKeyBatchSettingSelection = Record<PoolKeyBatchSettingField, boolean>

export interface PoolKeyBatchSettingsDraft {
  internal_priority: number
  rpm_limit: number | null
  concurrent_limit: number | null
  cache_ttl_minutes: number
  max_probe_interval_minutes: number
  is_active: boolean
  note: string
  proxy_mode: 'set' | 'clear'
  proxy_node_id: string
}

export function createPoolKeyBatchSettingSelection(): PoolKeyBatchSettingSelection {
  return {
    internal_priority: false,
    rpm_limit: false,
    concurrent_limit: false,
    cache_ttl_minutes: false,
    max_probe_interval_minutes: false,
    is_active: false,
    note: false,
    proxy_node_id: false,
  }
}

export function createPoolKeyBatchSettingsDraft(): PoolKeyBatchSettingsDraft {
  return {
    internal_priority: 50,
    rpm_limit: null,
    concurrent_limit: null,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    is_active: true,
    note: '',
    proxy_mode: 'set',
    proxy_node_id: '',
  }
}

export function validatePoolKeyBatchSettings(
  selection: PoolKeyBatchSettingSelection,
  draft: PoolKeyBatchSettingsDraft,
): string[] {
  const errors: string[] = []
  if (!Object.values(selection).some(Boolean)) errors.push('请至少选择一个要修改的设置')
  if (selection.internal_priority && (!Number.isInteger(draft.internal_priority) || draft.internal_priority < 0)) {
    errors.push('优先级必须是大于等于 0 的整数')
  }
  if (selection.rpm_limit && draft.rpm_limit !== null && (
    !Number.isInteger(draft.rpm_limit) || draft.rpm_limit < 1 || draft.rpm_limit > 10_000
  )) {
    errors.push('RPM 必须在 1 到 10000 之间，留空表示自适应')
  }
  if (selection.concurrent_limit && draft.concurrent_limit !== null && (
    !Number.isInteger(draft.concurrent_limit) || draft.concurrent_limit < 0
  )) {
    errors.push('并发上限必须是大于等于 0 的整数')
  }
  if (selection.cache_ttl_minutes && (
    !Number.isInteger(draft.cache_ttl_minutes) || draft.cache_ttl_minutes < 0 || draft.cache_ttl_minutes > 60
  )) {
    errors.push('缓存 TTL 必须在 0 到 60 分钟之间')
  }
  if (selection.max_probe_interval_minutes && (
    !Number.isInteger(draft.max_probe_interval_minutes)
    || draft.max_probe_interval_minutes < 0
    || draft.max_probe_interval_minutes > 32
  )) {
    errors.push('熔断探测必须在 0 到 32 分钟之间')
  }
  if (selection.proxy_node_id && draft.proxy_mode === 'set' && !draft.proxy_node_id.trim()) {
    errors.push('请选择要设置的代理节点')
  }
  return errors
}

export function buildPoolKeySettingsPatch(
  selection: PoolKeyBatchSettingSelection,
  draft: PoolKeyBatchSettingsDraft,
): PoolKeySettingsPatch {
  const patch: PoolKeySettingsPatch = {}
  if (selection.internal_priority) patch.internal_priority = draft.internal_priority
  if (selection.rpm_limit) patch.rpm_limit = draft.rpm_limit
  if (selection.concurrent_limit) patch.concurrent_limit = draft.concurrent_limit
  if (selection.cache_ttl_minutes) patch.cache_ttl_minutes = draft.cache_ttl_minutes
  if (selection.max_probe_interval_minutes) {
    patch.max_probe_interval_minutes = draft.max_probe_interval_minutes
  }
  if (selection.is_active) patch.is_active = draft.is_active
  if (selection.note) patch.note = draft.note.trim() || null
  if (selection.proxy_node_id) {
    patch.proxy_node_id = draft.proxy_mode === 'clear' ? null : draft.proxy_node_id.trim()
  }
  return patch
}