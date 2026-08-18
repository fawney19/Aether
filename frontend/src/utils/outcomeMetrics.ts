export interface RequestOutcomeMetricSource {
  request_count?: number | null
  total_requests?: number | null
  total_attempts?: number | null
  sla_eligible_count?: number | null
  success_count?: number | null
  service_error_count?: number | null
  error_count?: number | null
  error_requests?: number | null
  failed_count?: number | null
  user_error_count?: number | null
  success_rate?: number | null
  service_error_rate?: number | null
  error_rate?: number | null
  user_error_rate?: number | null
}

function finite(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function firstFinite(...values: Array<number | null | undefined>): number | null {
  for (const value of values) {
    const resolved = finite(value)
    if (resolved != null) return resolved
  }
  return null
}

export function resolveRequestCount(source: RequestOutcomeMetricSource): number | null {
  return firstFinite(source.request_count, source.total_requests, source.total_attempts)
}

export function resolveServiceErrorCount(source: RequestOutcomeMetricSource): number | null {
  return firstFinite(
    source.service_error_count,
    source.error_count,
    source.error_requests,
    source.failed_count,
  )
}

export function resolveUserErrorCount(source: RequestOutcomeMetricSource): number | null {
  return finite(source.user_error_count)
}

export function resolveSlaEligibleCount(source: RequestOutcomeMetricSource): number | null {
  const explicit = finite(source.sla_eligible_count)
  if (explicit != null) return explicit

  const successCount = finite(source.success_count)
  const serviceErrorCount = resolveServiceErrorCount(source)
  if (successCount != null && serviceErrorCount != null) {
    return successCount + serviceErrorCount
  }

  const requestCount = resolveRequestCount(source)
  const userErrorCount = resolveUserErrorCount(source)
  if (requestCount != null && userErrorCount != null) {
    return Math.max(0, requestCount - userErrorCount)
  }
  return requestCount
}

export function resolveSlaSuccessRate(
  source: RequestOutcomeMetricSource,
  unit = 100,
): number | null {
  const explicitEligibleCount = finite(source.sla_eligible_count)
  if (explicitEligibleCount != null && explicitEligibleCount <= 0) return null

  const explicit = finite(source.success_rate)
  if (explicit != null) return explicit

  const successCount = finite(source.success_count)
  const eligibleCount = resolveSlaEligibleCount(source)
  if (successCount != null && eligibleCount != null && eligibleCount > 0) {
    return successCount / eligibleCount * unit
  }

  const serviceErrorRate = resolveServiceErrorRate(source, unit, false)
  return serviceErrorRate == null ? null : Math.max(0, unit - serviceErrorRate)
}

export function resolveServiceErrorRate(
  source: RequestOutcomeMetricSource,
  unit = 100,
  allowSuccessRateFallback = true,
): number | null {
  const explicitEligibleCount = finite(source.sla_eligible_count)
  if (explicitEligibleCount != null && explicitEligibleCount <= 0) return null

  const explicit = firstFinite(source.service_error_rate, source.error_rate)
  if (explicit != null) return explicit

  const serviceErrorCount = resolveServiceErrorCount(source)
  const eligibleCount = resolveSlaEligibleCount(source)
  if (serviceErrorCount != null && eligibleCount != null && eligibleCount > 0) {
    return serviceErrorCount / eligibleCount * unit
  }

  if (allowSuccessRateFallback) {
    const successRate = finite(source.success_rate)
    if (successRate != null) return Math.max(0, unit - successRate)
  }
  return null
}

export function resolveUserErrorRate(
  source: RequestOutcomeMetricSource,
  unit = 100,
): number | null {
  const explicit = finite(source.user_error_rate)
  if (explicit != null) return explicit

  const userErrorCount = resolveUserErrorCount(source)
  const requestCount = resolveRequestCount(source)
  if (userErrorCount != null && requestCount != null && requestCount > 0) {
    return userErrorCount / requestCount * unit
  }
  return null
}
