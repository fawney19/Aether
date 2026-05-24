import client from '../client'

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

export interface OAuthBatchImportTaskStartResponse {
  task_id: string
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
  const resp = await client.post(`/api/admin/provider-oauth/keys/${keyId}/refresh`)
  return resp.data
}

// Provider-level OAuth (不需要预先创建 key)

export async function startProviderLevelOAuth(providerId: string): Promise<ProviderOAuthStartResponse> {
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/start`)
  return resp.data
}

export async function completeProviderLevelOAuth(
  providerId: string,
  data: ProviderOAuthCompleteRequest
): Promise<ProviderOAuthCompleteResponseWithKey> {
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/complete`, data)
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
  }
): Promise<ProviderOAuthCompleteResponseWithKey> {
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/import-refresh-token`, data)
  return resp.data
}

export async function startBatchImportOAuthTask(
  providerId: string,
  credentials: string,
  proxyNodeId?: string
): Promise<OAuthBatchImportTaskStartResponse> {
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/batch-import/tasks`, {
    credentials,
    proxy_node_id: proxyNodeId || undefined,
  })
  return resp.data
}

export async function getBatchImportOAuthTaskStatus(
  providerId: string,
  taskId: string
): Promise<OAuthBatchImportTaskStatusResponse> {
  const resp = await client.get(`/api/admin/provider-oauth/providers/${providerId}/batch-import/tasks/${taskId}`)
  return resp.data
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
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/device-authorize`, data)
  return resp.data
}

export async function pollDeviceAuthorize(
  providerId: string,
  data: DevicePollRequest
): Promise<DevicePollResponse> {
  const resp = await client.post(`/api/admin/provider-oauth/providers/${providerId}/device-poll`, data)
  return resp.data
}
