import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'

type JsonObject = Record<string, unknown>

export const diagnosticObject = (value: unknown): JsonObject =>
  value !== null && typeof value === 'object' && !Array.isArray(value) ? value as JsonObject : {}

const text = (value: unknown): string => typeof value === 'string' ? value.trim() : ''

export function unwrapDiagnosticMessage(message: string): string {
  let result = message.trim()
  for (let depth = 0; depth < 4; depth++) {
    const wrapped = result.match(/^(?:local sync attempt failed before terminal finalization:\s*)?Internal\("((?:\\.|[^"\\])*)"\)$/i)
    if (!wrapped) break
    try {
      result = JSON.parse(`"${wrapped[1]}"`) as string
    } catch {
      break
    }
  }
  return result
}

export function normalizeFailurePath(field: string): string {
  const normalized = field.trim().replace(/\[\]/g, '[*]')
  return !normalized || normalized === '$' ? '$' : normalized.startsWith('$') ? normalized : `$.${normalized}`
}

export function diagnosticPathFromMessage(message: string): string {
  const normalized = unwrapDiagnosticMessage(message)
  const patterns = [
    /\bfield\s+([^;=]+?)\s*(?:=|is unsupported|不支持)/i,
    /lossy conversion blocked from\s+\S+\s+to\s+\S+\s+at\s+([^:]+):/i,
    /(?:unsupported|unaudited) field\s+(.+?)\s+in\s+/i,
    /invalid target field\s+(.+?)\s+for\s+/i,
    /invalid enum value\s+.+?\s+for\s+[\w:-]+\.(.+)$/i,
  ]
  for (const pattern of patterns) {
    const match = normalized.match(pattern)
    if (match?.[1]) return normalizeFailurePath(match[1])
  }
  return ''
}

export function visibleFailureRecords(extra: JsonObject): JsonObject[] {
  if (diagnosticObject(extra.failure_diagnostic).safe_to_show === false) return []
  return ['failure_diagnostic', 'request_conversion_error', 'request_body_build_error']
    .map(key => diagnosticObject(extra[key]))
    .filter(record => Object.keys(record).length > 0 && record.safe_to_show !== false)
}

export function buildFailureDiagnosticBundle(
  payload: JsonObject,
  attempt: CandidateRecord,
  trace?: RequestTrace | null,
  rawMessage = '',
): JsonObject {
  const extra = diagnosticObject(attempt.extra_data)
  const structured = visibleFailureRecords(extra)[0] ?? {}
  const details = diagnosticObject(structured.details)
  const request = diagnosticObject(payload.request)
  const diagnosticContext = diagnosticObject(extra.diagnostic_context)
  const message = unwrapDiagnosticMessage(rawMessage || text(structured.message) || text(attempt.error_message) || text(payload.summary))
  const clientFormat = text(request.client_api_format) || text(structured.client_api_format)
  const providerFormat = text(request.provider_api_format) || text(structured.provider_api_format) || text(attempt.endpoint_name)
  let stage = text(structured.stage)
  if (!['request', 'response', 'stream'].includes(stage)) stage = ''
  const stageSource = stage ? 'structured' : 'inferred'
  if (!stage) {
    if (/stream/i.test(attempt.error_type ?? '') || /provider stream|stream ended/i.test(message)) stage = 'stream'
    else if (attempt.status === 'skipped' || /request|body_rules|header_rules/.test(text(structured.kind)) || /failed to (?:parse|emit) .+ request/i.test(message)) stage = 'request'
    else if (/local_sync|finaliz|response/i.test(attempt.error_type ?? '') || /failed to (?:parse|emit) .+ response/i.test(message)) stage = 'response'
    else stage = 'unknown'
  }
  const conversionPair = message.match(/(?:lossy conversion blocked from|unaudited field .+? in)\s+(\S+)\s+(?:to|cannot be converted to)\s+([^\s:]+:[^\s:]+(?:[:][^\s:]+)?)/i)
  if (stage === 'unknown' && conversionPair && clientFormat !== providerFormat) {
    if (conversionPair[1] === clientFormat && conversionPair[2] === providerFormat) stage = 'request'
    else if (conversionPair[1] === providerFormat && conversionPair[2] === clientFormat) stage = 'response'
  }
  let code = text(details.code) || text(structured.code)
  if (!code) {
    const patterns: Array<[RegExp, string]> = [
      [/(?:unsupported provider stream finish reason|upstream stream ended with finish reason):\s*error\s*$/i, 'stream_terminal_error'],
      [/unsupported provider stream finish reason/i, 'unsupported_finish_reason'],
      [/unsupported provider stream event/i, 'unsupported_stream_event'],
      [/lossy conversion blocked/i, 'lossy_conversion_blocked'],
      [/unaudited field/i, 'unaudited_field'],
      [/invalid enum value/i, 'invalid_enum_value'],
      [/invalid target field/i, 'invalid_target_field'],
      [/unsupported field/i, 'unsupported_field'],
      [/failed to parse .+ request/i, 'request_parse_failed'],
      [/failed to emit .+ request/i, 'request_emit_failed'],
      [/failed to parse .+ response/i, 'response_parse_failed'],
      [/failed to emit .+ response/i, 'response_emit_failed'],
    ]
    code = patterns.find(([pattern]) => pattern.test(message))?.[1] || attempt.error_type || text(structured.kind) || 'unknown_failure'
  }
  const structuredPath = text(details.path) || text(structured.path)
  const parsedPath = diagnosticPathFromMessage(message)
  const reportedPath = normalizeFailurePath(structuredPath && structuredPath !== '$' ? structuredPath : parsedPath || text(payload.breakpoint))
  const streamFinishPaths: Record<string, string> = {
    'claude:messages': '$.delta.stop_reason',
    'openai:chat': '$.choices[*].finish_reason',
    'gemini:generate_content': '$.candidates[*].finishReason',
  }
  const protocolPath = stage === 'stream' && /finish reason/i.test(message)
    && (!structuredPath || structuredPath === '$') && ['$', '$.finish_reason'].includes(reportedPath)
    ? streamFinishPaths[providerFormat] : undefined
  const path = protocolPath ?? reportedPath
  const missing = Array.isArray(details.missing_context) ? [...details.missing_context] : []
  if (path === '$' && !missing.includes('field_path')) missing.push('field_path')
  if (stage === 'unknown') missing.push('failure_stage')
  if (!clientFormat || !providerFormat) missing.push('api_formats')
  let actual: unknown = details.actual ?? null
  if (actual === null) {
    const enumMatch = message.match(/invalid enum value\s+(.+?)\s+for\s+/i)
    const fieldMatch = message.match(/\bfield\s+[^;=]+?\s*=\s*([^;]+)(?:;|$)/i)
    const finishMatch = message.match(/(?:unsupported provider stream finish reason|upstream stream ended with finish reason):\s*(.+?)\s*$/i)
    const literal = enumMatch?.[1] ?? fieldMatch?.[1] ?? finishMatch?.[1]
    if (literal) {
      try { actual = JSON.parse(literal) } catch { actual = literal }
    }
    if (code === 'stream_terminal_error' && /finish reason/.test(message)) actual = 'error'
  }
  return {
    ...payload,
    schema_version: 2,
    breakpoint: path,
    diagnostic: {
      code,
      stage,
      stage_source: stageSource,
      operation: details.operation ?? null,
      reported_format: details.format ?? null,
      reported_path: reportedPath,
      path,
      path_source: path === '$' ? 'unavailable' : structuredPath && structuredPath !== '$' ? 'structured' : protocolPath ? 'protocol_inference' : 'message_inference',
      source_format: details.source_format ?? structured.source_format ?? conversionPair?.[1] ?? (stage === 'request' ? clientFormat : stage !== 'unknown' ? providerFormat : null),
      target_format: details.target_format ?? structured.target_format ?? conversionPair?.[2] ?? (stage === 'request' ? providerFormat : stage !== 'unknown' ? clientFormat : null),
      converter: structured.source ?? null,
      expected: details.expected ?? details.reason ?? null,
      actual,
      missing_context: missing,
    },
    versions: {
      frontend: typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : null,
      gateway_at_export: trace?.gateway_version ?? null,
      runtime_at_failure: structured.runtime_version ?? null,
    },
    node: {
      ...diagnosticObject(payload.node),
      candidate_id: attempt.id,
      provider_id: attempt.provider_id ?? null,
      endpoint_id: attempt.endpoint_id ?? null,
      key_id: attempt.key_id ?? null,
    },
    request: {
      ...request,
      client_api_format: clientFormat || null,
      provider_api_format: providerFormat || null,
      model: diagnosticContext.model ?? extra.model ?? null,
      target_model: diagnosticContext.target_model ?? extra.target_model ?? null,
      created_at: attempt.created_at,
      started_at: attempt.started_at ?? null,
    },
    reproduction: { status: 'not_loaded', sources: {}, missing_context: [...missing, 'source_body'] },
  }
}
