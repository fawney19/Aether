#!/usr/bin/env node
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const test = require('node:test')
const { spawnSync } = require('node:child_process')

const checker = path.join(__dirname, 'check_gateway_stage_report.js')

test('realistic-stream report passes full-chain latency and throughput checks', () => {
  const report = reportFor({
    totalRequests: 1000,
    concurrency: 1000,
    throughputRps: 180,
    headersP95Ms: 120,
    firstBodyP95Ms: 350,
    p95Ms: 6000,
    p99Ms: 9000,
  })
  const result = runChecker('--stage', 'realistic-stream', writeReport(report))

  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /REALISTIC_STREAM PASS/)
  assert.match(result.stdout, /throughput_rps=180/)
})

for (const [stage, totalRequests, concurrency] of [
  ['S1', 1000, 1000],
  ['S2', 3000, 3000],
  ['S3', 6000, 6000],
  ['S4', 10000, 10000],
  ['S5', 10000, 10000],
]) {
  test(`${stage} report accepts the two-minute staged streaming contract`, () => {
    const report = reportFor({
      totalRequests,
      concurrency,
      throughputRps: 40,
      headersP95Ms: 120,
      firstBodyP95Ms: 350,
      p95Ms: 120000,
      p99Ms: 120100,
      responseMode: 'FirstBodyByte',
      firstBodyHoldMs: 120000,
    })
    const result = runChecker('--stage', stage, writeReport(report))

    assert.equal(result.status, 0, result.stderr)
    assert.match(result.stdout, new RegExp(`${stage} PASS`))
  })
}

test('S4 report rejects non-standard timeout, hold, and response mode', () => {
  const report = reportFor({
    totalRequests: 10000,
    concurrency: 10000,
    throughputRps: 40,
    headersP95Ms: 120,
    firstBodyP95Ms: 350,
    p95Ms: 120000,
    p99Ms: 120100,
    timeoutMs: 2100000,
    firstBodyHoldMs: 1800000,
    responseMode: 'FullBody',
  })
  const result = runChecker('--stage', 'S4', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /S4 FAIL: load\.timeout_ms=2100000, expected 150000/)
  assert.match(result.stderr, /S4 FAIL: load\.first_body_hold_ms=1800000, expected 120000/)
  assert.match(result.stderr, /S4 FAIL: load\.response_mode=FullBody, expected FirstBodyByte/)
})

test('S5 report rejects a legacy report without an effective timeout', () => {
  const report = reportFor({
    totalRequests: 10000,
    concurrency: 10000,
    throughputRps: 40,
    headersP95Ms: 120,
    firstBodyP95Ms: 350,
    p95Ms: 120000,
    p99Ms: 120100,
    responseMode: 'FirstBodyByte',
    firstBodyHoldMs: 120000,
  })
  delete report.load.timeout_ms
  const result = runChecker('--stage', 'S5', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /S5 FAIL: missing load\.timeout_ms, expected 150000/)
})

test('realistic-stream report rejects staged-stream timing and response mode', () => {
  const report = reportFor({
    totalRequests: 1000,
    concurrency: 1000,
    throughputRps: 180,
    headersP95Ms: 120,
    firstBodyP95Ms: 350,
    p95Ms: 6000,
    p99Ms: 9000,
    timeoutMs: 300000,
    firstBodyHoldMs: 120000,
    responseMode: 'FirstBodyByte',
  })
  const result = runChecker('--stage', 'realistic-stream', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /REALISTIC_STREAM FAIL: load\.timeout_ms=300000, expected 150000/)
  assert.match(result.stderr, /REALISTIC_STREAM FAIL: load\.first_body_hold_ms=120000, expected 0/)
  assert.match(
    result.stderr,
    /REALISTIC_STREAM FAIL: load\.response_mode=FirstBodyByte, expected FullBody/,
  )
})

test('tps report passes completed request throughput checks', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  const result = runChecker('--stage', 'TPS', writeReport(report))

  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /TPS PASS/)
})

test('tps report fails when throughput is below the acceptance threshold', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 999,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: load\.throughput_rps=999, expected >= 1000/)
})

test('tps report rejects the legacy 20k request and 500 concurrency baseline', () => {
  const report = reportFor({
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: load\.total_requests=20000, expected >= 30000/)
  assert.match(result.stderr, /TPS FAIL: load\.concurrency=500, expected >= 600/)
})

test('tps report fails when settle drain does not complete', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  report.settle_drain_completed = false
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: settle_drain_completed=false after 5000ms/)
})

test('tps report fails when lifecycle enqueue drops deferred events', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  report.metrics.usage_runtime_max_lifecycle_enqueue_deferred_dropped_total = 1
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(
    result.stderr,
    /TPS FAIL: usage_runtime_max_lifecycle_enqueue_deferred_dropped_total=1/,
  )
})

test('tps report fails when the local usage enqueue dispatcher does not fully drain', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  report.metrics.usage_runtime_final_enqueue_retry_pending = 1
  report.metrics.usage_runtime_enqueue_retry_scheduled_total_delta = 10
  report.metrics.usage_runtime_enqueue_retry_recovered_total_delta = 9
  report.metrics.usage_runtime_enqueue_retry_failed_total_delta = 1
  report.metrics.usage_runtime_enqueue_retry_closed_or_unavailable_total_delta = 1
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: usage_runtime_final_enqueue_retry_pending=1/)
  assert.match(
    result.stderr,
    /TPS FAIL: usage_runtime enqueue dispatcher did not fully recover: scheduled_delta=10 recovered_delta=9/,
  )
  assert.match(result.stderr, /TPS FAIL: usage_runtime_enqueue_retry_failed_total_delta=1/)
  assert.match(
    result.stderr,
    /TPS FAIL: usage_runtime_enqueue_retry_closed_or_unavailable_total_delta=1/,
  )
})

test('tps report ignores shared redis error replies when gateway lane errors are clean', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  report.metrics.redis_runtime_total_error_replies_delta = 12
  report.metrics.redis_runtime_lane_command_errors_total_delta = 0
  report.metrics.redis_runtime_lane_command_timeouts_total_delta = 0
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 0, result.stderr)
})

test('tps report falls back to redis error replies when lane metrics are absent', () => {
  const report = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  report.metrics.redis_runtime_total_error_replies_delta = 1
  delete report.metrics.redis_runtime_lane_command_errors_total_delta
  delete report.metrics.redis_runtime_lane_command_timeouts_total_delta
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: redis_runtime_total_error_replies_delta=1/)
})

test('tps report evaluates Redis latency by slow-command rate with a hard maximum', () => {
  const passing = reportFor({
    totalRequests: 30000,
    concurrency: 600,
    throughputRps: 1050,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  passing.metrics.redis_runtime_nonblocking_command_latency_ms_delta = 777
  passing.metrics.redis_runtime_nonblocking_command_over_500ms_rate_basis_points = 8
  const passingResult = runChecker('--stage', 'tps', writeReport(passing))
  assert.equal(passingResult.status, 0, passingResult.stderr)

  const excessiveRate = structuredClone(passing)
  excessiveRate.metrics.redis_runtime_nonblocking_command_over_500ms_rate_basis_points = 11
  const excessiveRateResult = runChecker('--stage', 'tps', writeReport(excessiveRate))
  assert.equal(excessiveRateResult.status, 1)
  assert.match(
    excessiveRateResult.stderr,
    /TPS FAIL: redis_runtime_nonblocking_command_over_500ms_rate_basis_points=11, expected <= 10/,
  )
  const overriddenRateResult = runChecker(
    '--stage',
    'tps',
    '--max-redis-runtime-nonblocking-over-500ms-rate-basis-points',
    '20',
    writeReport(excessiveRate),
  )
  assert.equal(overriddenRateResult.status, 0, overriddenRateResult.stderr)

  const excessiveMaximum = structuredClone(passing)
  excessiveMaximum.metrics.redis_runtime_nonblocking_command_latency_ms_delta = 1000
  const excessiveMaximumResult = runChecker('--stage', 'tps', writeReport(excessiveMaximum))
  assert.equal(excessiveMaximumResult.status, 1)
  assert.match(
    excessiveMaximumResult.stderr,
    /TPS FAIL: redis_runtime_nonblocking_command_latency_ms_delta=1000, expected < 1000/,
  )
})

function runChecker(...args) {
  return spawnSync(process.execPath, [checker, ...args], {
    cwd: path.resolve(__dirname, '../..'),
    encoding: 'utf8',
  })
}

function writeReport(report) {
  const file = path.join(
    fs.mkdtempSync(path.join(os.tmpdir(), 'aether-stage-report-')),
    'report.json',
  )
  fs.writeFileSync(file, `${JSON.stringify(report)}\n`)
  return file
}

function reportFor({
  totalRequests,
  concurrency,
  throughputRps,
  headersP95Ms,
  firstBodyP95Ms,
  p95Ms,
  p99Ms,
  timeoutMs = 150000,
  firstBodyHoldMs = 0,
  responseMode = 'FullBody',
}) {
  return {
    settle_after_ms: 5000,
    settle_drain_completed: true,
    settle_drain_elapsed_ms: 250,
    load: {
      response_mode: responseMode,
      total_requests: totalRequests,
      completed_requests: totalRequests,
      failed_requests: 0,
      concurrency,
      timeout_ms: timeoutMs,
      first_body_hold_ms: firstBodyHoldMs,
      throughput_rps: throughputRps,
      headers_p95_ms: headersP95Ms,
      first_body_p95_ms: firstBodyP95Ms,
      p95_ms: p95Ms,
      p99_ms: p99Ms,
      status_counts: { 200: totalRequests },
      error_counts: {},
    },
    metrics: {
      samples: 10,
      db_pool_max_usage_basis_points: 2500,
      db_pool_pressure_samples: 0,
      gateway_requests_max_rejected_total: 0,
      gateway_requests_distributed_max_rejected_total: 0,
      request_candidate_queue_final_depth: 0,
      request_candidate_queue_final_pending_depth: 0,
      request_candidate_queue_max_flush_failed_total: 0,
      request_candidate_queue_max_dropped_total: 0,
      request_candidate_queue_max_sync_fallback_total: 0,
      usage_runtime_max_terminal_enqueue_failed_total: 0,
      usage_runtime_max_lifecycle_enqueue_failed_total: 0,
      usage_runtime_max_lifecycle_enqueue_deferred_dropped_total: 0,
      usage_runtime_final_enqueue_retry_pending: 0,
      usage_runtime_enqueue_retry_scheduled_total_delta: totalRequests,
      usage_runtime_enqueue_retry_recovered_total_delta: totalRequests,
      usage_runtime_enqueue_retry_failed_total_delta: 0,
      usage_runtime_enqueue_retry_closed_or_unavailable_total_delta: 0,
      redis_runtime_lane_command_errors_total_delta: 0,
      redis_runtime_lane_command_timeouts_total_delta: 0,
      upstream_target_max_rejected_total: 0,
    },
  }
}
