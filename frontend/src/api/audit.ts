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

/**
 * Canonical request-outcome metrics returned by the admin user-behavior endpoint.
 *
 * `failed_requests` and `success_requests` are retained by the API as legacy
 * aliases, but the service SLA denominator is always `sla_eligible_count`.
 */
export interface AdminUserBehaviorResponse {
  user_id: string
  period_days: number
  event_counts: Record<string, number>
  total_requests: number
  sla_eligible_count: number
  success_count: number
  service_error_count: number
  user_error_count: number
  failed_requests: number
  success_requests: number
  success_rate: number
  service_error_rate: number
  user_error_rate: number
  suspicious_activities: number
  analysis_time: string
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

  // 分析用户行为 (管理员)
  async analyzeUserBehavior(userId: number | string, days: number = 7): Promise<AdminUserBehaviorResponse> {
    const response = await apiClient.get(`/api/admin/monitoring/user-behavior/${userId}`, {
      params: { days }
    })
    return response.data
  }
}
