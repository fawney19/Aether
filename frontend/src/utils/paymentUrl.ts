export function safePaymentTargetUrl(value: string): string | null {
  const trimmed = value.trim()
  if (!/^https:\/\//i.test(trimmed) || trimmed.includes('\\')) return null

  try {
    const parsed = new URL(trimmed)
    if (parsed.username || parsed.password) return null
    return parsed.protocol === 'https:' ? parsed.href : null
  } catch {
    return null
  }
}
