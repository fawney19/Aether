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
    const response = await apiClient.get<Record<string, unknown>>('/api/monitoring/my-audit-logs', { params: filters })
    return normalizeAuditResponse(response.data)
  },

  // 获取所有审计日志 (管理员)
  async getAuditLogs(filters?: AuditFilters): Promise<AuditLogsResponse> {
    const response = await apiClient.get<Record<string, unknown>>('/api/admin/monitoring/audit-logs', { params: filters })
    return normalizeAuditResponse(response.data)
  },

  // 获取可疑活动 (管理员)
  async getSuspiciousActivities(hours: number = 24, limit: number = 100): Promise<{
    activities: AuditLog[]
    count: number
  }> {
    const response = await apiClient.get<{
    activities: AuditLog[]
    count: number
  }>('/api/admin/monitoring/suspicious-activities', {
      params: { hours, limit }
    })
    return response.data
  },

  // 分析用户行为 (管理员)
  async analyzeUserBehavior(userId: number, days: number = 7): Promise<{
    analysis: Record<string, unknown>
    recommendations: string[]
  }> {
    const response = await apiClient.get<{
    analysis: Record<string, unknown>
    recommendations: string[]
  }>(`/api/admin/monitoring/user-behavior/${userId}`, {
      params: { days }
    })
    return response.data
  }
}
