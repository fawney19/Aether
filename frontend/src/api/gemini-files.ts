/**
 * Gemini Files 管理 API
 */

import apiClient from './client'

export interface FileMappingResponse {
  id: string
  file_name: string
  key_id: string
  key_name: string | null
  user_id: string | null
  username: string | null
  display_name: string | null
  mime_type: string | null
  created_at: string
  expires_at: string
  is_expired: boolean
}

export interface FileMappingListResponse {
  items: FileMappingResponse[]
  total: number
  page: number
  page_size: number
}

export interface FileMappingStatsResponse {
  total_mappings: number
  active_mappings: number
  expired_mappings: number
  by_mime_type: Record<string, number>
  capable_keys_count: number
}

export interface ListMappingsParams {
  page?: number
  page_size?: number
  include_expired?: boolean
  search?: string
}

export interface CapableKeyResponse {
  id: string
  name: string
  provider_name: string | null
}

export interface UploadResultItem {
  key_id: string
  key_name: string | null
  success: boolean
  file_name: string | null
  error: string | null
}

export interface UploadResponse {
  display_name: string
  mime_type: string
  size_bytes: number
  results: UploadResultItem[]
  success_count: number
  fail_count: number
}

export const geminiFilesApi = {
  /**
   * 获取文件映射统计
   */
  async getStats(): Promise<FileMappingStatsResponse> {
    const response = await apiClient.get<FileMappingStatsResponse>('/api/admin/gemini-files/stats')
    return response.data
  },

  /**
   * 列出文件映射
   */
  async listMappings(params?: ListMappingsParams): Promise<FileMappingListResponse> {
    const response = await apiClient.get<FileMappingListResponse>('/api/admin/gemini-files/mappings', { params })
    return response.data
  },

  /**
   * 删除指定映射
   */
  async deleteMapping(mappingId: string): Promise<{ message: string; file_name: string }> {
    const response = await apiClient.delete<{ message: string; file_name: string }>(`/api/admin/gemini-files/mappings/${mappingId}`)
    return response.data
  },

  /**
   * 清理过期映射
   */
  async cleanupExpired(): Promise<{ message: string; deleted_count: number }> {
    const response = await apiClient.delete<{ message: string; deleted_count: number }>('/api/admin/gemini-files/mappings')
    return response.data
  },

  /**
   * 获取可用的 Key 列表
   */
  async getCapableKeys(): Promise<CapableKeyResponse[]> {
    const response = await apiClient.get<CapableKeyResponse[]>('/api/admin/gemini-files/capable-keys')
    return response.data
  },

  /**
   * 上传文件到指定的 Keys
   */
  async uploadFile(file: File, keyIds: string[]): Promise<UploadResponse> {
    const formData = new FormData()
    formData.append('file', file)
    const response = await apiClient.post<UploadResponse>(
      `/api/admin/gemini-files/upload?key_ids=${keyIds.join(',')}`,
      formData,
      {
        headers: {
          'Content-Type': 'multipart/form-data'
        }
      }
    )
    return response.data
  }
}
