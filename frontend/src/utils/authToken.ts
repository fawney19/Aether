export type AccessTokenIdentity = {
  userId: string | null
  role: string | null
}

function decodeBase64Url(value: string): string | null {
  try {
    const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
    const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=')
    return atob(padded)
  } catch {
    return null
  }
}

export function parseAccessTokenIdentity(token: string | null): AccessTokenIdentity | null {
  if (!token) {
    return null
  }

  const parts = token.split('.')
  if (parts.length < 2) {
    return null
  }

  const decodedPayload = decodeBase64Url(parts[1])
  if (!decodedPayload) {
    return null
  }

  try {
    const payload = JSON.parse(decodedPayload) as Record<string, unknown>
    return {
      userId: typeof payload.user_id === 'string' ? payload.user_id : null,
      role: typeof payload.role === 'string' ? payload.role : null,
    }
  } catch {
    return null
  }
}

export function hasAuthIdentityChanged(
  previousToken: string | null,
  nextToken: string | null,
  currentUser: { id?: string | null; role?: string | null } | null,
): boolean {
  const nextIdentity = parseAccessTokenIdentity(nextToken)
  if (!nextIdentity) {
    return true
  }

  const previousIdentity = parseAccessTokenIdentity(previousToken)
  const previousUserId = previousIdentity?.userId ?? currentUser?.id ?? null
  const previousRole = previousIdentity?.role ?? currentUser?.role ?? null

  if (!previousUserId && !previousRole) {
    return true
  }

  return nextIdentity.userId !== previousUserId || nextIdentity.role !== previousRole
}
