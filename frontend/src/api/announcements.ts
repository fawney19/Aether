import apiClient from './client'

export interface Announcement {
  id: string // UUID
  title: string
  content: string  // Markdown格式
  type: 'info' | 'warning' | 'maintenance' | 'important'
  priority: number
  is_pinned: boolean
  is_active: boolean
  requires_ack: boolean
  author: {
    id: string // UUID
    username: string
  }
  start_time?: string
  end_time?: string
  created_at: string
  updated_at: string
  is_read?: boolean
}

export interface AnnouncementListResponse {
  items: Announcement[]
  total: number
  unread_count: number
}

export interface CreateAnnouncementRequest {
  title: string
  content: string
  type?: 'info' | 'warning' | 'maintenance' | 'important'
  priority?: number
  is_pinned?: boolean
  requires_ack?: boolean
  start_time?: string
  end_time?: string
}

export interface UpdateAnnouncementRequest {
  title?: string
  content?: string
  type?: string
  priority?: number
  is_active?: boolean
  is_pinned?: boolean
  requires_ack?: boolean
  start_time?: string
  end_time?: string
}

export const announcementApi = {
  // 获取公告列表
  async getAnnouncements(params?: {
    active_only?: boolean
    unread_only?: boolean
    limit?: number
    offset?: number
  }): Promise<AnnouncementListResponse> {
    const response = await apiClient.get<AnnouncementListResponse>('/api/announcements', { params })
    return response.data
  },

  // 获取当前有效的公告
  async getActiveAnnouncements(): Promise<AnnouncementListResponse> {
    const response = await apiClient.get<AnnouncementListResponse>('/api/announcements/active')
    return response.data
  },

  // 获取单个公告
  async getAnnouncement(id: string): Promise<Announcement> {
    const response = await apiClient.get<Announcement>(`/api/announcements/${id}`)
    return response.data
  },

  // 标记公告为已读
  async markAsRead(id: string): Promise<{ message: string }> {
    const response = await apiClient.patch<{ message: string }>(`/api/announcements/${id}/read-status`)
    return response.data
  },

  // 标记所有公告为已读
  async markAllAsRead(): Promise<{ message: string }> {
    const response = await apiClient.post<{ message: string }>('/api/announcements/read-all')
    return response.data
  },

  // 获取未读公告数量
  async getUnreadCount(): Promise<{ unread_count: number }> {
    const response = await apiClient.get<{ unread_count: number }>('/api/announcements/users/me/unread-count')
    return response.data
  },

  async getRequiredUnreadAnnouncements(): Promise<AnnouncementListResponse> {
    const response = await apiClient.get<AnnouncementListResponse>('/api/announcements/users/me/required-unread')
    return response.data
  },

  // 管理员方法
  // 创建公告
  async createAnnouncement(data: CreateAnnouncementRequest): Promise<{ id: string; title: string; message: string }> {
    const response = await apiClient.post<{ id: string; title: string; message: string }>('/api/announcements', data)
    return response.data
  },

  // 更新公告
  async updateAnnouncement(id: string, data: UpdateAnnouncementRequest): Promise<{ message: string }> {
    const response = await apiClient.put<{ message: string }>(`/api/announcements/${id}`, data)
    return response.data
  },

  // 删除公告
  async deleteAnnouncement(id: string): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string }>(`/api/announcements/${id}`)
    return response.data
  }
}
