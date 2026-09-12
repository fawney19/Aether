/**
 * IP 安全管理 API
 */
import apiClient from './client'

export interface IPBlacklistEntry {
  ip_address: string
  reason: string
  ttl?: number
}

export interface BlacklistListEntry {
  ip_address: string
  reason: string
  ttl_seconds?: number | null
}

export interface BlacklistResponse {
  items: BlacklistListEntry[]
  total: number
}

export interface IPWhitelistEntry {
  ip_address: string
}

export interface BlacklistStats {
  available: boolean
  total: number
  error?: string
}

export interface WhitelistResponse {
  whitelist: string[]
  total: number
}

/**
 * IP 黑名单管理
 */
export const blacklistApi = {
  /**
   * 添加 IP 到黑名单
   */
  async add(data: IPBlacklistEntry) {
    const response = await apiClient.post('/api/admin/security/ip/blacklist', data)
    return response.data
  },

  /**
   * 从黑名单移除 IP
   */
  async remove(ip_address: string) {
    const response = await apiClient.delete(`/api/admin/security/ip/blacklist/${encodeURIComponent(ip_address)}`)
    return response.data
  },

  /**
   * 获取黑名单统计
   */
  async getStats(): Promise<BlacklistStats> {
    const response = await apiClient.get<BlacklistStats>('/api/admin/security/ip/blacklist/stats')
    return response.data
  },

  /**
   * 获取黑名单列表
   */
  async getList(): Promise<BlacklistResponse> {
    const response = await apiClient.get<BlacklistResponse>('/api/admin/security/ip/blacklist')
    return response.data
  }
}

/**
 * IP 白名单管理
 */
export const whitelistApi = {
  /**
   * 添加 IP 到白名单
   */
  async add(data: IPWhitelistEntry) {
    const response = await apiClient.post('/api/admin/security/ip/whitelist', data)
    return response.data
  },

  /**
   * 从白名单移除 IP
   */
  async remove(ip_address: string) {
    const response = await apiClient.delete(`/api/admin/security/ip/whitelist/${encodeURIComponent(ip_address)}`)
    return response.data
  },

  /**
   * 获取白名单列表
   */
  async getList(): Promise<WhitelistResponse> {
    const response = await apiClient.get<WhitelistResponse>('/api/admin/security/ip/whitelist')
    return response.data
  }
}
