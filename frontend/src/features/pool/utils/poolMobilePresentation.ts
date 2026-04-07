export type PoolMobileTagTone = 'default' | 'muted' | 'warning' | 'danger' | 'accent'

export interface PoolMobileTagItem {
  key: string
  label: string
  tone: PoolMobileTagTone
}

export interface PoolMobileTagInput {
  priorityLabel?: string | null
  authLabel?: string | null
  oauthStatusLabel?: string | null
  oauthStatusTone?: PoolMobileTagTone | null
  accountStatusLabel?: string | null
  accountStatusTone?: PoolMobileTagTone | null
  planLabel?: string | null
  orgLabel?: string | null
  proxyLabel?: string | null
}

export type PoolMobileActionId =
  | 'copy_or_download'
  | 'refresh_token'
  | 'clear_cooldown'
  | 'recover_health'
  | 'permissions'
  | 'proxy'
  | 'edit'
  | 'toggle'
  | 'delete'

export interface PoolMobileActionInput {
  canDownloadOrCopy?: boolean
  canRefreshToken?: boolean
  canClearCooldown?: boolean
  canRecoverHealth?: boolean
  hasProxy?: boolean
}

function createTagItem(
  key: string,
  label: string | null | undefined,
  tone: PoolMobileTagTone,
): PoolMobileTagItem | null {
  if (!label) return null
  return { key, label, tone }
}

export function buildPoolMobileTagItems(input: PoolMobileTagInput): PoolMobileTagItem[] {
  return [
    createTagItem('account', input.accountStatusLabel, input.accountStatusTone ?? 'warning'),
    createTagItem('oauth', input.oauthStatusLabel, input.oauthStatusTone ?? 'warning'),
    createTagItem('priority', input.priorityLabel, 'muted'),
    createTagItem('auth', input.authLabel, 'default'),
    createTagItem('plan', input.planLabel, 'accent'),
    createTagItem('org', input.orgLabel, 'accent'),
    createTagItem('proxy', input.proxyLabel, 'muted'),
  ].filter((item): item is PoolMobileTagItem => item !== null)
}

export function splitPoolMobileActions(input: PoolMobileActionInput): {
  primary: PoolMobileActionId[]
  overflow: PoolMobileActionId[]
} {
  if (input.canDownloadOrCopy) {
    const primary: PoolMobileActionId[] = ['copy_or_download']
    if (input.canRefreshToken) {
      primary.push('refresh_token')
    }
    if (input.canClearCooldown) {
      primary.push('clear_cooldown')
    }
    if (input.canRecoverHealth) {
      primary.push('recover_health')
    }
    primary.push('permissions')
    if (input.hasProxy) {
      primary.push('proxy')
    }
    primary.push('edit', 'toggle', 'delete')

    return {
      primary,
      overflow: [],
    }
  }

  const primary: PoolMobileActionId[] = []
  if (input.canRefreshToken) {
    primary.push('refresh_token')
  }
  if (input.canClearCooldown) {
    primary.push('clear_cooldown')
  }
  if (input.canRecoverHealth) {
    primary.push('recover_health')
  }
  primary.push('permissions')
  if (input.hasProxy) {
    primary.push('proxy')
  }
  primary.push('edit', 'toggle', 'delete')

  return {
    primary,
    overflow: [],
  }
}
