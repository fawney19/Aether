export interface UserSession {
  id: string
  device_label: string
  device_type: string
  browser_name?: string | null
  browser_version?: string | null
  os_name?: string | null
  os_version?: string | null
  device_model?: string | null
  ip_address?: string | null
  last_seen_at?: string | null
  created_at: string
  is_current: boolean
  revoked_at?: string | null
  revoke_reason?: string | null
}

export function formatSessionMeta(session: UserSession): string {
  const parts = [
    session.browser_name && session.browser_version
      ? `${session.browser_name} ${session.browser_version}`
      : session.browser_name || null,
    session.os_name && session.os_version
      ? `${session.os_name} ${session.os_version}`
      : session.os_name || null,
    session.device_model || null,
  ].filter(Boolean)
  return parts.length > 0 ? parts.join(' \u00b7 ') : '\u8bbe\u5907\u4fe1\u606f\u672a\u77e5'
}
