import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'

import type { CandidateRecord, RequestTrace } from '@/api/requestTrace'
import HorizontalRequestTimeline from '../HorizontalRequestTimeline.vue'

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
      },
      setup(props) {
        return () => h('pre', JSON.stringify(props.data))
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

afterEach(() => {
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

  it('emits failed trace state when a streaming lifecycle already has terminal failure evidence', async () => {
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

    const lastCall = onTraceState.mock.calls[onTraceState.mock.calls.length - 1]?.[0]
    expect(lastCall).toMatchObject({
      finalStatus: 'failed',
    })
  })

  it('shows active streaming candidate error signals as failed on the node and detail panel', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-active-timeout',
        provider_id: 'provider-active-timeout',
        provider_name: 'Provider Active Timeout',
        key_id: 'key-active-timeout',
        key_name: 'Active Timeout Key',
        candidate_index: 0,
        status: 'streaming',
        status_code: 200,
        error_message: 'UpstreamRequest("provider stream first byte timeout after 10000 ms")',
        finished_at: undefined,
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    const nodeDot = root.querySelector<HTMLElement>('.node-dot')
    expect(nodeDot?.classList.contains('status-failed')).toBe(true)
    expect(root.textContent).toContain('错误信息')
    expect(root.textContent).toContain('请求超时（10秒）')
    expect(root.textContent).not.toContain('传输中')
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

  it('shows structured upstream response details inside the error block on trace nodes', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-upstream-response',
        provider_id: 'provider-upstream',
        provider_name: 'Provider Upstream',
        key_id: 'key-upstream',
        key_name: 'Upstream Key',
        candidate_index: 0,
        status: 'failed',
        error_message: 'execution runtime stream returned non-success status 302',
        extra_data: {
          upstream_response: {
            status_code: 302,
            headers: { location: '/' },
          },
          error_flow: {
            source: 'upstream_response',
            status_code: 302,
            classification: 'use_default',
            decision: 'use_default',
            propagation: 'none',
            retryable: false,
            safe_to_expose: false,
            message: 'execution runtime stream returned non-success status 302',
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
    expect(root.textContent).toContain('HTTP 302')
    expect(root.textContent).toContain('上游返回非成功状态 302')
    const errorBlock = root.querySelector('.error-block')
    expect(errorBlock?.textContent).not.toContain('供应商')
    expect(errorBlock?.textContent).not.toContain('Provider Upstream')
    expect(errorBlock?.textContent).not.toContain('Key')
    expect(errorBlock?.textContent).not.toContain('Upstream Key')
    expect(root.textContent).not.toContain('HTTP 状态')
    expect(root.querySelector('.error-block .error-json')?.textContent).toContain('"status_code":302')
    expect(root.querySelector('.error-block .error-json')?.textContent).toContain('"headers"')
    expect(root.textContent).not.toContain('上游真实响应')
    expect(root.textContent).not.toContain('execution runtime stream returned non-success status 302')
    expect(root.textContent).not.toContain('真实请求错误')
    expect(root.textContent).not.toContain('返回客户端响应')
    expect(root.textContent).not.toContain('上游响应')
    expect(root.textContent).not.toContain('默认处理')
    expect(root.textContent).not.toContain('none')
    expect(root.textContent).not.toContain('不再重试')
    expect(root.textContent).not.toContain('该错误被标记为敏感上游错误')
  })

  it('shows upstream body error message and typed fields by default', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-upstream-body-error',
        provider_id: 'provider-body-error',
        provider_name: 'GPUStack',
        key_id: 'key-body-error',
        key_name: 'key1',
        candidate_index: 0,
        status: 'failed',
        status_code: 400,
        extra_data: {
          upstream_response: {
            status_code: 400,
            body: {
              error: {
                type: 'BadRequestError',
                param: 'input_tokens',
                message: 'This model\'s maximum context length is 131072 tokens.',
              },
            },
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()
    const errorBlockText = root.querySelector('.error-block')?.textContent ?? ''

    expect(errorBlockText).toContain('This model\'s maximum context length is 131072 tokens.')
    expect(errorBlockText).toContain('错误类型')
    expect(errorBlockText).toContain('BadRequestError')
    expect(errorBlockText).toContain('错误参数')
    expect(errorBlockText).toContain('input_tokens')
    expect(errorBlockText).not.toContain('供应商')
    expect(errorBlockText).not.toContain('Key')
    expect(errorBlockText).not.toContain('GPUStack')
    expect(errorBlockText).not.toContain('key1')
    expect(root.querySelector('.error-block .error-json')?.textContent).toContain('"status_code":400')
  })

  it('localizes internal error types and keeps repeated provider context out of the error block', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-watchdog-timeout',
        provider_id: 'provider-watchdog',
        provider_name: 'aether 公益',
        key_id: 'key-watchdog',
        key_name: '公益 Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 504,
        error_type: 'local_stream_candidate_watchdog_timeout',
        error_message: 'Stream first byte timeout',
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()
    const errorBlockText = root.querySelector('.error-block')?.textContent ?? ''

    expect(errorBlockText).toContain('请求超时（等待上游首字超时）')
    expect(errorBlockText).toContain('错误类型')
    expect(errorBlockText).toContain('本地流式候选首字超时')
    expect(errorBlockText).not.toContain('local_stream_candidate_watchdog_timeout')
    expect(errorBlockText).not.toContain('供应商')
    expect(errorBlockText).not.toContain('Key')
    expect(errorBlockText).not.toContain('aether 公益')
    expect(errorBlockText).not.toContain('公益 Key')
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

  it('normalizes stream first byte timeout errors and hides empty body refs', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-timeout-body-ref',
        provider_id: 'provider-timeout',
        provider_name: 'Provider Timeout',
        key_id: 'key-timeout',
        key_name: 'Timeout Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 503,
        error_message: 'UpstreamRequest("provider stream first byte timeout after 10000 ms")',
        extra_data: {
          upstream_response: {
            body_ref: 'usage://request/c676f8a0-3185-46a0-af06-e7c6f5873c39/response_body',
            body_state: 'none',
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('请求超时（10秒）')
    expect(root.textContent).not.toContain('UpstreamRequest')
    expect(root.textContent).not.toContain('响应 Body')
    expect(root.textContent).not.toContain('usage://request/c676f8a0')
    expect(root.querySelector('.error-block .error-json')).toBeNull()
  })

  it('does not show numeric upstream body error codes as a competing HTTP status', async () => {
    const trace = buildTrace([
      buildCandidate({
        id: 'cand-wrapper-503-provider-404',
        provider_id: 'provider-mixed-status',
        provider_name: 'Provider Mixed Status',
        key_id: 'key-mixed-status',
        key_name: 'Mixed Status Key',
        candidate_index: 0,
        status: 'failed',
        status_code: 503,
        extra_data: {
          upstream_response: {
            status_code: 503,
            body: {
              error: {
                code: 404,
                type: 'not_found',
                message: 'model not found',
              },
            },
          },
        },
      }),
    ])

    const root = mountTimeline(trace)
    await nextTick()

    expect(root.textContent).toContain('HTTP 503')
    expect(root.textContent).toContain('model not found')
    expect(root.textContent).toContain('错误类型')
    expect(root.textContent).toContain('资源不存在')
    expect(root.textContent).not.toContain('错误代码')
  })

})
