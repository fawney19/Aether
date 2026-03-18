import { ref, onBeforeUnmount } from 'vue'
import { isAxiosError } from 'axios'
import { useToast } from './useToast'
import {
  testModelFailover,
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
  message?: string
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

  async function pollTestTrace(reqId: string, token: number) {
    try {
      const trace = await requestTraceApi.getRequestTrace(reqId, { attemptedOnly: false })
      if (tracePollToken !== token || requestId.value !== reqId) return
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
    void pollTestTrace(reqId, token)
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
      const normalizedMessage = typeof params.message === 'string' && params.message.trim()
        ? params.message.trim()
        : undefined

      const result = await testModelFailover({
        provider_id: providerId(),
        mode: params.mode,
        model_name: params.modelName,
        api_format: params.apiFormat,
        endpoint_id: params.endpointId,
        ...(normalizedMessage ? { message: normalizedMessage } : {}),
        request_id: reqId,
        concurrency: params.concurrency,
      }, {
        signal: abortController.signal,
      })

      if (result.success) {
        const successAttempt = result.attempts.find(a => a.status === 'success')
        const latency = successAttempt?.latency_ms != null ? ` (${successAttempt.latency_ms}ms)` : ''
        const mapped = successAttempt?.effective_model && successAttempt.effective_model !== params.modelName
          ? ` -> ${successAttempt.effective_model}`
          : ''
        params.onSuccess?.(result)
        showSuccess(`${params.displayLabel}${mapped} 测试成功${latency}`)
        resetState()
        return
      }

      stopPolling({ clearState: false })
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
