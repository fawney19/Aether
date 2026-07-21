import apiClient from './client'
import {
  findMetricSamples,
  findMetricValueNumber,
  parsePrometheusSamples,
  sumMetricValues,
  type PrometheusSample,
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
  candidatePlanning: GatewayGateMetrics
  upstreamExecution: GatewayGateMetrics
  databasePool: GatewayDatabasePoolMetrics
  postgres: GatewayPostgresObservabilityMetrics
  redisRuntime: GatewayRedisRuntimeMetrics
  process: GatewayProcessResourceMetrics
  allocator: GatewayAllocatorMetrics
  backgroundTasks: GatewayBackgroundTaskMetrics
  tokioRuntime: GatewayTokioRuntimeMetrics
  usageRuntime: GatewayUsageRuntimeMetrics
  usageQueue: GatewayUsageQueueMetrics
  usageCounter: GatewayUsageCounterMetrics
  requestCandidateQueue: GatewayRequestCandidateQueueMetrics
  upstreamTargets: GatewayUpstreamTargetMetrics
  stageLatency: GatewayStageLatencyMetrics
  tunnel: {
    proxyConnections: number | null
    availableProxyConnections: number | null
    closingProxyConnections: number | null
    drainingProxyConnections: number | null
    softAvoidProxyConnections: number | null
    nodes: number | null
    activeStreams: number | null
    outboundQueueDepthTotal: number | null
    outboundQueueDepthMax: number | null
    outboundQueueCapacityTotal: number | null
    outboundQueueRejectedFullTotal: number | null
    outboundQueueRejectedClosedTotal: number | null
    proxyConnectionCongestedTotal: number | null
    softAvoidSelectionTotal: number | null
    selectionRetryTotal: number | null
    selectionUnavailableTotal: number | null
  }
  fallbackTotal: number
  fallbacks: GatewayFallbackMetricSummary[]
}

export interface GatewayDatabasePoolMetrics {
  driver: string | null
  checkedOut: number | null
  idle: number | null
  size: number | null
  max: number | null
  usageBasisPoints: number | null
  idleReserve: number | null
  underMaintenancePressure: boolean | null
}

export interface GatewayPostgresObservabilityMetrics {
  driver: string | null
  available: boolean | null
  unavailable: boolean | null
  activeConnections: number | null
  idleConnections: number | null
  idleInTransactionConnections: number | null
  waitingConnections: number | null
  lockWaitingConnections: number | null
  oldestActiveQueryAgeMs: number | null
  oldestTransactionAgeMs: number | null
  deadlocksTotal: number | null
  blockReadTotal: number | null
  blockHitTotal: number | null
  blockCacheHitRateBasisPoints: number | null
  tempFilesTotal: number | null
  tempBytesTotal: number | null
  xactCommitTotal: number | null
  xactRollbackTotal: number | null
  walAvailable: boolean | null
  walUnavailable: boolean | null
  walRecordsTotal: number | null
  walFpiTotal: number | null
  walBytesTotal: number | null
  walBuffersFullTotal: number | null
  walWriteTotal: number | null
  walSyncTotal: number | null
  walWriteTimeMsTotal: number | null
  walSyncTimeMsTotal: number | null
  checkpointAvailable: boolean | null
  checkpointUnavailable: boolean | null
  checkpointsTimedTotal: number | null
  checkpointsRequestedTotal: number | null
  checkpointWriteTimeMsTotal: number | null
  checkpointSyncTimeMsTotal: number | null
  buffersCheckpointTotal: number | null
  buffersBackendTotal: number | null
  statementAvailable: boolean | null
  statementUnavailable: boolean | null
  statementTopCallsTotal: number | null
  statementTopExecTimeMsTotal: number | null
  statementTopMaxMeanExecTimeMs: number | null
  statementTopMaxExecTimeMs: number | null
  statementTopSharedBlksReadTotal: number | null
  statementTopSharedBlksHitTotal: number | null
  statementTopTempBlksTotal: number | null
}

export interface GatewayRedisRuntimeMetrics {
  enabled: boolean | null
  unavailable: boolean | null
  connectedClients: number | null
  blockedClients: number | null
  totalConnectionsReceived: number | null
  rejectedConnectionsTotal: number | null
  totalCommandsProcessed: number | null
  instantaneousOpsPerSec: number | null
  totalErrorReplies: number | null
  expiredKeysTotal: number | null
  evictedKeysTotal: number | null
  keyspaceHitsTotal: number | null
  keyspaceMissesTotal: number | null
  keyspaceHitRateBasisPoints: number | null
  usedMemoryBytes: number | null
  maxmemoryBytes: number | null
  memoryUsageBasisPoints: number | null
  memoryFragmentationRatioBasisPoints: number | null
  laneCommandErrorsTotal: number
  laneCommandTimeoutsTotal: number
  laneCommandCountTotal: number
  commandLatencyTotalMs: number
  commandLatencyObservationCount: number
  commandLatencyMaxMs: number | null
  nonblockingCommandLatencyMaxMs: number | null
}

export interface GatewayProcessResourceMetrics {
  sampledAtUnixSecs: number | null
  systemCpuUsageBasisPoints: number | null
  processCpuUsageBasisPoints: number | null
  systemMemoryTotalBytes: number | null
  systemMemoryUsedBytes: number | null
  systemMemoryAvailableBytes: number | null
  systemMemoryUsageBasisPoints: number | null
  processMemoryBytes: number | null
  processVirtualMemoryBytes: number | null
  processMemoryBasisPoints: number | null
  processUptimeSeconds: number | null
  processThreads: number | null
  openFds: number | null
  fdLimit: number | null
  fdUsageBasisPoints: number | null
  socketFds: number | null
  networkAvailable: boolean | null
  networkInterfaces: number | null
  networkReceivedBytesTotal: number | null
  networkTransmittedBytesTotal: number | null
  networkReceivedPacketsTotal: number | null
  networkTransmittedPacketsTotal: number | null
  networkReceiveErrorsTotal: number | null
  networkTransmitErrorsTotal: number | null
  networkReceiveDroppedTotal: number | null
  networkTransmitDroppedTotal: number | null
  tcpStateAvailable: boolean | null
  hostTcpConnections: number | null
  hostTcpEstablishedConnections: number | null
  hostTcpListenConnections: number | null
  hostTcpTimeWaitConnections: number | null
  hostTcpSynSentConnections: number | null
  hostTcpSynRecvConnections: number | null
  hostTcpCloseWaitConnections: number | null
  processTcpConnections: number | null
  processTcpEstablishedConnections: number | null
  processTcpListenConnections: number | null
  processTcpTimeWaitConnections: number | null
  processTcpSynSentConnections: number | null
  processTcpSynRecvConnections: number | null
  processTcpCloseWaitConnections: number | null
}

export interface GatewayAllocatorMetrics {
  available: boolean | null
  allocatedBytes: number | null
  activeBytes: number | null
  residentBytes: number | null
  mappedBytes: number | null
  retainedBytes: number | null
  metadataBytes: number | null
  activeToAllocatedBasisPoints: number | null
  residentToAllocatedBasisPoints: number | null
}

export interface GatewayBackgroundTaskMetrics {
  active: number | null
  supervisedTotal: number | null
  unexpectedExitsTotal: number | null
  completedTotal: number | null
  panickedTotal: number | null
  abortedTotal: number | null
  cancelledTotal: number | null
}

export interface GatewayTokioRuntimeMetrics {
  available: boolean | null
  workers: number | null
  aliveTasks: number | null
  globalQueueDepth: number | null
}

export interface GatewayUsageRuntimeMetrics {
  enabled: boolean | null
  terminalQueueEnabled: boolean | null
  lifecycleQueueEnabled: boolean | null
  workerCount: number | null
  workerAutoscaleEnabled: boolean | null
  workerActiveCount: number | null
  workerDesiredCount: number | null
  workerMaxCount: number | null
  workerReadBatchesTotal: number | null
  workerReadEntriesTotal: number | null
  workerReclaimedEntriesTotal: number | null
  workerAckedEntriesTotal: number | null
  workerDeadLetteredEntriesTotal: number | null
  workerProcessFailuresTotal: number | null
  workerReadFailuresTotal: number | null
  workerReclaimFailuresTotal: number | null
  terminalSubmissionLimit: number | null
  terminalSubmissionInFlight: number | null
  terminalSubmissionMaxInFlight: number | null
  terminalSubmissionRejectedTotal: number | null
  terminalEnqueueInFlight: number | null
  terminalEnqueueDeferredTotal: number | null
  terminalEnqueueDeferredDirectWriteTotal: number | null
  terminalEnqueueDeferredDroppedTotal: number | null
  terminalEnqueueDeferredRetryTotal: number | null
  terminalEnqueueFailedTotal: number | null
  terminalDirectFallbackLimit: number | null
  terminalDirectFallbackInFlight: number | null
  terminalDirectFallbackMaxInFlight: number | null
  terminalDirectFallbackSucceededTotal: number | null
  terminalDirectFallbackFailedTotal: number | null
  terminalDirectFallbackRejectedTotal: number | null
  lifecycleEnqueueInFlight: number | null
  lifecycleEnqueueDeferredTotal: number | null
  lifecycleEnqueueDeferredDroppedTotal: number | null
  lifecycleEnqueueDeferredRetryTotal: number | null
  lifecycleEnqueueFailedTotal: number | null
  enqueueRetryScheduledTotal: number | null
}

export interface GatewayUsageQueueMetrics {
  unavailable: boolean | null
  enabled: boolean | null
  configured: boolean | null
  stream: string | null
  group: string | null
  streamLength: number | null
  groupPending: number | null
  groupLag: number | null
  oldestPendingIdleMs: number | null
  dlqStream: string | null
  dlqLength: number | null
}

export interface GatewayUsageCounterKindMetrics {
  kind: string
  pendingRows: number | null
}

export interface GatewayUsageCounterMetrics {
  unavailable: boolean | null
  pendingRows: number | null
  processedRows: number | null
  oldestPendingAgeSeconds: number | null
  oldestPendingCreatedAtUnixSecs: number | null
  latestProcessedAtUnixSecs: number | null
  flushBatchesTotal: number | null
  flushRowsClaimedTotal: number | null
  flushTargetsTotal: number | null
  flushFailedBatchesTotal: number | null
  cleanupRowsTotal: number | null
  cleanupFailedBatchesTotal: number | null
  pendingByKind: GatewayUsageCounterKindMetrics[]
}

export interface GatewayRequestCandidateQueueMetrics {
  depth: number | null
  pendingDepth: number | null
  capacity: number | null
  enqueuedTotal: number | null
  droppedTotal: number | null
  flushedTotal: number | null
  flushFailedTotal: number | null
  flushBatchesTotal: number | null
  flushSqlOpsTotal: number | null
  flushSqlRecordsTotal: number | null
  compactedTotal: number | null
  syncFallbackTotal: number | null
}

export interface GatewayUpstreamTargetRow {
  target: string
  inFlight: number | null
  availablePermits: number | null
  highWatermark: number | null
  rejectedTotal: number | null
  selectedTotal: number | null
  saturatedTotal: number | null
}

export interface GatewayUpstreamTargetMetrics {
  activeTargets: number | null
  limit: number | null
  selectedTotal: number
  saturatedTotal: number
  rejectedTotal: number
  rows: GatewayUpstreamTargetRow[]
}

export interface GatewayStageLatencyRow {
  stage: string
  label: string
  count: number | null
  avgMs: number | null
  maxMs: number | null
}

export interface GatewayStageLatencyMetrics {
  rows: GatewayStageLatencyRow[]
}

const FALLBACK_METRICS: Array<{ name: string; label: string }> = [
  { name: 'decision_remote_total', label: '远端决策回退' },
  { name: 'plan_fallback_total', label: 'Plan 回退' },
  { name: 'control_execute_fallback_total', label: '控制执行回退' },
  { name: 'remote_execute_emergency_total', label: '紧急远端执行' },
  { name: 'local_execution_runtime_miss_total', label: '本地运行时缺失' },
]

const CAPACITY_STAGES: Array<{ stage: string; label: string }> = [
  { stage: 'frontdoor_handler_queue', label: '入口排队' },
  { stage: 'frontdoor_admission', label: '入口准入' },
  { stage: 'candidate_planning_gate_wait', label: '候选规划 Gate' },
  { stage: 'candidate_page_load', label: '候选分页读取' },
  { stage: 'candidate_page_resolve', label: '候选分页解析' },
  { stage: 'upstream_execution_gate_wait', label: '上游执行 Gate' },
  { stage: 'stream_upstream_target_admission', label: 'Target 准入' },
  { stage: 'stream_total', label: '流式总耗时' },
]

function metricBoolean(value: number | null): boolean | null {
  if (value == null) return null
  return value === 1
}

function buildGateMetrics(
  samples: PrometheusSample[],
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

function buildDatabasePoolMetrics(samples: PrometheusSample[]): GatewayDatabasePoolMetrics {
  const maxSample = findMetricSamples(samples, 'database_pool_max_connections')[0]
  return {
    driver: maxSample?.labels.driver ?? null,
    checkedOut: findMetricValueNumber(samples, 'database_pool_checked_out_connections'),
    idle: findMetricValueNumber(samples, 'database_pool_idle_connections'),
    size: findMetricValueNumber(samples, 'database_pool_size_connections'),
    max: findMetricValueNumber(samples, 'database_pool_max_connections'),
    usageBasisPoints: findMetricValueNumber(samples, 'database_pool_usage_basis_points'),
    idleReserve: findMetricValueNumber(samples, 'database_pool_idle_reserve_connections'),
    underMaintenancePressure: metricBoolean(findMetricValueNumber(samples, 'database_pool_under_maintenance_pressure')),
  }
}

function buildPostgresObservabilityMetrics(samples: PrometheusSample[]): GatewayPostgresObservabilityMetrics {
  const availabilitySample = findMetricSamples(samples, 'postgres_observability_available')[0]
  return {
    driver: availabilitySample?.labels.driver ?? null,
    available: metricBoolean(findMetricValueNumber(samples, 'postgres_observability_available')),
    unavailable: metricBoolean(findMetricValueNumber(samples, 'postgres_observability_unavailable')),
    activeConnections: findMetricValueNumber(samples, 'postgres_active_connections'),
    idleConnections: findMetricValueNumber(samples, 'postgres_idle_connections'),
    idleInTransactionConnections: findMetricValueNumber(samples, 'postgres_idle_in_transaction_connections'),
    waitingConnections: findMetricValueNumber(samples, 'postgres_waiting_connections'),
    lockWaitingConnections: findMetricValueNumber(samples, 'postgres_lock_waiting_connections'),
    oldestActiveQueryAgeMs: findMetricValueNumber(samples, 'postgres_oldest_active_query_age_ms'),
    oldestTransactionAgeMs: findMetricValueNumber(samples, 'postgres_oldest_transaction_age_ms'),
    deadlocksTotal: findMetricValueNumber(samples, 'postgres_deadlocks_total'),
    blockReadTotal: findMetricValueNumber(samples, 'postgres_block_read_total'),
    blockHitTotal: findMetricValueNumber(samples, 'postgres_block_hit_total'),
    blockCacheHitRateBasisPoints: findMetricValueNumber(samples, 'postgres_block_cache_hit_rate_basis_points'),
    tempFilesTotal: findMetricValueNumber(samples, 'postgres_temp_files_total'),
    tempBytesTotal: findMetricValueNumber(samples, 'postgres_temp_bytes_total'),
    xactCommitTotal: findMetricValueNumber(samples, 'postgres_xact_commit_total'),
    xactRollbackTotal: findMetricValueNumber(samples, 'postgres_xact_rollback_total'),
    walAvailable: metricBoolean(findMetricValueNumber(samples, 'postgres_wal_observability_available')),
    walUnavailable: metricBoolean(findMetricValueNumber(samples, 'postgres_wal_observability_unavailable')),
    walRecordsTotal: findMetricValueNumber(samples, 'postgres_wal_records_total'),
    walFpiTotal: findMetricValueNumber(samples, 'postgres_wal_fpi_total'),
    walBytesTotal: findMetricValueNumber(samples, 'postgres_wal_bytes_total'),
    walBuffersFullTotal: findMetricValueNumber(samples, 'postgres_wal_buffers_full_total'),
    walWriteTotal: findMetricValueNumber(samples, 'postgres_wal_write_total'),
    walSyncTotal: findMetricValueNumber(samples, 'postgres_wal_sync_total'),
    walWriteTimeMsTotal: findMetricValueNumber(samples, 'postgres_wal_write_time_ms_total'),
    walSyncTimeMsTotal: findMetricValueNumber(samples, 'postgres_wal_sync_time_ms_total'),
    checkpointAvailable: metricBoolean(findMetricValueNumber(samples, 'postgres_checkpoint_observability_available')),
    checkpointUnavailable: metricBoolean(findMetricValueNumber(samples, 'postgres_checkpoint_observability_unavailable')),
    checkpointsTimedTotal: findMetricValueNumber(samples, 'postgres_checkpoints_timed_total'),
    checkpointsRequestedTotal: findMetricValueNumber(samples, 'postgres_checkpoints_requested_total'),
    checkpointWriteTimeMsTotal: findMetricValueNumber(samples, 'postgres_checkpoint_write_time_ms_total'),
    checkpointSyncTimeMsTotal: findMetricValueNumber(samples, 'postgres_checkpoint_sync_time_ms_total'),
    buffersCheckpointTotal: findMetricValueNumber(samples, 'postgres_buffers_checkpoint_total'),
    buffersBackendTotal: findMetricValueNumber(samples, 'postgres_buffers_backend_total'),
    statementAvailable: metricBoolean(findMetricValueNumber(samples, 'postgres_statement_observability_available')),
    statementUnavailable: metricBoolean(findMetricValueNumber(samples, 'postgres_statement_observability_unavailable')),
    statementTopCallsTotal: findMetricValueNumber(samples, 'postgres_statement_top_calls_total'),
    statementTopExecTimeMsTotal: findMetricValueNumber(samples, 'postgres_statement_top_exec_time_ms_total'),
    statementTopMaxMeanExecTimeMs: findMetricValueNumber(samples, 'postgres_statement_top_max_mean_exec_time_ms'),
    statementTopMaxExecTimeMs: findMetricValueNumber(samples, 'postgres_statement_top_max_exec_time_ms'),
    statementTopSharedBlksReadTotal: findMetricValueNumber(samples, 'postgres_statement_top_shared_blks_read_total'),
    statementTopSharedBlksHitTotal: findMetricValueNumber(samples, 'postgres_statement_top_shared_blks_hit_total'),
    statementTopTempBlksTotal: findMetricValueNumber(samples, 'postgres_statement_top_temp_blks_total'),
  }
}

function buildRedisRuntimeMetrics(samples: PrometheusSample[]): GatewayRedisRuntimeMetrics {
  const commandLatencyMaxMs = maxMetricValue(samples, 'redis_runtime_lane_command_latency_ms_max')
  const nonblockingLatencyValues = [
    findMetricValueNumber(samples, 'redis_runtime_lane_command_latency_ms_max', { lane: 'fast' }),
    findMetricValueNumber(samples, 'redis_runtime_lane_command_latency_ms_max', { lane: 'stream' }),
    findMetricValueNumber(samples, 'redis_runtime_lane_command_latency_ms_max', { lane: 'admin' }),
  ].filter((value): value is number => value != null && Number.isFinite(value))
  const nonblockingCommandLatencyMaxMs = nonblockingLatencyValues.length > 0
    ? Math.max(...nonblockingLatencyValues)
    : null
  return {
    enabled: metricBoolean(findMetricValueNumber(samples, 'redis_runtime_enabled')),
    unavailable: metricBoolean(findMetricValueNumber(samples, 'redis_runtime_health_unavailable')),
    connectedClients: findMetricValueNumber(samples, 'redis_runtime_connected_clients'),
    blockedClients: findMetricValueNumber(samples, 'redis_runtime_blocked_clients'),
    totalConnectionsReceived: findMetricValueNumber(samples, 'redis_runtime_total_connections_received'),
    rejectedConnectionsTotal: findMetricValueNumber(samples, 'redis_runtime_rejected_connections_total'),
    totalCommandsProcessed: findMetricValueNumber(samples, 'redis_runtime_total_commands_processed'),
    instantaneousOpsPerSec: findMetricValueNumber(samples, 'redis_runtime_instantaneous_ops_per_sec'),
    totalErrorReplies: findMetricValueNumber(samples, 'redis_runtime_total_error_replies'),
    expiredKeysTotal: findMetricValueNumber(samples, 'redis_runtime_expired_keys_total'),
    evictedKeysTotal: findMetricValueNumber(samples, 'redis_runtime_evicted_keys_total'),
    keyspaceHitsTotal: findMetricValueNumber(samples, 'redis_runtime_keyspace_hits_total'),
    keyspaceMissesTotal: findMetricValueNumber(samples, 'redis_runtime_keyspace_misses_total'),
    keyspaceHitRateBasisPoints: findMetricValueNumber(samples, 'redis_runtime_keyspace_hit_rate_basis_points'),
    usedMemoryBytes: findMetricValueNumber(samples, 'redis_runtime_used_memory_bytes'),
    maxmemoryBytes: findMetricValueNumber(samples, 'redis_runtime_maxmemory_bytes'),
    memoryUsageBasisPoints: findMetricValueNumber(samples, 'redis_runtime_memory_usage_basis_points'),
    memoryFragmentationRatioBasisPoints: findMetricValueNumber(samples, 'redis_runtime_memory_fragmentation_ratio_basis_points'),
    laneCommandErrorsTotal: sumMetricValues(samples, 'redis_runtime_lane_command_errors_total'),
    laneCommandTimeoutsTotal: sumMetricValues(samples, 'redis_runtime_lane_command_timeouts_total'),
    laneCommandCountTotal: sumMetricValues(samples, 'redis_runtime_lane_command_count_total'),
    commandLatencyTotalMs: sumMetricValues(samples, 'redis_runtime_lane_command_latency_ms_sum'),
    commandLatencyObservationCount: sumMetricValues(samples, 'redis_runtime_lane_command_latency_ms_count'),
    commandLatencyMaxMs,
    nonblockingCommandLatencyMaxMs,
  }
}

function maxMetricValue(samples: PrometheusSample[], metricName: string): number | null {
  const values = findMetricSamples(samples, metricName)
    .map((sample) => Number(sample.value))
    .filter((value) => Number.isFinite(value))
  if (values.length === 0) return null
  return Math.max(...values)
}

function buildProcessResourceMetrics(samples: PrometheusSample[]): GatewayProcessResourceMetrics {
  return {
    sampledAtUnixSecs: findMetricValueNumber(samples, 'gateway_process_sampled_at_unix_secs'),
    systemCpuUsageBasisPoints: findMetricValueNumber(samples, 'gateway_system_cpu_usage_basis_points'),
    processCpuUsageBasisPoints: findMetricValueNumber(samples, 'gateway_process_cpu_usage_basis_points'),
    systemMemoryTotalBytes: findMetricValueNumber(samples, 'gateway_system_memory_total_bytes'),
    systemMemoryUsedBytes: findMetricValueNumber(samples, 'gateway_system_memory_used_bytes'),
    systemMemoryAvailableBytes: findMetricValueNumber(samples, 'gateway_system_memory_available_bytes'),
    systemMemoryUsageBasisPoints: findMetricValueNumber(samples, 'gateway_system_memory_usage_basis_points'),
    processMemoryBytes: findMetricValueNumber(samples, 'gateway_process_memory_bytes'),
    processVirtualMemoryBytes: findMetricValueNumber(samples, 'gateway_process_virtual_memory_bytes'),
    processMemoryBasisPoints: findMetricValueNumber(samples, 'gateway_process_memory_basis_points'),
    processUptimeSeconds: findMetricValueNumber(samples, 'gateway_process_uptime_seconds'),
    processThreads: findMetricValueNumber(samples, 'gateway_process_threads'),
    openFds: findMetricValueNumber(samples, 'gateway_process_open_fds'),
    fdLimit: findMetricValueNumber(samples, 'gateway_process_fd_limit'),
    fdUsageBasisPoints: findMetricValueNumber(samples, 'gateway_process_fd_usage_basis_points'),
    socketFds: findMetricValueNumber(samples, 'gateway_process_socket_fds'),
    networkAvailable: metricBoolean(findMetricValueNumber(samples, 'gateway_network_observability_available')),
    networkInterfaces: findMetricValueNumber(samples, 'gateway_network_interfaces'),
    networkReceivedBytesTotal: findMetricValueNumber(samples, 'gateway_network_received_bytes_total'),
    networkTransmittedBytesTotal: findMetricValueNumber(samples, 'gateway_network_transmitted_bytes_total'),
    networkReceivedPacketsTotal: findMetricValueNumber(samples, 'gateway_network_received_packets_total'),
    networkTransmittedPacketsTotal: findMetricValueNumber(samples, 'gateway_network_transmitted_packets_total'),
    networkReceiveErrorsTotal: findMetricValueNumber(samples, 'gateway_network_receive_errors_total'),
    networkTransmitErrorsTotal: findMetricValueNumber(samples, 'gateway_network_transmit_errors_total'),
    networkReceiveDroppedTotal: findMetricValueNumber(samples, 'gateway_network_receive_dropped_total'),
    networkTransmitDroppedTotal: findMetricValueNumber(samples, 'gateway_network_transmit_dropped_total'),
    tcpStateAvailable: metricBoolean(findMetricValueNumber(samples, 'gateway_tcp_state_observability_available')),
    hostTcpConnections: findMetricValueNumber(samples, 'gateway_host_tcp_connections'),
    hostTcpEstablishedConnections: findMetricValueNumber(samples, 'gateway_host_tcp_established_connections'),
    hostTcpListenConnections: findMetricValueNumber(samples, 'gateway_host_tcp_listen_connections'),
    hostTcpTimeWaitConnections: findMetricValueNumber(samples, 'gateway_host_tcp_time_wait_connections'),
    hostTcpSynSentConnections: findMetricValueNumber(samples, 'gateway_host_tcp_syn_sent_connections'),
    hostTcpSynRecvConnections: findMetricValueNumber(samples, 'gateway_host_tcp_syn_recv_connections'),
    hostTcpCloseWaitConnections: findMetricValueNumber(samples, 'gateway_host_tcp_close_wait_connections'),
    processTcpConnections: findMetricValueNumber(samples, 'gateway_process_tcp_connections'),
    processTcpEstablishedConnections: findMetricValueNumber(samples, 'gateway_process_tcp_established_connections'),
    processTcpListenConnections: findMetricValueNumber(samples, 'gateway_process_tcp_listen_connections'),
    processTcpTimeWaitConnections: findMetricValueNumber(samples, 'gateway_process_tcp_time_wait_connections'),
    processTcpSynSentConnections: findMetricValueNumber(samples, 'gateway_process_tcp_syn_sent_connections'),
    processTcpSynRecvConnections: findMetricValueNumber(samples, 'gateway_process_tcp_syn_recv_connections'),
    processTcpCloseWaitConnections: findMetricValueNumber(samples, 'gateway_process_tcp_close_wait_connections'),
  }
}

function buildAllocatorMetrics(samples: PrometheusSample[]): GatewayAllocatorMetrics {
  return {
    available: metricBoolean(findMetricValueNumber(samples, 'gateway_allocator_observability_available')),
    allocatedBytes: findMetricValueNumber(samples, 'gateway_allocator_allocated_bytes'),
    activeBytes: findMetricValueNumber(samples, 'gateway_allocator_active_bytes'),
    residentBytes: findMetricValueNumber(samples, 'gateway_allocator_resident_bytes'),
    mappedBytes: findMetricValueNumber(samples, 'gateway_allocator_mapped_bytes'),
    retainedBytes: findMetricValueNumber(samples, 'gateway_allocator_retained_bytes'),
    metadataBytes: findMetricValueNumber(samples, 'gateway_allocator_metadata_bytes'),
    activeToAllocatedBasisPoints: findMetricValueNumber(samples, 'gateway_allocator_active_to_allocated_basis_points'),
    residentToAllocatedBasisPoints: findMetricValueNumber(samples, 'gateway_allocator_resident_to_allocated_basis_points'),
  }
}

function buildBackgroundTaskMetrics(samples: PrometheusSample[]): GatewayBackgroundTaskMetrics {
  return {
    active: findMetricValueNumber(samples, 'gateway_background_tasks_active'),
    supervisedTotal: findMetricValueNumber(samples, 'gateway_background_tasks_supervised_total'),
    unexpectedExitsTotal: findMetricValueNumber(samples, 'gateway_background_tasks_unexpected_exits_total'),
    completedTotal: findMetricValueNumber(samples, 'gateway_background_tasks_completed_total'),
    panickedTotal: findMetricValueNumber(samples, 'gateway_background_tasks_panicked_total'),
    abortedTotal: findMetricValueNumber(samples, 'gateway_background_tasks_aborted_total'),
    cancelledTotal: findMetricValueNumber(samples, 'gateway_background_tasks_cancelled_total'),
  }
}

function buildTokioRuntimeMetrics(samples: PrometheusSample[]): GatewayTokioRuntimeMetrics {
  return {
    available: metricBoolean(findMetricValueNumber(samples, 'gateway_tokio_runtime_observability_available')),
    workers: findMetricValueNumber(samples, 'gateway_tokio_runtime_workers'),
    aliveTasks: findMetricValueNumber(samples, 'gateway_tokio_runtime_alive_tasks'),
    globalQueueDepth: findMetricValueNumber(samples, 'gateway_tokio_runtime_global_queue_depth'),
  }
}

function buildUsageRuntimeMetrics(samples: PrometheusSample[]): GatewayUsageRuntimeMetrics {
  return {
    enabled: metricBoolean(findMetricValueNumber(samples, 'usage_runtime_enabled')),
    terminalQueueEnabled: metricBoolean(findMetricValueNumber(samples, 'usage_runtime_queue_terminal_events_enabled')),
    lifecycleQueueEnabled: metricBoolean(findMetricValueNumber(samples, 'usage_runtime_queue_lifecycle_events_enabled')),
    workerCount: findMetricValueNumber(samples, 'usage_runtime_queue_worker_count'),
    workerAutoscaleEnabled: metricBoolean(findMetricValueNumber(samples, 'usage_runtime_queue_worker_autoscale_enabled')),
    workerActiveCount: findMetricValueNumber(samples, 'usage_runtime_queue_worker_active_count'),
    workerDesiredCount: findMetricValueNumber(samples, 'usage_runtime_queue_worker_desired_count'),
    workerMaxCount: findMetricValueNumber(samples, 'usage_runtime_queue_worker_max_count'),
    workerReadBatchesTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_read_batches_total'),
    workerReadEntriesTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_read_entries_total'),
    workerReclaimedEntriesTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_reclaimed_entries_total'),
    workerAckedEntriesTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_acked_entries_total'),
    workerDeadLetteredEntriesTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_dead_lettered_entries_total'),
    workerProcessFailuresTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_process_failures_total'),
    workerReadFailuresTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_read_failures_total'),
    workerReclaimFailuresTotal: findMetricValueNumber(samples, 'usage_runtime_queue_worker_reclaim_failures_total'),
    terminalSubmissionLimit: findMetricValueNumber(samples, 'usage_runtime_terminal_submission_limit'),
    terminalSubmissionInFlight: findMetricValueNumber(samples, 'usage_runtime_terminal_submission_in_flight'),
    terminalSubmissionMaxInFlight: findMetricValueNumber(samples, 'usage_runtime_terminal_submission_max_in_flight'),
    terminalSubmissionRejectedTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_submission_rejected_total'),
    terminalEnqueueInFlight: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_in_flight'),
    terminalEnqueueDeferredTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_deferred_total'),
    terminalEnqueueDeferredDirectWriteTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_deferred_direct_write_total'),
    terminalEnqueueDeferredDroppedTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_deferred_dropped_total'),
    terminalEnqueueDeferredRetryTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_deferred_retry_total'),
    terminalEnqueueFailedTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_enqueue_failed_total'),
    terminalDirectFallbackLimit: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_limit'),
    terminalDirectFallbackInFlight: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_in_flight'),
    terminalDirectFallbackMaxInFlight: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_max_in_flight'),
    terminalDirectFallbackSucceededTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_succeeded_total'),
    terminalDirectFallbackFailedTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_failed_total'),
    terminalDirectFallbackRejectedTotal: findMetricValueNumber(samples, 'usage_runtime_terminal_direct_fallback_rejected_total'),
    lifecycleEnqueueInFlight: findMetricValueNumber(samples, 'usage_runtime_lifecycle_enqueue_in_flight'),
    lifecycleEnqueueDeferredTotal: findMetricValueNumber(samples, 'usage_runtime_lifecycle_enqueue_deferred_total'),
    lifecycleEnqueueDeferredDroppedTotal: findMetricValueNumber(samples, 'usage_runtime_lifecycle_enqueue_deferred_dropped_total'),
    lifecycleEnqueueDeferredRetryTotal: findMetricValueNumber(samples, 'usage_runtime_lifecycle_enqueue_deferred_retry_total'),
    lifecycleEnqueueFailedTotal: findMetricValueNumber(samples, 'usage_runtime_lifecycle_enqueue_failed_total'),
    enqueueRetryScheduledTotal: findMetricValueNumber(samples, 'usage_runtime_enqueue_retry_scheduled_total'),
  }
}

function buildUsageQueueMetrics(samples: PrometheusSample[]): GatewayUsageQueueMetrics {
  const streamSample = findMetricSamples(samples, 'usage_queue_stream_length')[0]
  const dlqSample = findMetricSamples(samples, 'usage_queue_dlq_length')[0]
  return {
    unavailable: metricBoolean(findMetricValueNumber(samples, 'usage_queue_health_unavailable')),
    enabled: metricBoolean(findMetricValueNumber(samples, 'usage_queue_enabled')),
    configured: metricBoolean(findMetricValueNumber(samples, 'usage_queue_configured')),
    stream: streamSample?.labels.stream ?? null,
    group: streamSample?.labels.group ?? null,
    streamLength: findMetricValueNumber(samples, 'usage_queue_stream_length'),
    groupPending: findMetricValueNumber(samples, 'usage_queue_group_pending'),
    groupLag: findMetricValueNumber(samples, 'usage_queue_group_lag'),
    oldestPendingIdleMs: findMetricValueNumber(samples, 'usage_queue_oldest_pending_idle_ms'),
    dlqStream: dlqSample?.labels.stream ?? null,
    dlqLength: findMetricValueNumber(samples, 'usage_queue_dlq_length'),
  }
}

function buildUsageCounterMetrics(samples: PrometheusSample[]): GatewayUsageCounterMetrics {
  const pendingByKind = findMetricSamples(samples, 'usage_counter_outbox_pending_rows_by_kind')
    .map(sample => ({
      kind: sample.labels.kind || 'unknown',
      pendingRows: Number.isFinite(Number(sample.value)) ? Number(sample.value) : null,
    }))
    .sort((left, right) => (right.pendingRows ?? 0) - (left.pendingRows ?? 0))

  return {
    unavailable: metricBoolean(findMetricValueNumber(samples, 'usage_counter_health_unavailable')),
    pendingRows: findMetricValueNumber(samples, 'usage_counter_outbox_pending_rows'),
    processedRows: findMetricValueNumber(samples, 'usage_counter_outbox_processed_rows'),
    oldestPendingAgeSeconds: findMetricValueNumber(samples, 'usage_counter_outbox_oldest_pending_age_seconds'),
    oldestPendingCreatedAtUnixSecs: findMetricValueNumber(samples, 'usage_counter_outbox_oldest_pending_created_at_unix_secs'),
    latestProcessedAtUnixSecs: findMetricValueNumber(samples, 'usage_counter_outbox_latest_processed_at_unix_secs'),
    flushBatchesTotal: findMetricValueNumber(samples, 'usage_counter_outbox_flush_batches_total'),
    flushRowsClaimedTotal: findMetricValueNumber(samples, 'usage_counter_outbox_flush_rows_claimed_total'),
    flushTargetsTotal: sumMetricValues(samples, 'usage_counter_outbox_flush_targets_total'),
    flushFailedBatchesTotal: findMetricValueNumber(samples, 'usage_counter_outbox_flush_failed_batches_total'),
    cleanupRowsTotal: findMetricValueNumber(samples, 'usage_counter_outbox_cleanup_rows_total'),
    cleanupFailedBatchesTotal: findMetricValueNumber(samples, 'usage_counter_outbox_cleanup_failed_batches_total'),
    pendingByKind,
  }
}

function buildRequestCandidateQueueMetrics(samples: PrometheusSample[]): GatewayRequestCandidateQueueMetrics {
  return {
    depth: findMetricValueNumber(samples, 'request_candidate_queue_depth'),
    pendingDepth: findMetricValueNumber(samples, 'request_candidate_queue_pending_depth'),
    capacity: findMetricValueNumber(samples, 'request_candidate_queue_capacity'),
    enqueuedTotal: findMetricValueNumber(samples, 'request_candidate_queue_enqueued_total'),
    droppedTotal: findMetricValueNumber(samples, 'request_candidate_queue_dropped_total'),
    flushedTotal: findMetricValueNumber(samples, 'request_candidate_queue_flushed_total'),
    flushFailedTotal: findMetricValueNumber(samples, 'request_candidate_queue_flush_failed_total'),
    flushBatchesTotal: findMetricValueNumber(samples, 'request_candidate_queue_flush_batches_total'),
    flushSqlOpsTotal: findMetricValueNumber(samples, 'request_candidate_queue_flush_sql_ops_total'),
    flushSqlRecordsTotal: findMetricValueNumber(samples, 'request_candidate_queue_flush_sql_records_total'),
    compactedTotal: findMetricValueNumber(samples, 'request_candidate_queue_compacted_total'),
    syncFallbackTotal: findMetricValueNumber(samples, 'request_candidate_queue_sync_fallback_total'),
  }
}

function collectUpstreamTargets(samples: PrometheusSample[]): string[] {
  const targets = new Set<string>()
  const metricNames = [
    'upstream_target_gate_in_flight',
    'upstream_target_gate_available_permits',
    'upstream_target_gate_high_watermark',
    'upstream_target_gate_rejected_total',
    'upstream_target_selected_total',
    'upstream_target_saturated_total',
  ]
  for (const metricName of metricNames) {
    for (const sample of findMetricSamples(samples, metricName)) {
      if (sample.labels.target) {
        targets.add(sample.labels.target)
      }
    }
  }
  return Array.from(targets)
}

function buildUpstreamTargetMetrics(samples: PrometheusSample[]): GatewayUpstreamTargetMetrics {
  const rows = collectUpstreamTargets(samples)
    .map(target => ({
      target,
      inFlight: findMetricValueNumber(samples, 'upstream_target_gate_in_flight', { target }),
      availablePermits: findMetricValueNumber(samples, 'upstream_target_gate_available_permits', { target }),
      highWatermark: findMetricValueNumber(samples, 'upstream_target_gate_high_watermark', { target }),
      rejectedTotal: findMetricValueNumber(samples, 'upstream_target_gate_rejected_total', { target }),
      selectedTotal: findMetricValueNumber(samples, 'upstream_target_selected_total', { target }),
      saturatedTotal: findMetricValueNumber(samples, 'upstream_target_saturated_total', { target }),
    }))
    .sort((left, right) => (
      (right.inFlight ?? 0) - (left.inFlight ?? 0)
      || (right.saturatedTotal ?? 0) - (left.saturatedTotal ?? 0)
      || (right.selectedTotal ?? 0) - (left.selectedTotal ?? 0)
    ))
    .slice(0, 8)

  return {
    activeTargets: findMetricValueNumber(samples, 'upstream_target_gate_active_targets'),
    limit: findMetricValueNumber(samples, 'upstream_target_gate_limit'),
    selectedTotal: sumMetricValues(samples, 'upstream_target_selected_total'),
    saturatedTotal: sumMetricValues(samples, 'upstream_target_saturated_total'),
    rejectedTotal: sumMetricValues(samples, 'upstream_target_gate_rejected_total'),
    rows,
  }
}

function buildStageLatencyMetrics(samples: PrometheusSample[]): GatewayStageLatencyMetrics {
  return {
    rows: CAPACITY_STAGES.map(({ stage, label }) => {
      const count = findMetricValueNumber(samples, 'gateway_stage_latency_count', { stage })
      const sumMs = findMetricValueNumber(samples, 'gateway_stage_latency_sum_ms', { stage })
      return {
        stage,
        label,
        count,
        avgMs: count != null && count > 0 && sumMs != null ? sumMs / count : null,
        maxMs: findMetricValueNumber(samples, 'gateway_stage_latency_max_ms', { stage }),
      }
    }),
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
    candidatePlanning: buildGateMetrics(samples, 'gateway_candidate_planning'),
    upstreamExecution: buildGateMetrics(samples, 'gateway_upstream_execution'),
    databasePool: buildDatabasePoolMetrics(samples),
    postgres: buildPostgresObservabilityMetrics(samples),
    redisRuntime: buildRedisRuntimeMetrics(samples),
    process: buildProcessResourceMetrics(samples),
    allocator: buildAllocatorMetrics(samples),
    backgroundTasks: buildBackgroundTaskMetrics(samples),
    tokioRuntime: buildTokioRuntimeMetrics(samples),
    usageRuntime: buildUsageRuntimeMetrics(samples),
    usageQueue: buildUsageQueueMetrics(samples),
    usageCounter: buildUsageCounterMetrics(samples),
    requestCandidateQueue: buildRequestCandidateQueueMetrics(samples),
    upstreamTargets: buildUpstreamTargetMetrics(samples),
    stageLatency: buildStageLatencyMetrics(samples),
    tunnel: {
      proxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections'),
      availableProxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections_available'),
      closingProxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections_closing'),
      drainingProxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections_draining'),
      softAvoidProxyConnections: findMetricValueNumber(samples, 'tunnel_proxy_connections_soft_avoid'),
      nodes: findMetricValueNumber(samples, 'tunnel_nodes'),
      activeStreams: findMetricValueNumber(samples, 'tunnel_active_streams'),
      outboundQueueDepthTotal: findMetricValueNumber(samples, 'tunnel_proxy_outbound_queue_depth_total'),
      outboundQueueDepthMax: findMetricValueNumber(samples, 'tunnel_proxy_outbound_queue_depth_max'),
      outboundQueueCapacityTotal: findMetricValueNumber(samples, 'tunnel_proxy_outbound_queue_capacity_total'),
      outboundQueueRejectedFullTotal: findMetricValueNumber(samples, 'tunnel_proxy_outbound_queue_rejected_full_total'),
      outboundQueueRejectedClosedTotal: findMetricValueNumber(samples, 'tunnel_proxy_outbound_queue_rejected_closed_total'),
      proxyConnectionCongestedTotal: findMetricValueNumber(samples, 'tunnel_proxy_connection_congested_total'),
      softAvoidSelectionTotal: findMetricValueNumber(samples, 'tunnel_proxy_soft_avoid_selection_total'),
      selectionRetryTotal: findMetricValueNumber(samples, 'tunnel_proxy_selection_retry_total'),
      selectionUnavailableTotal: findMetricValueNumber(samples, 'tunnel_proxy_selection_unavailable_total'),
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
