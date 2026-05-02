export interface ProviderKeyAuthCarrier {
  auth_type?: string | null
  credential_kind?: string | null
  runtime_auth_kind?: string | null
  oauth_managed?: boolean | null
  oauth_temporary?: boolean | null
  can_refresh_oauth?: boolean | null
  can_export_oauth?: boolean | null
  can_edit_oauth?: boolean | null
}

function normalizeText(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const text = value.trim().toLowerCase()
  return text || null
}

export function getProviderCredentialKind(
  input: ProviderKeyAuthCarrier,
): 'raw_secret' | 'oauth_session' | 'service_account' {
  const credentialKind = normalizeText(input.credential_kind)
  if (
    credentialKind === 'raw_secret'
    || credentialKind === 'oauth_session'
    || credentialKind === 'service_account'
  ) {
    return credentialKind
  }

  if (typeof input.oauth_managed === 'boolean') {
    return input.oauth_managed ? 'oauth_session' : 'raw_secret'
  }

  const authType = normalizeText(input.auth_type)
  if (authType === 'oauth') return 'oauth_session'
  if (authType === 'service_account' || authType === 'vertex_ai') return 'service_account'
  return 'raw_secret'
}

export function getProviderRuntimeAuthKind(
  input: ProviderKeyAuthCarrier,
): 'api_key' | 'bearer' | 'service_account' | 'mixed' | 'unknown' {
  const runtimeAuthKind = normalizeText(input.runtime_auth_kind)
  if (
    runtimeAuthKind === 'api_key'
    || runtimeAuthKind === 'bearer'
    || runtimeAuthKind === 'service_account'
    || runtimeAuthKind === 'mixed'
  ) {
    return runtimeAuthKind
  }

  const authType = normalizeText(input.auth_type)
  if (authType === 'service_account' || authType === 'vertex_ai') return 'service_account'
  if (authType === 'bearer') return 'bearer'
  if (authType === 'api_key') return 'api_key'
  return 'unknown'
}

export function isOAuthManagedCredential(input: ProviderKeyAuthCarrier): boolean {
  if (typeof input.oauth_managed === 'boolean') {
    return input.oauth_managed
  }
  return getProviderCredentialKind(input) === 'oauth_session'
}

export function isServiceAccountCredential(input: ProviderKeyAuthCarrier): boolean {
  return getProviderCredentialKind(input) === 'service_account'
}

export function canRefreshOAuthCredential(input: ProviderKeyAuthCarrier): boolean {
  if (input.oauth_temporary === true) {
    return false
  }
  if (typeof input.can_refresh_oauth === 'boolean') {
    return input.can_refresh_oauth
  }
  return isOAuthManagedCredential(input)
}

export function shouldShowOAuthRefreshControl(input: ProviderKeyAuthCarrier): boolean {
  return isOAuthManagedCredential(input)
}

export function canExportOAuthCredential(input: ProviderKeyAuthCarrier): boolean {
  if (typeof input.can_export_oauth === 'boolean') {
    return input.can_export_oauth
  }
  return isOAuthManagedCredential(input)
}

export function canEditOAuthCredential(input: ProviderKeyAuthCarrier): boolean {
  if (typeof input.can_edit_oauth === 'boolean') {
    return input.can_edit_oauth
  }
  return isOAuthManagedCredential(input)
}

export function getProviderAuthLabel(input: ProviderKeyAuthCarrier): string {
  if (isOAuthManagedCredential(input)) return 'OAuth'
  if (isServiceAccountCredential(input)) return '服务账号'
  if (getProviderRuntimeAuthKind(input) === 'mixed') return '混合'
  return getProviderRuntimeAuthKind(input) === 'bearer' ? 'Bearer' : 'API Key'
}

export function getProviderMaskedSecretLabel(input: ProviderKeyAuthCarrier): string {
  if (isOAuthManagedCredential(input)) return '[OAuth Token]'
  if (isServiceAccountCredential(input)) return '[Service Account]'
  if (getProviderRuntimeAuthKind(input) === 'mixed') return '[Key]'
  return getProviderRuntimeAuthKind(input) === 'bearer' ? '[Bearer Token]' : '[Key]'
}
