import apiClient from './client'
import {
  findMetricValueNumber,
  parsePrometheusSamples,
  sumMetricValues,
} from '@/utils/prometheus'

export interface AdminMonitoringSystemStatus {
  timestamp: string
  users: {
    total: number
    active: number
  }
  providers: {
    total: number
    active: number
  }
  api_keys: {
    total: number
    active: number
  }
  today_stats: {
    requests: number
    tokens: number
    cost_usd: string
  }
  tunnel: {
    proxy_connections: number
    nodes: number
    active_streams: number
  }
  internal_gateway: {
    status: string
    path_prefixes: string[]
  }
  recent_errors: number
}

export interface AdminMonitoringCircuitBreakerSummary {
  state: string
  provider_id?: string
  provider_name?: string | null
  key_name?: string | null
  health_score?: number
  consecutive_failures?: number
  last_failure_at?: string | null
  open_formats?: string[]
}

export interface AdminMonitoringErrorStatistics {
  total_errors: number
  active_keys: number
  degraded_keys: number
  unhealthy_keys: number
  open_circuit_breakers: number
  circuit_breakers: Record<string, AdminMonitoringCircuitBreakerSummary>
}

export interface AdminMonitoringRecentError {
  error_id: string
  error_type: string
  operation: string
  timestamp: string | null
  context: {
    request_id?: string | null
    provider_id?: string | null
    provider_name?: string | null
    model?: string | null
    api_format?: string | null
    status_code?: number | null
    error_message?: string | null
  }
}

export interface AdminMonitoringResilienceStatus {
  timestamp: string
  health_score: number
  status: 'healthy' | 'degraded' | 'critical' | string
  error_statistics: AdminMonitoringErrorStatistics
  recent_errors: AdminMonitoringRecentError[]
  recommendations: string[]
}

export interface AdminMonitoringCircuitHistoryItem {
  event: string
  key_id: string
  provider_id: string
  provider_name?: string | null
  key_name?: string | null
  api_format?: string | null
  reason?: string | null
  recovery_seconds?: number | null
  timestamp?: string | null
}

export interface AdminMonitoringCircuitHistoryResponse {
  items: AdminMonitoringCircuitHistoryItem[]
  count: number
}

export interface GatewayGateMetrics {
  inFlight: number | null
  availablePermits: number | null
  highWatermark: number | null
  rejectedTotal: number | null
  unavailable: boolean
}

export interface GatewayFallbackMetricSummary {
  name: string
  label: string
  total: number
}

export interface GatewayMetricsSummary {
  serviceUp: number | null
  local: GatewayGateMetrics
  distributed: GatewayGateMetrics
  tunnel: {
    proxyConnections: number | null
    nodes: number | null
    activeStreams: number | null
  }
  fallbackTotal: number
  fallbacks: GatewayFallbackMetricSummary[]
}

const FALLBACK_METRICS: Array<{ name: string; label: string }> = [
  { name: 'decision_remote_total', label: '远端决策回退' },
  { name: 'plan_fallback_total', label: 'Plan 回退' },
  { name: 'control_execute_fallback_total', label: '控制执行回退' },
  { name: 'remote_execute_emergency_total', label: '紧急远端执行' },
  { name: 'local_execution_runtime_miss_total', label: '本地运行时缺失' },
]

function buildGateMetrics(
  samples: ReturnType<typeof parsePrometheusSamples>,
  gate: string
): GatewayGateMetrics {
  return {
    inFlight: findMetricValueNumber(samples, 'concurrency_in_flight', { gate }),
    availablePermits: findMetricValueNumber(samples, 'concurrency_available_permits', { gate }),
    highWatermark: findMetricValueNumber(samples, 'concurrency_high_watermark', { gate }),
    rejectedTotal: findMetricValueNumber(samples, 'concurrency_rejected_total', { gate }),
    unavailable: findMetricValueNumber(samples, 'concurrency_unavailable', { gate }) === 1,
  }
}

export function buildGatewayMetricsSummary(text: string): GatewayMetricsSummary {
  const samples = parsePrometheusSamples(text)
  const fallbacks = FALLBACK_METRICS.map(item => ({
    ...item,
    total: sumMetricValues(samples, item.name),
  }))

  return {
    serviceUp: findMetricValueNumber(samples, 'service_up', { service: 'aether-gateway' }),
    local: buildGateMetrics(samples, 'gateway_requests'),
    distributed: buildGateMetrics(samples, 'gateway_requests_distributed'),
    tunnel: {
      proxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections'),
      nodes: findMetricValueNumber(samples, 'tunnel_nodes'),
      activeStreams: findMetricValueNumber(samples, 'tunnel_active_streams'),
    },
    fallbackTotal: fallbacks.reduce((total, item) => total + item.total, 0),
    fallbacks,
  }
}

async function fetchGatewayMetricsText(): Promise<string> {
  const response = await apiClient.get<string>('/_gateway/metrics', {
    responseType: 'text',
    transformResponse: [(data: string) => data],
  })
  return typeof response.data === 'string' ? response.data : String(response.data ?? '')
}

export const monitoringApi = {
  async getSystemStatus(): Promise<AdminMonitoringSystemStatus> {
    const response = await apiClient.get<AdminMonitoringSystemStatus>(
      '/api/admin/monitoring/system-status'
    )
    return response.data
  },

  async getResilienceStatus(): Promise<AdminMonitoringResilienceStatus> {
    const response = await apiClient.get<AdminMonitoringResilienceStatus>(
      '/api/admin/monitoring/resilience-status'
    )
    return response.data
  },

  async getCircuitHistory(limit = 10): Promise<AdminMonitoringCircuitHistoryResponse> {
    const response = await apiClient.get<AdminMonitoringCircuitHistoryResponse>(
      '/api/admin/monitoring/resilience/circuit-history',
      { params: { limit } }
    )
    return response.data
  },

  async getGatewayMetricsText(): Promise<string> {
    return fetchGatewayMetricsText()
  },

  async getGatewayMetricsSummary(): Promise<GatewayMetricsSummary> {
    return buildGatewayMetricsSummary(await fetchGatewayMetricsText())
  },
}
