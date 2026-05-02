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
  data: { refresh_token?: string; access_token?: string; name?: string; proxy_node_id?: string }
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
  proxy_node_id?: string
}

export interface DeviceAuthorizeResponse {
  session_id: string
  user_code: string
  verification_uri: string
  verification_uri_complete: string
  expires_in: number
  interval: number
}

export interface DevicePollRequest {
  session_id: string
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
