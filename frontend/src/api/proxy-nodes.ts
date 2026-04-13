import apiClient from './client'

export interface ProxyNodeRemoteConfig {
  node_name?: string
  allowed_ports?: number[]
  log_level?: string
  heartbeat_interval?: number
  scheduling_state?: ProxyNodeSchedulingState | null
  upgrade_to?: string | null
}

export type ProxyNodeSchedulingState = 'active' | 'draining' | 'cordoned'

export interface ProxyNode {
  id: string
  name: string
  ip: string
  port: number
  region: string | null
  status: 'online' | 'offline'
  is_manual: boolean
  tunnel_mode: boolean
  tunnel_connected: boolean
  tunnel_connected_at: string | null
  // 手动节点专用字段
  proxy_url?: string
  proxy_username?: string
  proxy_password?: string  // 脱敏后的密码
  // 硬件信息（aether-proxy 节点）
  hardware_info: Record<string, unknown> | null
  estimated_max_concurrency: number | null
  // 远程配置（aether-proxy 节点）
  remote_config: ProxyNodeRemoteConfig | null
  config_version: number
  registered_by: string | null
  last_heartbeat_at: string | null
  heartbeat_interval: number
  active_connections: number
  total_requests: number
  avg_latency_ms: number | null
  failed_requests: number
  dns_failures: number
  stream_errors: number
  proxy_metadata: Record<string, unknown> | null
  created_at: string
  updated_at: string
}

export interface ProxyNodeEvent {
  id: number
  event_type: 'connected' | 'disconnected' | 'error'
  detail: string | null
  created_at: string
}

export interface ProxyNodeUpgradeRolloutProbe {
  url: string
  timeout_secs: number
}

export type ProxyNodeUpgradeRolloutTrackedNodeState =
  | 'awaiting_version'
  | 'awaiting_traffic'
  | 'cooling_down'
  | 'unhealthy'
  | 'ready_to_finalize'

export interface ProxyNodeUpgradeRolloutTrackedNode {
  node_id: string
  state: ProxyNodeUpgradeRolloutTrackedNodeState
  dispatched_at: string | null
  version_confirmed_at: string | null
  traffic_confirmed_at: string | null
  cooldown_remaining_secs: number | null
}

export interface ProxyNodeUpgradeRolloutStatus {
  version: string
  batch_size: number
  cooldown_secs: number
  started_at: string | null
  last_dispatched_at: string | null
  updated_at: string | null
  probe: ProxyNodeUpgradeRolloutProbe | null
  blocked: boolean
  online_eligible_total: number
  completed_node_ids: string[]
  pending_node_ids: string[]
  conflict_node_ids: string[]
  skipped_node_ids: string[]
  tracked_nodes: ProxyNodeUpgradeRolloutTrackedNode[]
}

export interface ProxyNodeListResponse {
  items: ProxyNode[]
  total: number
  skip: number
  limit: number
  rollout: ProxyNodeUpgradeRolloutStatus | null
}

export interface ManualProxyNodeCreateRequest {
  name: string
  proxy_url: string
  username?: string
  password?: string
  region?: string
}

export interface ManualProxyNodeUpdateRequest {
  name?: string
  proxy_url?: string
  username?: string
  password?: string
  region?: string
}

export interface ProxyNodeTestResult {
  success: boolean
  latency_ms: number | null
  exit_ip: string | null
  error: string | null
}

export interface ProxyNodeBatchUpgradeResult {
  version: string
  batch_size: number
  cooldown_secs: number
  probe_url?: string | null
  probe_timeout_secs?: number | null
  updated: number
  skipped: number
  node_ids: string[]
  blocked: boolean
  pending_node_ids: string[]
  rollout_active: boolean
  completed: number
  remaining: number
}

export interface ProxyNodeUpgradeRolloutCancelResult {
  cancelled: boolean
  rollout_active?: boolean
  version?: string | null
  pending_node_ids?: string[]
  conflict_node_ids?: string[]
  completed?: number
  remaining?: number
}

export interface ProxyNodeUpgradeRolloutClearConflictsResult {
  version: string | null
  cleared: number
  node_ids: string[]
  updated: number
  blocked: boolean
  pending_node_ids: string[]
  rollout_active: boolean
  completed: number
  remaining: number
}

export interface ProxyNodeUpgradeRolloutRestoreSkippedResult {
  version: string | null
  restored: number
  node_ids: string[]
  skipped_node_ids: string[]
  updated: number
  blocked: boolean
  pending_node_ids: string[]
  rollout_active: boolean
  completed: number
  remaining: number
}

export interface ProxyNodeUpgradeRolloutNodeActionResult {
  version: string | null
  node_id: string
  skipped_node_ids: string[]
  updated: number
  blocked: boolean
  pending_node_ids: string[]
  rollout_active: boolean
  completed: number
  remaining: number
}

export const proxyNodesApi = {
  async listProxyNodes(params?: { status?: string; skip?: number; limit?: number }): Promise<ProxyNodeListResponse> {
    const response = await apiClient.get<ProxyNodeListResponse>('/api/admin/proxy-nodes', { params })
    return response.data
  },

  async createManualNode(data: ManualProxyNodeCreateRequest): Promise<{ node_id: string; node: ProxyNode }> {
    const response = await apiClient.post<{ node_id: string; node: ProxyNode }>('/api/admin/proxy-nodes/manual', data)
    return response.data
  },

  async updateManualNode(nodeId: string, data: ManualProxyNodeUpdateRequest): Promise<{ node_id: string; node: ProxyNode }> {
    const response = await apiClient.patch<{ node_id: string; node: ProxyNode }>(`/api/admin/proxy-nodes/${nodeId}`, data)
    return response.data
  },

  async deleteProxyNode(nodeId: string): Promise<{ message: string; node_id: string; cleared_system_proxy: boolean }> {
    const response = await apiClient.delete<{ message: string; node_id: string; cleared_system_proxy: boolean }>(`/api/admin/proxy-nodes/${nodeId}`)
    return response.data
  },

  async testNode(nodeId: string): Promise<ProxyNodeTestResult> {
    const response = await apiClient.post<ProxyNodeTestResult>(`/api/admin/proxy-nodes/${nodeId}/test`)
    return response.data
  },

  async updateNodeConfig(nodeId: string, data: ProxyNodeRemoteConfig): Promise<{ node_id: string; config_version: number; remote_config: ProxyNodeRemoteConfig; node: ProxyNode }> {
    const response = await apiClient.put<{ node_id: string; config_version: number; remote_config: ProxyNodeRemoteConfig; node: ProxyNode }>(`/api/admin/proxy-nodes/${nodeId}/config`, data)
    return response.data
  },

  async batchUpgrade(
    version: string,
    options?: {
      batch_size?: number
      cooldown_secs?: number
      probe_url?: string | null
      probe_timeout_secs?: number
    },
  ): Promise<ProxyNodeBatchUpgradeResult> {
    const response = await apiClient.post<ProxyNodeBatchUpgradeResult>(
      '/api/admin/proxy-nodes/upgrade',
      { version, ...options }
    )
    return response.data
  },

  async cancelUpgradeRollout(): Promise<ProxyNodeUpgradeRolloutCancelResult> {
    const response = await apiClient.post<ProxyNodeUpgradeRolloutCancelResult>(
      '/api/admin/proxy-nodes/upgrade/cancel'
    )
    return response.data
  },

  async clearUpgradeConflicts(): Promise<ProxyNodeUpgradeRolloutClearConflictsResult> {
    const response = await apiClient.post<ProxyNodeUpgradeRolloutClearConflictsResult>(
      '/api/admin/proxy-nodes/upgrade/clear-conflicts'
    )
    return response.data
  },

  async restoreSkippedUpgradeNodes(): Promise<ProxyNodeUpgradeRolloutRestoreSkippedResult> {
    const response = await apiClient.post<ProxyNodeUpgradeRolloutRestoreSkippedResult>(
      '/api/admin/proxy-nodes/upgrade/restore-skipped'
    )
    return response.data
  },

  async skipUpgradeNode(nodeId: string): Promise<ProxyNodeUpgradeRolloutNodeActionResult> {
    const response = await apiClient.post<ProxyNodeUpgradeRolloutNodeActionResult>(
      `/api/admin/proxy-nodes/${nodeId}/upgrade/skip`
    )
    return response.data
  },

  async retryUpgradeNode(nodeId: string): Promise<ProxyNodeUpgradeRolloutNodeActionResult> {
    const response = await apiClient.post<ProxyNodeUpgradeRolloutNodeActionResult>(
      `/api/admin/proxy-nodes/${nodeId}/upgrade/retry`
    )
    return response.data
  },

  async testProxyUrl(data: { proxy_url: string; username?: string; password?: string }): Promise<ProxyNodeTestResult> {
    const response = await apiClient.post<ProxyNodeTestResult>('/api/admin/proxy-nodes/test-url', data)
    return response.data
  },

  async listNodeEvents(nodeId: string, limit = 50): Promise<{ items: ProxyNodeEvent[] }> {
    const response = await apiClient.get<{ items: ProxyNodeEvent[] }>(`/api/admin/proxy-nodes/${nodeId}/events`, { params: { limit } })
    return response.data
  },
}
