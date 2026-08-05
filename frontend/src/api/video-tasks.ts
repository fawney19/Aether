import apiClient from './client'

/**
 * Video generation tasks submitted by users, backed by the `video_tasks` table.
 *
 * These are distinct from the background worker runs in `./async-tasks`, which
 * cover system maintenance (backups, cleanup, quota resets) and are served by a
 * different API.
 */
export type VideoTaskStatus =
  | 'pending'
  | 'submitted'
  | 'queued'
  | 'processing'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'expired'
  | 'deleted'

export interface VideoTaskItem {
  id: string
  /** Correlates the task with its usage record. */
  request_id: string
  /**
   * Usage record primary key, null until the task settles.
   *
   * The admin usage detail endpoint looks records up by this id — passing
   * `request_id` there returns 404.
   */
  usage_id?: string | null
  external_task_id?: string | null
  user_id?: string | null
  username: string
  model?: string | null
  prompt?: string | null
  status: VideoTaskStatus
  progress_percent: number
  progress_message?: string | null
  provider_id?: string | null
  provider_name: string
  duration_seconds?: number | null
  resolution?: string | null
  aspect_ratio?: string | null
  video_url?: string | null
  error_code?: string | null
  error_message?: string | null
  poll_count?: number | null
  max_poll_count?: number | null
  /** Null until the task settles and a usage record exists. */
  input_tokens?: number | null
  output_tokens?: number | null
  total_tokens?: number | null
  cost?: number | null
  actual_cost?: number | null
  created_at?: string | null
  completed_at?: string | null
  submitted_at?: string | null
}

export interface VideoTaskListResponse {
  items: VideoTaskItem[]
  total: number
  page: number
  page_size: number
  pages: number
}

export interface VideoTaskStatsResponse {
  total: number
  by_status: Record<string, number>
  by_model: Record<string, number>
  today_count: number
  processing_count: number
}

export interface VideoTaskQueryParams {
  status?: VideoTaskStatus
  user_id?: string
  model?: string
  page?: number
  page_size?: number
}

export const videoTasksApi = {
  async list(params: VideoTaskQueryParams = {}): Promise<VideoTaskListResponse> {
    const searchParams = new URLSearchParams()
    if (params.status) searchParams.append('status', params.status)
    if (params.user_id) searchParams.append('user_id', params.user_id)
    if (params.model) searchParams.append('model', params.model)
    if (params.page) searchParams.append('page', params.page.toString())
    if (params.page_size) searchParams.append('page_size', params.page_size.toString())

    const query = searchParams.toString()
    const url = query ? `/api/admin/video-tasks?${query}` : '/api/admin/video-tasks'
    const response = await apiClient.get<VideoTaskListResponse>(url)
    return response.data
  },

  async getStats(): Promise<VideoTaskStatsResponse> {
    const response = await apiClient.get<VideoTaskStatsResponse>('/api/admin/video-tasks/stats')
    return response.data
  },

  async getDetail(taskId: string): Promise<VideoTaskItem & Record<string, unknown>> {
    const response = await apiClient.get<VideoTaskItem & Record<string, unknown>>(
      `/api/admin/video-tasks/${taskId}`,
    )
    return response.data
  },

  async cancel(taskId: string): Promise<{ id: string; status: string; message: string }> {
    const response = await apiClient.post<{ id: string; status: string; message: string }>(
      `/api/admin/video-tasks/${taskId}/cancel`,
    )
    return response.data
  },

  videoUrl(taskId: string, token?: string | null): string {
    if (token) {
      return `/api/admin/video-tasks/${taskId}/video?token=${encodeURIComponent(token)}`
    }
    return `/api/admin/video-tasks/${taskId}/video`
  },
}

export default videoTasksApi
