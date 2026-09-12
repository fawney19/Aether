import client from '../client'

const CLAUDE_COOKIE_AUTHORIZE_TIMEOUT_MS = 4 * 60 * 1000

export interface ProviderOAuthStartResponse {
  authorization_url: string
  redirect_uri: string
  provider_type: string
  instructions: string
}

export interface ProviderOAuthCompleteRequest {
  callback_url: string
  name?: string
  proxy_node_id?: string
}

export interface ProviderOAuthCompleteResponse {
  provider_type: string
  expires_at?: number | null
  has_refresh_token: boolean
  temporary?: boolean
  email?: string | null
  account_state_recheck_attempted?: boolean
  account_state_recheck_error?: string | null
}

export interface ProviderOAuthCompleteResponseWithKey {
  key_id: string
  provider_type: string
  expires_at?: number | null
  has_refresh_token: boolean
  temporary?: boolean
  email?: string | null
  replaced?: boolean
  task_ready?: boolean
  recoverable?: boolean
  detail?: string
}

export interface ProviderCookieAuthorizeRequest {
  cookie: string
  name?: string
  proxy_node_id?: string
}

export interface ProviderCookieAuthorizeBatchTaskRequest {
  cookies: string[]
  proxy_node_id?: string
}

export interface OAuthBatchImportResultItem {
  index: number
  status: 'success' | 'error'
  key_id?: string
  key_name?: string
  auth_method?: string
  error?: string
  replaced?: boolean
}

export type OAuthBatchImportTaskStatus = 'submitted' | 'processing' | 'completed' | 'failed'
export type OAuthBatchImportKind = 'oauth_batch' | 'agent_identity' | 'cookie_authorize'

export interface OAuthBatchImportTaskStartResponse {
  task_id: string
  import_kind?: OAuthBatchImportKind
  status: OAuthBatchImportTaskStatus
  total: number
  processed: number
  success: number
  failed: number
  created_count?: number
  replaced_count?: number
  progress_percent: number
  message?: string | null
}

export interface OAuthBatchImportTaskStatusResponse {
  task_id: string
  import_kind?: OAuthBatchImportKind
  provider_id: string
  provider_type: string
  status: OAuthBatchImportTaskStatus
  total: number
  processed: number
  success: number
  failed: number
  created_count?: number
  replaced_count?: number
  progress_percent: number
  message?: string | null
  error?: string | null
  error_samples: OAuthBatchImportResultItem[]
  created_at: number
  started_at?: number | null
  finished_at?: number | null
  updated_at: number
}

export type BatchImportCredentialsNormalization =
  | { ok: true; isBatch: boolean; credentials: string }
  | { ok: false; message: string }

function getImportCredentialLines(text: string): Array<{ lineNumber: number; text: string }> {
  return text
    .split('\n')
    .map((line, index) => ({ lineNumber: index + 1, text: line.trim() }))
    .filter(line => line.text && !line.text.startsWith('#'))
}

function jsonParseErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function normalizeBatchImportItem(
  value: unknown,
  location: string,
): { ok: true; value: string | Record<string, unknown> } | { ok: false; message: string } {
  if (typeof value === 'string') {
    const trimmed = value.trim()
    if (trimmed) return { ok: true, value: trimmed }
    return { ok: false, message: `${location} 不能为空字符串` }
  }
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    return { ok: true, value: value as Record<string, unknown> }
  }
  return { ok: false, message: `${location} 必须是 JSON 对象或字符串` }
}

function normalizeBatchImportArray(items: unknown[]): BatchImportCredentialsNormalization {
  if (items.length === 0) {
    return { ok: false, message: 'JSON 数组不能为空' }
  }

  const normalized: Array<string | Record<string, unknown>> = []
  for (const [index, item] of items.entries()) {
    const result = normalizeBatchImportItem(item, `JSON 数组第 ${index + 1} 项`)
    if (!result.ok) return result
    normalized.push(result.value)
  }

  return {
    ok: true,
    isBatch: true,
    credentials: JSON.stringify(normalized),
  }
}

function parseImportCredentialLines(
  lines: Array<{ lineNumber: number; text: string }>,
): BatchImportCredentialsNormalization {
  const normalized: Array<string | Record<string, unknown>> = []

  for (const line of lines) {
    const firstChar = line.text[0]
    if (firstChar === '{' || firstChar === '[') {
      let parsed: unknown
      try {
        parsed = JSON.parse(line.text)
      } catch (error) {
        return {
          ok: false,
          message: `JSON Lines 格式无效，请检查第 ${line.lineNumber} 行: ${jsonParseErrorMessage(error)}`,
        }
      }

      if (Array.isArray(parsed)) {
        for (const [index, item] of parsed.entries()) {
          const result = normalizeBatchImportItem(item, `第 ${line.lineNumber} 行数组第 ${index + 1} 项`)
          if (!result.ok) return result
          normalized.push(result.value)
        }
        continue
      }

      const result = normalizeBatchImportItem(parsed, `第 ${line.lineNumber} 行`)
      if (!result.ok) return result
      normalized.push(result.value)
      continue
    }

    normalized.push(line.text)
  }

  return normalizeBatchImportArray(normalized)
}

export function normalizeBatchImportCredentials(text: string): BatchImportCredentialsNormalization {
  const trimmed = text.trim()
  if (!trimmed) {
    return { ok: false, message: '请输入凭据数据' }
  }

  const firstChar = trimmed[0]
  if (firstChar === '[') {
    try {
      const parsed: unknown = JSON.parse(trimmed)
      if (!Array.isArray(parsed)) {
        return { ok: false, message: 'JSON 批量凭据必须是数组' }
      }
      return normalizeBatchImportArray(parsed)
    } catch (error) {
      return { ok: false, message: `JSON 数组格式无效: ${jsonParseErrorMessage(error)}` }
    }
  }

  if (firstChar === '{') {
    try {
      const parsed: unknown = JSON.parse(trimmed)
      if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
        return { ok: true, isBatch: false, credentials: trimmed }
      }
      return { ok: false, message: '单条 JSON 凭据必须是对象' }
    } catch (error) {
      const lines = getImportCredentialLines(trimmed)
      if (lines.length > 1) {
        return parseImportCredentialLines(lines)
      }
      return { ok: false, message: `JSON 格式无效: ${jsonParseErrorMessage(error)}` }
    }
  }

  const lines = getImportCredentialLines(trimmed)
  if (lines.length > 1) {
    return parseImportCredentialLines(lines)
  }

  return { ok: true, isBatch: false, credentials: trimmed }
}

export async function refreshProviderOAuth(keyId: string): Promise<ProviderOAuthCompleteResponse> {
  const resp = await client.post<ProviderOAuthCompleteResponse>(`/api/admin/provider-oauth/keys/${keyId}/refresh`)
  return resp.data
}

// Provider-level OAuth (不需要预先创建 key)

export async function startProviderLevelOAuth(providerId: string): Promise<ProviderOAuthStartResponse> {
  const resp = await client.post<ProviderOAuthStartResponse>(`/api/admin/provider-oauth/providers/${providerId}/start`)
  return resp.data
}

export async function completeProviderLevelOAuth(
  providerId: string,
  data: ProviderOAuthCompleteRequest
): Promise<ProviderOAuthCompleteResponseWithKey> {
  const resp = await client.post<ProviderOAuthCompleteResponseWithKey>(`/api/admin/provider-oauth/providers/${providerId}/complete`, data)
  return resp.data
}

export async function authorizeProviderWithCookie(
  providerId: string,
  data: ProviderCookieAuthorizeRequest
): Promise<ProviderOAuthCompleteResponseWithKey> {
  const resp = await client.post<ProviderOAuthCompleteResponseWithKey>(
    `/api/admin/provider-oauth/providers/${providerId}/cookie-authorize`,
    data,
    { timeout: CLAUDE_COOKIE_AUTHORIZE_TIMEOUT_MS },
  )
  return resp.data
}

export async function startProviderCookieAuthorizeTask(
  providerId: string,
  data: ProviderCookieAuthorizeBatchTaskRequest,
): Promise<OAuthBatchImportTaskStartResponse> {
  const resp = await client.post<OAuthBatchImportTaskStartResponse>(
    `/api/admin/provider-oauth/providers/${providerId}/cookie-authorize/tasks`,
    data,
  )
  return resp.data
}

export async function getProviderCookieAuthorizeTaskStatus(
  providerId: string,
  taskId: string,
): Promise<OAuthBatchImportTaskStatusResponse> {
  const resp = await client.get<OAuthBatchImportTaskStatusResponse>(
    `/api/admin/provider-oauth/providers/${providerId}/cookie-authorize/tasks/${taskId}`,
  )
  return resp.data
}

export async function importProviderRefreshToken(
  providerId: string,
  data: {
    api_key?: string
    apiKey?: string
    token?: string
    auth_token?: string
    authToken?: string
    refresh_token?: string
    access_token?: string
    session_token?: string
    create_agent_identity?: boolean
    password?: string
    expires_at?: number
    name?: string
    proxy_node_id?: string
    email?: string
    account_id?: string
    account_user_id?: string
    plan_type?: string
    pool_tier?: string
    sso_rw_token?: string
    cf_cookies?: string
    cf_clearance?: string
    user_agent?: string
    browser_profile?: string
    user_id?: string
    account_name?: string
    headers?: Record<string, string>
  }
): Promise<ProviderOAuthCompleteResponseWithKey> {
  const resp = await client.post<ProviderOAuthCompleteResponseWithKey>(`/api/admin/provider-oauth/providers/${providerId}/import-refresh-token`, data)
  return resp.data
}

export async function startBatchImportOAuthTask(
  providerId: string,
  credentials: string,
  proxyNodeId?: string
): Promise<OAuthBatchImportTaskStartResponse> {
  const route = containsAgentIdentityImport(credentials)
    ? 'agent-identity-import/tasks'
    : 'batch-import/tasks'
  const resp = await client.post<OAuthBatchImportTaskStartResponse>(`/api/admin/provider-oauth/providers/${providerId}/${route}`, {
    credentials,
    proxy_node_id: proxyNodeId || undefined,
  })
  return resp.data
}

export async function getBatchImportOAuthTaskStatus(
  providerId: string,
  taskId: string
): Promise<OAuthBatchImportTaskStatusResponse> {
  const route = taskId.startsWith('agent-identity-')
    ? 'agent-identity-import/tasks'
    : 'batch-import/tasks'
  const resp = await client.get<OAuthBatchImportTaskStatusResponse>(`/api/admin/provider-oauth/providers/${providerId}/${route}/${taskId}`)
  return resp.data
}

function containsAgentIdentityImport(credentials: string): boolean {
  let parsed: unknown
  try {
    parsed = JSON.parse(credentials)
  } catch {
    return false
  }
  return jsonValueContainsAgentIdentity(parsed)
}

function jsonValueContainsAgentIdentity(value: unknown): boolean {
  if (Array.isArray(value)) return value.some(jsonValueContainsAgentIdentity)
  if (typeof value !== 'object' || value === null) return false
  const root = value as Record<string, unknown>
  const nestedValue = root.agent_identity ?? root.agentIdentity
  const nested = typeof nestedValue === 'object' && nestedValue !== null && !Array.isArray(nestedValue)
    ? nestedValue as Record<string, unknown>
    : undefined
  const authMode = root.auth_mode ?? root.authMode ?? nested?.auth_mode ?? nested?.authMode
  if (typeof authMode === 'string' && authMode.trim().toLowerCase() === 'agentidentity') return true
  const runtimeId = nested?.agent_runtime_id ?? nested?.agentRuntimeId ?? root.agent_runtime_id ?? root.agentRuntimeId
  const privateKey = nested?.agent_private_key ?? nested?.agentPrivateKey ?? root.agent_private_key ?? root.agentPrivateKey
  if (typeof runtimeId === 'string' && runtimeId.trim().length > 0
    && typeof privateKey === 'string' && privateKey.trim().length > 0
  ) return true
  return Object.values(root).some(jsonValueContainsAgentIdentity)
}

// Device Authorization (AWS SSO OIDC)

export interface DeviceAuthorizeRequest {
  start_url?: string
  region?: string
  auth_type?: 'builder_id' | 'identity_center' | 'google' | 'github' | 'browser'
  login_option?: 'google' | 'github' | 'default'
  redirect_uri?: string
  proxy_node_id?: string
}

export interface DeviceAuthorizeResponse {
  session_id: string
  user_code: string
  verification_uri: string
  verification_uri_complete: string
  expires_in: number
  interval: number
  auth_type?: string
  redirect_uri?: string
  callback_required?: boolean
}

export interface DevicePollRequest {
  session_id: string
  callback_url?: string
  token?: string
}

export interface DevicePollResponse {
  status: 'pending' | 'authorized' | 'slow_down' | 'expired' | 'error'
  key_id?: string
  email?: string
  error?: string
  replaced?: boolean
}

export async function startDeviceAuthorize(
  providerId: string,
  data: DeviceAuthorizeRequest
): Promise<DeviceAuthorizeResponse> {
  const resp = await client.post<DeviceAuthorizeResponse>(`/api/admin/provider-oauth/providers/${providerId}/device-authorize`, data)
  return resp.data
}

export async function pollDeviceAuthorize(
  providerId: string,
  data: DevicePollRequest
): Promise<DevicePollResponse> {
  const resp = await client.post<DevicePollResponse>(`/api/admin/provider-oauth/providers/${providerId}/device-poll`, data)
  return resp.data
}
