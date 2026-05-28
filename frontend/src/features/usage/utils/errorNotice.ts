import type { RequestDetail, RequestErrorDomain, RequestSchedulingFailure } from '@/api/dashboard'

export interface RequestFailureNotice {
  title: string
  message: string
  meta: string[]
  isSchedulingFailure: boolean
}

function nonEmptyString(value: string | null | undefined): string | null {
  const trimmed = value?.trim()
  return trimmed ? trimmed : null
}

function normalizeErrorDomain(domain: RequestErrorDomain | null | undefined): RequestErrorDomain | null {
  if (!nonEmptyString(domain?.message)) return null
  return domain ?? null
}

function formatHttpStatus(statusCode: number | null | undefined): string | null {
  return typeof statusCode === 'number' && (statusCode < 200 || statusCode >= 300)
    ? `HTTP ${statusCode}`
    : null
}

function uniqueMeta(values: Array<string | null | undefined>): string[] {
  return Array.from(new Set(values.map(value => value?.trim()).filter((value): value is string => Boolean(value))))
}

function schedulingFailureMessage(
  failure: RequestSchedulingFailure,
  fallbackDomain: RequestErrorDomain | null,
  fallbackErrorMessage: string | null,
): string | null {
  return nonEmptyString(failure.message)
    ?? nonEmptyString(fallbackDomain?.message)
    ?? fallbackErrorMessage
    ?? nonEmptyString(failure.reason_label)
    ?? nonEmptyString(failure.reason)
}

function schedulingFailureTitle(failure: RequestSchedulingFailure): string {
  const summary = failure.candidate_failure_summary
  if (!summary) return nonEmptyString(failure.title) ?? '本地调度失败'
  const total = typeof summary.total === 'number' ? summary.total : null
  const failed = typeof summary.failed === 'number' ? summary.failed : 0
  const skipped = typeof summary.skipped === 'number' ? summary.skipped : 0
  if (total === 0) return nonEmptyString(failure.title) ?? '没有找到可调度候选'
  if (failed === 1 && total === 1) return nonEmptyString(failure.title) ?? '唯一候选执行失败，已无可重试上游'
  if (total != null && failed > 0 && failed + skipped >= total) {
    return nonEmptyString(failure.title) ?? '所有候选已完成重试，但全部执行失败'
  }
  if (total != null && total > 0 && skipped === total) {
    return nonEmptyString(failure.title) ?? '所有候选都被调度规则跳过'
  }
  return nonEmptyString(failure.title) ?? '本地调度失败'
}

export function resolveRequestFailureNotice(detail: RequestDetail | null | undefined): RequestFailureNotice | null {
  if (!detail) return null

  const fallbackDomain = normalizeErrorDomain(detail.failure_summary)
    ?? normalizeErrorDomain(detail.client_error)
    ?? normalizeErrorDomain(detail.upstream_error)
    ?? normalizeErrorDomain(detail.request_error)
  const fallbackErrorMessage = nonEmptyString(detail.error_message ?? null)
  const schedulingFailure = detail.scheduling_failure ?? null

  if (schedulingFailure) {
    const upstreamFailure = schedulingFailure.upstream_failure ?? null
    const upstreamMessage = nonEmptyString(upstreamFailure?.user_message)
      ?? nonEmptyString(schedulingFailure.message)
      ?? nonEmptyString(upstreamFailure?.message)
    if (upstreamFailure && upstreamMessage) {
      return {
        title: schedulingFailureTitle(schedulingFailure),
        message: upstreamMessage,
        isSchedulingFailure: true,
        meta: uniqueMeta([
          formatHttpStatus(upstreamFailure.status_code ?? schedulingFailure.status_code ?? detail.status_code),
          nonEmptyString(upstreamFailure.type),
          nonEmptyString(upstreamFailure.param),
          nonEmptyString(upstreamFailure.provider_name),
          nonEmptyString(upstreamFailure.model),
          schedulingFailure.no_upstream_attempt ? '未进入上游执行' : null,
        ]),
      }
    }

    const message = schedulingFailureMessage(schedulingFailure, fallbackDomain, fallbackErrorMessage)
    if (message) {
      return {
        title: schedulingFailureTitle(schedulingFailure),
        message,
        isSchedulingFailure: true,
        meta: uniqueMeta([
          nonEmptyString(schedulingFailure.reason_summary),
          nonEmptyString(schedulingFailure.reason_label),
          nonEmptyString(schedulingFailure.reason),
          formatHttpStatus(schedulingFailure.status_code ?? detail.status_code),
          schedulingFailure.no_upstream_attempt ? '未进入上游执行' : null,
        ]),
      }
    }
  }

  const domain = fallbackDomain
  const message = nonEmptyString(domain?.message) ?? fallbackErrorMessage
  if (!message) return null

  return {
    title: '执行失败原因',
    message,
    isSchedulingFailure: false,
    meta: uniqueMeta([
      formatHttpStatus(domain?.status_code ?? detail.status_code),
      nonEmptyString(domain?.type),
      nonEmptyString(domain?.source),
    ]),
  }
}
