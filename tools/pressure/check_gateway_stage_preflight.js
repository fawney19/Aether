#!/usr/bin/env node
const http = require('node:http')
const https = require('node:https')

const DEFAULT_STAGE = 'S1'
const DEFAULT_GATEWAY_BASE_URL = 'http://127.0.0.1:8084'
const DEFAULT_MOCK_UPSTREAM_METRICS_URL = 'http://127.0.0.1:18181/metrics'

const REQUIRED_M4_METRICS = [
  'gateway_process_open_fds',
  'gateway_process_fd_limit',
  'gateway_process_fd_usage_basis_points',
  'gateway_process_threads',
  'gateway_process_socket_fds',
  'gateway_process_tcp_established_connections',
  'gateway_host_tcp_established_connections',
  'gateway_network_observability_available',
  'gateway_network_received_bytes_total',
  'gateway_background_tasks_active',
  'gateway_background_tasks_unexpected_exits_total',
  'gateway_tokio_runtime_observability_available',
  'gateway_tokio_runtime_workers',
  'gateway_allocator_observability_available',
  'postgres_observability_available',
  'postgres_wal_observability_available',
  'postgres_checkpoint_observability_available',
  'postgres_statement_observability_available',
  'redis_runtime_enabled',
  'redis_runtime_lane_command_latency_ms_max',
  'usage_runtime_queue_worker_read_batches_total',
  'usage_runtime_queue_worker_acked_entries_total',
  'usage_counter_outbox_flush_batches_total',
  'usage_counter_outbox_cleanup_rows_total',
  'request_candidate_queue_flush_batches_total',
  'request_candidate_queue_flush_sql_ops_total',
]

function parseArgs(argv) {
  const gatewayBaseUrl = process.env.GATEWAY_BASE_URL || DEFAULT_GATEWAY_BASE_URL
  const options = {
    stage: process.env.PRESSURE_STAGE || DEFAULT_STAGE,
    gatewayBaseUrl,
    healthUrl: `${gatewayBaseUrl.replace(/\/$/, '')}/_gateway/health`,
    metricsUrl: process.env.METRICS_URL || `${gatewayBaseUrl.replace(/\/$/, '')}/_gateway/metrics`,
    targetUrl: process.env.TARGET_URL || `${gatewayBaseUrl.replace(/\/$/, '')}/v1/chat/completions`,
    mockUpstreamMetricsUrl:
      process.env.PRESSURE_MOCK_UPSTREAM_METRICS_URL || DEFAULT_MOCK_UPSTREAM_METRICS_URL,
    timeoutMs: numberEnv('PRESSURE_PREFLIGHT_TIMEOUT_MS') ?? 5000,
    requireAuth: boolEnv('PRESSURE_REQUIRE_AUTH', true),
    requireM4Metrics: boolEnv('PRESSURE_REQUIRE_M4_METRICS', true),
    requireMockUpstream: boolEnv('PRESSURE_REQUIRE_MOCK_UPSTREAM', true),
    apiKeyFile:
      process.env.AETHER_API_KEY_FILE ||
      process.env.API_KEY_FILE ||
      process.env.PRESSURE_API_KEY_FILE ||
      '',
    apiKeyListFile:
      process.env.AETHER_API_KEY_LIST_FILE ||
      process.env.API_KEY_LIST_FILE ||
      process.env.PRESSURE_API_KEY_LIST_FILE ||
      '',
  }

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    switch (arg) {
      case '--stage':
        options.stage = requireValue(argv, ++index, arg)
        break
      case '--gateway-base-url':
        options.gatewayBaseUrl = requireValue(argv, ++index, arg)
        options.healthUrl = `${options.gatewayBaseUrl.replace(/\/$/, '')}/_gateway/health`
        if (!process.env.METRICS_URL) {
          options.metricsUrl = `${options.gatewayBaseUrl.replace(/\/$/, '')}/_gateway/metrics`
        }
        if (!process.env.TARGET_URL) {
          options.targetUrl = `${options.gatewayBaseUrl.replace(/\/$/, '')}/v1/chat/completions`
        }
        break
      case '--health-url':
        options.healthUrl = requireValue(argv, ++index, arg)
        break
      case '--metrics-url':
        options.metricsUrl = requireValue(argv, ++index, arg)
        break
      case '--target-url':
        options.targetUrl = requireValue(argv, ++index, arg)
        break
      case '--mock-upstream-metrics-url':
        options.mockUpstreamMetricsUrl = requireValue(argv, ++index, arg)
        break
      case '--timeout-ms':
        options.timeoutMs = parsePositiveInteger(requireValue(argv, ++index, arg), arg)
        break
      case '--require-auth':
        options.requireAuth = true
        break
      case '--skip-auth':
        options.requireAuth = false
        break
      case '--api-key-file':
        options.apiKeyFile = requireValue(argv, ++index, arg)
        break
      case '--api-key-list-file':
        options.apiKeyListFile = requireValue(argv, ++index, arg)
        break
      case '--require-m4-metrics':
        options.requireM4Metrics = true
        break
      case '--skip-m4-metrics':
        options.requireM4Metrics = false
        break
      case '--require-mock-upstream':
        options.requireMockUpstream = true
        break
      case '--skip-mock-upstream':
        options.requireMockUpstream = false
        break
      case '--help':
      case '-h':
        printHelp()
        process.exit(0)
      default:
        throw new Error(`unknown option: ${arg}`)
    }
  }

  options.stage = String(options.stage).trim().toUpperCase()
  return options
}

function numberEnv(name) {
  const value = process.env[name]
  if (value == null || value === '') {
    return undefined
  }
  return parsePositiveInteger(value, name)
}

function boolEnv(name, fallback) {
  const value = process.env[name]
  if (value == null || value === '') {
    return fallback
  }
  return ['1', 'true', 'yes', 'on'].includes(value.toLowerCase())
}

function requireValue(argv, index, option) {
  const value = argv[index]
  if (!value || value.startsWith('--')) {
    throw new Error(`${option} requires a value`)
  }
  return value
}

function parsePositiveInteger(value, name) {
  const number = Number(value)
  if (!Number.isInteger(number) || number <= 0) {
    throw new Error(`${name} must be a positive integer, got ${value}`)
  }
  return number
}

function printHelp() {
  console.log(`Usage: tools/pressure/check_gateway_stage_preflight.js [options]

Checks whether a gateway is ready to run staged mock streaming pressure tests.
The script never prints auth header or API key values.

Options:
  --stage S1|S2|S3|S4|S5
  --gateway-base-url URL
  --health-url URL
  --metrics-url URL
  --target-url URL
  --mock-upstream-metrics-url URL
  --timeout-ms N
  --skip-auth
  --api-key-file PATH
  --api-key-list-file PATH
  --skip-mock-upstream
  --skip-m4-metrics
`)
}

async function main() {
  const options = parseArgs(process.argv.slice(2))
  const ok = []
  const failures = []

  if (options.requireAuth) {
    if (authConfigured(options)) {
      ok.push('auth configured')
    } else {
      failures.push(
        'missing auth: set AUTH_HEADER, AETHER_API_KEY, API_KEY, AETHER_API_KEY_FILE, or AETHER_API_KEY_LIST_FILE',
      )
    }
  }

  if (!isHttpUrl(options.targetUrl)) {
    failures.push(`target URL is not http(s): ${options.targetUrl}`)
  } else {
    ok.push(`target configured: ${options.targetUrl}`)
  }

  let metricsText = ''
  await checkHttpText('gateway health', options.healthUrl, options.timeoutMs, ok, failures, (body) => {
    const health = parseJson(body)
    if (!health) {
      failures.push('gateway health did not return JSON')
      return
    }
    if (health.status !== 'ok') {
      failures.push(`gateway health status=${health.status ?? 'missing'}, expected ok`)
      return
    }
    ok.push('gateway health status ok')
  })

  await checkHttpText('gateway metrics', options.metricsUrl, options.timeoutMs, ok, failures, (body) => {
    metricsText = body
    const metricNames = parseMetricNames(body)
    if (metricNames.size === 0) {
      failures.push('gateway metrics response did not contain Prometheus samples')
      return
    }
    ok.push(`gateway metrics samples available (${metricNames.size} metric names)`)

    if (options.requireM4Metrics) {
      const missing = REQUIRED_M4_METRICS.filter((name) => !metricNames.has(name))
      if (missing.length > 0) {
        failures.push(`gateway metrics missing M4 required metrics: ${missing.join(', ')}`)
      } else {
        ok.push('gateway M4 metrics present')
      }

      const tokioAvailable = metricMax(body, 'gateway_tokio_runtime_observability_available')
      if (tokioAvailable !== null && tokioAvailable !== 1) {
        failures.push(`gateway_tokio_runtime_observability_available=${tokioAvailable}, expected 1`)
      }
    }
  })

  if (options.requireMockUpstream) {
    await checkHttpText(
      'mock upstream metrics',
      options.mockUpstreamMetricsUrl,
      options.timeoutMs,
      ok,
      failures,
      (body) => {
        if (!body.trim()) {
          failures.push('mock upstream metrics response was empty')
          return
        }
        ok.push('mock upstream metrics available')
      },
    )
  }

  console.log(`gateway staged pressure preflight: ${options.stage}`)
  ok.forEach((line) => console.log(`OK ${line}`))

  if (failures.length > 0) {
    console.error('FAIL preflight checks failed:')
    failures.forEach((line) => console.error(`FAIL ${line}`))
    process.exit(1)
  }

  if (metricsText) {
    const dbPoolMax = metricMax(metricsText, 'database_pool_max_connections')
    const upstreamPermits = metricMax(metricsText, 'concurrency_available_permits', {
      gate: 'gateway_upstream_execution',
    })
    if (dbPoolMax !== null) {
      console.log(`OK database_pool_max_connections=${dbPoolMax}`)
    }
    if (upstreamPermits !== null) {
      console.log(`OK gateway_upstream_execution_available_permits=${upstreamPermits}`)
    }
  }

  console.log('PASS gateway staged pressure preflight')
}

function authConfigured(options) {
  if (['AUTH_HEADER', 'AETHER_API_KEY', 'API_KEY'].some((name) => {
    const value = process.env[name]
    return typeof value === 'string' && value.trim().length > 0
  })) {
    return true
  }
  return fileHasSecret(options.apiKeyFile) || fileHasSecret(options.apiKeyListFile)
}

function fileHasSecret(path) {
  if (!path || !path.trim()) {
    return false
  }
  try {
    const fs = require('node:fs')
    return fs.readFileSync(path, 'utf8').trim().length > 0
  } catch (_error) {
    return false
  }
}

async function checkHttpText(label, url, timeoutMs, ok, failures, inspect) {
  if (!isHttpUrl(url)) {
    failures.push(`${label} URL is not http(s): ${url}`)
    return
  }

  try {
    const response = await requestText(url, timeoutMs)
    if (response.statusCode < 200 || response.statusCode >= 300) {
      failures.push(`${label} returned HTTP ${response.statusCode}`)
      return
    }
    inspect(response.body)
  } catch (error) {
    failures.push(`${label} unreachable at ${url}: ${error.message}`)
  }
}

function isHttpUrl(value) {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:'
  } catch (_error) {
    return false
  }
}

function requestText(urlString, timeoutMs) {
  return new Promise((resolve, reject) => {
    const url = new URL(urlString)
    const client = url.protocol === 'https:' ? https : http
    const request = client.get(
      url,
      {
        headers: {
          accept: 'text/plain, application/json;q=0.9, */*;q=0.1',
        },
      },
      (response) => {
        response.setEncoding('utf8')
        let body = ''
        response.on('data', (chunk) => {
          body += chunk
        })
        response.on('end', () => {
          resolve({
            statusCode: response.statusCode ?? 0,
            body,
          })
        })
      },
    )
    request.setTimeout(timeoutMs, () => {
      request.destroy(new Error(`timed out after ${timeoutMs}ms`))
    })
    request.on('error', reject)
  })
}

function parseJson(value) {
  try {
    return JSON.parse(value)
  } catch (_error) {
    return null
  }
}

function parseMetricNames(text) {
  const names = new Set()
  for (const line of text.split(/\r?\n/)) {
    if (!line || line.startsWith('#')) {
      continue
    }
    const match = line.match(/^([a-zA-Z_:][a-zA-Z0-9_:.-]*)(?:\{|[\s])/)
    if (!match) {
      continue
    }
    addMetricName(names, match[1])
  }
  return names
}

function addMetricName(names, name) {
  names.add(name)
  if (name.startsWith('aether-gateway_')) {
    names.add(name.slice('aether-gateway_'.length))
  }
}

function metricMax(text, metricName, labels = {}) {
  let max = null
  for (const line of text.split(/\r?\n/)) {
    if (!line || line.startsWith('#')) {
      continue
    }
    const parsed = parseMetricSample(line)
    if (!parsed) {
      continue
    }
    if (parsed.name !== metricName && parsed.name !== `aether-gateway_${metricName}`) {
      continue
    }
    if (!labelsMatch(parsed.labels, labels)) {
      continue
    }
    max = max === null ? parsed.value : Math.max(max, parsed.value)
  }
  return max
}

function parseMetricSample(line) {
  const match = line.match(/^([a-zA-Z_:][a-zA-Z0-9_:.-]*)(\{[^}]*\})?\s+(-?(?:\d+\.?\d*|\d*\.\d+)(?:[eE][+-]?\d+)?)\s*$/)
  if (!match) {
    return null
  }
  return {
    name: match[1],
    labels: parseLabels(match[2]),
    value: Number(match[3]),
  }
}

function parseLabels(labelText) {
  if (!labelText) {
    return {}
  }
  const labels = {}
  const inner = labelText.slice(1, -1)
  const pattern = /([a-zA-Z_][a-zA-Z0-9_]*)="((?:\\.|[^"\\])*)"/g
  let match
  while ((match = pattern.exec(inner)) !== null) {
    labels[match[1]] = match[2].replace(/\\"/g, '"').replace(/\\\\/g, '\\')
  }
  return labels
}

function labelsMatch(actual, expected) {
  return Object.entries(expected).every(([name, value]) => actual[name] === value)
}

main().catch((error) => {
  console.error(`FAIL ${error.message}`)
  process.exit(1)
})
