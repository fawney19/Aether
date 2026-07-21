const CYBER_POLICY_TEXT_MARKERS = [
  'possible cybersecurity risk',
  'trusted access for cyber',
  'chatgpt.com/cyber',
]

const CYBER_POLICY_CLASSIFIER_FIELDS = [
  'code',
  'type',
  'category',
  'reason',
] as const

const CYBER_ERROR_OBJECT_FIELDS = [
  'error',
  'errors',
  'message',
  'error_message',
  'detail',
  'body',
  'response_body',
  'upstream_error',
  'failure_summary',
] as const

function normalizeCyberClassifier(value: string): string {
  return value.trim().toLowerCase().replace(/[\s-]+/g, '_')
}

function isCyberPolicyClassifier(value: unknown): boolean {
  if (typeof value !== 'string') return false
  const normalized = normalizeCyberClassifier(value)
  if (normalized === 'cyber' || normalized === 'cyber_policy') return true

  // Providers have used nearby classifier spellings while keeping the same
  // structured error contract. Keep this deliberately narrower than a generic
  // substring check so ordinary cybersecurity content is not badged.
  return /^(?:cyber|cybersecurity)_(?:policy|safety|risk)(?:_(?:violation|error|refusal|blocked))?$/.test(normalized)
}

function isCyberPolicyText(value: string): boolean {
  const normalized = value.trim().toLowerCase()
  if (!normalized) return false
  return CYBER_POLICY_TEXT_MARKERS.some(marker => normalized.includes(marker))
    || /["'](?:code|type|category|reason)["']\s*:\s*["'](?:cyber|cyber[-_ ]policy|cyber[-_ ]safety|cybersecurity[-_ ](?:policy|risk))["']/i.test(normalized)
}

function detectCyberPolicyError(value: unknown, seen: WeakSet<object>): boolean {
  if (typeof value === 'string') return isCyberPolicyText(value)
  if (value === null || typeof value !== 'object') return false
  if (seen.has(value)) return false
  seen.add(value)

  if (Array.isArray(value)) return value.some(item => detectCyberPolicyError(item, seen))

  const record = value as Record<string, unknown>
  if (CYBER_POLICY_CLASSIFIER_FIELDS.some(field => isCyberPolicyClassifier(record[field]))) {
    return true
  }

  return CYBER_ERROR_OBJECT_FIELDS.some(field => detectCyberPolicyError(record[field], seen))
}

/**
 * Detects the provider's Cyber Policy refusal without treating generic HTTP 400,
 * invalid_request, or ordinary uses of the word "cyber" as policy failures.
 */
export function isCyberPolicyError(value: unknown): boolean {
  return detectCyberPolicyError(value, new WeakSet<object>())
}
