import { describe, expect, it } from 'vitest'
import { buildGatewayMetricsSummary } from '../monitoring'

describe('buildGatewayMetricsSummary', () => {
  it('parses gateway process resource metrics with namespace prefixes', () => {
    const summary = buildGatewayMetricsSummary(`
aether_gateway_service_up{service="aether-gateway"} 1
aether_gateway_gateway_process_cpu_usage_basis_points 1234
aether_gateway_gateway_process_memory_bytes 268435456
aether_gateway_gateway_process_memory_basis_points 250
aether_gateway_gateway_process_threads 64
aether_gateway_gateway_process_open_fds 2048
aether_gateway_gateway_process_fd_limit 500000
aether_gateway_gateway_process_fd_usage_basis_points 40
aether_gateway_gateway_process_socket_fds 1800
aether_gateway_gateway_network_observability_available 1
aether_gateway_gateway_network_interfaces 4
aether_gateway_gateway_network_received_bytes_total 123456789
aether_gateway_gateway_network_transmitted_bytes_total 987654321
aether_gateway_gateway_network_received_packets_total 12345
aether_gateway_gateway_network_transmitted_packets_total 23456
aether_gateway_gateway_network_receive_errors_total 1
aether_gateway_gateway_network_transmit_errors_total 2
aether_gateway_gateway_network_receive_dropped_total 3
aether_gateway_gateway_network_transmit_dropped_total 4
aether_gateway_gateway_tcp_state_observability_available 1
aether_gateway_gateway_host_tcp_connections 2200
aether_gateway_gateway_host_tcp_established_connections 1801
aether_gateway_gateway_host_tcp_listen_connections 12
aether_gateway_gateway_host_tcp_time_wait_connections 210
aether_gateway_gateway_host_tcp_syn_sent_connections 5
aether_gateway_gateway_host_tcp_syn_recv_connections 6
aether_gateway_gateway_host_tcp_close_wait_connections 7
aether_gateway_gateway_process_tcp_connections 1812
aether_gateway_gateway_process_tcp_established_connections 1800
aether_gateway_gateway_process_tcp_listen_connections 2
aether_gateway_gateway_process_tcp_time_wait_connections 3
aether_gateway_gateway_process_tcp_syn_sent_connections 4
aether_gateway_gateway_process_tcp_syn_recv_connections 5
aether_gateway_gateway_process_tcp_close_wait_connections 0
aether_gateway_gateway_allocator_observability_available 1
aether_gateway_gateway_allocator_allocated_bytes 67108864
aether_gateway_gateway_allocator_active_bytes 83886080
aether_gateway_gateway_allocator_resident_bytes 100663296
aether_gateway_gateway_allocator_mapped_bytes 134217728
aether_gateway_gateway_allocator_retained_bytes 33554432
aether_gateway_gateway_allocator_metadata_bytes 4194304
aether_gateway_gateway_allocator_active_to_allocated_basis_points 12500
aether_gateway_gateway_allocator_resident_to_allocated_basis_points 15000
aether_gateway_gateway_background_tasks_active 18
aether_gateway_gateway_background_tasks_supervised_total 18
aether_gateway_gateway_background_tasks_unexpected_exits_total 1
aether_gateway_gateway_background_tasks_completed_total 1
aether_gateway_gateway_background_tasks_panicked_total 0
aether_gateway_gateway_background_tasks_aborted_total 0
aether_gateway_gateway_background_tasks_cancelled_total 2
aether_gateway_gateway_tokio_runtime_observability_available 1
aether_gateway_gateway_tokio_runtime_workers 16
aether_gateway_gateway_tokio_runtime_alive_tasks 123
aether_gateway_gateway_tokio_runtime_global_queue_depth 4
aether_gateway_postgres_observability_available{driver="postgres"} 1
aether_gateway_postgres_observability_unavailable{driver="postgres"} 0
aether_gateway_postgres_active_connections{driver="postgres"} 8
aether_gateway_postgres_idle_connections{driver="postgres"} 12
aether_gateway_postgres_idle_in_transaction_connections{driver="postgres"} 0
aether_gateway_postgres_waiting_connections{driver="postgres"} 1
aether_gateway_postgres_lock_waiting_connections{driver="postgres"} 0
aether_gateway_postgres_oldest_active_query_age_ms{driver="postgres"} 123
aether_gateway_postgres_oldest_transaction_age_ms{driver="postgres"} 456
aether_gateway_postgres_deadlocks_total{driver="postgres"} 2
aether_gateway_postgres_block_read_total{driver="postgres"} 100
aether_gateway_postgres_block_hit_total{driver="postgres"} 9900
aether_gateway_postgres_block_cache_hit_rate_basis_points{driver="postgres"} 9900
aether_gateway_postgres_temp_files_total{driver="postgres"} 3
aether_gateway_postgres_temp_bytes_total{driver="postgres"} 4096
aether_gateway_postgres_xact_commit_total{driver="postgres"} 1000
aether_gateway_postgres_xact_rollback_total{driver="postgres"} 4
aether_gateway_postgres_wal_observability_available{driver="postgres"} 1
aether_gateway_postgres_wal_observability_unavailable{driver="postgres"} 0
aether_gateway_postgres_wal_records_total{driver="postgres"} 5000
aether_gateway_postgres_wal_fpi_total{driver="postgres"} 6
aether_gateway_postgres_wal_bytes_total{driver="postgres"} 1048576
aether_gateway_postgres_wal_buffers_full_total{driver="postgres"} 0
aether_gateway_postgres_wal_write_total{driver="postgres"} 50
aether_gateway_postgres_wal_sync_total{driver="postgres"} 40
aether_gateway_postgres_wal_write_time_ms_total{driver="postgres"} 700
aether_gateway_postgres_wal_sync_time_ms_total{driver="postgres"} 80
aether_gateway_postgres_checkpoint_observability_available{driver="postgres"} 1
aether_gateway_postgres_checkpoint_observability_unavailable{driver="postgres"} 0
aether_gateway_postgres_checkpoints_timed_total{driver="postgres"} 2
aether_gateway_postgres_checkpoints_requested_total{driver="postgres"} 1
aether_gateway_postgres_checkpoint_write_time_ms_total{driver="postgres"} 900
aether_gateway_postgres_checkpoint_sync_time_ms_total{driver="postgres"} 120
aether_gateway_postgres_buffers_checkpoint_total{driver="postgres"} 123
aether_gateway_postgres_buffers_backend_total{driver="postgres"} 45
aether_gateway_postgres_statement_observability_available{driver="postgres"} 1
aether_gateway_postgres_statement_observability_unavailable{driver="postgres"} 0
aether_gateway_postgres_statement_top_calls_total{driver="postgres"} 42
aether_gateway_postgres_statement_top_exec_time_ms_total{driver="postgres"} 2345
aether_gateway_postgres_statement_top_max_mean_exec_time_ms{driver="postgres"} 12
aether_gateway_postgres_statement_top_max_exec_time_ms{driver="postgres"} 345
aether_gateway_postgres_statement_top_shared_blks_read_total{driver="postgres"} 22
aether_gateway_postgres_statement_top_shared_blks_hit_total{driver="postgres"} 900
aether_gateway_postgres_statement_top_temp_blks_total{driver="postgres"} 7
aether_gateway_redis_runtime_enabled{backend="redis"} 1
aether_gateway_redis_runtime_health_unavailable{backend="redis"} 0
aether_gateway_redis_runtime_connected_clients{backend="redis"} 9
aether_gateway_redis_runtime_blocked_clients{backend="redis"} 2
aether_gateway_redis_runtime_total_connections_received{backend="redis"} 40
aether_gateway_redis_runtime_rejected_connections_total{backend="redis"} 0
aether_gateway_redis_runtime_total_commands_processed{backend="redis"} 100
aether_gateway_redis_runtime_instantaneous_ops_per_sec{backend="redis"} 17
aether_gateway_redis_runtime_total_error_replies{backend="redis"} 1
aether_gateway_redis_runtime_expired_keys_total{backend="redis"} 3
aether_gateway_redis_runtime_evicted_keys_total{backend="redis"} 0
aether_gateway_redis_runtime_keyspace_hits_total{backend="redis"} 20
aether_gateway_redis_runtime_keyspace_misses_total{backend="redis"} 5
aether_gateway_redis_runtime_keyspace_hit_rate_basis_points{backend="redis"} 8000
aether_gateway_redis_runtime_used_memory_bytes{backend="redis"} 1048576
aether_gateway_redis_runtime_maxmemory_bytes{backend="redis"} 8388608
aether_gateway_redis_runtime_memory_usage_basis_points{backend="redis"} 1250
aether_gateway_redis_runtime_memory_fragmentation_ratio_basis_points{backend="redis"} 12500
aether_gateway_redis_runtime_lane_command_errors_total{backend="redis",lane="fast"} 1
aether_gateway_redis_runtime_lane_command_errors_total{backend="redis",lane="stream"} 2
aether_gateway_redis_runtime_lane_command_timeouts_total{backend="redis",lane="fast"} 3
aether_gateway_redis_runtime_lane_command_timeouts_total{backend="redis",lane="stream"} 4
aether_gateway_redis_runtime_lane_command_count_total{backend="redis",lane="fast"} 10
aether_gateway_redis_runtime_lane_command_count_total{backend="redis",lane="stream"} 5
aether_gateway_redis_runtime_lane_command_latency_ms_sum{backend="redis",lane="fast"} 70
aether_gateway_redis_runtime_lane_command_latency_ms_sum{backend="redis",lane="stream"} 55
aether_gateway_redis_runtime_lane_command_latency_ms_count{backend="redis",lane="fast"} 10
aether_gateway_redis_runtime_lane_command_latency_ms_count{backend="redis",lane="stream"} 5
aether_gateway_redis_runtime_lane_command_latency_ms_max{backend="redis",lane="fast"} 12
aether_gateway_redis_runtime_lane_command_latency_ms_max{backend="redis",lane="stream"} 23
aether_gateway_redis_runtime_lane_command_latency_ms_max{backend="redis",lane="blocking_stream"} 1001
aether_gateway_redis_runtime_lane_command_latency_ms_bucket{backend="redis",lane="fast",le="1"} 2
aether_gateway_redis_runtime_lane_command_latency_ms_bucket{backend="redis",lane="fast",le="+Inf"} 10
aether_gateway_usage_runtime_enabled 1
aether_gateway_usage_runtime_queue_terminal_events_enabled 1
aether_gateway_usage_runtime_queue_lifecycle_events_enabled 1
aether_gateway_usage_runtime_queue_worker_count 2
aether_gateway_usage_runtime_queue_worker_autoscale_enabled 1
aether_gateway_usage_runtime_queue_worker_active_count 3
aether_gateway_usage_runtime_queue_worker_desired_count 4
aether_gateway_usage_runtime_queue_worker_max_count 8
aether_gateway_usage_runtime_queue_worker_read_batches_total 10
aether_gateway_usage_runtime_queue_worker_read_entries_total 20
aether_gateway_usage_runtime_queue_worker_reclaimed_entries_total 2
aether_gateway_usage_runtime_queue_worker_acked_entries_total 18
aether_gateway_usage_runtime_queue_worker_dead_lettered_entries_total 1
aether_gateway_usage_runtime_queue_worker_process_failures_total 2
aether_gateway_usage_runtime_queue_worker_read_failures_total 3
aether_gateway_usage_runtime_queue_worker_reclaim_failures_total 4
aether_gateway_usage_runtime_terminal_enqueue_failed_total 5
aether_gateway_usage_runtime_lifecycle_enqueue_failed_total 6
aether_gateway_request_candidate_queue_depth 5
aether_gateway_request_candidate_queue_pending_depth 4
aether_gateway_request_candidate_queue_capacity 1024
aether_gateway_request_candidate_queue_enqueued_total 30
aether_gateway_request_candidate_queue_dropped_total 1
aether_gateway_request_candidate_queue_flushed_total 28
aether_gateway_request_candidate_queue_flush_failed_total 2
aether_gateway_request_candidate_queue_flush_batches_total 7
aether_gateway_request_candidate_queue_flush_sql_ops_total 8
aether_gateway_request_candidate_queue_flush_sql_records_total 24
aether_gateway_request_candidate_queue_compacted_total 4
aether_gateway_request_candidate_queue_sync_fallback_total 3
aether_gateway_usage_queue_health_unavailable 0
aether_gateway_usage_queue_enabled{stream="usage:events",group="usage_consumers"} 1
aether_gateway_usage_queue_configured{stream="usage:events",group="usage_consumers"} 1
aether_gateway_usage_queue_stream_length{stream="usage:events",group="usage_consumers"} 12
aether_gateway_usage_queue_group_pending{stream="usage:events",group="usage_consumers"} 2
aether_gateway_usage_queue_group_lag{stream="usage:events",group="usage_consumers"} 3
aether_gateway_usage_queue_oldest_pending_idle_ms{stream="usage:events",group="usage_consumers"} 4500
aether_gateway_usage_queue_dlq_length{stream="usage:events:dlq"} 1
aether_gateway_usage_counter_health_unavailable 0
aether_gateway_usage_counter_outbox_pending_rows 3
aether_gateway_usage_counter_outbox_processed_rows 42
aether_gateway_usage_counter_outbox_oldest_pending_age_seconds 12
aether_gateway_usage_counter_outbox_oldest_pending_created_at_unix_secs 1800000000
aether_gateway_usage_counter_outbox_latest_processed_at_unix_secs 1800000012
aether_gateway_usage_counter_outbox_pending_rows_by_kind{kind="api_key"} 2
aether_gateway_usage_counter_outbox_pending_rows_by_kind{kind="model"} 1
aether_gateway_usage_counter_outbox_flush_batches_total 7
aether_gateway_usage_counter_outbox_flush_rows_claimed_total 11
aether_gateway_usage_counter_outbox_flush_targets_total{kind="api_key"} 3
aether_gateway_usage_counter_outbox_flush_targets_total{kind="model"} 4
aether_gateway_usage_counter_outbox_flush_failed_batches_total 1
aether_gateway_usage_counter_outbox_cleanup_rows_total 5
aether_gateway_usage_counter_outbox_cleanup_failed_batches_total 2
`)

    expect(summary.process.processCpuUsageBasisPoints).toBe(1234)
    expect(summary.process.processMemoryBytes).toBe(268435456)
    expect(summary.process.processMemoryBasisPoints).toBe(250)
    expect(summary.process.processThreads).toBe(64)
    expect(summary.process.openFds).toBe(2048)
    expect(summary.process.fdLimit).toBe(500000)
    expect(summary.process.fdUsageBasisPoints).toBe(40)
    expect(summary.process.socketFds).toBe(1800)
    expect(summary.process.networkAvailable).toBe(true)
    expect(summary.process.networkInterfaces).toBe(4)
    expect(summary.process.networkReceivedBytesTotal).toBe(123456789)
    expect(summary.process.networkTransmittedBytesTotal).toBe(987654321)
    expect(summary.process.networkReceiveErrorsTotal).toBe(1)
    expect(summary.process.networkTransmitErrorsTotal).toBe(2)
    expect(summary.process.networkReceiveDroppedTotal).toBe(3)
    expect(summary.process.networkTransmitDroppedTotal).toBe(4)
    expect(summary.process.tcpStateAvailable).toBe(true)
    expect(summary.process.hostTcpEstablishedConnections).toBe(1801)
    expect(summary.process.hostTcpTimeWaitConnections).toBe(210)
    expect(summary.process.hostTcpCloseWaitConnections).toBe(7)
    expect(summary.process.processTcpConnections).toBe(1812)
    expect(summary.process.processTcpEstablishedConnections).toBe(1800)
    expect(summary.process.processTcpCloseWaitConnections).toBe(0)
    expect(summary.allocator.available).toBe(true)
    expect(summary.allocator.allocatedBytes).toBe(67108864)
    expect(summary.allocator.activeBytes).toBe(83886080)
    expect(summary.allocator.residentBytes).toBe(100663296)
    expect(summary.allocator.retainedBytes).toBe(33554432)
    expect(summary.allocator.activeToAllocatedBasisPoints).toBe(12500)
    expect(summary.allocator.residentToAllocatedBasisPoints).toBe(15000)
    expect(summary.backgroundTasks.active).toBe(18)
    expect(summary.backgroundTasks.supervisedTotal).toBe(18)
    expect(summary.backgroundTasks.unexpectedExitsTotal).toBe(1)
    expect(summary.backgroundTasks.completedTotal).toBe(1)
    expect(summary.backgroundTasks.cancelledTotal).toBe(2)
    expect(summary.tokioRuntime.available).toBe(true)
    expect(summary.tokioRuntime.workers).toBe(16)
    expect(summary.tokioRuntime.aliveTasks).toBe(123)
    expect(summary.tokioRuntime.globalQueueDepth).toBe(4)
    expect(summary.postgres.driver).toBe('postgres')
    expect(summary.postgres.available).toBe(true)
    expect(summary.postgres.unavailable).toBe(false)
    expect(summary.postgres.activeConnections).toBe(8)
    expect(summary.postgres.idleConnections).toBe(12)
    expect(summary.postgres.idleInTransactionConnections).toBe(0)
    expect(summary.postgres.waitingConnections).toBe(1)
    expect(summary.postgres.lockWaitingConnections).toBe(0)
    expect(summary.postgres.oldestActiveQueryAgeMs).toBe(123)
    expect(summary.postgres.oldestTransactionAgeMs).toBe(456)
    expect(summary.postgres.deadlocksTotal).toBe(2)
    expect(summary.postgres.blockReadTotal).toBe(100)
    expect(summary.postgres.blockHitTotal).toBe(9900)
    expect(summary.postgres.blockCacheHitRateBasisPoints).toBe(9900)
    expect(summary.postgres.tempBytesTotal).toBe(4096)
    expect(summary.postgres.xactRollbackTotal).toBe(4)
    expect(summary.postgres.walAvailable).toBe(true)
    expect(summary.postgres.walUnavailable).toBe(false)
    expect(summary.postgres.walBytesTotal).toBe(1048576)
    expect(summary.postgres.walWriteTimeMsTotal).toBe(700)
    expect(summary.postgres.walSyncTimeMsTotal).toBe(80)
    expect(summary.postgres.checkpointAvailable).toBe(true)
    expect(summary.postgres.checkpointUnavailable).toBe(false)
    expect(summary.postgres.checkpointWriteTimeMsTotal).toBe(900)
    expect(summary.postgres.checkpointSyncTimeMsTotal).toBe(120)
    expect(summary.postgres.buffersCheckpointTotal).toBe(123)
    expect(summary.postgres.buffersBackendTotal).toBe(45)
    expect(summary.postgres.statementAvailable).toBe(true)
    expect(summary.postgres.statementUnavailable).toBe(false)
    expect(summary.postgres.statementTopExecTimeMsTotal).toBe(2345)
    expect(summary.postgres.statementTopMaxMeanExecTimeMs).toBe(12)
    expect(summary.postgres.statementTopMaxExecTimeMs).toBe(345)
    expect(summary.postgres.statementTopTempBlksTotal).toBe(7)
    expect(summary.redisRuntime.enabled).toBe(true)
    expect(summary.redisRuntime.unavailable).toBe(false)
    expect(summary.redisRuntime.connectedClients).toBe(9)
    expect(summary.redisRuntime.blockedClients).toBe(2)
    expect(summary.redisRuntime.totalConnectionsReceived).toBe(40)
    expect(summary.redisRuntime.rejectedConnectionsTotal).toBe(0)
    expect(summary.redisRuntime.totalCommandsProcessed).toBe(100)
    expect(summary.redisRuntime.instantaneousOpsPerSec).toBe(17)
    expect(summary.redisRuntime.totalErrorReplies).toBe(1)
    expect(summary.redisRuntime.expiredKeysTotal).toBe(3)
    expect(summary.redisRuntime.evictedKeysTotal).toBe(0)
    expect(summary.redisRuntime.keyspaceHitsTotal).toBe(20)
    expect(summary.redisRuntime.keyspaceMissesTotal).toBe(5)
    expect(summary.redisRuntime.keyspaceHitRateBasisPoints).toBe(8000)
    expect(summary.redisRuntime.usedMemoryBytes).toBe(1048576)
    expect(summary.redisRuntime.maxmemoryBytes).toBe(8388608)
    expect(summary.redisRuntime.memoryUsageBasisPoints).toBe(1250)
    expect(summary.redisRuntime.memoryFragmentationRatioBasisPoints).toBe(12500)
    expect(summary.redisRuntime.laneCommandErrorsTotal).toBe(3)
    expect(summary.redisRuntime.laneCommandTimeoutsTotal).toBe(7)
    expect(summary.redisRuntime.laneCommandCountTotal).toBe(15)
    expect(summary.redisRuntime.commandLatencyTotalMs).toBe(125)
    expect(summary.redisRuntime.commandLatencyObservationCount).toBe(15)
    expect(summary.redisRuntime.commandLatencyMaxMs).toBe(1001)
    expect(summary.redisRuntime.nonblockingCommandLatencyMaxMs).toBe(23)
    expect(summary.usageRuntime.enabled).toBe(true)
    expect(summary.usageRuntime.terminalQueueEnabled).toBe(true)
    expect(summary.usageRuntime.lifecycleQueueEnabled).toBe(true)
    expect(summary.usageRuntime.workerCount).toBe(2)
    expect(summary.usageRuntime.workerAutoscaleEnabled).toBe(true)
    expect(summary.usageRuntime.workerActiveCount).toBe(3)
    expect(summary.usageRuntime.workerDesiredCount).toBe(4)
    expect(summary.usageRuntime.workerMaxCount).toBe(8)
    expect(summary.usageRuntime.workerReadBatchesTotal).toBe(10)
    expect(summary.usageRuntime.workerReadEntriesTotal).toBe(20)
    expect(summary.usageRuntime.workerReclaimedEntriesTotal).toBe(2)
    expect(summary.usageRuntime.workerAckedEntriesTotal).toBe(18)
    expect(summary.usageRuntime.workerDeadLetteredEntriesTotal).toBe(1)
    expect(summary.usageRuntime.workerProcessFailuresTotal).toBe(2)
    expect(summary.usageRuntime.workerReadFailuresTotal).toBe(3)
    expect(summary.usageRuntime.workerReclaimFailuresTotal).toBe(4)
    expect(summary.usageRuntime.terminalEnqueueFailedTotal).toBe(5)
    expect(summary.usageRuntime.lifecycleEnqueueFailedTotal).toBe(6)
    expect(summary.requestCandidateQueue.depth).toBe(5)
    expect(summary.requestCandidateQueue.pendingDepth).toBe(4)
    expect(summary.requestCandidateQueue.capacity).toBe(1024)
    expect(summary.requestCandidateQueue.enqueuedTotal).toBe(30)
    expect(summary.requestCandidateQueue.droppedTotal).toBe(1)
    expect(summary.requestCandidateQueue.flushedTotal).toBe(28)
    expect(summary.requestCandidateQueue.flushFailedTotal).toBe(2)
    expect(summary.requestCandidateQueue.flushBatchesTotal).toBe(7)
    expect(summary.requestCandidateQueue.flushSqlOpsTotal).toBe(8)
    expect(summary.requestCandidateQueue.flushSqlRecordsTotal).toBe(24)
    expect(summary.requestCandidateQueue.compactedTotal).toBe(4)
    expect(summary.requestCandidateQueue.syncFallbackTotal).toBe(3)
    expect(summary.usageQueue.unavailable).toBe(false)
    expect(summary.usageQueue.enabled).toBe(true)
    expect(summary.usageQueue.configured).toBe(true)
    expect(summary.usageQueue.stream).toBe('usage:events')
    expect(summary.usageQueue.group).toBe('usage_consumers')
    expect(summary.usageQueue.streamLength).toBe(12)
    expect(summary.usageQueue.groupPending).toBe(2)
    expect(summary.usageQueue.groupLag).toBe(3)
    expect(summary.usageQueue.oldestPendingIdleMs).toBe(4500)
    expect(summary.usageQueue.dlqStream).toBe('usage:events:dlq')
    expect(summary.usageQueue.dlqLength).toBe(1)
    expect(summary.usageCounter.unavailable).toBe(false)
    expect(summary.usageCounter.pendingRows).toBe(3)
    expect(summary.usageCounter.processedRows).toBe(42)
    expect(summary.usageCounter.oldestPendingAgeSeconds).toBe(12)
    expect(summary.usageCounter.oldestPendingCreatedAtUnixSecs).toBe(1800000000)
    expect(summary.usageCounter.latestProcessedAtUnixSecs).toBe(1800000012)
    expect(summary.usageCounter.flushBatchesTotal).toBe(7)
    expect(summary.usageCounter.flushRowsClaimedTotal).toBe(11)
    expect(summary.usageCounter.flushTargetsTotal).toBe(7)
    expect(summary.usageCounter.flushFailedBatchesTotal).toBe(1)
    expect(summary.usageCounter.cleanupRowsTotal).toBe(5)
    expect(summary.usageCounter.cleanupFailedBatchesTotal).toBe(2)
    expect(summary.usageCounter.pendingByKind).toEqual([
      { kind: 'api_key', pendingRows: 2 },
      { kind: 'model', pendingRows: 1 },
    ])
  })
})
