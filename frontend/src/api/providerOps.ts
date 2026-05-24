/**
 * Provider 操作 API
 *
 * 提供 Provider 扩展操作相关的 API：
 * - 架构管理
 * - 连接管理
 * - 操作执行（余额查询、签到等）
 */

import client from './client'

// ==================== Types ====================

/** 认证类型 */
export type ConnectorAuthType = 'api_key' | 'session_login' | 'oauth' | 'cookie' | 'none'

/** 操作类型 */
export type ProviderActionType =
  | 'query_balance'
  | 'checkin'
  | 'claim_quota'
  | 'refresh_token'
  | 'get_usage'
  | 'get_models'
  | 'custom'

/** 操作状态 */
export type ActionStatus =
  | 'success'
  | 'pending'
  | 'auth_failed'
  | 'auth_expired'
  | 'rate_limited'
  | 'network_error'
  | 'parse_error'
  | 'not_configured'
  | 'not_supported'
  | 'already_done'
  | 'unknown_error'

/** 连接状态 */
export type ConnectorStatus = 'disconnected' | 'connecting' | 'connected' | 'expired' | 'error'

/** 架构信息 */
export interface ArchitectureInfo {
  architecture_id: string
  display_name: string
  description: string
  credentials_schema: Record<string, unknown>
  supported_auth_types: Array<{
    type: string
    display_name: string
    credentials_schema?: Record<string, unknown>
  }>
  supported_actions: Array<{
    type: string
    display_name: string
    description: string
    config_schema: Record<string, unknown>
  }>
  default_connector: string | null
}

/** 连接状态响应 */
export interface ConnectionStatusResponse {
  status: ConnectorStatus
  auth_type: ConnectorAuthType
  connected_at: string | null
  expires_at: string | null
  last_error: string | null
}

/** Provider 操作状态响应 */
export interface ProviderOpsStatusResponse {
  provider_id: string
  is_configured: boolean
  architecture_id: string | null
  connection_status: ConnectionStatusResponse
  enabled_actions: string[]
}

/** 余额信息 */
export interface BalanceInfo {
  total_granted: number | null
  total_used: number | null
  total_available: number | null
  expires_at: string | null
  currency: string
  extra: Record<string, unknown> & {
    // Anyrouter 签到信息
    checkin_success?: boolean | null  // true=成功, false=失败, null=已签到/跳过
    checkin_message?: string
  }
}

/** 签到信息 */
export interface CheckinInfo {
  reward: number | null
  streak_days: number | null
  next_reward: number | null
  message: string | null
  extra: Record<string, unknown>
}

/** 操作结果响应 */
export interface ActionResultResponse {
  status: ActionStatus
  action_type: ProviderActionType
  data: BalanceInfo | CheckinInfo | Record<string, unknown> | null
  message: string | null
  executed_at: string
  response_time_ms: number | null
  cache_ttl_seconds: number
}

/** 连接器配置请求 */
export interface ConnectorConfigRequest {
  auth_type: ConnectorAuthType
  config: Record<string, unknown>
  credentials: Record<string, unknown>
}

/** 操作配置请求 */
export interface ActionConfigRequest {
  enabled: boolean
  config: Record<string, unknown>
}

/** 保存配置请求 */
export interface QuotaAlertConfig {
  enabled: boolean
  threshold_amount: number
  fetch_interval_seconds: number
}

export interface SaveConfigRequest {
  architecture_id: string
  base_url?: string
  connector: ConnectorConfigRequest
  actions: Record<string, ActionConfigRequest>
  schedule: Record<string, string>
  quota_alert?: QuotaAlertConfig
}

/** 连接请求 */
export interface ConnectRequest {
  credentials?: Record<string, unknown>
}

/** 执行操作请求 */
export interface ExecuteActionRequest {
  config?: Record<string, unknown>
}

// ==================== API Functions ====================

const BASE_URL = '/api/admin/provider-ops'

/**
 * 获取所有可用的架构
 */
export async function getArchitectures(): Promise<ArchitectureInfo[]> {
  const response = await client.get<ArchitectureInfo[]>(`${BASE_URL}/architectures`)
  return response.data
}

/**
 * 获取指定架构的详情
 */
export async function getArchitecture(architectureId: string): Promise<ArchitectureInfo> {
  const response = await client.get<ArchitectureInfo>(
    `${BASE_URL}/architectures/${architectureId}`
  )
  return response.data
}

/**
 * 获取 Provider 的操作状态
 */
export async function getProviderOpsStatus(
  providerId: string
): Promise<ProviderOpsStatusResponse> {
  const response = await client.get<ProviderOpsStatusResponse>(
    `${BASE_URL}/providers/${providerId}/status`
  )
  return response.data
}

/** Provider 操作配置响应（脱敏） */
export interface ProviderOpsConfigResponse {
  provider_id: string
  is_configured: boolean
  architecture_id?: string
  base_url?: string
  connector?: {
    auth_type: string
    config: Record<string, unknown>
    credentials: Record<string, unknown>
  }
  quota_alert?: QuotaAlertConfig
}

/**
 * 获取 Provider 的操作配置（脱敏）
 */
export async function getProviderOpsConfig(
  providerId: string
): Promise<ProviderOpsConfigResponse> {
  const response = await client.get<ProviderOpsConfigResponse>(
    `${BASE_URL}/providers/${providerId}/config`
  )
  return response.data
}

/**
 * 保存 Provider 的操作配置
 */
export async function saveProviderOpsConfig(
  providerId: string,
  config: SaveConfigRequest
): Promise<{ success: boolean; message: string }> {
  const response = await client.put<{ success: boolean; message: string }>(
    `${BASE_URL}/providers/${providerId}/config`,
    config
  )
  return response.data
}

/**
 * 删除 Provider 的操作配置
 */
export async function deleteProviderOpsConfig(
  providerId: string
): Promise<{ success: boolean; message: string }> {
  const response = await client.delete<{ success: boolean; message: string }>(
    `${BASE_URL}/providers/${providerId}/config`
  )
  return response.data
}

/**
 * 建立与 Provider 的连接
 */
export async function connectProvider(
  providerId: string,
  request?: ConnectRequest
): Promise<{ success: boolean; message: string }> {
  const response = await client.post<{ success: boolean; message: string }>(
    `${BASE_URL}/providers/${providerId}/connect`,
    request || {}
  )
  return response.data
}

/**
 * 断开与 Provider 的连接
 */
export async function disconnectProvider(
  providerId: string
): Promise<{ success: boolean; message: string }> {
  const response = await client.post<{ success: boolean; message: string }>(
    `${BASE_URL}/providers/${providerId}/disconnect`
  )
  return response.data
}

/**
 * 执行指定操作
 */
export async function executeAction(
  providerId: string,
  actionType: ProviderActionType,
  request?: ExecuteActionRequest
): Promise<ActionResultResponse> {
  const response = await client.post<ActionResultResponse>(
    `${BASE_URL}/providers/${providerId}/actions/${actionType}`,
    request || {}
  )
  return response.data
}

/**
 * 获取余额（优先返回缓存，后台异步刷新）
 * @param providerId Provider ID
 * @param refresh 是否触发后台刷新（默认 true）
 */
export async function getBalance(
  providerId: string,
  refresh: boolean = true
): Promise<ActionResultResponse> {
  const response = await client.get<ActionResultResponse>(
    `${BASE_URL}/providers/${providerId}/balance`,
    { params: { refresh } }
  )
  return response.data
}

/**
 * 立即刷新余额（同步等待结果）
 */
export async function refreshBalance(providerId: string): Promise<ActionResultResponse> {
  const response = await client.post<ActionResultResponse>(
    `${BASE_URL}/providers/${providerId}/balance`
  )
  return response.data
}

/**
 * 签到（快捷方法）
 */
export async function checkin(providerId: string): Promise<ActionResultResponse> {
  const response = await client.post<ActionResultResponse>(
    `${BASE_URL}/providers/${providerId}/checkin`
  )
  return response.data
}

/**
 * 批量查询余额
 */
export async function batchQueryBalance(
  providerIds?: string[]
): Promise<Record<string, ActionResultResponse>> {
  const response = await client.post<Record<string, ActionResultResponse>>(
    `${BASE_URL}/batch/balance`,
    providerIds
  )
  return response.data
}

/** 验证认证请求 */
export interface VerifyAuthRequest {
  architecture_id: string
  base_url: string
  connector: ConnectorConfigRequest
  actions?: Record<string, ActionConfigRequest>
  schedule?: Record<string, string>
}

/** 验证认证响应 */
export interface VerifyAuthResponse {
  success: boolean
  message?: string
  data?: {
    username?: string
    display_name?: string
    email?: string
    quota?: number
    used_quota?: number
    request_count?: number
    extra?: Record<string, unknown>
  }
  updated_credentials?: Record<string, unknown>
}

/**
 * 验证 Provider 认证配置
 * 在保存前测试认证是否有效
 */
export async function verifyProviderAuth(
  providerId: string,
  config: VerifyAuthRequest
): Promise<VerifyAuthResponse> {
  const response = await client.post<VerifyAuthResponse>(
    `${BASE_URL}/providers/${providerId}/verify`,
    config
  )
  return response.data
}
