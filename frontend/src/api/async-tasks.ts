import apiClient from './client'

export type AsyncTaskStatus =
  | 'queued'
  | 'running'
  | 'retrying'
  | 'succeeded'
  | 'failed'
  | 'cancelled'
  | 'skipped'
  | 'pending'
  | 'submitted'
  | 'processing'
  | 'completed'

export type AsyncTaskKind = 'scheduled' | 'daemon' | 'on_demand' | 'fire_and_forget'

/**
 * Background worker runs (backups, cleanup, quota resets, pollers).
 *
 * User-submitted video generation tasks are a separate system with its own
 * table and API; see `./video-tasks`. `/api/admin/tasks` never returns them.
 */
export interface AsyncTaskDefinition {
  task_key: string
  kind: AsyncTaskKind
  trigger: string
  max_attempts: number
  singleton: boolean
  persist_history: boolean
}

export interface AsyncTaskItem {
  id: string
  task_key?: string
  kind?: AsyncTaskKind
  trigger?: string
  task_type?: AsyncTaskKind
  external_task_id?: string
  user_id?: string
  username?: string
  model?: string
  prompt?: string
  status: AsyncTaskStatus
  attempt?: number
  max_attempts?: number
  owner_instance?: string | null
  progress_percent: number
  progress_message: string | null
  payload?: unknown
  result?: unknown
  error_message: string | null
  cancel_requested?: boolean
  created_by?: string | null
  provider_id?: string
  provider_name?: string
  duration_seconds?: number
  resolution?: string
  aspect_ratio?: string
  video_url?: string | null
  error_code?: string | null
  poll_count?: number
  max_poll_count?: number
  created_at: string
  started_at?: string | null
  updated_at?: string | null
  finished_at?: string | null
  completed_at?: string | null
  submitted_at?: string | null
}

export interface AsyncTaskEvent {
  id: string
  run_id: string
  event_type: string
  message: string
  payload: unknown
  created_at: string
}

export interface CandidateKeyInfo {
  index: number
  provider_id: string
  provider_name: string
  endpoint_id: string
  key_id: string
  key_name: string | null
  auth_type: string
  has_billing_rule: boolean
  priority: number
  selected?: boolean
}

export interface AsyncTaskRequestMetadata {
  candidate_keys: CandidateKeyInfo[]
  selected_key_id: string
  selected_endpoint_id: string
  client_ip: string
  user_agent: string
  request_id: string
  request_headers?: Record<string, string>
  poll_raw_response?: unknown
  billing_snapshot?: unknown
}

export interface AsyncTaskDetail extends AsyncTaskItem {
  api_key_id?: string
  endpoint_id?: string
  key_id?: string
  client_api_format?: string
  provider_api_format?: string
  format_converted?: boolean
  original_request_body?: unknown
  converted_request_body?: unknown
  size?: string | null
  video_urls?: string[] | null
  thumbnail_url?: string | null
  video_size_bytes?: number | null
  video_duration_seconds?: number | null
  video_expires_at?: string | null
  stored_video_path?: string | null
  storage_provider?: string | null
  retry_count?: number
  max_retries?: number
  poll_interval_seconds?: number
  next_poll_at?: string | null
  endpoint?: {
    id: string
    base_url: string
    api_format: string
  } | null
  request_metadata?: AsyncTaskRequestMetadata | null
}

export interface AsyncTaskListResponse {
  items: AsyncTaskItem[]
  total: number
  page: number
  page_size: number
  pages: number
  definitions?: AsyncTaskDefinition[]
}

export interface AsyncTaskStatsResponse {
  total: number
  running_count?: number
  registered_tasks?: number
  by_status: Partial<Record<AsyncTaskStatus, number>>
  by_kind?: Partial<Record<AsyncTaskKind, number>>
  by_model?: Record<string, number>
  today_count?: number
  active_users?: number
  processing_count?: number
}

export interface AsyncTaskQueryParams {
  status?: AsyncTaskStatus
  kind?: AsyncTaskKind
  task_type?: AsyncTaskKind
  task_key?: string
  trigger?: string
  user_id?: string
  model?: string
  page?: number
  page_size?: number
}

function normalizeStatus(status: AsyncTaskStatus): AsyncTaskStatus {
  if (status === 'processing') return 'running'
  if (status === 'submitted' || status === 'pending') return 'queued'
  if (status === 'completed') return 'succeeded'
  return status
}

export const asyncTasksApi = {
  async list(params: AsyncTaskQueryParams = {}): Promise<AsyncTaskListResponse> {
    const searchParams = new URLSearchParams()
    if (params.status) searchParams.append('status', normalizeStatus(params.status))
    if (params.kind) searchParams.append('kind', params.kind)
    if (params.task_type) searchParams.append('kind', params.task_type)
    if (params.task_key) searchParams.append('task_key', params.task_key)
    if (params.trigger) searchParams.append('trigger', params.trigger)
    if (params.page) searchParams.append('page', params.page.toString())
    if (params.page_size) searchParams.append('page_size', params.page_size.toString())

    const query = searchParams.toString()
    const url = query ? `/api/admin/tasks?${query}` : '/api/admin/tasks'
    const response = await apiClient.get(url)
    return response.data
  },

  async getStats(): Promise<AsyncTaskStatsResponse> {
    const response = await apiClient.get('/api/admin/tasks/stats')
    return response.data
  },

  async getDetail(taskId: string): Promise<AsyncTaskDetail> {
    const response = await apiClient.get(`/api/admin/tasks/${taskId}`)
    return response.data
  },

  async getEvents(taskId: string): Promise<{ items: AsyncTaskEvent[] }> {
    const response = await apiClient.get(`/api/admin/tasks/${taskId}/events`)
    return response.data
  },

  async cancel(taskId: string): Promise<{ id: string; status: string; message: string }> {
    const response = await apiClient.post(`/api/admin/tasks/${taskId}/cancel`)
    return response.data
  },

  async trigger(taskKey: string, payload: Record<string, unknown> = {}): Promise<{ run_id: string; status: string }> {
    const response = await apiClient.post(`/api/admin/tasks/${taskKey}/trigger`, payload)
    return response.data
  },
}

export default asyncTasksApi
