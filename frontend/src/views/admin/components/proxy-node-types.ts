import type {
  ProxyNode,
  ProxyNodeEvent,
  ProxyNodeMetricsResponse,
  ProxyNodeSchedulingState,
} from '@/api/proxy-nodes'

export type ProxyNodeAddMode = 'script' | 'manual' | 'batch'
export type ProxyNodeInstallSystem = 'unix' | 'windows'

export interface ProxyNodeStatusFilterOption {
  value: string
  label: string
}

export interface ProxyNodeManualForm {
  name: string
  proxy_url: string
  username: string
  password: string
  region: string
}

export interface ProxyNodeBatchForm {
  content: string
}

export interface ProxyNodeInstallForm {
  node_name: string
}

export interface ProxyNodeConfigForm {
  allowed_ports: string
  log_level: string
  heartbeat_interval: string
  scheduling_state: ProxyNodeSchedulingState
  upgrade_to: string
}

export interface ProxyNodeDetailState {
  loading: boolean
  error: string | null
  node: ProxyNode | null
  metrics: ProxyNodeMetricsResponse | null
  events: ProxyNodeEvent[]
  loadedAt: number | null
}
