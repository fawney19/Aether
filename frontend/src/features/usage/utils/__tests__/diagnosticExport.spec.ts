import { describe, expect, it, vi } from 'vitest'
import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'
import { diagnosticSamples, prepareDiagnosticExport, sanitizeDiagnostic } from '../diagnosticExport'

const candidate = (extra: Record<string, unknown> = {}): CandidateRecord => ({
  id: 'failed-candidate', request_id: 'request-1', candidate_index: 0, retry_index: 0,
  status: 'failed', is_cached: false, created_at: '2026-09-08T00:00:00Z', extra_data: extra,
})
const trace: RequestTrace = {
  request_id: 'request-1', final_status: 'failed', total_candidates: 1, total_latency_ms: 0, candidates: [],
  diagnostic_request: { usage_id: 'usage-1', body_state: 'reference' },
}
const bundle = (stage = 'response', path = '$.stop_reason', actual: unknown = 'future') => ({
  schema_version: 2, summary: 'conversion failed', request: { request_id: 'request-1' },
  diagnostic: { stage, path, actual, code: 'invalid_enum_value', missing_context: [] },
})
const context = { usage_id: 'usage-1', body_states: { request_body: 'reference', provider_request_body: 'disabled', response_body: 'reference' } }

describe('diagnostic export', () => {
  it('loads only the matched usage and preserves concrete field evidence', async () => {
    const loader = vi.fn(async (_usageId: string, field: string) => field === 'request_body'
      ? { messages: [{ role: 'user', content: 'private customer text' }] }
      : { stop_reason: 'future', authorization: 'opaque-secret', message: 'failed with opaque-secret' })
    const result = await prepareDiagnosticExport(bundle(), candidate({ diagnostic_context: context }), trace, new AbortController().signal, loader)
    expect(loader.mock.calls).toEqual([['usage-1', 'request_body', expect.any(AbortSignal)], ['usage-1', 'response_body', expect.any(AbortSignal)]])
    expect(result.reproduction).toMatchObject({ status: 'sanitized_context', replay_ready: false, sources: {
      response_body: { field_samples: [{ path: '$.stop_reason', value: { stop_reason: 'future' } }] },
    } })
    expect(JSON.stringify(result)).not.toContain('opaque-secret')
    expect(JSON.stringify(result)).not.toContain('private customer text')
  })

  it('never substitutes the final attempt response for a different failed candidate', async () => {
    const loader = vi.fn(async () => ({ messages: [] }))
    const result = await prepareDiagnosticExport(bundle(), candidate(), trace, new AbortController().signal, loader)
    expect(loader).toHaveBeenCalledTimes(1)
    expect(loader).toHaveBeenCalledWith('usage-1', 'request_body', expect.any(AbortSignal))
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context', missing_context: ['response_body'], sources: { response_body: { status: 'unavailable' } } })
  })

  it('does not follow body_ref URLs or fetch disabled bodies', async () => {
    const loader = vi.fn()
    const result = await prepareDiagnosticExport(bundle(), candidate({
      upstream_response: { body_ref: 'https://untrusted.example/private' },
      diagnostic_context: { usage_id: 'usage-1', body_states: { request_body: 'disabled', response_body: 'disabled', provider_request_body: 'disabled' } },
    }), null, new AbortController().signal, loader)
    expect(loader).not.toHaveBeenCalled()
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context', sources: { response_body: { status: 'disabled' } } })
  })

  it.each([[403, 'forbidden'], [404, 'missing'], [500, 'load_failed']])('exports an explicit gap for HTTP %s', async (status, expected) => {
    const loader = vi.fn().mockRejectedValue({ response: { status } })
    const result = await prepareDiagnosticExport(bundle(), candidate({ diagnostic_context: context }), trace, new AbortController().signal, loader)
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context', sources: { response_body: { status: expected } } })
  })

  it('keeps the failing SSE event, frame index and preceding context without leaking text', async () => {
    const raw = [
      ['message_start', { type: 'message_start', message: { id: 'msg-1' } }],
      ['content_block_delta', { type: 'content_block_delta', delta: { type: 'text_delta', text: 'confidential content' } }],
      ['message_delta', { type: 'message_delta', delta: { stop_reason: 'error' } }],
      ['message_stop', { type: 'message_stop' }],
    ].map(([event, payload]) => `event: ${event}\ndata: ${JSON.stringify(payload)}\n\n`).join('')
    const result = await prepareDiagnosticExport(bundle('stream', '$.delta.stop_reason', 'error'), candidate({ upstream_response: { body: { raw_response: raw } } }), null, new AbortController().signal, vi.fn())
    expect(result.reproduction).toMatchObject({ status: 'sanitized_context', sources: { response_body: { sample: { failure_frame_index: 2, selection: 'matched_failure', captured_frames: 4 } } } })
    expect(JSON.stringify(result)).toContain('message_start')
    expect(JSON.stringify(result)).not.toContain('confidential content')
  })

  it('marks a truncated stream without the failing event as incomplete', async () => {
    const result = await prepareDiagnosticExport(bundle('stream', '$.type', 'future.event'), candidate({ upstream_response: { body: { raw_response: 'event: message_start\ndata: {"type":"message_start"}\n\n' } } }), null, new AbortController().signal, vi.fn())
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context', missing_context: ['failed_stream_event'] })
  })

  it('cancels instead of copying another candidate after navigation', async () => {
    const controller = new AbortController()
    const loader = vi.fn(async () => { controller.abort(); return {} })
    await expect(prepareDiagnosticExport(bundle(), candidate({ diagnostic_context: context }), trace, controller.signal, loader)).rejects.toMatchObject({ name: 'AbortError' })
  })

  it('identifies the actual failing element beyond the initial excerpt', () => {
    const choices = Array.from({ length: 50 }, (_value, index) => ({ finish_reason: index === 40 ? 'future' : 'stop' }))
    expect(diagnosticSamples({ choices }, '$.choices[*].finish_reason', 'future')).toEqual([{ path: '$.choices[40].finish_reason', value: { finish_reason: 'future' } }])
  })

  it('redacts credentials, signed URLs, private text and error echoes', () => {
    const result = sanitizeDiagnostic({
      diagnostic: { path: '$.api_key', actual: 'plain-secret-value' },
      summary: 'invalid enum value "plain-secret-value" for api_key',
      raw: { headers: { Authorization: 'Bearer authorization-private-value', Cookie: 'session=abc', 'x-api-key': 'plain-secret-value', 'x-goog-api-key': 'google-private-key', 'x-auth-token': 'provider-private-token' },
        error: 'Bearer other-token https://user:password@example.com/test?token=abc admin@example.com',
        content: 'private customer text',
      },
    })
    const output = JSON.stringify(result)
    for (const privateValue of ['plain-secret-value', 'authorization-private-value', 'session=abc', 'other-token', 'password@', 'token=abc', 'admin@example.com', 'private customer text', 'google-private-key', 'provider-private-token']) expect(output).not.toContain(privateValue)
  })

  it('bounds the exported payload even for very large inline responses', async () => {
    const body = Object.fromEntries(Array.from({ length: 64 }, (_value, index) => [`field${index}`, 'value'.repeat(10000)]))
    const result = await prepareDiagnosticExport(bundle(), candidate({ upstream_response: { body } }), null, new AbortController().signal, vi.fn())
    expect(JSON.stringify(result, null, 2).length).toBeLessThanOrEqual(64 * 1024)
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context' })
  })

  it('never drops the failure frame when many earlier block starts match', async () => {
    const events = Array.from({ length: 30 }, () => ({ type: 'content_block_start', index: 0 }))
    const raw = [...events, { type: 'content_block_delta', index: 0, delta: { type: 'future_delta' } }]
      .map(payload => `event: ${payload.type}\ndata: ${JSON.stringify(payload)}\n\n`).join('')
    const result = await prepareDiagnosticExport(bundle('stream', '$.delta.type', 'future_delta'), candidate({ upstream_response: { body: { raw_response: raw } } }), null, new AbortController().signal, vi.fn())
    expect(JSON.stringify(result)).toContain('future_delta')
    expect(JSON.stringify(result)).toContain('"frame_index":30')
  })

  it.each(['too_large', 'decode_failed', 'timeout'])('retains the specific body failure code %s', async code => {
    const result = await prepareDiagnosticExport(bundle(), candidate({ diagnostic_context: context }), trace, new AbortController().signal, vi.fn().mockRejectedValue(new Error(code)))
    expect(result.reproduction).toMatchObject({ status: 'insufficient_context', sources: { response_body: { status: code } } })
  })
})
