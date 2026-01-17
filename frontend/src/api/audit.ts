import apiClient from './client'

export interface AuditLog {
  id: string
  event_type: string
  user_id?: number
  description: string
  ip_address?: string
  status_code?: number
  error_message?: string
  metadata?: any
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
  filters?: Record<string, any>
}

export interface AuditFilters {
  username?: string
  event_type?: string
  days?: number
  limit?: number
  offset?: number
}

function normalizeAuditResponse(data: any): AuditLogsResponse {
  const items: AuditLog[] = data.items ?? data.logs ?? []
  const meta: PaginationMeta = data.meta ?? {
    total: data.total ?? items.length,
    limit: data.limit ?? items.length,
    offset: data.offset ?? 0,
    count: data.count ?? items.length
  }

  return {
    items,
    meta,
    filters: data.filters
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
  async analyzeUserBehavior(userId: number, days: number = 7): Promise<{
    analysis: any
    recommendations: string[]
  }> {
    const response = await apiClient.get(`/api/admin/monitoring/user-behavior/${userId}`, {
      params: { days }
    })
    return response.data
  }
}
