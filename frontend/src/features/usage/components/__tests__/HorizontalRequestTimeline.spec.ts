import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'
import HorizontalRequestTimeline from '../HorizontalRequestTimeline.vue'

const requestTraceApiMock = vi.hoisted(() => ({
  getRequestTrace: vi.fn(),
}))

vi.mock('@/api/requestTrace', () => ({
  requestTraceApi: requestTraceApiMock,
}))

vi.mock('@/components/ui/card.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'CardStub',
      setup(_, { slots }) {
        return () => h('section', slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/badge.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'BadgeStub',
      setup(_, { slots }) {
        return () => h('span', slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/skeleton.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'SkeletonStub',
      setup() {
        return () => h('div')
      },
    }),
  }
})

vi.mock('../JsonContentPanel.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'JsonContentPanelStub',
      props: {
        data: {
          type: null,
          default: null,
        },
        title: {
          type: String,
          default: 'JSON',
        },
      },
      setup(props) {
        return () => h('pre', { 'data-title': props.title }, JSON.stringify(props.data))
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    ChevronLeft: Icon,
    ChevronRight: Icon,
    ExternalLink: Icon,
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []
type TimelineExpose = { refresh: () => Promise<void> | undefined }

function buildCandidate(overrides: Partial<CandidateRecord> = {}): CandidateRecord {
  return {
    id: 'cand-1',
    request_id: 'req-1',
    candidate_index: 0,
    retry_index: 0,
    provider_id: 'provider-1',
    provider_name: 'Provider 1',
    key_id: 'key-1',
    key_name: 'Key 1',
    status: 'failed',
    is_cached: false,
    created_at: '2026-05-06T12:00:00.000Z',
    started_at: '2026-05-06T12:00:00.000Z',
    finished_at: '2026-05-06T12:00:01.000Z',
    ...overrides,
  }
}

function buildTrace(candidates: CandidateRecord[]): RequestTrace {
  return {
    request_id: 'req-1',
    total_candidates: candidates.length,
    final_status: 'success',
    total_latency_ms: 1000,
    candidates,
  }
}

async function flushPendingUpdates() {
  await Promise.resolve()
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

function mountTimeline(
  traceData: RequestTrace,
  extraProps: Record<string, unknown> = {},
) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(HorizontalRequestTimeline, {
    requestId: traceData.request_id,
    traceData,
    ...extraProps,
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

function mountTimelineFromApi(
  requestId: string,
  extraProps: Record<string, unknown> = {},
) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  let timeline: TimelineExpose | null = null
  const Host = defineComponent({
    setup() {
      return () => h(HorizontalRequestTimeline, {
        ref: (value: unknown) => {
          timeline = value as TimelineExpose | null
        },
        requestId,
        ...extraProps,
      })
    },
  })
  const app = createApp(Host)
  app.mount(root)
  mountedApps.push({ app, root })
  return {
    root,
    refresh: async () => {
      await timeline?.refresh()
      await flushPendingUpdates()
    },
  }
}

afterEach(() => {
  requestTraceApiMock.getRequestTrace.mockReset()
  vi.useRealTimers()
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('HorizontalRequestTimeline', () => {
  it('keeps attempted keys visible for ordinary provider groups that are not selected', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'provider-a-key-1',
        provider_id: 'provider-a',
        provider_name: 'Provider A',
        key_id: 'key-a-1',
        key_name: 'Key A1',
        candidate_index: 0,
        status: 'failed',
      }),
      buildCandidate({
        id: 'provider-a-key-2',
        provider_id: 'provider-a',
        provider_name: 'Provider A',
        key_id: 'key-a-2',
        key_name: 'Key A2',
        candidate_index: 1,
        status: 'failed',
      }),
      buildCandidate({
        id: 'provider-b-key-1',
        provider_id: 'provider-b',
        provider_name: 'Provider B',
        key_id: 'key-b-1',
        key_name: 'Key B1',
        candidate_index: 2,
        status: 'failed',
      }),
      buildCandidate({
        id: 'provider-b-key-2',
        provider_id: 'provider-b',
        provider_name: 'Provider B',
        key_id: 'key-b-2',
        key_name: 'Key B2',
        candidate_index: 3,
        status: 'success',
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const subDots = [...root.querySelectorAll<HTMLButtonElement>('.sub-dot')]
    expect(subDots).toHaveLength(2)
    expect(subDots.map(dot => dot.getAttribute('title'))).toEqual([
      '#1 · Key A2 · 失败',
      '#3 · Key B2 · 成功',
    ])
  })

  it('orders visible candidates by scheduling index and includes unattempted candidates', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-success',
        provider_id: 'provider-success',
        provider_name: 'Provider Success',
        key_id: 'key-success',
        key_name: 'Success Key',
        candidate_index: 4,
        status: 'success',
        started_at: '2026-05-06T12:00:04.000Z',
        finished_at: '2026-05-06T12:00:05.000Z',
      }),
      buildCandidate({
        id: 'cand-available',
        provider_id: 'provider-available',
        provider_name: 'Provider Available',
        key_id: 'key-available',
        key_name: 'Available Key',
        candidate_index: 0,
        status: 'available',
        started_at: undefined,
        finished_at: undefined,
      }),
      buildCandidate({
        id: 'cand-skipped',
        provider_id: 'provider-skipped',
        provider_name: 'Provider Skipped',
        key_id: 'key-skipped',
        key_name: 'Skipped Key',
        candidate_index: 1,
        status: 'skipped',
        started_at: undefined,
        finished_at: undefined,
      }),
      buildCandidate({
        id: 'cand-pending-unstarted',
        provider_id: 'provider-pending',
        provider_name: 'Provider Pending',
        key_id: 'key-pending',
        key_name: 'Pending Key',
        candidate_index: 2,
        status: 'pending',
        started_at: undefined,
        finished_at: undefined,
      }),
      buildCandidate({
        id: 'cand-failed',
        provider_id: 'provider-failed',
        provider_name: 'Provider Failed',
        key_id: 'key-failed',
        key_name: 'Failed Key',
        candidate_index: 3,
        status: 'failed',
        started_at: '2026-05-06T12:00:03.000Z',
        finished_at: '2026-05-06T12:00:04.000Z',
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const labels = [...root.querySelectorAll<HTMLElement>('.node-label')]
      .map(label => label.textContent?.trim())
    expect(labels).toEqual([
      'Provider Available',
      'Provider Skipped',
      'Provider Pending',
      'Provider Failed',
      'Provider Success',
    ])

    const nodeDots = [...root.querySelectorAll<HTMLElement>('.node-dot')]
    expect(nodeDots[0].classList.contains('status-available')).toBe(true)
    expect(nodeDots[2].classList.contains('status-pending')).toBe(true)
  })

  it('keeps successful runtime pool key visible when only pool_key_index is recorded', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'pool-skipped',
        provider_id: 'provider-pool',
        provider_name: 'CodexFree2',
        key_id: 'pool-group',
        key_name: 'CodexFree2',
        candidate_index: 0,
        status: 'skipped',
        started_at: undefined,
        finished_at: undefined,
        extra_data: { pool_group_id: 'provider-pool' },
      }),
      buildCandidate({
        id: 'pool-success',
        provider_id: 'provider-pool',
        provider_name: 'CodexFree2',
        key_id: 'key-success',
        key_name: 'Success Key',
        candidate_index: 1,
        status: 'success',
        extra_data: { pool_key_index: 0 },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const labels = [...root.querySelectorAll<HTMLElement>('.node-label')]
      .map(label => label.textContent?.trim())
    expect(labels).toEqual(['CodexFree2'])
    expect(root.querySelector<HTMLElement>('.node-dot')?.classList.contains('status-success'))
      .toBe(true)
    expect([...root.querySelectorAll<HTMLButtonElement>('.sub-dot')]
      .map(dot => dot.getAttribute('title'))).toEqual([
      '#1 · Success Key · 成功',
    ])
  })

  it('uses candidate terminal status for node colors instead of overriding with HTTP code', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-body-error',
        provider_id: 'provider-body-error',
        provider_name: 'Provider Body Error',
        key_id: 'key-body-error',
        key_name: 'Body Error Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 200,
      }),
      buildCandidate({
        id: 'cand-success',
        provider_id: 'provider-success',
        provider_name: 'Provider Success',
        key_id: 'key-success',
        key_name: 'Success Key',
        candidate_index: 1,
        status: 'success',
        status_code: 200,
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const nodeDots = [...root.querySelectorAll<HTMLElement>('.node-dot')]
    expect(nodeDots[0].classList.contains('status-failed')).toBe(true)
    expect(nodeDots[0].classList.contains('status-success')).toBe(false)
    expect(nodeDots[1].classList.contains('status-success')).toBe(true)
  })

  it('renders Codex image progress from candidate image_progress', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-image-progress',
        provider_id: 'provider-image',
        provider_name: 'Codex Image',
        key_id: 'key-image',
        key_name: 'Image Key',
        candidate_index: 0,
        status: 'streaming',
        finished_at: undefined,
        image_progress: {
          phase: 'upstream_streaming',
          upstream_ttfb_ms: 3807,
          upstream_sse_frame_count: 12,
          partial_image_count: 1,
          last_upstream_event: 'response.output_item.added',
          last_upstream_frame_at_unix_ms: Date.now(),
          last_client_visible_event: 'image_generation.partial_image',
          downstream_heartbeat_count: 3,
          downstream_heartbeat_interval_ms: 15000,
          last_downstream_heartbeat_at_unix_ms: Date.now(),
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('图片生成进度')
    expect(root.textContent).toContain('上游生成中')
    expect(root.textContent).toContain('3.81s')
    expect(root.textContent).toContain('下游心跳')
    expect(root.textContent).toContain('15.00s')
    expect(root.textContent).toContain('response.output_item.added')
    expect(root.textContent).toContain('image_generation.partial_image')
  })

  it('treats 3xx terminal responses as failed for node display', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-redirect',
        provider_id: 'provider-redirect',
        provider_name: 'Provider Redirect',
        key_id: 'key-redirect',
        key_name: 'Redirect Key',
        candidate_index: 0,
        status: 'success',
        status_code: 302,
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const nodeDot = root.querySelector<HTMLElement>('.node-dot')
    expect(nodeDot?.classList.contains('status-failed')).toBe(true)
    expect(nodeDot?.classList.contains('status-success')).toBe(false)
  })

  it('keeps emitted trace state active while the request lifecycle is still streaming', async () => {
    const onTraceState = vi.fn()
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-stale-failed',
        provider_id: 'provider-stale',
        provider_name: 'Provider Stale',
        key_id: 'key-stale',
        key_name: 'Stale Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 503,
      }),
    ])
    trace.final_status = 'failed'

    mountTimeline(trace, {
      requestStatus: 'streaming',
      overrideStatusCode: 200,
      onTraceState,
    })
    await nextTick()

    const lastCall = onTraceState.mock.calls.at(-1)?.[0]
    expect(lastCall).toMatchObject({
      finalStatus: 'streaming',
    })
  })

  it('shows request path from request metadata', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-path',
        provider_id: 'provider-path',
        provider_name: 'Provider Path',
        key_id: 'key-path',
        key_name: 'Path Key',
        candidate_index: 0,
        status: 'failed',
      }),
    ])

    const root = mountTimeline(trace, {
      requestMetadata: {
        request_path: '/v1beta/models/gemini-2.5-pro:generateContent',
        request_query_string: 'alt=sse',
      },
    })
    await nextTick()

    expect(root.textContent).toContain('请求路径')
    const requestPathCode = root.querySelector<HTMLElement>('.request-path-code')
    expect(requestPathCode?.textContent).toContain('/v1beta/models/gemini-2.5-pro:generateContent?alt=sse')
  })

  it('shows request path from trace payload', async () => {
    const trace: RequestTrace = {
      ...buildTrace([
        buildCandidate({
          id: 'cand-trace-path',
          provider_id: 'provider-path',
          provider_name: 'Provider Path',
          key_id: 'key-path',
          key_name: 'Path Key',
          candidate_index: 0,
          status: 'failed',
        }),
      ]),
      request_path: '/v1/images/generations',
    }

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('请求路径')
    const requestPathCode = root.querySelector<HTMLElement>('.request-path-code')
    expect(requestPathCode?.textContent).toContain('/v1/images/generations')
  })

  it('shows upstream response headers and body in one error envelope', async () => {
    const upstreamErrorMessage = 'This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request. To get authorized for security work, join the Trusted Access for Cyber program: https://chatgpt.com/cyber'
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-upstream-response',
        provider_id: 'provider-upstream',
        provider_name: 'Provider Upstream',
        key_id: 'key-upstream',
        key_name: 'Upstream Key',
        candidate_index: 0,
        status: 'failed',
        error_message: 'execution runtime stream returned non-success status 400',
        extra_data: {
          upstream_response: {
            status_code: 400,
            headers: {
              'content-type': 'application/json',
              'x-request-id': 'req_usage-cyber-risk-demo',
            },
            body: {
              error: {
                type: 'invalid_request',
                message: upstreamErrorMessage,
                code: 400,
              },
            },
            body_ref: 'usage://request/req-1/response_body',
            body_state: 'reference',
          },
          error_flow: {
            source: 'upstream_response',
            status_code: 400,
            classification: 'use_default',
            decision: 'use_default',
            propagation: 'none',
            retryable: false,
            safe_to_expose: false,
            message: 'execution runtime stream returned non-success status 400',
          },
          client_response: {
            status_code: 502,
            headers: { 'content-type': 'application/json' },
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('错误信息')
    expect(root.textContent).toContain('HTTP 400')
    expect(root.textContent).not.toContain('上游返回非成功状态 400')
    const upstreamResponse = root.querySelector<HTMLElement>('.error-upstream-response-json pre')
    expect(upstreamResponse?.dataset.title).toBe('上游响应')
    expect(JSON.parse(upstreamResponse?.textContent ?? '{}')).toEqual({
      header: {
        'content-type': 'application/json',
        'x-request-id': 'req_usage-cyber-risk-demo',
      },
      body: {
        error: {
          type: 'invalid_request',
          message: upstreamErrorMessage,
          code: 400,
        },
      },
    })
    expect(upstreamResponse?.textContent).not.toContain('"status_code"')
    expect(upstreamResponse?.textContent).not.toContain('"headers"')
    expect(upstreamResponse?.textContent).not.toContain('"body_ref"')
    expect(upstreamResponse?.textContent).not.toContain('"body_state"')
    expect(root.textContent).not.toContain('上游真实响应')
    expect(root.textContent).not.toContain('execution runtime stream returned non-success status 400')
    expect(root.textContent).not.toContain('真实请求错误')
    expect(root.textContent).not.toContain('返回客户端响应')
    expect(root.textContent).not.toContain('默认处理')
    expect(root.textContent).not.toContain('none')
    expect(root.textContent).not.toContain('不再重试')
    expect(root.textContent).not.toContain('该错误被标记为敏感上游错误')
  })

  it('keeps local sync diagnostics visible when upstream response body capture is disabled', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-local-sync-diagnostic',
        provider_id: 'provider-local-sync',
        provider_name: 'Provider Local Sync',
        key_id: 'key-local-sync',
        key_name: 'Local Sync Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 500,
        error_type: 'local_sync_attempt_aborted',
        error_message: 'Local sync attempt failed before terminal finalization: Internal("Unsupported provider stream event cannot be converted losslessly: field $.type = \\"response.future.delta\\"; fields: payload, response, type")',
        extra_data: {
          upstream_response: {
            status_code: 500,
            headers: { 'content-type': 'application/json' },
            body_state: 'disabled',
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('错误信息')
    expect(root.textContent).toContain('HTTP 500')
    expect(root.textContent).toContain('流式格式转换失败')
    expect(root.textContent).toContain('上游返回了当前不支持的 stream event')
    expect(root.textContent).toContain('字段 $.type = "response.future.delta"')
    const diagnosticText = root.querySelector('.error-diagnostic-json')?.textContent ?? ''
    expect(diagnosticText).toContain('"breakpoint":"$.type"')
    expect(diagnosticText).toContain('"analysis_hint"')
    expect(diagnosticText).toContain('"raw"')
    expect(diagnosticText).toContain('"body_state":"disabled"')
    expect(root.querySelector('.error-upstream-response-json')?.textContent).toContain('"header":{"content-type":"application/json"}')
  })

  it('formats request conversion diagnostics with field paths on skipped trace nodes', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-request-conversion',
        provider_id: 'provider-request-conversion',
        provider_name: 'Provider Request Conversion',
        key_id: 'key-request-conversion',
        key_name: 'Request Conversion Key',
        candidate_index: 0,
        status: 'skipped',
        skip_reason: 'provider_request_body_build_failed',
        extra_data: {
          failure_diagnostic: {
            kind: 'request_conversion',
            path: '$.n',
            message: 'lossy conversion blocked from openai:chat to openai:responses at n: multiple completions cannot be represented losslessly',
            safe_to_show: true,
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('跳过原因')
    expect(root.textContent).toContain('上游请求体转换失败')
    expect(root.textContent).toContain('$.n')
    expect(root.textContent).toContain('格式转换失败')
    expect(root.textContent).toContain('OpenAI Chat → OpenAI Responses')
    expect(root.textContent).toContain('字段 $.n 会丢失信息')
    expect(root.querySelector('.diagnostic-json-panel')).toBeNull()
  })

  it('formats unsupported stream finish reasons with the failing field', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-finish-reason',
        provider_id: 'provider-finish-reason',
        provider_name: 'Provider Finish Reason',
        key_id: 'key-finish-reason',
        key_name: 'Finish Reason Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 500,
        error_message: 'Internal("Unsupported provider stream finish reason cannot be converted losslessly: field $.finish_reason = \\"future_reason\\"")',
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('流式格式转换失败')
    expect(root.textContent).toContain('finish reason')
    expect(root.textContent).toContain('字段 $.finish_reason = "future_reason"')
    const diagnosticText = root.querySelector('.error-diagnostic-json')?.textContent ?? ''
    expect(diagnosticText).toContain('"breakpoint":"$.finish_reason"')
  })

  it('uses conversion messages from error_flow as the diagnostic breakpoint source', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-error-flow-conversion',
        provider_id: 'provider-error-flow-conversion',
        provider_name: 'Provider Error Flow Conversion',
        key_id: 'key-error-flow-conversion',
        key_name: 'Error Flow Conversion Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 500,
        error_message: 'execution runtime stream returned non-success status 500',
        extra_data: {
          upstream_response: {
            status_code: 500,
            body_state: 'disabled',
          },
          error_flow: {
            status_code: 500,
            message: 'lossy conversion blocked from openai:chat to openai:responses at n: multiple completions cannot be represented losslessly',
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('格式转换失败')
    expect(root.textContent).toContain('OpenAI Chat → OpenAI Responses')
    expect(root.textContent).toContain('字段 $.n 会丢失信息')
    expect(root.textContent).not.toContain('上游返回非成功状态 500')
    const diagnosticText = root.querySelector('.error-diagnostic-json')?.textContent ?? ''
    expect(diagnosticText).toContain('"body_state":"disabled"')
    expect(diagnosticText).toContain('"breakpoint":"$.n"')
    expect(diagnosticText).toContain('断点在请求/响应格式转换器')
  })

  it('shows failed diagnostic messages even when the only response panel data is diagnostic metadata', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-diagnostic-only',
        provider_id: 'provider-diagnostic-only',
        provider_name: 'Provider Diagnostic Only',
        key_id: 'key-diagnostic-only',
        key_name: 'Diagnostic Only Key',
        candidate_index: 0,
        status: 'failed',
        error_type: 'request_conversion_failed',
        extra_data: {
          failure_diagnostic: {
            kind: 'request_conversion',
            path: '$.temperature',
            message: 'unsupported field temperature in openai:responses: temperature cannot be represented',
            safe_to_show: true,
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('错误信息')
    expect(root.textContent).toContain('格式转换失败')
    expect(root.textContent).toContain('OpenAI Responses 不支持字段 $.temperature')
    const diagnosticText = root.querySelector('.error-diagnostic-json')?.textContent ?? ''
    expect(diagnosticText).toContain('"breakpoint":"$.temperature"')
  })

  it('keeps the failure message when upstream response only records an empty body state', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-empty-body-state',
        provider_id: 'provider-empty-body-state',
        provider_name: 'Provider Empty Body State',
        key_id: 'key-empty-body-state',
        key_name: 'Empty Body State Key',
        candidate_index: 0,
        status: 'failed',
        error_type: 'stream_missing_terminal_event',
        error_message: 'execution runtime stream ended before provider terminal event',
        extra_data: {
          upstream_response: {
            body_state: 'none',
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('错误信息')
    expect(root.textContent).toContain('execution runtime stream ended before provider terminal event')
    expect(root.querySelector('.error-block .error-json')).toBeNull()
    expect(root.textContent).not.toContain('"body_state":"none"')
  })

  it('falls back to key id while an active candidate is missing key name', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-active-key-id',
        provider_id: 'provider-active',
        provider_name: 'Provider Active',
        key_id: 'key-active-id',
        key_name: undefined,
        candidate_index: 0,
        status: 'pending',
        finished_at: undefined,
      }),
    ])
    trace.final_status = 'pending'

    const root = mountTimeline(trace)
    await nextTick()

    const detailText = root.querySelector('.detail-panel')?.textContent ?? ''
    expect(detailText).toContain('key-active-id')
    expect(detailText).not.toContain('未知')
  })

  it('uses latency to display the time range when recorded finish time equals start time', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-success-same-time',
        status: 'success',
        started_at: '2026-05-06T12:00:00.000Z',
        finished_at: '2026-05-06T12:00:00.000Z',
        latency_ms: 22200,
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const detailText = root.querySelector('.detail-panel')?.textContent ?? ''
    expect(detailText).toContain('+22.20s')
    expect(detailText).not.toContain('+0ms')
  })

  it('follows the active key when silent polling updates the trace', async () => {
    const initialTrace = buildTrace([
      buildCandidate({
        id: 'cand-available-a',
        provider_id: 'provider-a',
        provider_name: 'Provider A',
        key_id: 'key-a',
        key_name: 'Available Key',
        candidate_index: 0,
        status: 'available',
        started_at: undefined,
        finished_at: undefined,
      }),
      buildCandidate({
        id: 'cand-available-b',
        provider_id: 'provider-b',
        provider_name: 'Provider B',
        key_id: 'key-b',
        key_name: 'Streaming Key',
        candidate_index: 1,
        status: 'available',
        started_at: undefined,
        finished_at: undefined,
      }),
    ])
    initialTrace.final_status = 'pending'

    const activeTrace = buildTrace([
      initialTrace.candidates[0],
      buildCandidate({
        id: 'cand-streaming-b',
        provider_id: 'provider-b',
        provider_name: 'Provider B',
        key_id: 'key-b',
        key_name: 'Streaming Key',
        candidate_index: 1,
        status: 'pending',
        started_at: '2026-05-06T12:00:02.000Z',
        finished_at: undefined,
      }),
    ])
    activeTrace.final_status = 'streaming'

    requestTraceApiMock.getRequestTrace.mockResolvedValueOnce(initialTrace)
    const { root, refresh } = mountTimelineFromApi('req-1', {
      requestStatus: 'streaming',
    })
    await flushPendingUpdates()

    expect(root.querySelector('.detail-panel')?.textContent).toContain('Available Key')

    requestTraceApiMock.getRequestTrace.mockResolvedValueOnce(activeTrace)
    await refresh()

    const detailText = root.querySelector('.detail-panel')?.textContent ?? ''
    expect(detailText).toContain('Streaming Key')
    expect(detailText).toContain('进行中')
  })
})
