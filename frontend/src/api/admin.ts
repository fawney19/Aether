import apiClient from './client'

// 配置导出数据结构
export interface ConfigExportData {
  version: string
  exported_at: string
  global_models: GlobalModelExport[]
  providers: ProviderExport[]
}

// 用户导出数据结构
export interface UsersExportData {
  version: string
  exported_at: string
  users: UserExport[]
  standalone_keys?: StandaloneKeyExport[]
}

export interface UserExport {
  email: string
  username: string
  password_hash: string
  role: string
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
  model_capability_settings?: any
  quota_usd?: number | null
  used_usd?: number
  total_usd?: number
  is_active: boolean
  api_keys: UserApiKeyExport[]
}

export interface UserApiKeyExport {
  key_hash: string
  key_encrypted?: string | null
  name?: string | null
  is_standalone: boolean
  balance_used_usd?: number
  current_balance_usd?: number | null
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
  rate_limit?: number | null  // null = 无限制
  concurrent_limit?: number | null
  force_capabilities?: any
  is_active: boolean
  expires_at?: string | null
  auto_delete_on_expiry?: boolean
  total_requests?: number
  total_cost_usd?: number
}

// 独立余额 Key 导出结构（与 UserApiKeyExport 相同，但不包含 is_standalone）
export type StandaloneKeyExport = Omit<UserApiKeyExport, 'is_standalone'>

export interface GlobalModelExport {
  name: string
  display_name: string
  default_price_per_request?: number | null
  default_tiered_pricing: any
  supported_capabilities?: string[] | null
  config?: any
  is_active: boolean
}

export interface ProviderExport {
  name: string
  description?: string | null
  website?: string | null
  billing_type?: string | null
  monthly_quota_usd?: number | null
  quota_reset_day?: number
  rpm_limit?: number | null
  provider_priority?: number
  is_active: boolean
  concurrent_limit?: number | null
  timeout?: number | null
  max_retries?: number | null
  proxy?: any
  config?: any
  endpoints: EndpointExport[]
  api_keys: ProviderKeyExport[]
  models: ModelExport[]
}

export interface EndpointExport {
  api_format: string
  base_url: string
  headers?: any
  timeout?: number
  max_retries?: number
  is_active: boolean
  custom_path?: string | null
  config?: any
  proxy?: any
}

export interface ProviderKeyExport {
  api_key: string
  name?: string | null
  note?: string | null
  api_formats: string[]
  rate_multiplier?: number
  rate_multipliers?: Record<string, number> | null
  internal_priority?: number
  global_priority?: number | null
  rpm_limit?: number | null
  allowed_models?: any
  capabilities?: any
  cache_ttl_minutes?: number
  max_probe_interval_minutes?: number
  is_active: boolean
}

export interface ModelExport {
  global_model_name: string | null
  provider_model_name: string
  provider_model_mappings?: any
  price_per_request?: number | null
  tiered_pricing?: any
  supports_vision?: boolean | null
  supports_function_calling?: boolean | null
  supports_streaming?: boolean | null
  supports_extended_thinking?: boolean | null
  supports_image_generation?: boolean | null
  is_active: boolean
  config?: any
}

// 邮件模板接口
export interface EmailTemplateInfo {
  type: string
  name: string
  variables: string[]
  subject: string
  html: string
  is_custom: boolean
  default_subject?: string
  default_html?: string
}

export interface EmailTemplatesResponse {
  templates: EmailTemplateInfo[]
}

export interface EmailTemplatePreviewResponse {
  html: string
  variables: Record<string, string>
}

export interface EmailTemplateResetResponse {
  message: string
  template: {
    type: string
    name: string
    subject: string
    html: string
  }
}

// 检查更新响应
export interface CheckUpdateResponse {
  current_version: string
  latest_version: string | null
  has_update: boolean
  release_url: string | null
  release_notes: string | null
  published_at: string | null
  error: string | null
}

// LDAP 配置响应
export interface LdapConfigResponse {
  server_url: string | null
  bind_dn: string | null
  base_dn: string | null
  has_bind_password: boolean
  user_search_filter: string
  username_attr: string
  email_attr: string
  display_name_attr: string
  is_enabled: boolean
  is_exclusive: boolean
  use_starttls: boolean
  connect_timeout: number
}

// LDAP 配置更新请求
export interface LdapConfigUpdateRequest {
  server_url: string
  bind_dn: string
  bind_password?: string
  base_dn: string
  user_search_filter?: string
  username_attr?: string
  email_attr?: string
  display_name_attr?: string
  is_enabled?: boolean
  is_exclusive?: boolean
  use_starttls?: boolean
  connect_timeout?: number
}

// LDAP 连接测试响应
export interface LdapTestResponse {
  success: boolean
  message: string
}

// Provider 模型查询响应
export interface ProviderModelsQueryResponse {
  success: boolean
  data: {
    models: Array<{
      id: string
      object?: string
      created?: number
      owned_by?: string
      display_name?: string
      api_format?: string
    }>
    error?: string
  }
  provider: {
    id: string
    name: string
    display_name: string
  }
}

export interface ConfigImportRequest extends ConfigExportData {
  merge_mode: 'skip' | 'overwrite' | 'error'
}

export interface UsersImportRequest extends UsersExportData {
  merge_mode: 'skip' | 'overwrite' | 'error'
}

export interface UsersImportResponse {
  message: string
  stats: {
    users: { created: number; updated: number; skipped: number }
    api_keys: { created: number; skipped: number }
    standalone_keys?: { created: number; skipped: number }
    errors: string[]
  }
}

export interface ConfigImportResponse {
  message: string
  stats: {
    global_models: { created: number; updated: number; skipped: number }
    providers: { created: number; updated: number; skipped: number }
    endpoints: { created: number; updated: number; skipped: number }
    keys: { created: number; updated: number; skipped: number }
    models: { created: number; updated: number; skipped: number }
    errors: string[]
  }
}

// API密钥管理相关接口定义
export interface AdminApiKey {
  id: string // UUID
  user_id: string // UUID
  user_email?: string
  username?: string
  name?: string
  key_display?: string  // 脱敏后的密钥显示
  is_active: boolean
  is_standalone: boolean  // 是否为独立余额Key
  balance_used_usd?: number  // 已使用余额（仅独立Key）
  current_balance_usd?: number | null  // 当前余额（独立Key预付费模式，null表示无限制）
  total_requests?: number
  total_tokens?: number
  total_cost_usd?: number
  rate_limit?: number | null  // null = 无限制
  allowed_providers?: string[] | null  // 允许的提供商列表
  allowed_api_formats?: string[] | null  // 允许的 API 格式列表
  allowed_models?: string[] | null  // 允许的模型列表
  auto_delete_on_expiry?: boolean  // 过期后是否自动删除
  last_used_at?: string
  expires_at?: string
  created_at: string
  updated_at?: string
}

export interface CreateStandaloneApiKeyRequest {
  name?: string
  allowed_providers?: string[] | null
  allowed_api_formats?: string[] | null
  allowed_models?: string[] | null
  rate_limit?: number | null  // null = 无限制
  expires_at?: string | null  // ISO 日期字符串，如 "2025-12-31"，null = 永不过期
  initial_balance_usd: number  // 初始余额，必须设置
  auto_delete_on_expiry?: boolean  // 过期后是否自动删除
}

export interface AdminApiKeysResponse {
  api_keys: AdminApiKey[]
  total: number
  limit: number
  skip: number
}

export interface ApiKeyToggleResponse {
  id: string // UUID
  is_active: boolean
  message: string
}

// 管理员API密钥管理相关API
export const adminApi = {
  // 获取所有独立余额Keys列表
  async getAllApiKeys(params?: {
    skip?: number
    limit?: number
    is_active?: boolean
  }): Promise<AdminApiKeysResponse> {
    const response = await apiClient.get<AdminApiKeysResponse>('/api/admin/api-keys', {
      params
    })
    return response.data
  },

  // 创建独立余额Key
  async createStandaloneApiKey(data: CreateStandaloneApiKeyRequest): Promise<AdminApiKey & { key: string }> {
    const response = await apiClient.post<AdminApiKey & { key: string }>(
      '/api/admin/api-keys',
      data
    )
    return response.data
  },

  // 更新独立余额Key
  async updateApiKey(keyId: string, data: Partial<CreateStandaloneApiKeyRequest>): Promise<AdminApiKey & { message: string }> {
    const response = await apiClient.put<AdminApiKey & { message: string }>(
      `/api/admin/api-keys/${keyId}`,
      data
    )
    return response.data
  },

  // 切换API密钥状态（启用/禁用）
  async toggleApiKey(keyId: string): Promise<ApiKeyToggleResponse> {
    const response = await apiClient.patch<ApiKeyToggleResponse>(
      `/api/admin/api-keys/${keyId}`
    )
    return response.data
  },

  // 删除API密钥
  async deleteApiKey(keyId: string): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string}>(
      `/api/admin/api-keys/${keyId}`
    )
    return response.data
  },

  // 为独立余额Key调整余额
  async addApiKeyBalance(keyId: string, amountUsd: number): Promise<AdminApiKey & { message: string }> {
    const response = await apiClient.patch<AdminApiKey & { message: string }>(
      `/api/admin/api-keys/${keyId}/balance`,
      { amount_usd: amountUsd }
    )
    return response.data
  },

  // 获取API密钥详情（可选包含完整密钥）
  async getApiKeyDetail(keyId: string, includeKey: boolean = false): Promise<AdminApiKey & { key?: string }> {
    const response = await apiClient.get<AdminApiKey & { key?: string }>(
      `/api/admin/api-keys/${keyId}`,
      { params: { include_key: includeKey } }
    )
    return response.data
  },

  // 获取完整的API密钥（用于复制）- 便捷方法
  async getFullApiKey(keyId: string): Promise<{ key: string }> {
    const response = await apiClient.get<{ key: string }>(
      `/api/admin/api-keys/${keyId}`,
      { params: { include_key: true } }
    )
    return response.data
  },

  // 系统配置相关
  // 获取所有系统配置
  async getAllSystemConfigs(): Promise<any[]> {
    const response = await apiClient.get<any[]>('/api/admin/system/configs')
    return response.data
  },

  // 获取特定系统配置
  async getSystemConfig(key: string): Promise<{ key: string; value: any }> {
    const response = await apiClient.get<{ key: string; value: any }>(
      `/api/admin/system/configs/${key}`
    )
    return response.data
  },

  // 更新系统配置
  async updateSystemConfig(
    key: string,
    value: any,
    description?: string
  ): Promise<{ key: string; value: any; description?: string }> {
    const response = await apiClient.put<{ key: string; value: any; description?: string }>(
      `/api/admin/system/configs/${key}`,
      { value, description }
    )
    return response.data
  },

  // 删除系统配置
  async deleteSystemConfig(key: string): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string }>(
      `/api/admin/system/configs/${key}`
    )
    return response.data
  },

  // 获取系统统计
  async getSystemStats(): Promise<any> {
    const response = await apiClient.get<any>('/api/admin/system/stats')
    return response.data
  },

  // 获取可用的API格式列表
  async getApiFormats(): Promise<{ formats: Array<{ value: string; label: string; default_path: string; aliases: string[] }> }> {
    const response = await apiClient.get<{ formats: Array<{ value: string; label: string; default_path: string; aliases: string[] }> }>(
      '/api/admin/system/api-formats'
    )
    return response.data
  },

  // 导出配置
  async exportConfig(): Promise<ConfigExportData> {
    const response = await apiClient.get<ConfigExportData>('/api/admin/system/config/export')
    return response.data
  },

  // 导入配置
  async importConfig(data: ConfigImportRequest): Promise<ConfigImportResponse> {
    const response = await apiClient.post<ConfigImportResponse>(
      '/api/admin/system/config/import',
      data
    )
    return response.data
  },

  // 导出用户数据
  async exportUsers(): Promise<UsersExportData> {
    const response = await apiClient.get<UsersExportData>('/api/admin/system/users/export')
    return response.data
  },

  // 导入用户数据
  async importUsers(data: UsersImportRequest): Promise<UsersImportResponse> {
    const response = await apiClient.post<UsersImportResponse>(
      '/api/admin/system/users/import',
      data
    )
    return response.data
  },

  // 查询 Provider 可用模型（从上游 API 获取）
  async queryProviderModels(providerId: string, apiKeyId?: string): Promise<ProviderModelsQueryResponse> {
    const response = await apiClient.post<ProviderModelsQueryResponse>(
      '/api/admin/provider-query/models',
      { provider_id: providerId, api_key_id: apiKeyId }
    )
    return response.data
  },

  // 测试 SMTP 连接，支持传入未保存的配置
  async testSmtpConnection(config: Record<string, any> = {}): Promise<{ success: boolean; message: string }> {
    const response = await apiClient.post<{ success: boolean; message: string }>(
      '/api/admin/system/smtp/test',
      config
    )
    return response.data
  },

  // 邮件模板相关
  // 获取所有邮件模板
  async getEmailTemplates(): Promise<EmailTemplatesResponse> {
    const response = await apiClient.get<EmailTemplatesResponse>('/api/admin/system/email/templates')
    return response.data
  },

  // 获取指定类型的邮件模板
  async getEmailTemplate(templateType: string): Promise<EmailTemplateInfo> {
    const response = await apiClient.get<EmailTemplateInfo>(
      `/api/admin/system/email/templates/${templateType}`
    )
    return response.data
  },

  // 更新邮件模板
  async updateEmailTemplate(
    templateType: string,
    data: { subject?: string; html?: string }
  ): Promise<{ message: string }> {
    const response = await apiClient.put<{ message: string }>(
      `/api/admin/system/email/templates/${templateType}`,
      data
    )
    return response.data
  },

  // 预览邮件模板
  async previewEmailTemplate(
    templateType: string,
    data?: { html?: string } & Record<string, string>
  ): Promise<EmailTemplatePreviewResponse> {
    const response = await apiClient.post<EmailTemplatePreviewResponse>(
      `/api/admin/system/email/templates/${templateType}/preview`,
      data || {}
    )
    return response.data
  },

  // 重置邮件模板为默认值
  async resetEmailTemplate(templateType: string): Promise<EmailTemplateResetResponse> {
    const response = await apiClient.post<EmailTemplateResetResponse>(
      `/api/admin/system/email/templates/${templateType}/reset`
    )
    return response.data
  },

  // 获取系统版本信息
  async getSystemVersion(): Promise<{ version: string }> {
    const response = await apiClient.get<{ version: string }>(
      '/api/admin/system/version'
    )
    return response.data
  },

  // 检查系统更新
  async checkUpdate(): Promise<CheckUpdateResponse> {
    const response = await apiClient.get<CheckUpdateResponse>(
      '/api/admin/system/check-update'
    )
    return response.data
  },

  // LDAP 配置相关
  // 获取 LDAP 配置
  async getLdapConfig(): Promise<LdapConfigResponse> {
    const response = await apiClient.get<LdapConfigResponse>('/api/admin/ldap/config')
    return response.data
  },

  // 更新 LDAP 配置
  async updateLdapConfig(config: LdapConfigUpdateRequest): Promise<{ message: string }> {
    const response = await apiClient.put<{ message: string }>(
      '/api/admin/ldap/config',
      config
    )
    return response.data
  },

  // 测试 LDAP 连接
  async testLdapConnection(config: LdapConfigUpdateRequest): Promise<LdapTestResponse> {
    const response = await apiClient.post<LdapTestResponse>('/api/admin/ldap/test', config)
    return response.data
  }
}
