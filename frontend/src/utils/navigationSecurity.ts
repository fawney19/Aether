const INTERNAL_NAVIGATION_BASE = 'https://aether.invalid'
const UNSAFE_EXTERNAL_URL_CHARACTERS = /[\\\p{Cc}]/u

function safeAbsoluteExternalUrl(
  value: string | null | undefined,
  allowedProtocols: readonly string[],
): string | null {
  if (!value) return null

  const candidate = value.trim()
  if (
    !/^[a-z][a-z0-9+.-]*:\/\//i.test(candidate)
    || UNSAFE_EXTERNAL_URL_CHARACTERS.test(candidate)
  ) {
    return null
  }

  try {
    const parsed = new URL(candidate)
    if (
      !allowedProtocols.includes(parsed.protocol)
      || parsed.username
      || parsed.password
    ) {
      return null
    }
    return parsed.href
  } catch {
    return null
  }
}

export function safeInternalNavigationPath(value: string | null | undefined): string | null {
  if (!value) return null

  const candidate = value.trim()
  if (
    !candidate.startsWith('/')
    || candidate.startsWith('//')
    || candidate.includes('\\')
    || /\p{Cc}/u.test(candidate)
  ) {
    return null
  }

  try {
    const parsed = new URL(candidate, INTERNAL_NAVIGATION_BASE)
    if (parsed.origin !== INTERNAL_NAVIGATION_BASE) return null
    return `${parsed.pathname}${parsed.search}${parsed.hash}`
  } catch {
    return null
  }
}

export function safeExternalHttpsUrl(value: string | null | undefined): string | null {
  return safeAbsoluteExternalUrl(value, ['https:'])
}

export function safeExternalWebUrl(value: string | null | undefined): string | null {
  return safeAbsoluteExternalUrl(value, ['http:', 'https:'])
}
