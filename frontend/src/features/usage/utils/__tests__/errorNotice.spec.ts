import { describe, expect, it } from 'vitest'

import type { RequestDetail } from '@/api/dashboard'
import { resolveRequestFailureNotice } from '../errorNotice'

function buildRequestDetail(overrides: Partial<RequestDetail> = {}): RequestDetail {
  return {
    id: 'usage-1',
    request_id: 'req-1',
    user: {
      id: 'user-1',
      username: 'alice',
      email: 'alice@example.com',
    },
    api_key: {
      id: 'key-1',
      name: 'primary',
      display: 'primary',
    },
    provider: 'OpenAI',
    model: 'gpt-5',
    tokens: {
      input: 0,
      output: 0,
      total: 0,
    },
    cost: {
      input: 0,
      output: 0,
      total: 0,
    },
    request_type: 'chat',
    is_stream: true,
    status_code: 503,
    status: 'failed',
    response_time_ms: 0,
    created_at: '2026-05-14T10:33:21Z',
    ...overrides,
  }
}

describe('request failure notice', () => {
  it('prioritizes local scheduling failure details over generic 503 status', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      error_message: 'generic 503',
      failure_summary: {
        status_code: 503,
        message: '没有可用提供商支持模型 gpt-5 的流式请求',
      },
      scheduling_failure: {
        source: 'local_execution_runtime_miss',
        reason: 'all_candidates_skipped',
        reason_label: '所有候选均被跳过',
        title: '本地调度失败：所有候选均被跳过',
        message: '没有可用提供商支持模型 gpt-5 的流式请求',
        reason_summary: 'pool_account_exhausted 2 次',
        status_code: 503,
        no_upstream_attempt: true,
      },
    }))

    expect(notice).toEqual({
      title: '本地调度失败：所有候选均被跳过',
      message: '没有可用提供商支持模型 gpt-5 的流式请求',
      isSchedulingFailure: true,
      meta: [
        'pool_account_exhausted 2 次',
        '所有候选均被跳过',
        'all_candidates_skipped',
        'HTTP 503',
        '未进入上游执行',
      ],
    })
  })

  it('falls back to the failure summary for upstream failures', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      failure_summary: {
        source: 'upstream_response',
        status_code: 429,
        type: 'insufficient_quota',
        message: 'quota exceeded',
      },
    }))

    expect(notice).toEqual({
      title: '执行失败原因',
      message: 'quota exceeded',
      isSchedulingFailure: false,
      meta: ['HTTP 429', 'insufficient_quota', 'upstream_response'],
    })
  })

  it('prefers structured upstream failure details when scheduling failure includes them', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      scheduling_failure: {
        source: 'local_execution_runtime_miss',
        reason: 'execution_runtime_candidates_exhausted',
        reason_label: '候选执行失败且已耗尽',
        title: '唯一候选执行失败，已无可重试上游',
        message: 'gpustack 返回 HTTP 400',
        reason_summary: '候选 1 个',
        status_code: 503,
        no_upstream_attempt: false,
        upstream_failure: {
          provider_name: 'gpustack',
          endpoint_id: 'endpoint-1',
          key_name: 'key-1',
          model: 'qwen3.6-27b',
          status_code: 400,
          type: 'BadRequestError',
          param: 'input_tokens',
          message: 'This model\'s maximum context length is 131072 tokens.',
          user_message: '输入上下文超过模型 qwen3.6-27b 的最大长度限制。',
        },
        candidate_failure_summary: {
          total: 1,
          failed: 1,
          skipped: 0,
          retried: 1,
        },
      },
    }))

    expect(notice).toEqual({
      title: '唯一候选执行失败，已无可重试上游',
      message: '输入上下文超过模型 qwen3.6-27b 的最大长度限制。',
      isSchedulingFailure: true,
      meta: ['HTTP 400', 'BadRequestError', 'input_tokens', 'gpustack', 'qwen3.6-27b'],
    })
  })

  it('does not present HTTP 200 as the cause of stream terminal failures', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      status_code: 200,
      status: 'failed',
      error_message: 'This content was flagged for possible cybersecurity risk',
      failure_summary: {
        source: 'client_response',
        status_code: 200,
        type: 'stream_terminal_error',
        message: 'This content was flagged for possible cybersecurity risk',
      },
    }))

    expect(notice).toEqual({
      title: '执行失败原因',
      message: 'This content was flagged for possible cybersecurity risk',
      isSchedulingFailure: false,
      meta: ['stream_terminal_error', 'client_response'],
    })
  })

  it('does not show a stale notice when the refreshed detail has no error fields', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      status_code: 200,
      status: 'completed',
      error_message: undefined,
      scheduling_failure: null,
      failure_summary: null,
      client_error: null,
      upstream_error: null,
      request_error: null,
    }))

    expect(notice).toBeNull()
  })

  it('normalizes equivalent upstream timeout messages to Chinese wording', () => {
    const notice = resolveRequestFailureNotice(buildRequestDetail({
      status_code: 503,
      status: 'failed',
      error_message: 'UpstreamRequest("provider stream first byte timeout after 10000 ms")',
    }))

    expect(notice?.message).toBe('请求超时（10秒）')
    expect(notice?.meta).toEqual([])
  })
})
