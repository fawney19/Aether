import type { PoolKeyBatchUpdatePatch } from '@/api/endpoints/pool'

export interface PoolKeyBatchEditState {
  applyApiFormats: boolean
  apiFormats: string[]
  applyActive: boolean
  isActive: boolean
  applyInternalPriority: boolean
  internalPriority: string
  applyRpmLimit: boolean
  rpmLimit: string
  applyConcurrentLimit: boolean
  concurrentLimit: string
  applyCacheTtl: boolean
  cacheTtlMinutes: string
  applyProbeInterval: boolean
  maxProbeIntervalMinutes: string
  applyNote: boolean
  note: string
  applyAutoFetchModels: boolean
  autoFetchModels: boolean
  includePatterns: string
  excludePatterns: string
  applyAllowedModels: boolean
  unrestrictedModels: boolean
  selectedModels: string[]
  lockSelectedModels: boolean
}

export interface PoolKeyBatchPatchBuildResult {
  patch: PoolKeyBatchUpdatePatch | null
  fieldLabels: string[]
  error: string | null
}

function uniqueTrimmed(values: string[]): string[] {
  return [...new Set(values.map(value => value.trim()).filter(Boolean))]
}

export function parsePoolKeyModelPatterns(value: string): string[] {
  return uniqueTrimmed(value.split(/[,\n]/))
}

function parseIntegerField(
  value: string,
  label: string,
  min: number,
  max?: number,
  nullable = false,
): { value?: number | null; error?: string } {
  const normalized = value.trim()
  if (!normalized) {
    return nullable ? { value: null } : { error: `${label} 不能为空` }
  }
  const parsed = Number(normalized)
  if (!Number.isInteger(parsed) || parsed < min || (max !== undefined && parsed > max)) {
    const range = max === undefined ? `不小于 ${min}` : `${min}-${max}`
    return { error: `${label} 必须是 ${range} 的整数` }
  }
  return { value: parsed }
}

export function buildPoolKeyBatchUpdatePatch(
  state: PoolKeyBatchEditState,
): PoolKeyBatchPatchBuildResult {
  const patch: PoolKeyBatchUpdatePatch = {}
  const fieldLabels: string[] = []

  if (state.applyApiFormats) {
    const apiFormats = uniqueTrimmed(state.apiFormats)
    if (apiFormats.length === 0) {
      return { patch: null, fieldLabels, error: '请至少选择一个支持的 API' }
    }
    patch.api_formats = apiFormats
    fieldLabels.push('支持 API')
  }

  if (state.applyActive) {
    patch.is_active = state.isActive
    fieldLabels.push('启用状态')
  }

  if (state.applyInternalPriority) {
    const parsed = parseIntegerField(state.internalPriority, '优先级', 0)
    if (parsed.error) return { patch: null, fieldLabels, error: parsed.error }
    patch.internal_priority = parsed.value as number
    fieldLabels.push('优先级')
  }

  if (state.applyRpmLimit) {
    const parsed = parseIntegerField(state.rpmLimit, 'RPM 限制', 1, 10000, true)
    if (parsed.error) return { patch: null, fieldLabels, error: parsed.error }
    patch.rpm_limit = parsed.value
    fieldLabels.push('RPM 限制')
  }

  if (state.applyConcurrentLimit) {
    const parsed = parseIntegerField(state.concurrentLimit, '并发请求上限', 0, undefined, true)
    if (parsed.error) return { patch: null, fieldLabels, error: parsed.error }
    patch.concurrent_limit = parsed.value
    fieldLabels.push('并发请求上限')
  }

  if (state.applyCacheTtl) {
    const parsed = parseIntegerField(state.cacheTtlMinutes, '缓存 TTL', 0, 60)
    if (parsed.error) return { patch: null, fieldLabels, error: parsed.error }
    patch.cache_ttl_minutes = parsed.value as number
    fieldLabels.push('缓存 TTL')
  }

  if (state.applyProbeInterval) {
    const parsed = parseIntegerField(state.maxProbeIntervalMinutes, '熔断探测', 0, 32)
    if (parsed.error) return { patch: null, fieldLabels, error: parsed.error }
    patch.max_probe_interval_minutes = parsed.value as number
    fieldLabels.push('熔断探测')
  }

  if (state.applyNote) {
    patch.note = state.note.trim() || null
    fieldLabels.push('备注')
  }

  if (state.applyAutoFetchModels) {
    patch.auto_fetch_models = state.autoFetchModels
    if (state.autoFetchModels) {
      patch.model_include_patterns = parsePoolKeyModelPatterns(state.includePatterns)
      patch.model_exclude_patterns = parsePoolKeyModelPatterns(state.excludePatterns)
    }
    fieldLabels.push('自动获取上游可用模型')
  }

  if (state.applyAllowedModels) {
    const selectedModels = uniqueTrimmed(state.selectedModels)
    if (!state.unrestrictedModels && selectedModels.length === 0) {
      return { patch: null, fieldLabels, error: '请至少选择一个可用模型' }
    }
    patch.allowed_models = state.unrestrictedModels ? null : selectedModels
    patch.locked_models = state.unrestrictedModels || !state.lockSelectedModels
      ? []
      : selectedModels
    fieldLabels.push('可用模型范围')
  }

  if (fieldLabels.length === 0) {
    return { patch: null, fieldLabels, error: '请至少启用一个批量编辑字段' }
  }

  return { patch, fieldLabels, error: null }
}
