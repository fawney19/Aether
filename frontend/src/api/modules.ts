import apiClient from './client'

export interface ModuleStatus {
  name: string
  available: boolean
  enabled: boolean
  active: boolean
  config_validated: boolean
  config_error: string | null
  display_name: string
  description: string
  category: 'auth' | 'monitoring' | 'security' | 'integration'
  admin_route: string | null
  admin_menu_icon: string | null
  admin_menu_group: string | null
  admin_menu_order: number
  health: 'healthy' | 'degraded' | 'unhealthy' | 'unknown'
}

export interface AuthModuleInfo {
  name: string
  display_name: string
  active: boolean
}

export type ChatPiiRedactionProviderScope = 'all_providers' | 'selected_providers'
export type ChatPiiRedactionTtlSeconds = 300 | 3600
export type ChatPiiRedactionEntity =
  | 'email'
  | 'cn_phone'
  | 'global_phone'
  | 'cn_id'
  | 'payment_card'
  | 'ipv4'
  | 'ipv6'
  | 'api_key'
  | 'access_token'
  | 'secret_key'
  | 'bearer_token'
  | 'jwt'

export interface ChatPiiRedactionConfig {
  enabled: boolean
  provider_scope: ChatPiiRedactionProviderScope
  entities: ChatPiiRedactionEntity[]
  cache_ttl_seconds: ChatPiiRedactionTtlSeconds
  inject_model_instruction: boolean
}

const CHAT_PII_REDACTION_ENTITIES: ChatPiiRedactionEntity[] = [
  'email',
  'cn_phone',
  'global_phone',
  'cn_id',
  'payment_card',
  'ipv4',
  'ipv6',
  'api_key',
  'access_token',
  'secret_key',
  'bearer_token',
  'jwt',
]

const CHAT_PII_REDACTION_CONFIG_KEYS = {
  enabled: 'module.chat_pii_redaction.enabled',
  provider_scope: 'module.chat_pii_redaction.provider_scope',
  entities: 'module.chat_pii_redaction.entities',
  cache_ttl_seconds: 'module.chat_pii_redaction.cache_ttl_seconds',
  inject_model_instruction: 'module.chat_pii_redaction.inject_model_instruction',
} as const

const CHAT_PII_REDACTION_DEFAULT_CONFIG: ChatPiiRedactionConfig = {
  enabled: false,
  provider_scope: 'selected_providers',
  entities: [...CHAT_PII_REDACTION_ENTITIES],
  cache_ttl_seconds: 300,
  inject_model_instruction: true,
}

function isChatPiiRedactionEntity(value: unknown): value is ChatPiiRedactionEntity {
  return typeof value === 'string' && CHAT_PII_REDACTION_ENTITIES.includes(value as ChatPiiRedactionEntity)
}

function normalizeChatPiiRedactionEntities(value: unknown): ChatPiiRedactionEntity[] {
  if (!Array.isArray(value)) return [...CHAT_PII_REDACTION_DEFAULT_CONFIG.entities]
  const unique = new Set<ChatPiiRedactionEntity>()
  for (const item of value) {
    if (isChatPiiRedactionEntity(item)) unique.add(item)
  }
  return CHAT_PII_REDACTION_ENTITIES.filter((item) => unique.has(item))
}

function normalizeChatPiiRedactionConfig(values: Record<keyof ChatPiiRedactionConfig, unknown>): ChatPiiRedactionConfig {
  return {
    enabled: values.enabled === true,
    provider_scope: values.provider_scope === 'all_providers' ? 'all_providers' : 'selected_providers',
    entities: normalizeChatPiiRedactionEntities(values.entities),
    cache_ttl_seconds: values.cache_ttl_seconds === 3600 ? 3600 : 300,
    inject_model_instruction: values.inject_model_instruction !== false,
  }
}

async function getSystemConfigValue(key: string): Promise<unknown> {
  const response = await apiClient.get<{ key: string; value: unknown }>(`/api/admin/system/configs/${key}`)
  return response.data.value
}

async function updateSystemConfigValue(key: string, value: unknown, description: string) {
  const response = await apiClient.put<{ key: string; value: unknown; description?: string }>(
    `/api/admin/system/configs/${key}`,
    { value, description },
  )
  return response.data.value
}

export const modulesApi = {
  /**
   * 获取所有模块状态（管理员）
   */
  async getAllStatus(): Promise<Record<string, ModuleStatus>> {
    const response = await apiClient.get<Record<string, ModuleStatus>>(
      '/api/admin/modules/status'
    )
    return response.data
  },

  /**
   * 获取单个模块状态（管理员）
   */
  async getStatus(moduleName: string): Promise<ModuleStatus> {
    const response = await apiClient.get<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}`
    )
    return response.data
  },

  /**
   * 设置模块启用状态（管理员）
   */
  async setEnabled(moduleName: string, enabled: boolean): Promise<ModuleStatus> {
    const response = await apiClient.put<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}/enabled`,
      { enabled }
    )
    return response.data
  },

  async getChatPiiRedactionConfig(): Promise<ChatPiiRedactionConfig> {
    const [enabled, providerScope, entities, cacheTtlSeconds, injectModelInstruction] = await Promise.all([
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.enabled),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.provider_scope),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.entities),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.inject_model_instruction),
    ])

    return normalizeChatPiiRedactionConfig({
      enabled,
      provider_scope: providerScope,
      entities,
      cache_ttl_seconds: cacheTtlSeconds,
      inject_model_instruction: injectModelInstruction,
    })
  },

  async updateChatPiiRedactionConfig(config: ChatPiiRedactionConfig): Promise<ChatPiiRedactionConfig> {
    const [enabled, providerScope, entities, cacheTtlSeconds, injectModelInstruction] = await Promise.all([
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.enabled, config.enabled, '敏感信息替换保护总开关'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.provider_scope, config.provider_scope, '敏感信息替换保护启用范围'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.entities, config.entities, '敏感信息替换保护检测类型'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds, config.cache_ttl_seconds, '敏感信息替换保护缓存 TTL'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.inject_model_instruction, config.inject_model_instruction, '敏感信息替换保护模型提示说明'),
    ])

    return normalizeChatPiiRedactionConfig({
      enabled,
      provider_scope: providerScope,
      entities,
      cache_ttl_seconds: cacheTtlSeconds,
      inject_model_instruction: injectModelInstruction,
    })
  },

  /**
   * 获取认证模块状态（公开接口，供登录页使用）
   */
  async getAuthModulesStatus(): Promise<AuthModuleInfo[]> {
    const response = await apiClient.get<AuthModuleInfo[]>('/api/modules/auth-status')
    return response.data
  },
}
