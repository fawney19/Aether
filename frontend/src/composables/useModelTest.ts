import { ref, onBeforeUnmount } from 'vue'
import { isAxiosError } from 'axios'
import { useToast } from './useToast'
import {
  testModel,
  testModelFailover,
  type TestAttemptDetail,
  type TestModelResponse,
  type TestModelFailoverResponse,
} from '@/api/endpoints/providers'
import { requestTraceApi, type RequestTrace } from '@/api/requestTrace'
import { parseApiError } from '@/utils/errorParser'

export interface StartTestParams {
  mode: 'global' | 'direct'
  modelName: string
  displayLabel: string
  apiFormat?: string
  endpointId?: string
  endpointBaseUrl?: string
  message?: string
  requestHeaders?: Record<string, unknown>
  requestBody?: Record<string, unknown>
  concurrency?: number
  onSuccess?: (result: TestModelFailoverResponse) => void
  /** Return `true` to indicate the failure has been handled; otherwise the composable sets `testResult`. */
  onFailure?: (result: TestModelFailoverResponse) => boolean | void
  /** Return `true` to indicate the error has been handled; otherwise a toast is shown and state is reset. */
  onError?: (err: unknown) => boolean | void
}

export interface UseModelTestOptions {
  providerId: () => string
  pollInterval?: number
}

export function useModelTest(options: UseModelTestOptions) {
  const { providerId, pollInterval = 800 } = options
  const { success: showSuccess, error: showError } = useToast()
  const LOCAL_FAILOVER_UNCONFIGURED_MESSAGE = 'Rust local provider-query failover simulation is not configured'

  const testing = ref(false)
  const testMode = ref<'global' | 'direct'>('global')
  const testResult = ref<TestModelFailoverResponse | null>(null)
  const testTrace = ref<RequestTrace | null>(null)
  const requestId = ref<string | null>(null)
  const dialogOpen = ref(false)

  let tracePollTimer: ReturnType<typeof setInterval> | null = null
  let tracePollToken = 0
  let activeAbortController: AbortController | null = null

  function buildTestRequestId(): string {
    const randomUUID = globalThis.crypto?.randomUUID?.bind(globalThis.crypto)
    if (randomUUID) {
      return `provider-test-${randomUUID().replace(/-/g, '').slice(0, 20)}`
    }
    return `provider-test-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`
  }

  function resultHasTraceContext(result: TestModelFailoverResponse): boolean {
    if (result.success) return true
    if (Array.isArray(result.attempts) && result.attempts.length > 0) return true
    if (typeof result.total_attempts === 'number' && result.total_attempts > 0) return true
    if (typeof result.total_candidates === 'number' && result.total_candidates > 0) return true
    return false
  }

  function normalizeDirectTestResult(
    params: StartTestParams,
    result: TestModelResponse,
  ): TestModelFailoverResponse {
    const responsePayload = result.data?.response
    const failureMessage = typeof result.error === 'string' && result.error.trim()
      ? result.error.trim()
      : (
          typeof responsePayload?.error === 'string'
            ? responsePayload.error
            : responsePayload?.error?.message
        ) || null
    const syntheticAttempt: TestAttemptDetail = {
      candidate_index: 1,
      endpoint_api_format: params.apiFormat || '-',
      endpoint_base_url: params.endpointBaseUrl || '',
      key_name: null,
      key_id: '',
      auth_type: '',
      effective_model: result.model || params.modelName,
      status: result.success ? 'success' : 'failed',
      skip_reason: null,
      error_message: result.success ? null : failureMessage,
      status_code: responsePayload?.status_code ?? null,
      latency_ms: null,
      request_url: null,
      request_headers: (params.requestHeaders as Record<string, unknown> | undefined) ?? null,
      request_body: params.requestBody ?? null,
      response_headers: null,
      response_body: (responsePayload as Record<string, unknown> | undefined)
        ?? (result.data as Record<string, unknown> | undefined)
        ?? null,
    }

    return {
      success: result.success,
      model: result.model || params.modelName,
      provider: result.provider || { id: providerId(), name: providerId() },
      attempts: [syntheticAttempt],
      total_candidates: 1,
      total_attempts: 1,
      data: (result.data as Record<string, unknown> | undefined) ?? null,
      error: failureMessage,
    }
  }

  async function runDirectTest(
    params: StartTestParams,
    reqId: string,
    signal?: AbortSignal,
  ): Promise<TestModelFailoverResponse> {
    return normalizeDirectTestResult(params, await testModel({
      provider_id: providerId(),
      model_name: params.modelName,
      api_format: params.apiFormat,
      endpoint_id: params.endpointId,
      ...(normalizedMessage(params.message) ? { message: normalizedMessage(params.message) } : {}),
      ...(params.requestHeaders ? { request_headers: params.requestHeaders } : {}),
      ...(params.requestBody ? { request_body: params.requestBody } : {}),
      request_id: reqId,
      concurrency: params.concurrency,
    }, {
      signal,
    }))
  }

  function normalizedMessage(message?: string): string | undefined {
    return typeof message === 'string' && message.trim()
      ? message.trim()
      : undefined
  }

  async function pollTestTrace(reqId: string, token: number) {
    try {
      const trace = await requestTraceApi.getRequestTrace(reqId, { attemptedOnly: false })
      if (tracePollToken !== token || requestId.value !== reqId) return
      testTrace.value = trace
    } catch (err: unknown) {
      if (isAxiosError(err) && err.response?.status === 404) return
    }
  }

  async function refreshTraceSnapshot(reqId: string) {
    try {
      const trace = await requestTraceApi.getRequestTrace(reqId, { attemptedOnly: false })
      if (requestId.value !== reqId) return
      testTrace.value = trace
    } catch (err: unknown) {
      if (isAxiosError(err) && err.response?.status === 404) return
    }
  }

  function stopPolling(opts: { clearState?: boolean } = {}) {
    tracePollToken += 1
    if (tracePollTimer) {
      clearInterval(tracePollTimer)
      tracePollTimer = null
    }
    if (opts.clearState !== false) {
      requestId.value = null
      testTrace.value = null
    }
  }

  function startPolling(reqId: string) {
    stopPolling()
    requestId.value = reqId
    testTrace.value = null
    const token = ++tracePollToken
    tracePollTimer = setInterval(() => {
      void pollTestTrace(reqId, token)
    }, pollInterval)
  }

  function abortActiveRequest() {
    if (!activeAbortController) return
    activeAbortController.abort()
    activeAbortController = null
  }

  function isRequestCancelled(err: unknown): boolean {
    if (isAxiosError(err)) {
      return err.code === 'ERR_CANCELED'
    }
    return err instanceof DOMException && err.name === 'AbortError'
  }

  function resetState() {
    abortActiveRequest()
    stopPolling()
    dialogOpen.value = false
    testResult.value = null
  }

  async function startTest(params: StartTestParams) {
    abortActiveRequest()
    testing.value = true
    testMode.value = params.mode
    dialogOpen.value = true
    testResult.value = null

    const abortController = new AbortController()
    activeAbortController = abortController
    const reqId = buildTestRequestId()
    startPolling(reqId)

    try {
      const message = normalizedMessage(params.message)

      let result = params.mode === 'direct'
        ? await runDirectTest(params, reqId, abortController.signal)
        : await testModelFailover({
          provider_id: providerId(),
          mode: params.mode,
          model_name: params.modelName,
          failover_models: [params.modelName],
          api_format: params.apiFormat,
          endpoint_id: params.endpointId,
          ...(message ? { message } : {}),
          ...(params.requestHeaders ? { request_headers: params.requestHeaders } : {}),
          ...(params.requestBody ? { request_body: params.requestBody } : {}),
          request_id: reqId,
          concurrency: params.concurrency,
        }, {
          signal: abortController.signal,
        })

      if (
        params.mode === 'global'
        && !result.success
        && result.error === LOCAL_FAILOVER_UNCONFIGURED_MESSAGE
      ) {
        result = await runDirectTest(params, reqId, abortController.signal)
      }

      const keepTraceContext = resultHasTraceContext(result)
      if (result.success) {
        if (keepTraceContext) {
          await refreshTraceSnapshot(reqId)
          stopPolling({ clearState: false })
        } else {
          stopPolling()
        }
        testResult.value = result
        const successAttempt = result.attempts.find(a => a.status === 'success')
        const latency = successAttempt?.latency_ms != null ? ` (${successAttempt.latency_ms}ms)` : ''
        const mapped = successAttempt?.effective_model && successAttempt.effective_model !== params.modelName
          ? ` -> ${successAttempt.effective_model}`
          : ''
        params.onSuccess?.(result)
        showSuccess(`${params.displayLabel}${mapped} 测试成功${latency}`)
        return
      }

      if (keepTraceContext) {
        await refreshTraceSnapshot(reqId)
        stopPolling({ clearState: false })
      } else {
        stopPolling()
      }
      const handled = params.onFailure?.(result)
      if (!handled) {
        testResult.value = result
      }
    } catch (err: unknown) {
      if (isRequestCancelled(err)) {
        return
      }
      stopPolling()
      const handled = params.onError?.(err)
      if (!handled) {
        showError(`模型测试失败: ${parseApiError(err, '测试请求失败')}`)
        resetState()
      }
    } finally {
      if (activeAbortController === abortController) {
        activeAbortController = null
      }
      testing.value = false
    }
  }

  onBeforeUnmount(() => {
    resetState()
  })

  return {
    testing,
    testMode,
    testResult,
    testTrace,
    requestId,
    dialogOpen,
    startTest,
    resetState,
    stopPolling,
  }
}
