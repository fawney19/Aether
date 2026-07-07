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

test('tps report passes completed request throughput checks', () => {
  const report = reportFor({
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 750,
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
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 499,
    headersP95Ms: 90,
    firstBodyP95Ms: 180,
    p95Ms: 900,
    p99Ms: 1600,
  })
  const result = runChecker('--stage', 'tps', writeReport(report))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /TPS FAIL: load\.throughput_rps=499, expected >= 500/)
})

test('tps report fails when settle drain does not complete', () => {
  const report = reportFor({
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 750,
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
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 750,
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

test('tps report ignores shared redis error replies when gateway lane errors are clean', () => {
  const report = reportFor({
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 750,
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
    totalRequests: 20000,
    concurrency: 500,
    throughputRps: 750,
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
}) {
  return {
    settle_after_ms: 5000,
    settle_drain_completed: true,
    settle_drain_elapsed_ms: 250,
    load: {
      response_mode: 'FullBody',
      total_requests: totalRequests,
      completed_requests: totalRequests,
      failed_requests: 0,
      concurrency,
      first_body_hold_ms: 0,
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
      redis_runtime_lane_command_errors_total_delta: 0,
      redis_runtime_lane_command_timeouts_total_delta: 0,
      upstream_target_max_rejected_total: 0,
    },
  }
}
