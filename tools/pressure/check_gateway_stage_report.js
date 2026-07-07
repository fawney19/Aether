#!/usr/bin/env node
const fs = require('node:fs')

const STAGE_DEFAULTS = {
  S1: {
    minRequests: 1000,
    minConcurrency: 1000,
    dbPoolMaxUsageBasisPoints: 5000,
    maxDbPoolPressureSamples: 0,
    maxDbPoolPressureSampleRateBasisPoints: 0,
    maxPostgresObservabilityUnavailableSamples: 0,
    maxPostgresWalObservabilityUnavailableSamples: 0,
    maxPostgresCheckpointObservabilityUnavailableSamples: 0,
    maxPostgresStatementObservabilityUnavailableSamples: 0,
    postgresMaxLockWaitingConnections: 0,
    postgresMaxIdleInTransactionConnections: 0,
    postgresMaxOldestActiveQueryAgeMs: 60000,
    postgresMaxOldestTransactionAgeMs: 60000,
    postgresMaxStatementTopMaxExecTimeMs: 60000,
    maxRedisRuntimeHealthUnavailableSamples: 0,
    redisRuntimeMaxMemoryUsageBasisPoints: 9000,
    redisRuntimeMaxRejectedConnectionsTotal: 0,
    redisRuntimeMaxEvictedKeysTotal: 0,
    redisRuntimeMaxTotalErrorReplies: 0,
    redisRuntimeMaxLaneCommandErrorsTotal: 0,
    redisRuntimeMaxLaneCommandTimeoutsTotal: 0,
    redisRuntimeMaxNonblockingCommandLatencyMs: 500,
    gatewayProcessMaxFdUsageBasisPoints: 7000,
    gatewayProcessMaxTcpCloseWaitConnections: 0,
    usageQueueMaxFinalPending: 10,
    usageQueueMaxFinalLag: 0,
    usageQueueMaxFinalDlqLength: 0,
    usageQueueMaxOldestPendingIdleMs: 60000,
    maxUsageQueueHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFinalPendingRows: 0,
    usageCounterOutboxMaxOldestPendingAgeSeconds: 60,
    maxUsageCounterHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFlushFailedBatchesTotal: 0,
    usageCounterOutboxMaxCleanupFailedBatchesTotal: 0,
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: 0,
    usageRuntimeMaxWorkerProcessFailuresTotal: 0,
    usageRuntimeMaxWorkerReadFailuresTotal: 0,
    usageRuntimeMaxWorkerReclaimFailuresTotal: 0,
    defaultReport: '/tmp/aether_gateway_pressure_s1_1k.json',
  },
  S2: {
    minRequests: 3000,
    minConcurrency: 3000,
    dbPoolMaxUsageBasisPoints: 7000,
    maxDbPoolPressureSamples: 60,
    maxDbPoolPressureSampleRateBasisPoints: 100,
    maxPostgresObservabilityUnavailableSamples: 0,
    maxPostgresWalObservabilityUnavailableSamples: 0,
    maxPostgresCheckpointObservabilityUnavailableSamples: 0,
    maxPostgresStatementObservabilityUnavailableSamples: 0,
    postgresMaxLockWaitingConnections: 0,
    postgresMaxIdleInTransactionConnections: 0,
    postgresMaxOldestActiveQueryAgeMs: 60000,
    postgresMaxOldestTransactionAgeMs: 60000,
    postgresMaxStatementTopMaxExecTimeMs: 60000,
    maxRedisRuntimeHealthUnavailableSamples: 0,
    redisRuntimeMaxMemoryUsageBasisPoints: 9000,
    redisRuntimeMaxRejectedConnectionsTotal: 0,
    redisRuntimeMaxEvictedKeysTotal: 0,
    redisRuntimeMaxTotalErrorReplies: 0,
    redisRuntimeMaxLaneCommandErrorsTotal: 0,
    redisRuntimeMaxLaneCommandTimeoutsTotal: 0,
    redisRuntimeMaxNonblockingCommandLatencyMs: 500,
    gatewayProcessMaxFdUsageBasisPoints: 7000,
    gatewayProcessMaxTcpCloseWaitConnections: 0,
    usageQueueMaxFinalPending: 10,
    usageQueueMaxFinalLag: 0,
    usageQueueMaxFinalDlqLength: 0,
    usageQueueMaxOldestPendingIdleMs: 60000,
    maxUsageQueueHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFinalPendingRows: 0,
    usageCounterOutboxMaxOldestPendingAgeSeconds: 60,
    maxUsageCounterHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFlushFailedBatchesTotal: 0,
    usageCounterOutboxMaxCleanupFailedBatchesTotal: 0,
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: 0,
    usageRuntimeMaxWorkerProcessFailuresTotal: 0,
    usageRuntimeMaxWorkerReadFailuresTotal: 0,
    usageRuntimeMaxWorkerReclaimFailuresTotal: 0,
    defaultReport: '/tmp/aether_gateway_pressure_s2_3k.json',
  },
  S3: {
    minRequests: 6000,
    minConcurrency: 6000,
    dbPoolMaxUsageBasisPoints: 8000,
    maxDbPoolPressureSamples: 60,
    maxDbPoolPressureSampleRateBasisPoints: 100,
    maxPostgresObservabilityUnavailableSamples: 0,
    maxPostgresWalObservabilityUnavailableSamples: 0,
    maxPostgresCheckpointObservabilityUnavailableSamples: 0,
    maxPostgresStatementObservabilityUnavailableSamples: 0,
    postgresMaxLockWaitingConnections: 0,
    postgresMaxIdleInTransactionConnections: 0,
    postgresMaxOldestActiveQueryAgeMs: 60000,
    postgresMaxOldestTransactionAgeMs: 60000,
    postgresMaxStatementTopMaxExecTimeMs: 60000,
    maxRedisRuntimeHealthUnavailableSamples: 0,
    redisRuntimeMaxMemoryUsageBasisPoints: 9000,
    redisRuntimeMaxRejectedConnectionsTotal: 0,
    redisRuntimeMaxEvictedKeysTotal: 0,
    redisRuntimeMaxTotalErrorReplies: 0,
    redisRuntimeMaxLaneCommandErrorsTotal: 0,
    redisRuntimeMaxLaneCommandTimeoutsTotal: 0,
    redisRuntimeMaxNonblockingCommandLatencyMs: 500,
    gatewayProcessMaxFdUsageBasisPoints: 7000,
    gatewayProcessMaxTcpCloseWaitConnections: 0,
    usageQueueMaxFinalPending: 10,
    usageQueueMaxFinalLag: 0,
    usageQueueMaxFinalDlqLength: 0,
    usageQueueMaxOldestPendingIdleMs: 60000,
    maxUsageQueueHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFinalPendingRows: 0,
    usageCounterOutboxMaxOldestPendingAgeSeconds: 60,
    maxUsageCounterHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFlushFailedBatchesTotal: 0,
    usageCounterOutboxMaxCleanupFailedBatchesTotal: 0,
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: 0,
    usageRuntimeMaxWorkerProcessFailuresTotal: 0,
    usageRuntimeMaxWorkerReadFailuresTotal: 0,
    usageRuntimeMaxWorkerReclaimFailuresTotal: 0,
    defaultReport: '/tmp/aether_gateway_pressure_s3_6k.json',
  },
  S4: {
    minRequests: 10000,
    minConcurrency: 10000,
    dbPoolMaxUsageBasisPoints: 8000,
    maxDbPoolPressureSamples: 60,
    maxDbPoolPressureSampleRateBasisPoints: 100,
    maxPostgresObservabilityUnavailableSamples: 0,
    maxPostgresWalObservabilityUnavailableSamples: 0,
    maxPostgresCheckpointObservabilityUnavailableSamples: 0,
    maxPostgresStatementObservabilityUnavailableSamples: 0,
    postgresMaxLockWaitingConnections: 0,
    postgresMaxIdleInTransactionConnections: 0,
    postgresMaxOldestActiveQueryAgeMs: 60000,
    postgresMaxOldestTransactionAgeMs: 60000,
    postgresMaxStatementTopMaxExecTimeMs: 60000,
    maxRedisRuntimeHealthUnavailableSamples: 0,
    redisRuntimeMaxMemoryUsageBasisPoints: 9000,
    redisRuntimeMaxRejectedConnectionsTotal: 0,
    redisRuntimeMaxEvictedKeysTotal: 0,
    redisRuntimeMaxTotalErrorReplies: 0,
    redisRuntimeMaxLaneCommandErrorsTotal: 0,
    redisRuntimeMaxLaneCommandTimeoutsTotal: 0,
    redisRuntimeMaxNonblockingCommandLatencyMs: 500,
    gatewayProcessMaxFdUsageBasisPoints: 7000,
    gatewayProcessMaxTcpCloseWaitConnections: 0,
    usageQueueMaxFinalPending: 10,
    usageQueueMaxFinalLag: 0,
    usageQueueMaxFinalDlqLength: 0,
    usageQueueMaxOldestPendingIdleMs: 60000,
    maxUsageQueueHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFinalPendingRows: 0,
    usageCounterOutboxMaxOldestPendingAgeSeconds: 60,
    maxUsageCounterHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFlushFailedBatchesTotal: 0,
    usageCounterOutboxMaxCleanupFailedBatchesTotal: 0,
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: 0,
    usageRuntimeMaxWorkerProcessFailuresTotal: 0,
    usageRuntimeMaxWorkerReadFailuresTotal: 0,
    usageRuntimeMaxWorkerReclaimFailuresTotal: 0,
    defaultReport: '/tmp/aether_gateway_pressure_s4_10k.json',
  },
  S5: {
    minRequests: 10000,
    minConcurrency: 10000,
    dbPoolMaxUsageBasisPoints: 8000,
    maxDbPoolPressureSamples: 60,
    maxDbPoolPressureSampleRateBasisPoints: 100,
    maxPostgresObservabilityUnavailableSamples: 0,
    maxPostgresWalObservabilityUnavailableSamples: 0,
    maxPostgresCheckpointObservabilityUnavailableSamples: 0,
    maxPostgresStatementObservabilityUnavailableSamples: 0,
    postgresMaxLockWaitingConnections: 0,
    postgresMaxIdleInTransactionConnections: 0,
    postgresMaxOldestActiveQueryAgeMs: 60000,
    postgresMaxOldestTransactionAgeMs: 60000,
    postgresMaxStatementTopMaxExecTimeMs: 60000,
    maxRedisRuntimeHealthUnavailableSamples: 0,
    redisRuntimeMaxMemoryUsageBasisPoints: 9000,
    redisRuntimeMaxRejectedConnectionsTotal: 0,
    redisRuntimeMaxEvictedKeysTotal: 0,
    redisRuntimeMaxTotalErrorReplies: 0,
    redisRuntimeMaxLaneCommandErrorsTotal: 0,
    redisRuntimeMaxLaneCommandTimeoutsTotal: 0,
    redisRuntimeMaxNonblockingCommandLatencyMs: 500,
    gatewayProcessMaxFdUsageBasisPoints: 7000,
    gatewayProcessMaxTcpCloseWaitConnections: 0,
    usageQueueMaxFinalPending: 10,
    usageQueueMaxFinalLag: 0,
    usageQueueMaxFinalDlqLength: 0,
    usageQueueMaxOldestPendingIdleMs: 60000,
    maxUsageQueueHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFinalPendingRows: 0,
    usageCounterOutboxMaxOldestPendingAgeSeconds: 60,
    maxUsageCounterHealthUnavailableSamples: 0,
    usageCounterOutboxMaxFlushFailedBatchesTotal: 0,
    usageCounterOutboxMaxCleanupFailedBatchesTotal: 0,
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: 0,
    usageRuntimeMaxWorkerProcessFailuresTotal: 0,
    usageRuntimeMaxWorkerReadFailuresTotal: 0,
    usageRuntimeMaxWorkerReclaimFailuresTotal: 0,
    defaultReport: '/tmp/aether_gateway_pressure_s5_10k_soak.json',
  },
}

STAGE_DEFAULTS.REALISTIC_STREAM = {
  ...STAGE_DEFAULTS.S4,
  minRequests: 1000,
  minConcurrency: 1000,
  minThroughputRps: 150,
  maxHeadersP95Ms: 3000,
  maxFirstBodyP95Ms: 5000,
  maxP95Ms: 20000,
  maxP99Ms: 40000,
  maxFirstBodyHoldMs: 0,
  expectedResponseMode: 'FullBody',
  defaultReport: '/tmp/aether_gateway_realistic_stream_1k.json',
}

STAGE_DEFAULTS.TPS = {
  ...STAGE_DEFAULTS.S4,
  minRequests: 20000,
  minConcurrency: 500,
  minThroughputRps: 500,
  maxHeadersP95Ms: 2000,
  maxFirstBodyP95Ms: 3000,
  maxP95Ms: 10000,
  maxP99Ms: 20000,
  maxFirstBodyHoldMs: 0,
  expectedResponseMode: 'FullBody',
  defaultReport: '/tmp/aether_gateway_tps_20k_c500.json',
}

function parseArgs(argv) {
  const options = {
    stage: process.env.PRESSURE_STAGE || 'S1',
    reportPath: null,
    minRequests: numberEnv('PRESSURE_MIN_REQUESTS'),
    minConcurrency: numberEnv('PRESSURE_MIN_CONCURRENCY'),
    minThroughputRps: numberEnv('PRESSURE_MIN_THROUGHPUT_RPS'),
    maxHeadersP95Ms: numberEnv('PRESSURE_MAX_HEADERS_P95_MS'),
    maxFirstBodyP95Ms: numberEnv('PRESSURE_MAX_FIRST_BODY_P95_MS'),
    maxP95Ms: numberEnv('PRESSURE_MAX_P95_MS'),
    maxP99Ms: numberEnv('PRESSURE_MAX_P99_MS'),
    maxFirstBodyHoldMs: numberEnv('PRESSURE_MAX_FIRST_BODY_HOLD_MS'),
    expectedResponseMode: responseModeEnv('PRESSURE_EXPECTED_RESPONSE_MODE'),
    dbPoolMaxUsageBasisPoints: numberEnv('PRESSURE_MAX_DB_POOL_USAGE_BASIS_POINTS'),
    maxDbPoolPressureSamples: numberEnv('PRESSURE_MAX_DB_POOL_PRESSURE_SAMPLES'),
    maxDbPoolPressureSampleRateBasisPoints: numberEnv('PRESSURE_MAX_DB_POOL_PRESSURE_SAMPLE_RATE_BASIS_POINTS'),
    maxPostgresObservabilityUnavailableSamples: numberEnv('PRESSURE_MAX_POSTGRES_OBSERVABILITY_UNAVAILABLE_SAMPLES'),
    maxPostgresWalObservabilityUnavailableSamples: numberEnv('PRESSURE_MAX_POSTGRES_WAL_OBSERVABILITY_UNAVAILABLE_SAMPLES'),
    maxPostgresCheckpointObservabilityUnavailableSamples: numberEnv('PRESSURE_MAX_POSTGRES_CHECKPOINT_OBSERVABILITY_UNAVAILABLE_SAMPLES'),
    maxPostgresStatementObservabilityUnavailableSamples: numberEnv('PRESSURE_MAX_POSTGRES_STATEMENT_OBSERVABILITY_UNAVAILABLE_SAMPLES'),
    postgresMaxLockWaitingConnections: numberEnv('PRESSURE_MAX_POSTGRES_LOCK_WAITING_CONNECTIONS'),
    postgresMaxIdleInTransactionConnections: numberEnv('PRESSURE_MAX_POSTGRES_IDLE_IN_TRANSACTION_CONNECTIONS'),
    postgresMaxOldestActiveQueryAgeMs: numberEnv('PRESSURE_MAX_POSTGRES_OLDEST_ACTIVE_QUERY_AGE_MS'),
    postgresMaxOldestTransactionAgeMs: numberEnv('PRESSURE_MAX_POSTGRES_OLDEST_TRANSACTION_AGE_MS'),
    postgresMaxStatementTopMaxExecTimeMs: numberEnv('PRESSURE_MAX_POSTGRES_STATEMENT_TOP_MAX_EXEC_TIME_MS'),
    maxRedisRuntimeHealthUnavailableSamples: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_HEALTH_UNAVAILABLE_SAMPLES'),
    redisRuntimeMaxMemoryUsageBasisPoints: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_MEMORY_USAGE_BASIS_POINTS'),
    redisRuntimeMaxRejectedConnectionsTotal: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_REJECTED_CONNECTIONS_TOTAL'),
    redisRuntimeMaxEvictedKeysTotal: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_EVICTED_KEYS_TOTAL'),
    redisRuntimeMaxTotalErrorReplies: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_ERROR_REPLIES_TOTAL'),
    redisRuntimeMaxLaneCommandErrorsTotal: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_LANE_COMMAND_ERRORS_TOTAL'),
    redisRuntimeMaxLaneCommandTimeoutsTotal: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_LANE_COMMAND_TIMEOUTS_TOTAL'),
    redisRuntimeMaxNonblockingCommandLatencyMs: numberEnv('PRESSURE_MAX_REDIS_RUNTIME_NONBLOCKING_COMMAND_LATENCY_MS'),
    gatewayProcessMaxFdUsageBasisPoints: numberEnv('PRESSURE_MAX_GATEWAY_PROCESS_FD_USAGE_BASIS_POINTS'),
    gatewayProcessMaxTcpCloseWaitConnections: numberEnv('PRESSURE_MAX_GATEWAY_PROCESS_TCP_CLOSE_WAIT_CONNECTIONS'),
    gatewayBackgroundTasksMaxUnexpectedExitsTotal: numberEnv('PRESSURE_MAX_GATEWAY_BACKGROUND_TASKS_UNEXPECTED_EXITS_TOTAL'),
    usageQueueMaxFinalPending: numberEnv('PRESSURE_MAX_USAGE_QUEUE_FINAL_PENDING'),
    usageQueueMaxFinalLag: numberEnv('PRESSURE_MAX_USAGE_QUEUE_FINAL_LAG'),
    usageQueueMaxFinalDlqLength: numberEnv('PRESSURE_MAX_USAGE_QUEUE_FINAL_DLQ_LENGTH'),
    usageQueueMaxOldestPendingIdleMs: numberEnv('PRESSURE_MAX_USAGE_QUEUE_OLDEST_PENDING_IDLE_MS'),
    maxUsageQueueHealthUnavailableSamples: numberEnv('PRESSURE_MAX_USAGE_QUEUE_HEALTH_UNAVAILABLE_SAMPLES'),
    usageCounterOutboxMaxFinalPendingRows: numberEnv('PRESSURE_MAX_USAGE_COUNTER_OUTBOX_FINAL_PENDING_ROWS'),
    usageCounterOutboxMaxOldestPendingAgeSeconds: numberEnv('PRESSURE_MAX_USAGE_COUNTER_OUTBOX_OLDEST_PENDING_AGE_SECONDS'),
    maxUsageCounterHealthUnavailableSamples: numberEnv('PRESSURE_MAX_USAGE_COUNTER_HEALTH_UNAVAILABLE_SAMPLES'),
    usageCounterOutboxMaxFlushFailedBatchesTotal: numberEnv('PRESSURE_MAX_USAGE_COUNTER_OUTBOX_FLUSH_FAILED_BATCHES_TOTAL'),
    usageCounterOutboxMaxCleanupFailedBatchesTotal: numberEnv('PRESSURE_MAX_USAGE_COUNTER_OUTBOX_CLEANUP_FAILED_BATCHES_TOTAL'),
    usageRuntimeMaxWorkerDeadLetteredEntriesTotal: numberEnv('PRESSURE_MAX_USAGE_RUNTIME_WORKER_DEAD_LETTERED_ENTRIES_TOTAL'),
    usageRuntimeMaxWorkerProcessFailuresTotal: numberEnv('PRESSURE_MAX_USAGE_RUNTIME_WORKER_PROCESS_FAILURES_TOTAL'),
    usageRuntimeMaxWorkerReadFailuresTotal: numberEnv('PRESSURE_MAX_USAGE_RUNTIME_WORKER_READ_FAILURES_TOTAL'),
    usageRuntimeMaxWorkerReclaimFailuresTotal: numberEnv('PRESSURE_MAX_USAGE_RUNTIME_WORKER_RECLAIM_FAILURES_TOTAL'),
  }

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--stage') {
      options.stage = nextValue(argv, ++index, arg)
    } else if (arg === '--min-requests') {
      options.minRequests = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--min-concurrency') {
      options.minConcurrency = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--min-throughput-rps') {
      options.minThroughputRps = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-headers-p95-ms') {
      options.maxHeadersP95Ms = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-first-body-p95-ms') {
      options.maxFirstBodyP95Ms = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-p95-ms') {
      options.maxP95Ms = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-p99-ms') {
      options.maxP99Ms = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-first-body-hold-ms') {
      options.maxFirstBodyHoldMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--expected-response-mode') {
      options.expectedResponseMode = responseModeArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--db-pool-max-usage-basis-points') {
      options.dbPoolMaxUsageBasisPoints = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-db-pool-pressure-samples') {
      options.maxDbPoolPressureSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-db-pool-pressure-sample-rate-basis-points') {
      options.maxDbPoolPressureSampleRateBasisPoints = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-observability-unavailable-samples') {
      options.maxPostgresObservabilityUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-wal-observability-unavailable-samples') {
      options.maxPostgresWalObservabilityUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-checkpoint-observability-unavailable-samples') {
      options.maxPostgresCheckpointObservabilityUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-statement-observability-unavailable-samples') {
      options.maxPostgresStatementObservabilityUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-lock-waiting-connections') {
      options.postgresMaxLockWaitingConnections = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-idle-in-transaction-connections') {
      options.postgresMaxIdleInTransactionConnections = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-oldest-active-query-age-ms') {
      options.postgresMaxOldestActiveQueryAgeMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-oldest-transaction-age-ms') {
      options.postgresMaxOldestTransactionAgeMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-postgres-statement-top-max-exec-time-ms') {
      options.postgresMaxStatementTopMaxExecTimeMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-health-unavailable-samples') {
      options.maxRedisRuntimeHealthUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-memory-usage-basis-points') {
      options.redisRuntimeMaxMemoryUsageBasisPoints = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-rejected-connections-total') {
      options.redisRuntimeMaxRejectedConnectionsTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-evicted-keys-total') {
      options.redisRuntimeMaxEvictedKeysTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-error-replies-total') {
      options.redisRuntimeMaxTotalErrorReplies = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-lane-command-errors-total') {
      options.redisRuntimeMaxLaneCommandErrorsTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-lane-command-timeouts-total') {
      options.redisRuntimeMaxLaneCommandTimeoutsTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-redis-runtime-nonblocking-command-latency-ms') {
      options.redisRuntimeMaxNonblockingCommandLatencyMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-gateway-process-fd-usage-basis-points') {
      options.gatewayProcessMaxFdUsageBasisPoints = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-gateway-process-tcp-close-wait-connections') {
      options.gatewayProcessMaxTcpCloseWaitConnections = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-gateway-background-tasks-unexpected-exits-total') {
      options.gatewayBackgroundTasksMaxUnexpectedExitsTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-queue-final-pending') {
      options.usageQueueMaxFinalPending = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-queue-final-lag') {
      options.usageQueueMaxFinalLag = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-queue-final-dlq-length') {
      options.usageQueueMaxFinalDlqLength = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-queue-oldest-pending-idle-ms') {
      options.usageQueueMaxOldestPendingIdleMs = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-queue-health-unavailable-samples') {
      options.maxUsageQueueHealthUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-counter-outbox-final-pending-rows') {
      options.usageCounterOutboxMaxFinalPendingRows = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-counter-outbox-oldest-pending-age-seconds') {
      options.usageCounterOutboxMaxOldestPendingAgeSeconds = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-counter-health-unavailable-samples') {
      options.maxUsageCounterHealthUnavailableSamples = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-counter-outbox-flush-failed-batches-total') {
      options.usageCounterOutboxMaxFlushFailedBatchesTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-counter-outbox-cleanup-failed-batches-total') {
      options.usageCounterOutboxMaxCleanupFailedBatchesTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-runtime-worker-dead-lettered-entries-total') {
      options.usageRuntimeMaxWorkerDeadLetteredEntriesTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-runtime-worker-process-failures-total') {
      options.usageRuntimeMaxWorkerProcessFailuresTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-runtime-worker-read-failures-total') {
      options.usageRuntimeMaxWorkerReadFailuresTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--max-usage-runtime-worker-reclaim-failures-total') {
      options.usageRuntimeMaxWorkerReclaimFailuresTotal = numberArg(nextValue(argv, ++index, arg), arg)
    } else if (arg === '--help' || arg === '-h') {
      printUsage()
      process.exit(0)
    } else if (arg.startsWith('--')) {
      console.error(`unknown argument: ${arg}`)
      printUsage()
      process.exit(2)
    } else if (!options.reportPath) {
      options.reportPath = arg
    } else {
      console.error(`unexpected positional argument: ${arg}`)
      printUsage()
      process.exit(2)
    }
  }

  options.stage = normalizeStage(options.stage)
  const defaults = STAGE_DEFAULTS[options.stage]
  if (!defaults) {
    console.error(`unsupported stage ${options.stage}; expected ${Object.keys(STAGE_DEFAULTS).join(', ')}`)
    process.exit(2)
  }
  return {
    ...defaults,
    ...definedOnly({
      minRequests: options.minRequests,
      minConcurrency: options.minConcurrency,
      minThroughputRps: options.minThroughputRps,
      maxHeadersP95Ms: options.maxHeadersP95Ms,
      maxFirstBodyP95Ms: options.maxFirstBodyP95Ms,
      maxP95Ms: options.maxP95Ms,
      maxP99Ms: options.maxP99Ms,
      maxFirstBodyHoldMs: options.maxFirstBodyHoldMs,
      expectedResponseMode: options.expectedResponseMode,
      dbPoolMaxUsageBasisPoints: options.dbPoolMaxUsageBasisPoints,
      maxDbPoolPressureSamples: options.maxDbPoolPressureSamples,
      maxDbPoolPressureSampleRateBasisPoints: options.maxDbPoolPressureSampleRateBasisPoints,
      maxPostgresObservabilityUnavailableSamples: options.maxPostgresObservabilityUnavailableSamples,
      maxPostgresWalObservabilityUnavailableSamples: options.maxPostgresWalObservabilityUnavailableSamples,
      maxPostgresCheckpointObservabilityUnavailableSamples: options.maxPostgresCheckpointObservabilityUnavailableSamples,
      maxPostgresStatementObservabilityUnavailableSamples: options.maxPostgresStatementObservabilityUnavailableSamples,
      postgresMaxLockWaitingConnections: options.postgresMaxLockWaitingConnections,
      postgresMaxIdleInTransactionConnections: options.postgresMaxIdleInTransactionConnections,
      postgresMaxOldestActiveQueryAgeMs: options.postgresMaxOldestActiveQueryAgeMs,
      postgresMaxOldestTransactionAgeMs: options.postgresMaxOldestTransactionAgeMs,
      postgresMaxStatementTopMaxExecTimeMs: options.postgresMaxStatementTopMaxExecTimeMs,
      maxRedisRuntimeHealthUnavailableSamples: options.maxRedisRuntimeHealthUnavailableSamples,
      redisRuntimeMaxMemoryUsageBasisPoints: options.redisRuntimeMaxMemoryUsageBasisPoints,
      redisRuntimeMaxRejectedConnectionsTotal: options.redisRuntimeMaxRejectedConnectionsTotal,
      redisRuntimeMaxEvictedKeysTotal: options.redisRuntimeMaxEvictedKeysTotal,
      redisRuntimeMaxTotalErrorReplies: options.redisRuntimeMaxTotalErrorReplies,
      redisRuntimeMaxLaneCommandErrorsTotal: options.redisRuntimeMaxLaneCommandErrorsTotal,
      redisRuntimeMaxLaneCommandTimeoutsTotal: options.redisRuntimeMaxLaneCommandTimeoutsTotal,
      redisRuntimeMaxNonblockingCommandLatencyMs: options.redisRuntimeMaxNonblockingCommandLatencyMs,
      gatewayProcessMaxFdUsageBasisPoints: options.gatewayProcessMaxFdUsageBasisPoints,
      gatewayProcessMaxTcpCloseWaitConnections: options.gatewayProcessMaxTcpCloseWaitConnections,
      gatewayBackgroundTasksMaxUnexpectedExitsTotal: options.gatewayBackgroundTasksMaxUnexpectedExitsTotal,
      usageQueueMaxFinalPending: options.usageQueueMaxFinalPending,
      usageQueueMaxFinalLag: options.usageQueueMaxFinalLag,
      usageQueueMaxFinalDlqLength: options.usageQueueMaxFinalDlqLength,
      usageQueueMaxOldestPendingIdleMs: options.usageQueueMaxOldestPendingIdleMs,
      maxUsageQueueHealthUnavailableSamples: options.maxUsageQueueHealthUnavailableSamples,
      usageCounterOutboxMaxFinalPendingRows: options.usageCounterOutboxMaxFinalPendingRows,
      usageCounterOutboxMaxOldestPendingAgeSeconds: options.usageCounterOutboxMaxOldestPendingAgeSeconds,
      maxUsageCounterHealthUnavailableSamples: options.maxUsageCounterHealthUnavailableSamples,
      usageCounterOutboxMaxFlushFailedBatchesTotal: options.usageCounterOutboxMaxFlushFailedBatchesTotal,
      usageCounterOutboxMaxCleanupFailedBatchesTotal: options.usageCounterOutboxMaxCleanupFailedBatchesTotal,
      usageRuntimeMaxWorkerDeadLetteredEntriesTotal: options.usageRuntimeMaxWorkerDeadLetteredEntriesTotal,
      usageRuntimeMaxWorkerProcessFailuresTotal: options.usageRuntimeMaxWorkerProcessFailuresTotal,
      usageRuntimeMaxWorkerReadFailuresTotal: options.usageRuntimeMaxWorkerReadFailuresTotal,
      usageRuntimeMaxWorkerReclaimFailuresTotal: options.usageRuntimeMaxWorkerReclaimFailuresTotal,
    }),
    stage: options.stage,
    reportPath: options.reportPath || defaults.defaultReport,
  }
}

function normalizeStage(value) {
  return String(value || 'S1')
    .trim()
    .toUpperCase()
    .replace(/[-\s]+/g, '_')
}

function numberEnv(name) {
  const value = process.env[name]
  if (value == null || value.trim() === '') return null
  return numberArg(value, name)
}

function responseModeEnv(name) {
  const value = process.env[name]
  if (value == null || value.trim() === '') return null
  return responseModeArg(value, name)
}

function numberArg(value, name) {
  const parsed = Number(value)
  if (!Number.isInteger(parsed) || parsed < 0) {
    console.error(`${name} must be a non-negative integer, got ${value}`)
    process.exit(2)
  }
  return parsed
}

function responseModeArg(value, name) {
  const normalized = normalizeResponseMode(value)
  if (normalized != null) {
    return normalized
  }
  console.error(`${name} must be headers, first-body-byte, or full, got ${value}`)
  process.exit(2)
}

function normalizeResponseMode(value) {
  const compact = String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[-_\s]+/g, '')
  if (compact === 'headers' || compact === 'headersonly' || compact === 'header') {
    return 'HeadersOnly'
  }
  if (compact === 'firstbodybyte' || compact === 'firstbody' || compact === 'ttft') {
    return 'FirstBodyByte'
  }
  if (compact === 'full' || compact === 'fullbody') {
    return 'FullBody'
  }
  return null
}

function nextValue(argv, index, flag) {
  const value = argv[index]
  if (value == null) {
    console.error(`missing value for ${flag}`)
    process.exit(2)
  }
  return value
}

function definedOnly(values) {
  return Object.fromEntries(Object.entries(values).filter(([, value]) => value != null))
}

function readReport(path) {
  try {
    return JSON.parse(fs.readFileSync(path, 'utf8'))
  } catch (error) {
    console.error(`FAIL: cannot read report ${path}: ${error.message}`)
    process.exit(1)
  }
}

const options = parseArgs(process.argv.slice(2))
const report = readReport(options.reportPath)
const load = report.load || {}
const metrics = report.metrics || {}
let failures = 0

function fail(message) {
  console.error(`${options.stage} FAIL: ${message}`)
  failures += 1
}

function metric(name, { required = true } = {}) {
  const value = metrics[name]
  if (value == null) {
    if (required) fail(`missing metrics.${name}`)
    return 0
  }
  const parsed = Number(value)
  if (!Number.isFinite(parsed)) {
    fail(`metrics.${name} is not numeric: ${value}`)
    return 0
  }
  return parsed
}

function optionalMetric(name) {
  if (metrics[name] == null) return null
  return metric(name)
}

function loadNumber(name, { required = true } = {}) {
  const value = load[name]
  if (value == null) {
    if (required) fail(`missing load.${name}`)
    return 0
  }
  const parsed = Number(value)
  if (!Number.isFinite(parsed)) {
    fail(`load.${name} is not numeric: ${value}`)
    return 0
  }
  return parsed
}

function assertLoadAtLeast(name, minimum) {
  if (minimum == null) return
  const value = loadNumber(name)
  if (value < minimum) {
    fail(`load.${name}=${value}, expected >= ${minimum}`)
  }
}

function assertLoadAtMost(name, maximum) {
  if (maximum == null) return
  const value = loadNumber(name)
  if (value > maximum) {
    fail(`load.${name}=${value}, expected <= ${maximum}`)
  }
}

const statusCounts = load.status_counts || {}
const non2xx = Object.entries(statusCounts)
  .filter(([status]) => !String(status).startsWith('2'))
  .reduce((total, [, count]) => total + Number(count || 0), 0)

const totalRequests = loadNumber('total_requests')
const completedRequests = loadNumber('completed_requests')
const failedRequests = loadNumber('failed_requests')
const concurrency = loadNumber('concurrency')

if (totalRequests < options.minRequests) {
  fail(`load.total_requests=${totalRequests}, expected >= ${options.minRequests}`)
}

if (completedRequests !== totalRequests) {
  fail(`load.completed_requests=${completedRequests}, expected ${totalRequests}`)
}

if (failedRequests !== 0) {
  fail(`load.failed_requests=${failedRequests}`)
}

if (concurrency < options.minConcurrency) {
  fail(`load.concurrency=${concurrency}, expected >= ${options.minConcurrency}`)
}

if (non2xx !== 0) {
  fail(`non-2xx responses=${non2xx}`)
}

if (Object.keys(load.error_counts || {}).length > 0) {
  fail(`load.error_counts is not empty: ${JSON.stringify(load.error_counts)}`)
}

if (options.expectedResponseMode != null) {
  const responseMode = normalizeResponseMode(load.response_mode)
  if (responseMode == null) {
    fail(`load.response_mode=${load.response_mode ?? 'missing'}, expected ${options.expectedResponseMode}`)
  } else if (responseMode !== options.expectedResponseMode) {
    fail(`load.response_mode=${load.response_mode}, expected ${options.expectedResponseMode}`)
  }
}

assertLoadAtLeast('throughput_rps', options.minThroughputRps)
assertLoadAtMost('headers_p95_ms', options.maxHeadersP95Ms)
assertLoadAtMost('first_body_p95_ms', options.maxFirstBodyP95Ms)
assertLoadAtMost('p95_ms', options.maxP95Ms)
assertLoadAtMost('p99_ms', options.maxP99Ms)
assertLoadAtMost('first_body_hold_ms', options.maxFirstBodyHoldMs)

const dbPoolUsage = metric('db_pool_max_usage_basis_points')
if (dbPoolUsage >= options.dbPoolMaxUsageBasisPoints) {
  fail(`db_pool_max_usage_basis_points=${dbPoolUsage}, expected < ${options.dbPoolMaxUsageBasisPoints}`)
}

const dbPoolPressureSamples = metric('db_pool_pressure_samples')
if (dbPoolPressureSamples > options.maxDbPoolPressureSamples) {
  fail(`db_pool_pressure_samples=${dbPoolPressureSamples}, expected <= ${options.maxDbPoolPressureSamples}`)
}
const sampleCount = metric('samples')
const dbPoolPressureSampleRateBasisPoints =
  sampleCount > 0 ? Math.ceil((dbPoolPressureSamples * 10000) / sampleCount) : 0
if (dbPoolPressureSampleRateBasisPoints > options.maxDbPoolPressureSampleRateBasisPoints) {
  fail(
    `db_pool_pressure_sample_rate_basis_points=${dbPoolPressureSampleRateBasisPoints}, expected <= ${options.maxDbPoolPressureSampleRateBasisPoints}`,
  )
}

const postgresObservabilityUnavailableSamples = optionalMetric('postgres_observability_unavailable_samples')
if (
  postgresObservabilityUnavailableSamples != null
  && postgresObservabilityUnavailableSamples > options.maxPostgresObservabilityUnavailableSamples
) {
  fail(`postgres_observability_unavailable_samples=${postgresObservabilityUnavailableSamples}, expected <= ${options.maxPostgresObservabilityUnavailableSamples}`)
}

const postgresWalObservabilityUnavailableSamples = optionalMetric('postgres_wal_observability_unavailable_samples')
if (
  postgresWalObservabilityUnavailableSamples != null
  && postgresWalObservabilityUnavailableSamples > options.maxPostgresWalObservabilityUnavailableSamples
) {
  fail(`postgres_wal_observability_unavailable_samples=${postgresWalObservabilityUnavailableSamples}, expected <= ${options.maxPostgresWalObservabilityUnavailableSamples}`)
}

const postgresCheckpointObservabilityUnavailableSamples = optionalMetric('postgres_checkpoint_observability_unavailable_samples')
if (
  postgresCheckpointObservabilityUnavailableSamples != null
  && postgresCheckpointObservabilityUnavailableSamples > options.maxPostgresCheckpointObservabilityUnavailableSamples
) {
  fail(`postgres_checkpoint_observability_unavailable_samples=${postgresCheckpointObservabilityUnavailableSamples}, expected <= ${options.maxPostgresCheckpointObservabilityUnavailableSamples}`)
}

const postgresStatementObservabilityUnavailableSamples = optionalMetric('postgres_statement_observability_unavailable_samples')
if (
  postgresStatementObservabilityUnavailableSamples != null
  && postgresStatementObservabilityUnavailableSamples > options.maxPostgresStatementObservabilityUnavailableSamples
) {
  fail(`postgres_statement_observability_unavailable_samples=${postgresStatementObservabilityUnavailableSamples}, expected <= ${options.maxPostgresStatementObservabilityUnavailableSamples}`)
}

const postgresMaxLockWaitingConnections = optionalMetric('postgres_final_lock_waiting_connections') ?? optionalMetric('postgres_max_lock_waiting_connections')
if (
  postgresMaxLockWaitingConnections != null
  && postgresMaxLockWaitingConnections > options.postgresMaxLockWaitingConnections
) {
  fail(`postgres_final_lock_waiting_connections=${postgresMaxLockWaitingConnections}, expected <= ${options.postgresMaxLockWaitingConnections}`)
}

const postgresMaxIdleInTransactionConnections = optionalMetric('postgres_final_idle_in_transaction_connections') ?? optionalMetric('postgres_max_idle_in_transaction_connections')
if (
  postgresMaxIdleInTransactionConnections != null
  && postgresMaxIdleInTransactionConnections > options.postgresMaxIdleInTransactionConnections
) {
  fail(`postgres_final_idle_in_transaction_connections=${postgresMaxIdleInTransactionConnections}, expected <= ${options.postgresMaxIdleInTransactionConnections}`)
}

const postgresMaxOldestActiveQueryAgeMs = optionalMetric('postgres_max_oldest_active_query_age_ms')
if (
  postgresMaxOldestActiveQueryAgeMs != null
  && postgresMaxOldestActiveQueryAgeMs >= options.postgresMaxOldestActiveQueryAgeMs
) {
  fail(`postgres_max_oldest_active_query_age_ms=${postgresMaxOldestActiveQueryAgeMs}, expected < ${options.postgresMaxOldestActiveQueryAgeMs}`)
}

const postgresMaxOldestTransactionAgeMs = optionalMetric('postgres_max_oldest_transaction_age_ms')
if (
  postgresMaxOldestTransactionAgeMs != null
  && postgresMaxOldestTransactionAgeMs >= options.postgresMaxOldestTransactionAgeMs
) {
  fail(`postgres_max_oldest_transaction_age_ms=${postgresMaxOldestTransactionAgeMs}, expected < ${options.postgresMaxOldestTransactionAgeMs}`)
}

const postgresMaxStatementTopMaxExecTimeMs = optionalMetric('postgres_max_statement_top_max_exec_time_ms')
const postgresStatementTopMaxExecTimeMsDelta = optionalMetric('postgres_statement_top_max_exec_time_ms_delta')
if (
  postgresStatementTopMaxExecTimeMsDelta != null
  && postgresStatementTopMaxExecTimeMsDelta >= options.postgresMaxStatementTopMaxExecTimeMs
) {
  fail(`postgres_statement_top_max_exec_time_ms_delta=${postgresStatementTopMaxExecTimeMsDelta}, expected < ${options.postgresMaxStatementTopMaxExecTimeMs}`)
} else if (
  postgresStatementTopMaxExecTimeMsDelta == null
  && postgresMaxStatementTopMaxExecTimeMs != null
  && postgresMaxStatementTopMaxExecTimeMs >= options.postgresMaxStatementTopMaxExecTimeMs
) {
  fail(`postgres_max_statement_top_max_exec_time_ms=${postgresMaxStatementTopMaxExecTimeMs}, expected < ${options.postgresMaxStatementTopMaxExecTimeMs}`)
}

const redisRuntimeHealthUnavailableSamples = optionalMetric('redis_runtime_health_unavailable_samples')
if (
  redisRuntimeHealthUnavailableSamples != null
  && redisRuntimeHealthUnavailableSamples > options.maxRedisRuntimeHealthUnavailableSamples
) {
  fail(`redis_runtime_health_unavailable_samples=${redisRuntimeHealthUnavailableSamples}, expected <= ${options.maxRedisRuntimeHealthUnavailableSamples}`)
}

const redisRuntimeMaxMemoryUsageBasisPoints = optionalMetric('redis_runtime_max_memory_usage_basis_points')
if (
  redisRuntimeMaxMemoryUsageBasisPoints != null
  && redisRuntimeMaxMemoryUsageBasisPoints >= options.redisRuntimeMaxMemoryUsageBasisPoints
) {
  fail(`redis_runtime_max_memory_usage_basis_points=${redisRuntimeMaxMemoryUsageBasisPoints}, expected < ${options.redisRuntimeMaxMemoryUsageBasisPoints}`)
}

const redisRuntimeMaxRejectedConnectionsTotal = optionalMetric('redis_runtime_rejected_connections_total_delta') ?? optionalMetric('redis_runtime_max_rejected_connections_total')
if (
  redisRuntimeMaxRejectedConnectionsTotal != null
  && redisRuntimeMaxRejectedConnectionsTotal > options.redisRuntimeMaxRejectedConnectionsTotal
) {
  fail(`redis_runtime_rejected_connections_total_delta=${redisRuntimeMaxRejectedConnectionsTotal}, expected <= ${options.redisRuntimeMaxRejectedConnectionsTotal}`)
}

const redisRuntimeMaxEvictedKeysTotal = optionalMetric('redis_runtime_evicted_keys_total_delta') ?? optionalMetric('redis_runtime_max_evicted_keys_total')
if (
  redisRuntimeMaxEvictedKeysTotal != null
  && redisRuntimeMaxEvictedKeysTotal > options.redisRuntimeMaxEvictedKeysTotal
) {
  fail(`redis_runtime_evicted_keys_total_delta=${redisRuntimeMaxEvictedKeysTotal}, expected <= ${options.redisRuntimeMaxEvictedKeysTotal}`)
}

const redisRuntimeMaxLaneCommandErrorsTotal = optionalMetric('redis_runtime_lane_command_errors_total_delta') ?? optionalMetric('redis_runtime_max_lane_command_errors_total')
if (
  redisRuntimeMaxLaneCommandErrorsTotal != null
  && redisRuntimeMaxLaneCommandErrorsTotal > options.redisRuntimeMaxLaneCommandErrorsTotal
) {
  fail(`redis_runtime_lane_command_errors_total_delta=${redisRuntimeMaxLaneCommandErrorsTotal}, expected <= ${options.redisRuntimeMaxLaneCommandErrorsTotal}`)
}

const redisRuntimeMaxLaneCommandTimeoutsTotal = optionalMetric('redis_runtime_lane_command_timeouts_total_delta') ?? optionalMetric('redis_runtime_max_lane_command_timeouts_total')
if (
  redisRuntimeMaxLaneCommandTimeoutsTotal != null
  && redisRuntimeMaxLaneCommandTimeoutsTotal > options.redisRuntimeMaxLaneCommandTimeoutsTotal
) {
  fail(`redis_runtime_lane_command_timeouts_total_delta=${redisRuntimeMaxLaneCommandTimeoutsTotal}, expected <= ${options.redisRuntimeMaxLaneCommandTimeoutsTotal}`)
}

const redisRuntimeMaxTotalErrorReplies = optionalMetric('redis_runtime_total_error_replies_delta') ?? optionalMetric('redis_runtime_max_total_error_replies')
if (
  redisRuntimeMaxLaneCommandErrorsTotal == null
  && redisRuntimeMaxLaneCommandTimeoutsTotal == null
  && redisRuntimeMaxTotalErrorReplies != null
  && redisRuntimeMaxTotalErrorReplies > options.redisRuntimeMaxTotalErrorReplies
) {
  fail(`redis_runtime_total_error_replies_delta=${redisRuntimeMaxTotalErrorReplies}, expected <= ${options.redisRuntimeMaxTotalErrorReplies}`)
}

const redisRuntimeMaxNonblockingCommandLatencyMs = optionalMetric('redis_runtime_max_nonblocking_command_latency_ms')
const redisRuntimeNonblockingCommandLatencyMs =
  optionalMetric('redis_runtime_nonblocking_command_latency_ms_delta') ?? redisRuntimeMaxNonblockingCommandLatencyMs
if (
  redisRuntimeNonblockingCommandLatencyMs != null
  && redisRuntimeNonblockingCommandLatencyMs >= options.redisRuntimeMaxNonblockingCommandLatencyMs
) {
  fail(`redis_runtime_nonblocking_command_latency_ms_delta=${redisRuntimeNonblockingCommandLatencyMs}, expected < ${options.redisRuntimeMaxNonblockingCommandLatencyMs}`)
}

if (metric('gateway_requests_max_rejected_total') !== 0) {
  fail(`gateway_requests_max_rejected_total=${metrics.gateway_requests_max_rejected_total}`)
}

if (metric('gateway_requests_distributed_max_rejected_total', { required: false }) !== 0) {
  fail(`gateway_requests_distributed_max_rejected_total=${metrics.gateway_requests_distributed_max_rejected_total}`)
}

if (metric('request_candidate_queue_final_depth') !== 0) {
  fail(`request_candidate_queue_final_depth=${metrics.request_candidate_queue_final_depth}`)
}

if (metric('request_candidate_queue_final_pending_depth') !== 0) {
  fail(`request_candidate_queue_final_pending_depth=${metrics.request_candidate_queue_final_pending_depth}`)
}

if (metric('request_candidate_queue_max_flush_failed_total') !== 0) {
  fail(`request_candidate_queue_max_flush_failed_total=${metrics.request_candidate_queue_max_flush_failed_total}`)
}

if (metric('request_candidate_queue_max_dropped_total') !== 0) {
  fail(`request_candidate_queue_max_dropped_total=${metrics.request_candidate_queue_max_dropped_total}`)
}

if (metric('request_candidate_queue_max_sync_fallback_total') !== 0) {
  fail(`request_candidate_queue_max_sync_fallback_total=${metrics.request_candidate_queue_max_sync_fallback_total}`)
}

if (metric('usage_runtime_max_terminal_enqueue_failed_total') !== 0) {
  fail(`usage_runtime_max_terminal_enqueue_failed_total=${metrics.usage_runtime_max_terminal_enqueue_failed_total}`)
}

if (metric('usage_runtime_max_lifecycle_enqueue_failed_total') !== 0) {
  fail(`usage_runtime_max_lifecycle_enqueue_failed_total=${metrics.usage_runtime_max_lifecycle_enqueue_failed_total}`)
}

if (metric('usage_runtime_max_lifecycle_enqueue_deferred_dropped_total') !== 0) {
  fail(`usage_runtime_max_lifecycle_enqueue_deferred_dropped_total=${metrics.usage_runtime_max_lifecycle_enqueue_deferred_dropped_total}`)
}

if (report.settle_after_ms > 0 && report.settle_drain_completed === false) {
  fail(`settle_drain_completed=false after ${report.settle_after_ms}ms`)
}

if (
  report.settle_after_ms > 0
  && typeof report.settle_drain_elapsed_ms === 'number'
  && report.settle_drain_elapsed_ms > report.settle_after_ms + 1000
) {
  fail(`settle_drain_elapsed_ms=${report.settle_drain_elapsed_ms}, expected <= ${report.settle_after_ms + 1000}`)
}

const usageRuntimeMaxWorkerDeadLetteredEntriesTotal = optionalMetric('usage_runtime_max_worker_dead_lettered_entries_total')
if (
  usageRuntimeMaxWorkerDeadLetteredEntriesTotal != null
  && usageRuntimeMaxWorkerDeadLetteredEntriesTotal > options.usageRuntimeMaxWorkerDeadLetteredEntriesTotal
) {
  fail(`usage_runtime_max_worker_dead_lettered_entries_total=${usageRuntimeMaxWorkerDeadLetteredEntriesTotal}, expected <= ${options.usageRuntimeMaxWorkerDeadLetteredEntriesTotal}`)
}

const usageRuntimeMaxWorkerProcessFailuresTotal = optionalMetric('usage_runtime_max_worker_process_failures_total')
const usageRuntimeWorkerProcessFailuresTotal =
  optionalMetric('usage_runtime_worker_process_failures_total_delta') ?? usageRuntimeMaxWorkerProcessFailuresTotal
if (
  usageRuntimeWorkerProcessFailuresTotal != null
  && usageRuntimeWorkerProcessFailuresTotal > options.usageRuntimeMaxWorkerProcessFailuresTotal
) {
  fail(`usage_runtime_worker_process_failures_total_delta=${usageRuntimeWorkerProcessFailuresTotal}, expected <= ${options.usageRuntimeMaxWorkerProcessFailuresTotal}`)
}

const usageRuntimeMaxWorkerReadFailuresTotal = optionalMetric('usage_runtime_max_worker_read_failures_total')
const usageRuntimeWorkerReadFailuresTotal =
  optionalMetric('usage_runtime_worker_read_failures_total_delta') ?? usageRuntimeMaxWorkerReadFailuresTotal
if (
  usageRuntimeWorkerReadFailuresTotal != null
  && usageRuntimeWorkerReadFailuresTotal > options.usageRuntimeMaxWorkerReadFailuresTotal
) {
  fail(`usage_runtime_worker_read_failures_total_delta=${usageRuntimeWorkerReadFailuresTotal}, expected <= ${options.usageRuntimeMaxWorkerReadFailuresTotal}`)
}

const usageRuntimeMaxWorkerReclaimFailuresTotal = optionalMetric('usage_runtime_max_worker_reclaim_failures_total')
const usageRuntimeWorkerReclaimFailuresTotal =
  optionalMetric('usage_runtime_worker_reclaim_failures_total_delta') ?? usageRuntimeMaxWorkerReclaimFailuresTotal
if (
  usageRuntimeWorkerReclaimFailuresTotal != null
  && usageRuntimeWorkerReclaimFailuresTotal > options.usageRuntimeMaxWorkerReclaimFailuresTotal
) {
  fail(`usage_runtime_worker_reclaim_failures_total_delta=${usageRuntimeWorkerReclaimFailuresTotal}, expected <= ${options.usageRuntimeMaxWorkerReclaimFailuresTotal}`)
}

const usageQueueHealthUnavailableSamples = optionalMetric('usage_queue_health_unavailable_samples')
if (
  usageQueueHealthUnavailableSamples != null
  && usageQueueHealthUnavailableSamples > options.maxUsageQueueHealthUnavailableSamples
) {
  fail(`usage_queue_health_unavailable_samples=${usageQueueHealthUnavailableSamples}, expected <= ${options.maxUsageQueueHealthUnavailableSamples}`)
}

const usageQueueFinalGroupPending = optionalMetric('usage_queue_final_group_pending')
if (
  usageQueueFinalGroupPending != null
  && usageQueueFinalGroupPending > options.usageQueueMaxFinalPending
) {
  fail(`usage_queue_final_group_pending=${usageQueueFinalGroupPending}, expected <= ${options.usageQueueMaxFinalPending}`)
}

const usageQueueFinalGroupLag = optionalMetric('usage_queue_final_group_lag')
if (
  usageQueueFinalGroupLag != null
  && usageQueueFinalGroupLag > options.usageQueueMaxFinalLag
) {
  fail(`usage_queue_final_group_lag=${usageQueueFinalGroupLag}, expected <= ${options.usageQueueMaxFinalLag}`)
}

const usageQueueFinalDlqLength = optionalMetric('usage_queue_final_dlq_length')
if (
  usageQueueFinalDlqLength != null
  && usageQueueFinalDlqLength > options.usageQueueMaxFinalDlqLength
) {
  fail(`usage_queue_final_dlq_length=${usageQueueFinalDlqLength}, expected <= ${options.usageQueueMaxFinalDlqLength}`)
}

const usageQueueMaxOldestPendingIdleMs = optionalMetric('usage_queue_max_oldest_pending_idle_ms')
if (
  usageQueueMaxOldestPendingIdleMs != null
  && usageQueueMaxOldestPendingIdleMs >= options.usageQueueMaxOldestPendingIdleMs
) {
  fail(`usage_queue_max_oldest_pending_idle_ms=${usageQueueMaxOldestPendingIdleMs}, expected < ${options.usageQueueMaxOldestPendingIdleMs}`)
}

const usageCounterHealthUnavailableSamples = optionalMetric('usage_counter_health_unavailable_samples')
if (
  usageCounterHealthUnavailableSamples != null
  && usageCounterHealthUnavailableSamples > options.maxUsageCounterHealthUnavailableSamples
) {
  fail(`usage_counter_health_unavailable_samples=${usageCounterHealthUnavailableSamples}, expected <= ${options.maxUsageCounterHealthUnavailableSamples}`)
}

const usageCounterOutboxFinalPendingRows = optionalMetric('usage_counter_outbox_final_pending_rows')
if (
  usageCounterOutboxFinalPendingRows != null
  && usageCounterOutboxFinalPendingRows > options.usageCounterOutboxMaxFinalPendingRows
) {
  fail(`usage_counter_outbox_final_pending_rows=${usageCounterOutboxFinalPendingRows}, expected <= ${options.usageCounterOutboxMaxFinalPendingRows}`)
}

const usageCounterOutboxMaxOldestPendingAgeSeconds = optionalMetric('usage_counter_outbox_max_oldest_pending_age_seconds')
if (
  usageCounterOutboxMaxOldestPendingAgeSeconds != null
  && usageCounterOutboxMaxOldestPendingAgeSeconds >= options.usageCounterOutboxMaxOldestPendingAgeSeconds
) {
  fail(`usage_counter_outbox_max_oldest_pending_age_seconds=${usageCounterOutboxMaxOldestPendingAgeSeconds}, expected < ${options.usageCounterOutboxMaxOldestPendingAgeSeconds}`)
}

const usageCounterOutboxMaxFlushFailedBatchesTotal = optionalMetric('usage_counter_outbox_max_flush_failed_batches_total')
const usageCounterOutboxFlushFailedBatchesTotal =
  optionalMetric('usage_counter_outbox_flush_failed_batches_total_delta') ?? usageCounterOutboxMaxFlushFailedBatchesTotal
if (
  usageCounterOutboxFlushFailedBatchesTotal != null
  && usageCounterOutboxFlushFailedBatchesTotal > options.usageCounterOutboxMaxFlushFailedBatchesTotal
) {
  fail(`usage_counter_outbox_flush_failed_batches_total_delta=${usageCounterOutboxFlushFailedBatchesTotal}, expected <= ${options.usageCounterOutboxMaxFlushFailedBatchesTotal}`)
}

const usageCounterOutboxMaxCleanupFailedBatchesTotal = optionalMetric('usage_counter_outbox_max_cleanup_failed_batches_total')
const usageCounterOutboxCleanupFailedBatchesTotal =
  optionalMetric('usage_counter_outbox_cleanup_failed_batches_total_delta') ?? usageCounterOutboxMaxCleanupFailedBatchesTotal
if (
  usageCounterOutboxCleanupFailedBatchesTotal != null
  && usageCounterOutboxCleanupFailedBatchesTotal > options.usageCounterOutboxMaxCleanupFailedBatchesTotal
) {
  fail(`usage_counter_outbox_cleanup_failed_batches_total_delta=${usageCounterOutboxCleanupFailedBatchesTotal}, expected <= ${options.usageCounterOutboxMaxCleanupFailedBatchesTotal}`)
}

if (metric('upstream_target_max_rejected_total') !== 0) {
  fail(`upstream_target_max_rejected_total=${metrics.upstream_target_max_rejected_total}`)
}

const gatewayProcessMaxOpenFds = optionalMetric('gateway_process_max_open_fds')
const gatewayProcessFdLimit = optionalMetric('gateway_process_fd_limit')
let gatewayProcessMaxFdUsageBasisPoints = optionalMetric('gateway_process_max_fd_usage_basis_points')
if (
  gatewayProcessMaxFdUsageBasisPoints == null
  && gatewayProcessMaxOpenFds != null
  && gatewayProcessFdLimit != null
  && gatewayProcessFdLimit > 0
) {
  gatewayProcessMaxFdUsageBasisPoints = Math.floor(gatewayProcessMaxOpenFds * 10000 / gatewayProcessFdLimit)
}
if (
  gatewayProcessMaxFdUsageBasisPoints != null
  && gatewayProcessMaxFdUsageBasisPoints >= options.gatewayProcessMaxFdUsageBasisPoints
) {
  fail(`gateway_process_max_fd_usage_basis_points=${gatewayProcessMaxFdUsageBasisPoints}, expected < ${options.gatewayProcessMaxFdUsageBasisPoints}`)
}

const gatewayProcessMaxTcpCloseWaitConnections = optionalMetric('gateway_process_max_tcp_close_wait_connections')
if (
  gatewayProcessMaxTcpCloseWaitConnections != null
  && gatewayProcessMaxTcpCloseWaitConnections > options.gatewayProcessMaxTcpCloseWaitConnections
) {
  fail(`gateway_process_max_tcp_close_wait_connections=${gatewayProcessMaxTcpCloseWaitConnections}, expected <= ${options.gatewayProcessMaxTcpCloseWaitConnections}`)
}

const gatewayBackgroundTasksMaxUnexpectedExitsTotal = optionalMetric('gateway_background_tasks_max_unexpected_exits_total')
const gatewayBackgroundTasksUnexpectedExitLimit = options.gatewayBackgroundTasksMaxUnexpectedExitsTotal ?? 0
if (
  gatewayBackgroundTasksMaxUnexpectedExitsTotal != null
  && gatewayBackgroundTasksMaxUnexpectedExitsTotal > gatewayBackgroundTasksUnexpectedExitLimit
) {
  fail(`gateway_background_tasks_max_unexpected_exits_total=${gatewayBackgroundTasksMaxUnexpectedExitsTotal}, expected <= ${gatewayBackgroundTasksUnexpectedExitLimit}`)
}

const gatewayTokioRuntimeObservabilityAvailable = optionalMetric('gateway_tokio_runtime_observability_available')
if (
  gatewayTokioRuntimeObservabilityAvailable != null
  && gatewayTokioRuntimeObservabilityAvailable !== 1
) {
  fail(`gateway_tokio_runtime_observability_available=${gatewayTokioRuntimeObservabilityAvailable}, expected 1`)
}

if (failures > 0) {
  process.exit(1)
}

console.log(`${options.stage} PASS`)
console.log(`report=${options.reportPath}`)
console.log(`completed=${completedRequests} concurrency=${concurrency} throughput_rps=${load.throughput_rps ?? '-'} p95_ms=${load.p95_ms ?? '-'} p99_ms=${load.p99_ms ?? '-'}`)
if (load.headers_p95_ms != null || load.first_body_p95_ms != null) {
  console.log(`headers_p95_ms=${load.headers_p95_ms ?? '-'} first_body_p95_ms=${load.first_body_p95_ms ?? '-'}`)
}
console.log(`db_pool_max_usage_basis_points=${dbPoolUsage}`)
console.log(`db_pool_pressure_samples=${dbPoolPressureSamples}/${sampleCount}`)
console.log(`db_pool_pressure_sample_rate_basis_points=${dbPoolPressureSampleRateBasisPoints}`)
console.log(`candidate_final_depth=${metrics.request_candidate_queue_final_depth}`)
if (metrics.request_candidate_queue_max_enqueued_total != null) {
  console.log(`request_candidate_queue_max_enqueued_total=${metrics.request_candidate_queue_max_enqueued_total}`)
}
if (metrics.request_candidate_queue_max_flushed_total != null) {
  console.log(`request_candidate_queue_max_flushed_total=${metrics.request_candidate_queue_max_flushed_total}`)
}
if (metrics.request_candidate_queue_max_flush_batches_total != null) {
  console.log(`request_candidate_queue_max_flush_batches_total=${metrics.request_candidate_queue_max_flush_batches_total}`)
}
if (metrics.request_candidate_queue_max_flush_sql_ops_total != null) {
  console.log(`request_candidate_queue_max_flush_sql_ops_total=${metrics.request_candidate_queue_max_flush_sql_ops_total}`)
}
if (metrics.request_candidate_queue_max_flush_sql_records_total != null) {
  console.log(`request_candidate_queue_max_flush_sql_records_total=${metrics.request_candidate_queue_max_flush_sql_records_total}`)
}
if (metrics.request_candidate_queue_max_db_write_concurrency_limit != null) {
  console.log(`request_candidate_queue_max_db_write_concurrency_limit=${metrics.request_candidate_queue_max_db_write_concurrency_limit}`)
}
if (metrics.request_candidate_queue_max_db_write_max_in_flight != null) {
  console.log(`request_candidate_queue_max_db_write_max_in_flight=${metrics.request_candidate_queue_max_db_write_max_in_flight}`)
}
if (metrics.request_candidate_queue_max_db_write_wait_total != null) {
  console.log(`request_candidate_queue_max_db_write_wait_total=${metrics.request_candidate_queue_max_db_write_wait_total}`)
}
if (metrics.request_candidate_queue_max_compacted_total != null) {
  console.log(`request_candidate_queue_max_compacted_total=${metrics.request_candidate_queue_max_compacted_total}`)
}
if (postgresMaxLockWaitingConnections != null) {
  console.log(`postgres_max_lock_waiting_connections=${postgresMaxLockWaitingConnections}`)
}
if (postgresMaxOldestActiveQueryAgeMs != null) {
  console.log(`postgres_max_oldest_active_query_age_ms=${postgresMaxOldestActiveQueryAgeMs}`)
}
if (postgresMaxOldestTransactionAgeMs != null) {
  console.log(`postgres_max_oldest_transaction_age_ms=${postgresMaxOldestTransactionAgeMs}`)
}
if (metrics.postgres_final_block_cache_hit_rate_basis_points != null) {
  console.log(`postgres_final_block_cache_hit_rate_basis_points=${metrics.postgres_final_block_cache_hit_rate_basis_points}`)
}
if (metrics.postgres_final_temp_bytes_total != null) {
  console.log(`postgres_final_temp_bytes_total=${metrics.postgres_final_temp_bytes_total}`)
}
if (postgresWalObservabilityUnavailableSamples != null) {
  console.log(`postgres_wal_observability_unavailable_samples=${postgresWalObservabilityUnavailableSamples}`)
}
if (metrics.postgres_final_wal_bytes_total != null) {
  console.log(`postgres_final_wal_bytes_total=${metrics.postgres_final_wal_bytes_total}`)
}
if (postgresCheckpointObservabilityUnavailableSamples != null) {
  console.log(`postgres_checkpoint_observability_unavailable_samples=${postgresCheckpointObservabilityUnavailableSamples}`)
}
if (metrics.postgres_final_checkpoint_write_time_ms_total != null) {
  console.log(`postgres_final_checkpoint_write_time_ms_total=${metrics.postgres_final_checkpoint_write_time_ms_total}`)
}
if (postgresStatementObservabilityUnavailableSamples != null) {
  console.log(`postgres_statement_observability_unavailable_samples=${postgresStatementObservabilityUnavailableSamples}`)
}
if (postgresMaxStatementTopMaxExecTimeMs != null) {
  console.log(`postgres_max_statement_top_max_exec_time_ms=${postgresMaxStatementTopMaxExecTimeMs}`)
}
if (redisRuntimeHealthUnavailableSamples != null) {
  console.log(`redis_runtime_health_unavailable_samples=${redisRuntimeHealthUnavailableSamples}`)
}
if (redisRuntimeMaxMemoryUsageBasisPoints != null) {
  console.log(`redis_runtime_max_memory_usage_basis_points=${redisRuntimeMaxMemoryUsageBasisPoints}`)
}
if (redisRuntimeMaxLaneCommandTimeoutsTotal != null) {
  console.log(`redis_runtime_max_lane_command_timeouts_total=${redisRuntimeMaxLaneCommandTimeoutsTotal}`)
}
if (redisRuntimeNonblockingCommandLatencyMs != null) {
  console.log(`redis_runtime_nonblocking_command_latency_ms_delta=${redisRuntimeNonblockingCommandLatencyMs}`)
}
if (gatewayProcessMaxFdUsageBasisPoints != null) {
  console.log(`gateway_process_max_fd_usage_basis_points=${gatewayProcessMaxFdUsageBasisPoints}`)
}
if (metrics.gateway_process_max_threads != null) {
  console.log(`gateway_process_max_threads=${metrics.gateway_process_max_threads}`)
}
if (metrics.gateway_allocator_observability_available != null) {
  console.log(`gateway_allocator_observability_available=${metrics.gateway_allocator_observability_available}`)
}
if (metrics.gateway_allocator_max_allocated_bytes != null) {
  console.log(`gateway_allocator_max_allocated_bytes=${metrics.gateway_allocator_max_allocated_bytes}`)
}
if (metrics.gateway_allocator_max_resident_bytes != null) {
  console.log(`gateway_allocator_max_resident_bytes=${metrics.gateway_allocator_max_resident_bytes}`)
}
if (metrics.gateway_allocator_max_retained_bytes != null) {
  console.log(`gateway_allocator_max_retained_bytes=${metrics.gateway_allocator_max_retained_bytes}`)
}
if (metrics.gateway_allocator_max_active_to_allocated_basis_points != null) {
  console.log(`gateway_allocator_max_active_to_allocated_basis_points=${metrics.gateway_allocator_max_active_to_allocated_basis_points}`)
}
if (metrics.gateway_allocator_max_resident_to_allocated_basis_points != null) {
  console.log(`gateway_allocator_max_resident_to_allocated_basis_points=${metrics.gateway_allocator_max_resident_to_allocated_basis_points}`)
}
if (metrics.gateway_background_tasks_max_active != null) {
  console.log(`gateway_background_tasks_max_active=${metrics.gateway_background_tasks_max_active}`)
}
if (metrics.gateway_background_tasks_max_supervised_total != null) {
  console.log(`gateway_background_tasks_max_supervised_total=${metrics.gateway_background_tasks_max_supervised_total}`)
}
if (gatewayBackgroundTasksMaxUnexpectedExitsTotal != null) {
  console.log(`gateway_background_tasks_max_unexpected_exits_total=${gatewayBackgroundTasksMaxUnexpectedExitsTotal}`)
}
if (gatewayTokioRuntimeObservabilityAvailable != null) {
  console.log(`gateway_tokio_runtime_observability_available=${gatewayTokioRuntimeObservabilityAvailable}`)
}
if (metrics.gateway_tokio_runtime_max_workers != null) {
  console.log(`gateway_tokio_runtime_max_workers=${metrics.gateway_tokio_runtime_max_workers}`)
}
if (metrics.gateway_tokio_runtime_max_alive_tasks != null) {
  console.log(`gateway_tokio_runtime_max_alive_tasks=${metrics.gateway_tokio_runtime_max_alive_tasks}`)
}
if (metrics.gateway_tokio_runtime_max_global_queue_depth != null) {
  console.log(`gateway_tokio_runtime_max_global_queue_depth=${metrics.gateway_tokio_runtime_max_global_queue_depth}`)
}
if (metrics.gateway_process_max_socket_fds != null) {
  console.log(`gateway_process_max_socket_fds=${metrics.gateway_process_max_socket_fds}`)
}
if (metrics.gateway_process_max_tcp_established_connections != null) {
  console.log(`gateway_process_max_tcp_established_connections=${metrics.gateway_process_max_tcp_established_connections}`)
}
if (gatewayProcessMaxTcpCloseWaitConnections != null) {
  console.log(`gateway_process_max_tcp_close_wait_connections=${gatewayProcessMaxTcpCloseWaitConnections}`)
}
if (metrics.gateway_host_max_tcp_time_wait_connections != null) {
  console.log(`gateway_host_max_tcp_time_wait_connections=${metrics.gateway_host_max_tcp_time_wait_connections}`)
}
if (metrics.gateway_network_receive_dropped_total_final != null || metrics.gateway_network_transmit_dropped_total_final != null) {
  console.log(`gateway_network_drops_final=rx:${metrics.gateway_network_receive_dropped_total_final ?? '-'} tx:${metrics.gateway_network_transmit_dropped_total_final ?? '-'}`)
}
if (usageCounterOutboxFinalPendingRows != null) {
  console.log(`usage_counter_outbox_final_pending_rows=${usageCounterOutboxFinalPendingRows}`)
}
if (usageCounterOutboxMaxOldestPendingAgeSeconds != null) {
  console.log(`usage_counter_outbox_max_oldest_pending_age_seconds=${usageCounterOutboxMaxOldestPendingAgeSeconds}`)
}
if (metrics.usage_counter_outbox_max_flush_batches_total != null) {
  console.log(`usage_counter_outbox_max_flush_batches_total=${metrics.usage_counter_outbox_max_flush_batches_total}`)
}
if (metrics.usage_counter_outbox_max_flush_rows_claimed_total != null) {
  console.log(`usage_counter_outbox_max_flush_rows_claimed_total=${metrics.usage_counter_outbox_max_flush_rows_claimed_total}`)
}
if (metrics.usage_counter_outbox_max_flush_targets_total != null) {
  console.log(`usage_counter_outbox_max_flush_targets_total=${metrics.usage_counter_outbox_max_flush_targets_total}`)
}
if (usageCounterOutboxFlushFailedBatchesTotal != null) {
  console.log(`usage_counter_outbox_flush_failed_batches_total_delta=${usageCounterOutboxFlushFailedBatchesTotal}`)
}
if (metrics.usage_counter_outbox_max_cleanup_rows_total != null) {
  console.log(`usage_counter_outbox_max_cleanup_rows_total=${metrics.usage_counter_outbox_max_cleanup_rows_total}`)
}
if (usageCounterOutboxCleanupFailedBatchesTotal != null) {
  console.log(`usage_counter_outbox_cleanup_failed_batches_total_delta=${usageCounterOutboxCleanupFailedBatchesTotal}`)
}
if (usageQueueFinalGroupPending != null) {
  console.log(`usage_queue_final_group_pending=${usageQueueFinalGroupPending}`)
}
if (usageQueueFinalGroupLag != null) {
  console.log(`usage_queue_final_group_lag=${usageQueueFinalGroupLag}`)
}
if (usageQueueFinalDlqLength != null) {
  console.log(`usage_queue_final_dlq_length=${usageQueueFinalDlqLength}`)
}
if (metrics.usage_runtime_max_worker_read_batches_total != null) {
  console.log(`usage_runtime_max_worker_read_batches_total=${metrics.usage_runtime_max_worker_read_batches_total}`)
}
if (metrics.usage_runtime_max_worker_read_entries_total != null) {
  console.log(`usage_runtime_max_worker_read_entries_total=${metrics.usage_runtime_max_worker_read_entries_total}`)
}
if (metrics.usage_runtime_max_worker_acked_entries_total != null) {
  console.log(`usage_runtime_max_worker_acked_entries_total=${metrics.usage_runtime_max_worker_acked_entries_total}`)
}
if (metrics.usage_runtime_max_worker_reclaimed_entries_total != null) {
  console.log(`usage_runtime_max_worker_reclaimed_entries_total=${metrics.usage_runtime_max_worker_reclaimed_entries_total}`)
}
if (metrics.usage_runtime_max_worker_record_concurrency_limit != null) {
  console.log(`usage_runtime_max_worker_record_concurrency_limit=${metrics.usage_runtime_max_worker_record_concurrency_limit}`)
}
if (metrics.usage_runtime_max_worker_record_concurrency_max_in_flight != null) {
  console.log(`usage_runtime_max_worker_record_concurrency_max_in_flight=${metrics.usage_runtime_max_worker_record_concurrency_max_in_flight}`)
}
if (metrics.usage_runtime_max_worker_record_concurrency_wait_total != null) {
  console.log(`usage_runtime_max_worker_record_concurrency_wait_total=${metrics.usage_runtime_max_worker_record_concurrency_wait_total}`)
}
if (usageRuntimeMaxWorkerDeadLetteredEntriesTotal != null) {
  console.log(`usage_runtime_max_worker_dead_lettered_entries_total=${usageRuntimeMaxWorkerDeadLetteredEntriesTotal}`)
}
if (usageRuntimeWorkerProcessFailuresTotal != null) {
  console.log(`usage_runtime_worker_process_failures_total_delta=${usageRuntimeWorkerProcessFailuresTotal}`)
}
if (usageRuntimeWorkerReadFailuresTotal != null) {
  console.log(`usage_runtime_worker_read_failures_total_delta=${usageRuntimeWorkerReadFailuresTotal}`)
}
if (usageRuntimeWorkerReclaimFailuresTotal != null) {
  console.log(`usage_runtime_worker_reclaim_failures_total_delta=${usageRuntimeWorkerReclaimFailuresTotal}`)
}

function printUsage() {
  console.error('usage: check_gateway_stage_report.js [--stage S1|S2|S3|S4|S5|realistic-stream|tps] [--min-requests N] [--min-concurrency N] [--min-throughput-rps N] [--max-headers-p95-ms N] [--max-first-body-p95-ms N] [--max-p95-ms N] [--max-p99-ms N] [--max-first-body-hold-ms N] [--expected-response-mode headers|first-body-byte|full] [--db-pool-max-usage-basis-points N] [--max-db-pool-pressure-samples N] [--max-db-pool-pressure-sample-rate-basis-points N] [--max-postgres-observability-unavailable-samples N] [--max-postgres-wal-observability-unavailable-samples N] [--max-postgres-checkpoint-observability-unavailable-samples N] [--max-postgres-statement-observability-unavailable-samples N] [--max-postgres-lock-waiting-connections N] [--max-postgres-idle-in-transaction-connections N] [--max-postgres-oldest-active-query-age-ms N] [--max-postgres-oldest-transaction-age-ms N] [--max-postgres-statement-top-max-exec-time-ms N] [--max-redis-runtime-health-unavailable-samples N] [--max-redis-runtime-memory-usage-basis-points N] [--max-redis-runtime-rejected-connections-total N] [--max-redis-runtime-evicted-keys-total N] [--max-redis-runtime-error-replies-total N] [--max-redis-runtime-lane-command-errors-total N] [--max-redis-runtime-lane-command-timeouts-total N] [--max-redis-runtime-nonblocking-command-latency-ms N] [--max-gateway-process-fd-usage-basis-points N] [--max-gateway-process-tcp-close-wait-connections N] [--max-gateway-background-tasks-unexpected-exits-total N] [--max-usage-queue-final-pending N] [--max-usage-queue-final-lag N] [--max-usage-queue-final-dlq-length N] [--max-usage-queue-oldest-pending-idle-ms N] [--max-usage-queue-health-unavailable-samples N] [--max-usage-counter-outbox-final-pending-rows N] [--max-usage-counter-outbox-oldest-pending-age-seconds N] [--max-usage-counter-health-unavailable-samples N] [--max-usage-counter-outbox-flush-failed-batches-total N] [--max-usage-counter-outbox-cleanup-failed-batches-total N] [--max-usage-runtime-worker-dead-lettered-entries-total N] [--max-usage-runtime-worker-process-failures-total N] [--max-usage-runtime-worker-read-failures-total N] [--max-usage-runtime-worker-reclaim-failures-total N] [report.json]')
}
