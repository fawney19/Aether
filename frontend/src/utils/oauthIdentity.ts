import type { OAuthOrganizationInfo } from '@/api/endpoints/types/provider'

type OAuthIdentityDisplayValue = {
  oauth_account_name?: string | null
  oauth_organizations?: OAuthOrganizationInfo[] | null
} | null | undefined

function formatOAuthIdentityShort(
  value: string | null | undefined,
  head = 8,
  tail = 6,
): string {
  const normalized = String(value || '').trim()
  if (!normalized) return ''
  if (normalized.length <= head + tail + 3) return normalized
  return `${normalized.slice(0, head)}...${normalized.slice(-tail)}`
}

function getPrimaryOAuthOrganization(
  value: OAuthIdentityDisplayValue,
): { id: string; title: string } | null {
  const organizations: OAuthOrganizationInfo[] = Array.isArray(value?.oauth_organizations)
    ? value.oauth_organizations
    : []
  let firstWithId: OAuthOrganizationInfo | null = null

  for (let index = 0; index < organizations.length; index += 1) {
    const org = organizations[index]
    if (typeof org?.id !== 'string' || !org.id.trim()) continue
    if (!firstWithId) firstWithId = org
    if (org.is_default) {
      firstWithId = org
      break
    }
  }

  if (!firstWithId?.id) return null

  return {
    id: firstWithId.id.trim(),
    title: typeof firstWithId.title === 'string' ? firstWithId.title.trim() : '',
  }
}

export function getOAuthOrgBadge(
  value: OAuthIdentityDisplayValue,
): { id: string; label: string } | null {
  const org = getPrimaryOAuthOrganization(value)
  if (!org) return null

  const accountName = typeof value?.oauth_account_name === 'string'
    ? value.oauth_account_name.trim()
    : ''

  return {
    id: org.id,
    label: accountName || org.title || formatOAuthIdentityShort(org.id),
  }
}
