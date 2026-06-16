// -- 按原因细分的关键词组 --

// 封禁类 (suspended / banned)
const KEYWORDS_SUSPENDED = [
  'suspended',
  'account_block',
  'account blocked',
  '封禁',
  '封号',
  '被封',
  '账户已封禁',
  '账号异常',
]

// 停用类 (disabled / deactivated)
const KEYWORDS_DISABLED = [
  'account has been disabled',
  'account disabled',
  'account has been deactivated',
  'account_deactivated',
  'account deactivated',
  'organization has been disabled',
  'organization_disabled',
  'deactivated_workspace',
  'deactivated',
  '访问被禁止',
  '账户访问被禁止',
]

const KEYWORDS_TOKEN_INVALID = [
  'oauth_token_invalid',
  'token_invalidated',
  'authentication token has been invalidated',
  'token has been invalidated',
  'token invalidated',
  'invalidated',
  'revoked',
  '已撤销',
  '被撤销',
  '撤销',
  '作废',
  '已失效',
  'token 失效',
  '令牌失效',
]

const KEYWORDS_TOKEN_EXPIRED = [
  'oauth_token_expired',
  'session has expired',
  'session expired',
  'access token expired',
  'expired access token',
  'token has expired',
  'token expired',
  'security token included in the request is expired',
  '已过期',
  '过期',
]

// 需要验证类
const KEYWORDS_VERIFICATION = [
  'validation_required',
  'verify your account',
  '需要验证',
  '验证账号',
  '验证身份',
]

// 合并的完整列表
const ACCOUNT_BLOCK_REASON_KEYWORDS = [
  ...KEYWORDS_SUSPENDED,
  ...KEYWORDS_DISABLED,
  ...KEYWORDS_TOKEN_INVALID,
  ...KEYWORDS_TOKEN_EXPIRED,
  ...KEYWORDS_VERIFICATION,
]

function normalizeOAuthReasonDetail(reason: string): string {
  return reason
    .replace(/^\[(ACCOUNT_BLOCK|OAUTH_EXPIRED)\]\s*/i, '')
    .replace(/\s*\[REFRESH_FAILED\][\s\S]*$/i, '')
    .trim()
}

function isHardTokenInvalidReason(reason: string): boolean {
  const lowered = reason.toLowerCase()
  if (KEYWORDS_TOKEN_INVALID.some(keyword => lowered.includes(keyword))) return true
  return (lowered.includes('token 无效') || lowered.includes('令牌无效'))
    && !KEYWORDS_TOKEN_EXPIRED.some(keyword => lowered.includes(keyword))
}

function isTokenExpiredReason(reason: string): boolean {
  const lowered = reason.toLowerCase()
  return KEYWORDS_TOKEN_EXPIRED.some(keyword => lowered.includes(keyword))
}

export function isAccountLevelBlockReason(reason: string | null | undefined): boolean {
  if (!reason) return false
  const text = reason.trim()
  if (!text) return false
  if (text.startsWith('[ACCOUNT_BLOCK]')) return true
  if (text.startsWith('[OAUTH_EXPIRED]')) return true
  if (text.startsWith('[REFRESH_FAILED]')) return false
  const lowered = text.toLowerCase()
  return ACCOUNT_BLOCK_REASON_KEYWORDS.some(keyword => lowered.includes(keyword))
}

export function classifyAccountBlockLabel(reason: string): string {
  const detail = normalizeOAuthReasonDetail(reason) || reason
  if (reason.trim().startsWith('[OAUTH_EXPIRED]')) {
    return isHardTokenInvalidReason(detail) ? 'Token 失效' : 'Token 过期'
  }
  const lowered = detail.toLowerCase()
  if (isHardTokenInvalidReason(detail)) return 'Token 失效'
  if (isTokenExpiredReason(detail)) return 'Token 过期'
  if (KEYWORDS_VERIFICATION.some(kw => lowered.includes(kw))) return '需要验证'
  if (lowered.includes('deactivated_workspace')) return '工作区停用'
  if (KEYWORDS_DISABLED.some(kw => lowered.includes(kw))) return '账号停用'
  if (KEYWORDS_SUSPENDED.some(kw => lowered.includes(kw))) return '账号封禁'
  return '账号异常'
}

export function cleanAccountBlockReason(reason: string): string {
  return normalizeOAuthReasonDetail(reason)
}

export function isRefreshFailedReason(reason: string | null | undefined): boolean {
  if (!reason) return false
  return reason.includes('[REFRESH_FAILED]')
}

export function isOAuthExpiredReason(reason: string | null | undefined): boolean {
  if (!reason) return false
  return reason.trim().startsWith('[OAUTH_EXPIRED]')
}
