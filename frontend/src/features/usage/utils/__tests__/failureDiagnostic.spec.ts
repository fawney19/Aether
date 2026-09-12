import { describe, expect, it } from 'vitest'
import type { CandidateRecord } from '@/api/requestTrace'
import { buildFailureDiagnosticBundle, diagnosticPathFromMessage, visibleFailureRecords } from '../failureDiagnostic'

const candidate = (overrides: Partial<CandidateRecord> = {}): CandidateRecord => ({
  id: 'candidate-1', request_id: 'request-1', candidate_index: 0, retry_index: 0,
  status: 'failed', is_cached: false, created_at: '2026-09-08T00:00:00Z',
  error_type: 'local_sync_attempt_aborted', ...overrides,
})

const payload = {
  breakpoint: '$',
  request: { client_api_format: 'openai:chat', provider_api_format: 'claude:messages' },
}

describe('failure diagnostic paths', () => {
  it.each([
    ['invalid enum value "future" for claude:messages.content[2].type', '$.content[2].type'],
    ['invalid enum value "future" for openai:chat.choices[].finish_reason', '$.choices[*].finish_reason'],
    ['unaudited field metadata.private in openai:responses cannot be converted to claude:messages: no mapping', '$.metadata.private'],
    ['lossy conversion blocked from openai:chat to claude:messages at messages[1].content[0].type: cannot preserve', '$.messages[1].content[0].type'],
    ['Internal("invalid enum value \\"future\\" for claude:messages.content[2].type")', '$.content[2].type'],
    ['Local sync attempt failed before terminal finalization: Internal("invalid enum value \\"future\\" for claude:messages.content[2].type")', '$.content[2].type'],
    ['unsupported field tools[12].function.strict in claude:messages: unsupported', '$.tools[12].function.strict'],
    ['failed to parse claude:messages response', ''],
  ])('extracts the complete path from %s', (message, path) => {
    expect(diagnosticPathFromMessage(message)).toBe(path)
  })

  it('prefers structured diagnostics over legacy text and preserves the actual value', () => {
    const result = buildFailureDiagnosticBundle(payload, candidate({
      status: 'skipped',
      error_message: 'failed to parse claude:messages request',
      extra_data: { failure_diagnostic: {
        stage: 'request', source: 'request_converter', path: '$.tools[3].type',
        details: { code: 'invalid_enum_value', actual: 'future', path: '$.tools[3].type' },
      } },
    }))
    expect(result.schema_version).toBe(2)
    expect(result.diagnostic).toMatchObject({
      code: 'invalid_enum_value', stage: 'request', stage_source: 'structured',
      path: '$.tools[3].type', path_source: 'structured', actual: 'future',
      source_format: 'openai:chat', target_format: 'claude:messages', converter: 'request_converter',
    })
  })

  it('uses the reverse format direction for response conversion', () => {
    const result = buildFailureDiagnosticBundle(payload, candidate({ error_message: 'invalid enum value "future" for claude:messages.stop_reason' }))
    expect(result.diagnostic).toMatchObject({
      stage: 'response', path_source: 'message_inference', actual: 'future',
      source_format: 'claude:messages', target_format: 'openai:chat',
    })
  })

  it.each([
    ['claude:messages', '$.delta.stop_reason'],
    ['openai:chat', '$.choices[*].finish_reason'],
    ['gemini:generate_content', '$.candidates[*].finishReason'],
  ])('resolves canonical finish reasons to the %s source field', (providerFormat, path) => {
    const result = buildFailureDiagnosticBundle({
      breakpoint: '$.finish_reason',
      request: { client_api_format: 'claude:messages', provider_api_format: providerFormat },
    }, candidate({ error_type: 'stream_terminal_error', error_message: 'unsupported provider stream finish reason: future_reason' }))
    expect(result.diagnostic).toMatchObject({
      stage: 'stream', code: 'unsupported_finish_reason', actual: 'future_reason',
      reported_path: '$.finish_reason', path, path_source: 'protocol_inference',
    })
  })

  it('does not replace a structured finish reason path with a protocol guess', () => {
    const result = buildFailureDiagnosticBundle(payload, candidate({
      error_type: 'stream_terminal_error', error_message: 'unsupported provider stream finish reason: future_reason',
      extra_data: { failure_diagnostic: { stage: 'stream', path: '$.message.stop_reason' } },
    }))
    expect(result.diagnostic).toMatchObject({ path: '$.message.stop_reason', path_source: 'structured' })
  })

  it('keeps generic parse failures explicitly incomplete instead of inventing a path', () => {
    const result = buildFailureDiagnosticBundle(payload, candidate({ error_message: 'failed to parse claude:messages response' }))
    expect(result.diagnostic).toMatchObject({ code: 'response_parse_failed', path: '$', path_source: 'unavailable', missing_context: ['field_path'] })
    expect(result.reproduction).toMatchObject({ status: 'not_loaded' })
  })

  it('uses the selected error-flow message rather than a generic fallback', () => {
    const result = buildFailureDiagnosticBundle(payload, candidate({ error_message: 'execution runtime returned non-success status 500' }), null,
      'unaudited field metadata.private in claude:messages cannot be converted to openai:chat: no mapping')
    expect(result.diagnostic).toMatchObject({ code: 'unaudited_field', path: '$.metadata.private' })
  })

  it('does not expose hidden diagnostics through compatibility aliases', () => {
    expect(visibleFailureRecords({
      failure_diagnostic: { safe_to_show: false, message: 'private' },
      request_conversion_error: { message: 'private' },
      request_body_build_error: { message: 'private' },
    })).toEqual([])
  })
})
