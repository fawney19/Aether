import apiClient from './client'

export interface ProxyNodeRemoteConfig {
  node_name?: string
  allowed_ports?: number[]
  log_level?: string
  heartbeat_interval?: number
  upgrade_to?: string | null
}

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

export interface ProxyNodeListResponse {
  items: ProxyNode[]
  total: number
  skip: number
  limit: number
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

  async batchUpgrade(version: string): Promise<{ version: string; updated: number; skipped: number; node_ids: string[] }> {
    const response = await apiClient.post<{ version: string; updated: number; skipped: number; node_ids: string[] }>(
      '/api/admin/proxy-nodes/upgrade',
      { version }
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
