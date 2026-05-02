import type { ProviderKeyStatusSnapshot } from '@/api/endpoints/types/statusSnapshot'
import { getOAuthExpiresCountdown, type OAuthStatusInfo } from '@/composables/useCountdownTimer'
import {
  classifyAccountBlockLabel,
  cleanAccountBlockReason,
  isAccountLevelBlockReason,
  isRefreshFailedReason,
} from './accountBlock'
import {
  canRefreshOAuthCredential,
  isOAuthManagedCredential,
  type ProviderKeyAuthCarrier,
} from './providerKeyAuth'

export interface ProviderKeyStatusCarrier extends ProviderKeyAuthCarrier {
  oauth_expires_at?: number | null
  oauth_invalid_at?: number | null  // compatibility only
  oauth_invalid_reason?: string | null  // compatibility only
  account_status_label?: string | null  // compatibility only
  account_status_reason?: string | null  // compatibility only
  account_status_blocked?: boolean | null  // compatibility only
  status_snapshot?: ProviderKeyStatusSnapshot | null
}

export interface AccountStatusDisplay {
  code: string
  label: string | null
  reason: string | null
  blocked: boolean
}

function normalizeText(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const text = value.trim()
  return text || null
}

function buildLegacyAccountStatus(input: ProviderKeyStatusCarrier): AccountStatusDisplay {
  const explicitLabel = normalizeText(input.account_status_label)
  if (input.account_status_blocked && explicitLabel) {
    return {
      code: 'legacy',
      label: explicitLabel,
      reason: normalizeText(input.account_status_reason),
      blocked: true,
    }
  }

  const invalidReason = normalizeText(input.oauth_invalid_reason)
  if (!invalidReason || !isAccountLevelBlockReason(invalidReason)) {
    return { code: 'ok', label: null, reason: null, blocked: false }
  }

  const cleaned = cleanAccountBlockReason(invalidReason) || invalidReason
  return {
    code: 'legacy',
    label: classifyAccountBlockLabel(cleaned || invalidReason),
    reason: normalizeText(cleaned),
    blocked: true,
  }
}

export function getAccountStatusDisplay(input: ProviderKeyStatusCarrier): AccountStatusDisplay {
  const snapshot = input.status_snapshot?.account
  if (snapshot) {
    return {
      code: normalizeText(snapshot.code) || 'ok',
      label: normalizeText(snapshot.label),
      reason: normalizeText(snapshot.reason),
      blocked: Boolean(snapshot.blocked),
    }
  }
  return buildLegacyAccountStatus(input)
}

function getSnapshotOAuthState(
  input: ProviderKeyStatusCarrier,
  tick: number,
): OAuthStatusInfo | null {
  const oauth = input.status_snapshot?.oauth
  if (!oauth) return null

  const code = normalizeText(oauth.code) || 'none'
  const expiresAt = oauth.expires_at ?? input.oauth_expires_at ?? null
  const reason = normalizeText(oauth.reason)

  if (code === 'invalid') {
    return {
      text: '已失效',
      isExpired: false,
      isExpiringSoon: false,
      isInvalid: true,
      invalidReason: reason || undefined,
    }
  }

  if (code === 'expired') {
    return { text: '已过期', isExpired: true, isExpiringSoon: false, isInvalid: false }
  }

  if (code === 'check_failed') {
    if (expiresAt == null) return null
    return getOAuthExpiresCountdown(expiresAt, tick, null, null)
  }

  if (expiresAt == null) return null
  return getOAuthExpiresCountdown(expiresAt, tick, null, null)
}

function getLegacyOAuthState(
  input: ProviderKeyStatusCarrier,
  tick: number,
): OAuthStatusInfo | null {
  if (!isOAuthManagedCredential(input)) return null
  if (!input.oauth_expires_at && !input.oauth_invalid_at && !input.oauth_invalid_reason) return null

  const rawReason = normalizeText(input.oauth_invalid_reason)
  if (rawReason && isAccountLevelBlockReason(rawReason) && !isRefreshFailedReason(rawReason)) {
    if (!input.oauth_expires_at) return null
    return getOAuthExpiresCountdown(input.oauth_expires_at, tick, null, null)
  }

  return getOAuthExpiresCountdown(
    input.oauth_expires_at,
    tick,
    input.oauth_invalid_at,
    input.oauth_invalid_reason,
  )
}

function getOAuthStatusSeverity(status: OAuthStatusInfo | null): number {
  if (!status) return 0
  if (status.isInvalid) return 3
  if (status.isExpired) return 2
  return 1
}

function mergeOAuthStatusDisplay(
  snapshotStatus: OAuthStatusInfo | null,
  legacyStatus: OAuthStatusInfo | null,
): OAuthStatusInfo | null {
  if (getOAuthStatusSeverity(legacyStatus) > getOAuthStatusSeverity(snapshotStatus)) {
    return legacyStatus
  }

  if (
    snapshotStatus?.isInvalid
    && !snapshotStatus.invalidReason
    && legacyStatus?.isInvalid
    && legacyStatus.invalidReason
  ) {
    return {
      ...snapshotStatus,
      invalidReason: legacyStatus.invalidReason,
    }
  }

  return snapshotStatus ?? legacyStatus
}

function isOAuthCredentialWithoutRefreshToken(input: ProviderKeyStatusCarrier): boolean {
  return isOAuthManagedCredential(input) && !canRefreshOAuthCredential(input)
}

function getMissingRefreshTokenStatus(): OAuthStatusInfo {
  return {
    text: '未添加',
    isExpired: false,
    isExpiringSoon: false,
    isInvalid: false,
  }
}

export function getOAuthStatusDisplay(
  input: ProviderKeyStatusCarrier,
  tick: number,
): OAuthStatusInfo | null {
  return mergeOAuthStatusDisplay(
    getSnapshotOAuthState(input, tick),
    getLegacyOAuthState(input, tick),
  )
}

export function getOAuthStatusDisplayWithFallback(
  input: ProviderKeyStatusCarrier,
  tick: number,
): OAuthStatusInfo | null {
  if (isOAuthCredentialWithoutRefreshToken(input)) {
    return getMissingRefreshTokenStatus()
  }

  const status = getOAuthStatusDisplay(input, tick)
  if (status) return status
  if (!isOAuthManagedCredential(input)) return null

  return {
    text: '有效期未知',
    isExpired: false,
    isExpiringSoon: false,
    isInvalid: false,
  }
}

export function getOAuthStatusTitle(
  input: ProviderKeyStatusCarrier,
  tick: number,
): string {
  if (isOAuthCredentialWithoutRefreshToken(input)) {
    return 'Refresh Token 未添加，无法自动刷新'
  }

  const status = getOAuthStatusDisplay(input, tick)
  if (!status) {
    return isOAuthManagedCredential(input) ? 'Token 有效期未知' : ''
  }
  if (status.isInvalid) {
    const reason = normalizeText(status.invalidReason)
    return reason ? `Token 已失效: ${reason}` : 'Token 已失效'
  }
  if (status.isExpired) {
    return 'Token 已过期，请重新授权'
  }
  return `Token 剩余有效期: ${status.text}`
}

export function getOAuthRefreshButtonTitle(
  input: ProviderKeyStatusCarrier,
  tick: number,
): string {
  if (isOAuthManagedCredential(input) && !canRefreshOAuthCredential(input)) {
    if (input.oauth_temporary === true) {
      return '仅 Access Token 导入，无法自动刷新，到期后需要重新导入'
    }
    return '当前 OAuth 凭据无法自动刷新，到期后需要重新导入'
  }

  const status = getOAuthStatusDisplay(input, tick)
  if (status?.isInvalid || status?.isExpired) {
    return '重新授权'
  }
  return '刷新 Token'
}

export function getAccountStatusTitle(input: ProviderKeyStatusCarrier): string {
  const account = getAccountStatusDisplay(input)
  if (!account.blocked || !account.label) return ''
  return account.reason ? `${account.label}: ${account.reason}` : account.label
}
