import apiClient from './client'

export interface AuditLog {
  id: string
  event_type: string
  user_id?: number
  description: string
  ip_address?: string
  status_code?: number
  error_message?: string
  metadata?: Record<string, unknown>
  created_at: string
}

export interface PaginationMeta {
  total: number
  limit: number
  offset: number
  count: number
}

export interface AuditLogsResponse {
  items: AuditLog[]
  meta: PaginationMeta
  filters?: Record<string, unknown>
}

export interface AuditFilters {
  username?: string
  event_type?: string
  days?: number
  limit?: number
  offset?: number
}

export type MonitoringMetricStatus = 'ok' | 'warning' | 'danger' | 'degraded' | 'error' | 'unknown'

export interface MonitoringCpuMetric {
  status: MonitoringMetricStatus
  label: string
  usage_percent: number | null
  load_percent: number | null
  core_count: number
  message?: string | null
}

export interface MonitoringMemoryMetric {
  status: MonitoringMetricStatus
  label: string
  used_percent: number | null
  used_bytes: number | null
  available_bytes: number | null
  total_bytes: number | null
  message?: string | null
}

export interface MonitoringServiceMetric {
  status: MonitoringMetricStatus
  label: string
  latency_ms: number | null
  memory_status?: MonitoringMetricStatus
  memory_label?: string | null
  used_memory_bytes?: number | null
  peak_memory_bytes?: number | null
  maxmemory_bytes?: number | null
  memory_ceiling_bytes?: number | null
  memory_source?: 'configured' | 'maxmemory' | 'system' | 'unknown'
  available_memory_bytes?: number | null
  memory_percent?: number | null
  message?: string | null
}

export interface MonitoringPostgresMetric {
  status: MonitoringMetricStatus
  label: string
  usage_percent: number | null
  pool_usage_percent?: number | null
  checked_out: number
  pool_size: number
  overflow: number
  max_capacity: number
  pool_timeout: number
  server_connections?: number | null
  server_max_connections?: number | null
  server_usage_percent?: number | null
  storage_status?: MonitoringMetricStatus
  storage_label?: string | null
  storage_total_bytes?: number | null
  storage_free_bytes?: number | null
  storage_free_percent?: number | null
  database_size_bytes?: number | null
  storage_message?: string | null
  message?: string | null
}

export interface MonitoringSystemStatus {
  timestamp: string
  users: {
    total: number
    active: number
  }
  providers: {
    total: number
    active: number
  }
  api_keys: {
    total: number
    active: number
  }
  today_stats: {
    requests: number
    tokens: number
    cost_usd: number
  }
  recent_errors: number
  system_metrics?: {
    cpu: MonitoringCpuMetric
    memory: MonitoringMemoryMetric
    redis: MonitoringServiceMetric
    postgres: MonitoringPostgresMetric
  }
}

function parseNumericLike(value: unknown): number {
  if (typeof value === 'number') {
    return Number.isFinite(value) ? value : 0
  }
  if (typeof value === 'string') {
    const normalized = value.trim().replace(/[$,\s]/g, '')
    if (!normalized) return 0
    const parsed = Number(normalized)
    return Number.isFinite(parsed) ? parsed : 0
  }
  return 0
}

function parseNullableNumericLike(value: unknown): number | null {
  if (value == null) {
    return null
  }
  if (typeof value === 'number') {
    return Number.isFinite(value) ? value : null
  }
  if (typeof value === 'string') {
    const normalized = value.trim().replace(/[$,\s]/g, '')
    if (!normalized) return null
    const parsed = Number(normalized)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function normalizeMonitoringSystemStatus(data: MonitoringSystemStatus): MonitoringSystemStatus {
  return {
    ...data,
    users: {
      total: parseNumericLike(data.users?.total),
      active: parseNumericLike(data.users?.active),
    },
    providers: {
      total: parseNumericLike(data.providers?.total),
      active: parseNumericLike(data.providers?.active),
    },
    api_keys: {
      total: parseNumericLike(data.api_keys?.total),
      active: parseNumericLike(data.api_keys?.active),
    },
    today_stats: {
      requests: parseNumericLike(data.today_stats?.requests),
      tokens: parseNumericLike(data.today_stats?.tokens),
      cost_usd: parseNumericLike(data.today_stats?.cost_usd),
    },
    recent_errors: parseNumericLike(data.recent_errors),
    system_metrics: data.system_metrics ? {
      cpu: {
        status: data.system_metrics.cpu?.status ?? 'unknown',
        label: data.system_metrics.cpu?.label ?? '未知',
        usage_percent: parseNullableNumericLike(data.system_metrics.cpu?.usage_percent),
        load_percent: parseNullableNumericLike(data.system_metrics.cpu?.load_percent),
        core_count: parseNumericLike(data.system_metrics.cpu?.core_count),
        message: data.system_metrics.cpu?.message ?? null,
      },
      memory: {
        status: data.system_metrics.memory?.status ?? 'unknown',
        label: data.system_metrics.memory?.label ?? '未知',
        used_percent: parseNullableNumericLike(data.system_metrics.memory?.used_percent),
        used_bytes: parseNullableNumericLike(data.system_metrics.memory?.used_bytes),
        available_bytes: parseNullableNumericLike(data.system_metrics.memory?.available_bytes),
        total_bytes: parseNullableNumericLike(data.system_metrics.memory?.total_bytes),
        message: data.system_metrics.memory?.message ?? null,
      },
      redis: {
        status: data.system_metrics.redis?.status ?? 'unknown',
        label: data.system_metrics.redis?.label ?? '未知',
        latency_ms: parseNullableNumericLike(data.system_metrics.redis?.latency_ms),
        memory_status: data.system_metrics.redis?.memory_status ?? 'unknown',
        memory_label: data.system_metrics.redis?.memory_label ?? '未知',
        used_memory_bytes: parseNullableNumericLike(data.system_metrics.redis?.used_memory_bytes),
        peak_memory_bytes: parseNullableNumericLike(data.system_metrics.redis?.peak_memory_bytes),
        maxmemory_bytes: parseNullableNumericLike(data.system_metrics.redis?.maxmemory_bytes),
        memory_ceiling_bytes: parseNullableNumericLike(data.system_metrics.redis?.memory_ceiling_bytes),
        memory_source: (data.system_metrics.redis?.memory_source as MonitoringServiceMetric['memory_source']) ?? 'unknown',
        available_memory_bytes: parseNullableNumericLike(data.system_metrics.redis?.available_memory_bytes),
        memory_percent: parseNullableNumericLike(data.system_metrics.redis?.memory_percent),
        message: data.system_metrics.redis?.message ?? null,
      },
      postgres: {
        status: data.system_metrics.postgres?.status ?? 'unknown',
        label: data.system_metrics.postgres?.label ?? '未知',
        usage_percent: parseNullableNumericLike(data.system_metrics.postgres?.usage_percent),
        pool_usage_percent: parseNullableNumericLike(data.system_metrics.postgres?.pool_usage_percent),
        checked_out: parseNumericLike(data.system_metrics.postgres?.checked_out),
        pool_size: parseNumericLike(data.system_metrics.postgres?.pool_size),
        overflow: parseNumericLike(data.system_metrics.postgres?.overflow),
        max_capacity: parseNumericLike(data.system_metrics.postgres?.max_capacity),
        pool_timeout: parseNumericLike(data.system_metrics.postgres?.pool_timeout),
        server_connections: parseNullableNumericLike(data.system_metrics.postgres?.server_connections),
        server_max_connections: parseNullableNumericLike(data.system_metrics.postgres?.server_max_connections),
        server_usage_percent: parseNullableNumericLike(data.system_metrics.postgres?.server_usage_percent),
        storage_status: data.system_metrics.postgres?.storage_status ?? 'unknown',
        storage_label: data.system_metrics.postgres?.storage_label ?? '未知',
        storage_total_bytes: parseNullableNumericLike(data.system_metrics.postgres?.storage_total_bytes),
        storage_free_bytes: parseNullableNumericLike(data.system_metrics.postgres?.storage_free_bytes),
        storage_free_percent: parseNullableNumericLike(data.system_metrics.postgres?.storage_free_percent),
        database_size_bytes: parseNullableNumericLike(data.system_metrics.postgres?.database_size_bytes),
        storage_message: data.system_metrics.postgres?.storage_message ?? null,
        message: data.system_metrics.postgres?.message ?? null,
      },
    } : undefined,
  }
}

function normalizeAuditResponse(data: Record<string, unknown>): AuditLogsResponse {
  const items: AuditLog[] = (data.items ?? data.logs ?? []) as AuditLog[]
  const meta: PaginationMeta = (data.meta as PaginationMeta) ?? {
    total: (data.total as number) ?? items.length,
    limit: (data.limit as number) ?? items.length,
    offset: (data.offset as number) ?? 0,
    count: (data.count as number) ?? items.length
  }

  return {
    items,
    meta,
    filters: data.filters as Record<string, unknown> | undefined
  }
}

export const auditApi = {
  // 获取当前用户的活动日志
  async getMyAuditLogs(filters?: {
    event_type?: string
    days?: number
    limit?: number
    offset?: number
  }): Promise<AuditLogsResponse> {
    const response = await apiClient.get('/api/monitoring/my-audit-logs', { params: filters })
    return normalizeAuditResponse(response.data)
  },

  // 获取所有审计日志 (管理员)
  async getAuditLogs(filters?: AuditFilters): Promise<AuditLogsResponse> {
    const response = await apiClient.get('/api/admin/monitoring/audit-logs', { params: filters })
    return normalizeAuditResponse(response.data)
  },

  // 获取可疑活动 (管理员)
  async getSuspiciousActivities(hours: number = 24, limit: number = 100): Promise<{
    activities: AuditLog[]
    count: number
  }> {
    const response = await apiClient.get('/api/admin/monitoring/suspicious-activities', {
      params: { hours, limit }
    })
    return response.data
  },

  async getSystemStatus(): Promise<MonitoringSystemStatus> {
    const response = await apiClient.get<MonitoringSystemStatus>('/api/admin/monitoring/system-status')
    return normalizeMonitoringSystemStatus(response.data)
  },

  // 分析用户行为 (管理员)
  async analyzeUserBehavior(userId: number, days: number = 7): Promise<{
    analysis: Record<string, unknown>
    recommendations: string[]
  }> {
    const response = await apiClient.get(`/api/admin/monitoring/user-behavior/${userId}`, {
      params: { days }
    })
    return response.data
  }
}
