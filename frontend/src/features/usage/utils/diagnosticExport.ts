import { dashboardApi, type RequestBodyField } from '@/api/dashboard'
import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'
import { decodeBody } from './body-document-engine'
import { diagnosticObject } from './failureDiagnostic'

const MAX_SOURCE_BYTES = 1024 * 1024
const MAX_EXPORT_CHARS = 64 * 1024
const SECRET_KEY = /^(?:authorization|proxy[-_]authorization|cookie|set[-_]cookie|(?:x[-_])?api[-_]?key|x[-_]goog[-_]api[-_]key|(?:openai|anthropic)[-_]api[-_]?key|(?:x[-_])?auth[-_]token|api[-_]secret|secret[-_]key|session(?:[-_]id)?|key|token|access[-_]token|refresh[-_]token|id[-_]token|client[-_]secret|secret|password|passwd|credential|credentials|private[-_]key)$/i
const CONTENT_KEY = /^(?:text|content|prompt|system|instructions|input|arguments|partial_json|thinking|signature|data|image|url|image_url|audio|video|raw_response|body|description|refusal)$/i

function redactString(value: string): string {
  return value
    .replace(/\b(?:Bearer|Basic)\s+[A-Za-z0-9_+./=:-]+/gi, '[REDACTED_AUTH]')
    .replace(/\b(?:sk-[A-Za-z0-9_-]{6,}|AIza[A-Za-z0-9_-]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)/g, '[REDACTED_TOKEN]')
    .replace(/\b(api[_-]?key|access_token|refresh_token|password|secret)\s*[=:]\s*[^\s&,;"']+/gi, '$1=[REDACTED]')
    .replace(/https?:\/\/[^\s"<>]+/gi, value => {
      try {
        const url = new URL(value)
        url.username = ''
        url.password = ''
        if (url.search) url.search = '?REDACTED'
        url.hash = ''
        return url.toString()
      } catch { return '[REDACTED_URL]' }
    })
    .replace(/[\w.+-]+@[\w.-]+\.[a-z]{2,}/gi, '[REDACTED_EMAIL]')
    .replace(/data:[^;\s]+;base64,[A-Za-z0-9+/=]+/gi, '[REDACTED_BINARY]')
}

export function sanitizeDiagnostic(value: unknown): unknown {
  const secrets = new Set<string>()
  let scanned = 2500
  const collect = (input: unknown, key = '', depth = 0) => {
    if (--scanned < 0 || depth > 16) return
    if (SECRET_KEY.test(key) && typeof input === 'string') secrets.add(input.slice(0, 4096))
    if (Array.isArray(input)) input.slice(0, 64).forEach(child => collect(child, key, depth + 1))
    else if (input && typeof input === 'object') Object.entries(input).slice(0, 64).forEach(([name, child]) => collect(child, name, depth + 1))
  }
  collect(value)
  const originalDiagnostic = diagnosticObject(diagnosticObject(value).diagnostic)
  if (typeof originalDiagnostic.path === 'string' && originalDiagnostic.path.split(/[.[\]]/).some(part => SECRET_KEY.test(part))) {
    if (typeof originalDiagnostic.actual === 'string') secrets.add(originalDiagnostic.actual)
  }
  let remaining = 1200
  const visit = (input: unknown, key = '', depth = 0): unknown => {
    if (--remaining < 0 || depth > 16) return '[TRUNCATED]'
    if (SECRET_KEY.test(key)) return '[REDACTED]'
    if (typeof input === 'string') {
      if (CONTENT_KEY.test(key)) return `[REDACTED_TEXT length=${input.length}]`
      let redacted = input.slice(0, 2048)
      for (const secret of secrets) {
        if (secret.length >= 4) {
          const prefix = secret.slice(0, 32)
          for (let count = 0; count < 16; count++) {
            const index = redacted.indexOf(prefix)
            if (index < 0) break
            redacted = `${redacted.slice(0, index)}[REDACTED]${redacted.slice(index + secret.length)}`
          }
        }
        else if (secret && redacted === secret) redacted = '[REDACTED]'
      }
      redacted = redactString(redacted)
      return input.length > 2048 ? `${redacted}[TRUNCATED]` : redacted
    }
    if (Array.isArray(input)) {
      const output = input.slice(0, 32).map(item => visit(item, key, depth + 1))
      if (input.length > 32) output.push(`[TRUNCATED ${input.length - 32} items]`)
      return output
    }
    if (input && typeof input === 'object') {
      const entries = Object.entries(input).slice(0, 64)
      const output: Record<string, unknown> = {}
      for (const [name, child] of entries) {
        if (['__proto__', 'constructor', 'prototype'].includes(name)) continue
        output[redactString(name.slice(0, 128))] = visit(child, name, depth + 1)
      }
      if (Object.keys(input).length > 64) output.__truncated__ = true
      return output
    }
    return input
  }
  const sanitized = visit(value)
  const safe = diagnosticObject(sanitized)
  const diagnostic = diagnosticObject(safe.diagnostic)
  if (typeof diagnostic.path === 'string' && diagnostic.path.split(/[.[\]]/).some(part => SECRET_KEY.test(part))) {
    diagnostic.actual = '[REDACTED]'
  }
  return sanitized
}

export function diagnosticSamples(body: unknown, path: string, actual?: unknown): Array<{ path: string, value: unknown }> {
  if (!/^\$(?:\.[\w:-]+|\[(?:\d+|\*)\])+$/.test(path)) return []
  const parts = path.slice(1).replace(/\[(\d+|\*)\]/g, '.$1').split('.').filter(Boolean)
  let matches: Array<{ path: string, value: unknown }> = [{ path: '$', value: body }]
  for (const part of parts) {
    matches = matches.flatMap(match => {
      if (part === '*' && Array.isArray(match.value)) {
        return match.value.slice(0, 64).map((value, index) => ({ path: `${match.path}[${index}]`, value }))
      }
      if (match.value === null || typeof match.value !== 'object' || !Object.prototype.hasOwnProperty.call(match.value, part)) return []
      const value = (match.value as Record<string, unknown>)[part]
      return [{ path: Array.isArray(match.value) ? `${match.path}[${part}]` : `${match.path}.${part}`, value }]
    }).slice(0, 64)
  }
  return matches.filter(match => actual == null || Object.is(match.value, actual)).slice(0, 8).map(match => ({
    path: match.path,
    value: match.path.split(/[.[\]]/).some(part => SECRET_KEY.test(part)) ? '[REDACTED]' : sanitizeDiagnostic({ [parts[parts.length - 1] ?? 'value']: match.value }),
  }))
}

function streamEvidence(body: unknown, path: string, actual: unknown): Record<string, unknown> | null {
  const object = diagnosticObject(body)
  const raw = typeof body === 'string' ? body : object.raw_response ?? object.sse
  if (typeof raw !== 'string' || !/(?:^|\n)(?:event|data):/.test(raw)) return null
  const events = raw.slice(0, MAX_SOURCE_BYTES).split(/\r?\n\r?\n/).flatMap((block, index) => {
    const lines = block.split(/\r?\n/)
    const data = lines.filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n')
    if (!data || data === '[DONE]') return []
    let payload: unknown
    try { payload = JSON.parse(data) } catch { payload = { unparsed: true, bytes: data.length } }
    return [{ frame_index: index, event: lines.find(line => line.startsWith('event:'))?.slice(6).trim() ?? diagnosticObject(payload).type ?? null, payload }]
  })
  const targetIndex = events.findIndex(event => {
    const payload = diagnosticObject(event.payload)
    if (actual !== null && actual !== undefined) {
      const samples = diagnosticSamples(payload, path, actual)
      if (samples.some(sample => Object.values(diagnosticObject(sample.value)).some(value => value === actual))) return true
    }
    return event.event === 'error' || event.event === 'response.failed' || Boolean(payload.error)
  })
  const center = targetIndex >= 0 ? targetIndex : Math.max(0, events.length - 1)
  const selected = new Set([0])
  for (let index = Math.max(0, center - 3); index <= Math.min(events.length - 1, center + 1); index++) selected.add(index)
  const target = diagnosticObject(events[center]?.payload)
  for (let index = center - 1; index >= 0; index--) {
    const event = events[index]
    if (event.event === 'content_block_start' && diagnosticObject(event.payload).index === target.index) {
      selected.add(index)
      break
    }
  }
  return {
    captured_frames: events.length,
    source_truncated: raw.length > MAX_SOURCE_BYTES,
    failure_frame_index: targetIndex >= 0 ? events[targetIndex]?.frame_index : null,
    selection: targetIndex >= 0 ? 'matched_failure' : 'unmatched_tail',
    windowed: selected.size < events.length,
    events: [...selected].sort((left, right) => left - right).slice(0, 12).map(index => events[index]).filter(Boolean),
  }
}

type BodyLoader = (usageId: string, field: RequestBodyField, signal: AbortSignal) => Promise<unknown>

async function loadBody(usageId: string, field: RequestBodyField, signal: AbortSignal): Promise<unknown> {
  const controller = new AbortController()
  let tooLarge = false
  const abort = () => controller.abort()
  signal.addEventListener('abort', abort, { once: true })
  const timer = setTimeout(abort, 5000)
  try {
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
    const response = await dashboardApi.getRequestBody(usageId, field, controller.signal, loaded => {
      if (loaded > MAX_SOURCE_BYTES) {
        tooLarge = true
        controller.abort()
      }
    })
    if (response.bytes.byteLength > MAX_SOURCE_BYTES) throw new Error('too_large')
    return (await decodeBody(response.bytes, response.encoding, MAX_SOURCE_BYTES)).value
  } catch (error) {
    if (tooLarge) throw new Error('too_large')
    if (controller.signal.aborted && !signal.aborted) throw new Error('timeout')
    throw error
  } finally {
    clearTimeout(timer)
    signal.removeEventListener('abort', abort)
  }
}

export async function prepareDiagnosticExport(
  bundle: Record<string, unknown>,
  attempt: CandidateRecord,
  trace: RequestTrace | null,
  signal: AbortSignal,
  loader: BodyLoader = loadBody,
): Promise<Record<string, unknown>> {
  const diagnostic = diagnosticObject(bundle.diagnostic)
  const stage = diagnostic.stage
  const context = diagnosticObject(attempt.extra_data?.diagnostic_context)
  const states = diagnosticObject(context.body_states)
  const primary = stage === 'request' ? 'request_body' : 'response_body'
  const fields: RequestBodyField[] = stage === 'request' ? ['request_body', 'provider_request_body'] : ['request_body', 'provider_request_body', 'response_body']
  const sources: Record<string, unknown> = {}
  const missing = Array.isArray(diagnostic.missing_context) ? [...diagnostic.missing_context] : []
  for (const field of fields) {
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
    const usageId = field === 'request_body' ? trace?.diagnostic_request?.usage_id ?? context.usage_id : context.usage_id
    const state = field === 'request_body' ? trace?.diagnostic_request?.body_state ?? states[field] : states[field]
    const inline = field === 'response_body' ? diagnosticObject(attempt.extra_data?.upstream_response).body : undefined
    if (inline === undefined && (typeof usageId !== 'string' || !usageId || ['disabled', 'none'].includes(String(state)))) {
      sources[field] = { status: state === 'disabled' ? 'disabled' : 'unavailable', candidate_matched: Boolean(context.usage_id) }
      if (field === primary) missing.push(field)
      continue
    }
    try {
      const body = inline !== undefined ? inline : await loader(usageId as string, field, signal)
      if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
      const sse = field === 'response_body' && stage === 'stream' ? streamEvidence(body, String(diagnostic.path ?? '$'), diagnostic.actual) : null
      const samples = field === primary ? diagnosticSamples(body, String(diagnostic.path ?? '$'), diagnostic.actual) : []
      sources[field] = {
        status: 'captured',
        origin: inline !== undefined ? 'candidate_inline' : 'stored_body',
        usage_id: usageId ?? null,
        sample: sse ?? body,
        field_samples: samples,
      }
      if (field === primary && stage === 'stream' && (!sse || sse.failure_frame_index === null)) missing.push('failed_stream_event')
      if (sse?.source_truncated) missing.push('body_size_limit')
      if (field === primary && state === 'truncated') missing.push('body_capture_truncated')
      if (field === primary && stage !== 'stream' && diagnostic.path !== '$' && samples.length === 0) missing.push('field_not_found_in_source')
    } catch (error) {
      if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
      const response = diagnosticObject(diagnosticObject(error).response)
      const status = response.status
      const cause = diagnosticObject(response.headers)['x-aether-body-error'] ?? diagnosticObject(error).code ?? (error instanceof Error ? error.message : '')
      const code = status === 403 ? 'forbidden' : status === 404 ? 'missing' : ['too_large', 'decode_failed', 'timeout', 'missing', 'storage_unavailable'].includes(String(cause)) ? String(cause) : 'load_failed'
      sources[field] = { status: code }
      if (field === primary) missing.push(field)
    }
  }
  const result = sanitizeDiagnostic({
    ...bundle,
    reproduction: {
      status: missing.length ? 'insufficient_context' : 'sanitized_context',
      replay_ready: false,
      missing_context: [...new Set(missing)],
      redaction: 'Credentials, private text, URLs and binary data are removed; samples may be truncated. Review before sharing.',
      source_limit_bytes: MAX_SOURCE_BYTES,
      sources,
    },
  }) as Record<string, unknown>
  if (JSON.stringify(result, null, 2).length > MAX_EXPORT_CHARS) {
    const reproduction = diagnosticObject(result.reproduction)
    reproduction.status = 'insufficient_context'
    reproduction.missing_context = [...new Set([...missing, 'export_size_limit'])]
    for (const source of Object.values(diagnosticObject(reproduction.sources))) {
      const object = diagnosticObject(source)
      object.sample = '[TRUNCATED_EXPORT_SIZE_LIMIT]'
      object.field_samples = '[TRUNCATED_EXPORT_SIZE_LIMIT]'
    }
    result.raw = '[TRUNCATED_EXPORT_SIZE_LIMIT]'
    if (JSON.stringify(result, null, 2).length > MAX_EXPORT_CHARS) {
      return {
        schema_version: 2,
        summary: '[TRUNCATED_EXPORT_SIZE_LIMIT]',
        diagnostic: { code: diagnosticObject(result.diagnostic).code, stage: diagnosticObject(result.diagnostic).stage, path: diagnosticObject(result.diagnostic).path },
        reproduction: { status: 'insufficient_context', missing_context: ['export_size_limit'] },
      }
    }
  }
  return result
}
