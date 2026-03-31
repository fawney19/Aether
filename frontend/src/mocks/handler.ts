/**
 * Mock API Handler
 * 演示模式的 API 请求拦截和模拟响应
 */

import type { AxiosRequestConfig, AxiosResponse } from 'axios'
import { isDemoMode, DEMO_ACCOUNTS } from '@/config/demo'
import {
  MOCK_ADMIN_USER,
  MOCK_NORMAL_USER,
  MOCK_LOGIN_RESPONSE_ADMIN,
  MOCK_LOGIN_RESPONSE_USER,
  MOCK_ADMIN_PROFILE,
  MOCK_USER_PROFILE,
  MOCK_ALL_USERS,
  MOCK_USER_GROUPS,
  MOCK_MODEL_GROUPS,
  MOCK_USER_API_KEYS,
  MOCK_ADMIN_API_KEYS,
  MOCK_PROVIDERS,
  MOCK_GLOBAL_MODELS,
  MOCK_SYSTEM_CONFIGS,
  MOCK_API_FORMATS
} from './data'

// 当前登录用户的 token（用于判断角色）
let currentUserToken: string | null = null

// 模拟网络延迟
function delay(ms: number = 150): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms + Math.random() * 200))
}

// 创建模拟响应
function createMockResponse<T>(data: T, status: number = 200): AxiosResponse<T> {
  return {
    data,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    headers: {},
    config: {} as AxiosRequestConfig
  }
}

// 判断当前是否为管理员
function isCurrentUserAdmin(): boolean {
  return currentUserToken === 'demo-access-token-admin'
}

// 获取当前用户
function getCurrentUser() {
  return isCurrentUserAdmin() ? MOCK_ADMIN_USER : MOCK_NORMAL_USER
}

// 获取当前用户 Profile
function getCurrentProfile() {
  return isCurrentUserAdmin() ? MOCK_ADMIN_PROFILE : MOCK_USER_PROFILE
}

function cloneNullableList(value: string[] | null | undefined): string[] | null {
  if (value == null) return null
  return [...value]
}

function getMockUserGroupById(groupId: string | null | undefined) {
  if (!groupId) return null
  return MOCK_USER_GROUPS.find(group => group.id === groupId) ?? null
}

function getDefaultMockUserGroup() {
  return MOCK_USER_GROUPS.find(group => group.is_default) ?? MOCK_USER_GROUPS[0] ?? null
}

function getMockModelGroupById(modelGroupId: string | null | undefined) {
  if (!modelGroupId) return null
  return MOCK_MODEL_GROUPS.find(group => group.id === modelGroupId) ?? null
}

function normalizeMockUserGroupBindings(bindings: unknown): NonNullable<(typeof MOCK_USER_GROUPS)[number]['model_group_bindings']> {
  if (!Array.isArray(bindings)) return []
  return bindings
    .map((binding, index) => {
      const raw = (binding || {}) as Record<string, unknown>
      const modelGroupId = String(raw.model_group_id ?? '').trim()
      if (!modelGroupId) return null
      const modelGroup = getMockModelGroupById(modelGroupId)
      return {
        model_group_id: modelGroupId,
        priority: Number(raw.priority ?? (index + 1) * 10),
        is_active: raw.is_active !== false,
        model_group_name: modelGroup?.name ?? null,
        model_group_display_name: modelGroup?.display_name ?? null,
        model_group_is_default: modelGroup?.is_default ?? false,
      }
    })
    .filter(Boolean) as NonNullable<(typeof MOCK_USER_GROUPS)[number]['model_group_bindings']>
}

function syncMockModelGroups() {
  for (const group of MOCK_MODEL_GROUPS) {
    const modelRefs = MOCK_GLOBAL_MODELS
      .filter(model => (model.model_group_ids || []).includes(group.id))
      .map(model => ({
        id: `mgm-${group.id}-${model.id}`,
        global_model_id: model.id,
        model_name: model.name,
        model_display_name: model.display_name,
        is_active: model.is_active,
      }))

    const userGroupRefs = MOCK_USER_GROUPS
      .flatMap((userGroup) =>
        (userGroup.model_group_bindings || [])
          .filter((binding) => binding.model_group_id === group.id)
          .map((binding) => ({
            user_group_id: userGroup.id,
            user_group_name: userGroup.name,
            priority: binding.priority,
            is_active: binding.is_active,
          })),
      )
      .sort((a, b) => a.priority - b.priority)

    group.models = modelRefs
    group.user_groups = userGroupRefs
    group.model_count = modelRefs.length
    group.user_group_count = userGroupRefs.length
  }
}

function syncMockUserAccess(user: (typeof MOCK_ALL_USERS)[number]) {
  if (!user.group_id) {
    user.group_id = getDefaultMockUserGroup()?.id ?? null
  }
  const group = getMockUserGroupById(user.group_id)
  const bindings = (group?.model_group_bindings || [])
    .filter(binding => binding.is_active)
    .sort((a, b) => a.priority - b.priority)
  const linkedModelGroups = bindings
    .map(binding => getMockModelGroupById(binding.model_group_id))
    .filter(Boolean)

  user.group_name = group?.name ?? null
  user.effective_allowed_api_formats = cloneNullableList(group?.allowed_api_formats ?? null)
  const effectiveModels = Array.from(
    new Set(
      linkedModelGroups.flatMap(modelGroup =>
        (modelGroup?.models || []).map(model => model.model_name),
      ),
    ),
  )
  user.effective_allowed_models = effectiveModels.length > 0 ? effectiveModels : null

  const hasInheritedRouting = linkedModelGroups.some(modelGroup => modelGroup?.routing_mode !== 'custom')
  if (hasInheritedRouting) {
    user.effective_allowed_providers = null
  } else {
    const providerIds = Array.from(
      new Set(
        linkedModelGroups.flatMap(modelGroup =>
          (modelGroup?.routes || [])
            .filter(route => route.is_active)
            .map(route => route.provider_id),
        ),
      ),
    )
    user.effective_allowed_providers = providerIds.length > 0 ? providerIds : null
  }
  user.effective_rate_limit = group?.rate_limit ?? null
}

function syncMockUserGroupsAndUsers() {
  syncMockModelGroups()
  for (const user of MOCK_ALL_USERS) {
    syncMockUserAccess(user)
  }

  for (const group of MOCK_USER_GROUPS) {
    group.user_count = MOCK_ALL_USERS.filter(user => user.group_id === group.id).length
  }
}

function getMockModelGroupsList() {
  syncMockModelGroups()
  return MOCK_MODEL_GROUPS.map(group => ({
    id: group.id,
    name: group.name,
    display_name: group.display_name,
    description: group.description ?? null,
    default_user_billing_multiplier: group.default_user_billing_multiplier,
    routing_mode: group.routing_mode,
    is_default: group.is_default,
    is_active: group.is_active,
    sort_order: group.sort_order,
    model_count: group.model_count,
    user_group_count: group.user_group_count,
    created_at: group.created_at,
    updated_at: group.updated_at ?? null,
  }))
}

function getMockModelGroupDetail(groupId: string) {
  syncMockModelGroups()
  return MOCK_MODEL_GROUPS.find(group => group.id === groupId) ?? null
}

function replaceMockModelGroupMemberships(groupId: string, modelIds: string[]) {
  const normalized = Array.from(new Set((modelIds || []).map((modelId: unknown) => String(modelId ?? '').trim()).filter(Boolean)))
  for (const model of MOCK_GLOBAL_MODELS) {
    const existing = new Set(model.model_group_ids || [])
    if (normalized.includes(model.id)) {
      existing.add(groupId)
    } else {
      existing.delete(groupId)
    }
    model.model_group_ids = Array.from(existing)
  }
  syncMockGlobalModelGroups()
}

function syncMockGlobalModelGroups() {
  for (const model of MOCK_GLOBAL_MODELS) {
    const groups = MOCK_MODEL_GROUPS
      .filter(group => (model.model_group_ids || []).includes(group.id))
      .map(group => ({
        id: group.id,
        name: group.name,
        display_name: group.display_name,
        is_default: group.is_default,
      }))
    model.model_groups = groups
  }
  syncMockModelGroups()
}

syncMockGlobalModelGroups()
syncMockUserGroupsAndUsers()

function sanitizePublicModelConfig(config: unknown): Record<string, unknown> | null {
  if (!config || typeof config !== 'object') return null

  const raw = config as Record<string, unknown>
  const allowedKeys = [
    'description',
    'icon_url',
    'streaming',
    'vision',
    'function_calling',
    'extended_thinking',
    'image_generation',
    'structured_output',
    'family',
    'knowledge_cutoff',
    'input_modalities',
    'output_modalities',
    'context_limit',
    'output_limit'
  ]

  const sanitized = Object.fromEntries(
    allowedKeys
      .filter(key => key in raw)
      .map(key => [key, raw[key]])
  )

  return Object.keys(sanitized).length > 0 ? sanitized : null
}

// 检查管理员权限
function requireAdmin() {
  if (!isCurrentUserAdmin()) {
    throw { response: createMockResponse({ detail: '需要管理员权限' }, 403) }
  }
}

// Mock 公告数据
const MOCK_ANNOUNCEMENTS = [
  {
    id: 'ann-001',
    title: '系统升级通知',
    content: '系统将于本周六凌晨 2:00-4:00 进行维护升级，届时服务将暂停访问。',
    type: 'maintenance',
    priority: 100,
    is_pinned: true,
    is_active: true,
    author: { id: 'demo-admin-uuid-0001', username: 'Demo Admin' },
    created_at: '2024-12-01T00:00:00Z',
    updated_at: '2024-12-01T00:00:00Z',
    is_read: false
  },
  {
    id: 'ann-002',
    title: '新模型上线：Claude Sonnet 4',
    content: 'Anthropic 最新模型 Claude Sonnet 4 已上线，支持更长上下文和更强推理能力。',
    type: 'info',
    priority: 50,
    is_pinned: false,
    is_active: true,
    author: { id: 'demo-admin-uuid-0001', username: 'Demo Admin' },
    created_at: '2024-11-28T00:00:00Z',
    updated_at: '2024-11-28T00:00:00Z',
    is_read: true
  }
]

// 生成模拟健康事件
// status: success(绿), failed(红), skipped(黄)
// 无事件的时间段会显示为灰色
function generateHealthEvents(
  count: number,
  successRate: number,
  failRate: number,
  _skipRate: number,
  baseLatency: number,
  latencyVariance: number
) {
  const events = []
  const now = Date.now()
  // 6小时内随机分布事件，留一些空白时段（灰色）
  const timeSpan = 6 * 60 * 60 * 1000
  // skipRate 由 1 - successRate - failRate 隐含计算
  for (let i = 0; i < count; i++) {
    const rand = Math.random()
    let status: string
    let statusCode: number
    if (rand < successRate) {
      status = 'success'
      statusCode = 200
    } else if (rand < successRate + failRate) {
      status = 'failed'
      statusCode = [500, 502, 503, 429, 400][Math.floor(Math.random() * 5)]
    } else {
      status = 'skipped'
      statusCode = 0
    }
    events.push({
      timestamp: new Date(now - Math.random() * timeSpan).toISOString(),
      status,
      status_code: statusCode,
      latency_ms: Math.round(baseLatency + Math.random() * latencyVariance),
      error_type: status === 'failed' ? ['RateLimitError', 'TimeoutError', 'ServerError'][Math.floor(Math.random() * 3)] : undefined
    })
  }
  // 按时间排序
  return events.sort((a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime())
}

// Mock 端点健康数据
// 注意：success_rate 使用 0-1 之间的小数，前端会乘以 100 显示为百分比
// 事件的成功/失败/跳过比例必须与 success_rate 保持一致
// 覆盖所有 API 格式：claude, claude_cli, openai, openai_cli, gemini, gemini_cli
const MOCK_ENDPOINT_STATUS = {
  generated_at: new Date().toISOString(),
  formats: [
    {
      api_format: 'claude:chat',
      api_path: '/v1/messages',
      total_attempts: 2580,
      success_count: 2540,
      failed_count: 30,
      skipped_count: 10,
      success_rate: 0.984,
      provider_count: 2,
      key_count: 4,
      last_event_at: new Date().toISOString(),
      // 98.4% 成功率：successRate=0.984, failRate=0.012, skipRate=0.004
      events: generateHealthEvents(80, 0.984, 0.012, 0.004, 900, 500)
    },
    {
      api_format: 'claude:cli',
      api_path: '/v1/messages',
      total_attempts: 1890,
      success_count: 1780,
      failed_count: 85,
      skipped_count: 25,
      success_rate: 0.942,
      provider_count: 5,
      key_count: 9,
      last_event_at: new Date().toISOString(),
      // 94.2% 成功率：successRate=0.942, failRate=0.045, skipRate=0.013
      events: generateHealthEvents(120, 0.942, 0.045, 0.013, 1200, 800)
    },
    {
      api_format: 'gemini:chat',
      api_path: '/v1beta/models',
      total_attempts: 890,
      success_count: 890,
      failed_count: 0,
      skipped_count: 0,
      success_rate: 1.0,
      provider_count: 3,
      key_count: 3,
      last_event_at: new Date().toISOString(),
      // 100% 成功率：全部成功
      events: generateHealthEvents(45, 1.0, 0, 0, 400, 200)
    },
    {
      api_format: 'gemini:cli',
      api_path: '/v1beta/models',
      total_attempts: 456,
      success_count: 450,
      failed_count: 4,
      skipped_count: 2,
      success_rate: 0.987,
      provider_count: 3,
      key_count: 3,
      last_event_at: new Date().toISOString(),
      // 98.7% 成功率：successRate=0.987, failRate=0.009, skipRate=0.004
      events: generateHealthEvents(25, 0.987, 0.009, 0.004, 500, 300)
    },
    {
      api_format: 'openai:chat',
      api_path: '/v1/chat/completions',
      total_attempts: 1560,
      success_count: 1520,
      failed_count: 35,
      skipped_count: 5,
      success_rate: 0.974,
      provider_count: 1,
      key_count: 2,
      last_event_at: new Date().toISOString(),
      // 97.4% 成功率：successRate=0.974, failRate=0.022, skipRate=0.004
      events: generateHealthEvents(60, 0.974, 0.022, 0.004, 700, 400)
    },
    {
      api_format: 'openai:cli',
      api_path: '/v1/responses',
      total_attempts: 2340,
      success_count: 2200,
      failed_count: 100,
      skipped_count: 40,
      success_rate: 0.940,
      provider_count: 4,
      key_count: 5,
      last_event_at: new Date().toISOString(),
      // 94.0% 成功率：successRate=0.940, failRate=0.043, skipRate=0.017
      events: generateHealthEvents(100, 0.940, 0.043, 0.017, 800, 600)
    }
  ]
}

// 生成活跃热力图数据（最近365天）
function generateActivityHeatmap() {
  const days: Array<{
    date: string
    requests: number
    total_tokens: number
    total_cost: number
    actual_total_cost?: number
  }> = []

  const now = new Date()
  const startDate = new Date(now)
  startDate.setDate(startDate.getDate() - 364) // 365天数据（一年）

  let maxRequests = 0

  // 生成每天的数据
  for (let i = 0; i < 365; i++) {
    const date = new Date(startDate)
    date.setDate(startDate.getDate() + i)
    const dateStr = date.toISOString().split('T')[0]

    // 工作日请求量更高
    const dayOfWeek = date.getDay()
    const isWeekend = dayOfWeek === 0 || dayOfWeek === 6

    // 基础请求量 + 随机波动 + 周末减少
    // 加入一些趋势：越近的日期请求量可能越高
    const trendFactor = 0.7 + (i / 365) * 0.5 // 从0.7到1.2的增长趋势
    const baseRequests = isWeekend ? 40 : 120
    const variance = Math.floor(Math.random() * 80)
    // 有些天可能没有请求（约5%的天数）
    const noActivity = Math.random() < 0.05
    const requests = noActivity ? 0 : Math.round((baseRequests + variance) * trendFactor)

    if (requests > maxRequests) maxRequests = requests

    // 根据请求量计算 tokens 和 cost
    const avgTokensPerRequest = 3000 + Math.floor(Math.random() * 2000)
    const totalTokens = requests * avgTokensPerRequest
    const avgCostPerRequest = 0.02 + Math.random() * 0.03
    const totalCost = Number((requests * avgCostPerRequest).toFixed(2))
    const actualTotalCost = Number((totalCost * 0.8).toFixed(2)) // 实际成本约为 80%

    days.push({
      date: dateStr,
      requests,
      total_tokens: totalTokens,
      total_cost: totalCost,
      actual_total_cost: actualTotalCost
    })
  }

  return {
    start_date: days[0].date,
    end_date: days[days.length - 1].date,
    total_days: days.length,
    max_requests: maxRequests,
    days
  }
}

// 缓存热力图数据（避免每次请求都重新生成）
let cachedHeatmap: ReturnType<typeof generateActivityHeatmap> | null = null
function getActivityHeatmap() {
  if (!cachedHeatmap) {
    cachedHeatmap = generateActivityHeatmap()
  }
  return cachedHeatmap
}

// 生成更真实的使用记录
function generateMockUsageRecords(count: number = 100) {
  const records = []
  const now = Date.now()

  const models = [
    { name: 'claude-sonnet-4-5-20250929', provider: 'anthropic', inputPrice: 3, outputPrice: 15 },
    { name: 'claude-haiku-4-5-20251001', provider: 'anthropic', inputPrice: 1, outputPrice: 5 },
    { name: 'claude-opus-4-5-20251101', provider: 'anthropic', inputPrice: 15, outputPrice: 75 },
    { name: 'gpt-5.1', provider: 'openai', inputPrice: 2.5, outputPrice: 10 },
    { name: 'gpt-5.1-codex', provider: 'openai', inputPrice: 2.5, outputPrice: 10 },
    { name: 'gemini-3-pro-preview', provider: 'google', inputPrice: 2, outputPrice: 12 }
  ]

  const users = [
    { id: 'demo-admin-uuid-0001', username: 'Demo Admin', email: 'admin@demo.aether.ai' },
    { id: 'demo-user-uuid-0002', username: 'Demo User', email: 'user@demo.aether.ai' },
    { id: 'demo-user-uuid-0003', username: 'Alice Chen', email: 'alice@demo.aether.ai' },
    { id: 'demo-user-uuid-0004', username: 'Bob Zhang', email: 'bob@demo.aether.ai' }
  ]

  const apiFormats = ['claude:chat', 'claude:cli', 'openai:chat', 'openai:cli', 'gemini:chat', 'gemini:cli']
  const statusOptions: Array<'completed' | 'failed' | 'streaming'> = ['completed', 'completed', 'completed', 'completed', 'failed', 'streaming']

  for (let i = 0; i < count; i++) {
    const model = models[Math.floor(Math.random() * models.length)]
    const user = users[Math.floor(Math.random() * users.length)]
    const status = statusOptions[Math.floor(Math.random() * statusOptions.length)]

    // 根据模型类型选择 API 格式
    let apiFormat = apiFormats[0]
    if (model.provider === 'anthropic') {
      apiFormat = Math.random() > 0.3 ? 'claude:cli' : 'claude:chat'
    } else if (model.provider === 'openai') {
      apiFormat = Math.random() > 0.3 ? 'openai:cli' : 'openai:chat'
    } else {
      apiFormat = Math.random() > 0.3 ? 'gemini:cli' : 'gemini:chat'
    }

    const inputTokens = 500 + Math.floor(Math.random() * 10000)
    const outputTokens = 200 + Math.floor(Math.random() * 4000)
    const cacheCreation = Math.random() > 0.7 ? Math.floor(Math.random() * 2000) : 0
    const cacheRead = Math.random() > 0.5 ? Math.floor(Math.random() * 5000) : 0
    const totalTokens = inputTokens + outputTokens

    // 计算成本（每百万 token）
    const inputCost = (inputTokens / 1000000) * model.inputPrice
    const outputCost = (outputTokens / 1000000) * model.outputPrice
    const cost = Number((inputCost + outputCost).toFixed(6))
    const actualCost = Number((cost * (0.7 + Math.random() * 0.3)).toFixed(6))

    // 时间分布：最近的记录更密集
    const timeOffset = Math.pow(i / count, 1.5) * 7 * 24 * 60 * 60 * 1000 // 7天内
    const createdAt = new Date(now - timeOffset)

    // 响应时间：根据模型和 token 数量
    const baseResponseTime = model.name.includes('opus') ? 2000 : model.name.includes('haiku') ? 500 : 1000
    const responseTime = status === 'failed' ? null : baseResponseTime + Math.floor(Math.random() * outputTokens * 0.5)

    records.push({
      id: `usage-${String(i + 1).padStart(4, '0')}`,
      user_id: user.id,
      username: user.username,
      user_email: user.email,
      api_key: {
        id: `key-${user.id}-${Math.ceil(Math.random() * 2)}`,
        name: `${user.username} Key ${Math.ceil(Math.random() * 3)}`,
        display: `sk-ae...${String(1000 + Math.floor(Math.random() * 9000))}`
      },
      provider: model.provider,
      api_key_name: `${model.provider}-key-${Math.ceil(Math.random() * 3)}`,
      rate_multiplier: 1.0,
      model: model.name,
      target_model: model.name,
      api_format: apiFormat,
      input_tokens: inputTokens,
      output_tokens: outputTokens,
      cache_creation_input_tokens: cacheCreation,
      cache_read_input_tokens: cacheRead,
      total_tokens: totalTokens,
      cost,
      actual_cost: actualCost,
      response_time_ms: responseTime,
      is_stream: apiFormat.includes(':cli'),
      status_code: status === 'failed' ? [500, 502, 429, 400][Math.floor(Math.random() * 4)] : 200,
      error_message: status === 'failed' ? ['Rate limit exceeded', 'Internal server error', 'Model overloaded'][Math.floor(Math.random() * 3)] : undefined,
      status,
      created_at: createdAt.toISOString(),
      has_fallback: Math.random() > 0.9,
      model_version: model.provider === 'google' ? 'gemini-3-pro-preview-2025-01' : undefined
    })
  }

  return records
}

// 缓存使用记录
let cachedUsageRecords: ReturnType<typeof generateMockUsageRecords> | null = null
function getUsageRecords() {
  if (!cachedUsageRecords) {
    cachedUsageRecords = generateMockUsageRecords(100)
  }
  return cachedUsageRecords
}

function buildMockAnalyticsBreakdownRows(
  dimension: 'model' | 'provider' | 'api_format' | 'api_key' | 'user',
  records: ReturnType<typeof getUsageRecords> = getUsageRecords(),
) {
  const groups = new Map<string, {
    key: string
    label: string
    requests_total: number
    requests_success: number
    requests_error: number
    requests_stream: number
    input_tokens: number
    output_tokens: number
    cache_creation_input_tokens: number
    cache_read_input_tokens: number
    total_tokens: number
    total_cost_usd: number
    actual_total_cost_usd: number
    total_response_time_ms: number
    response_samples: number
  }>()

  for (const record of records) {
    const key = (
      dimension === 'model'
        ? record.model
        : dimension === 'provider'
          ? record.provider
          : dimension === 'api_key'
            ? (record.api_key?.id || 'unknown')
          : dimension === 'api_format'
            ? record.api_format
            : record.user_id
    ) || 'unknown'

    const label = dimension === 'user'
      ? (record.username || record.user_email || key)
      : dimension === 'api_key'
        ? (record.api_key?.name || key)
        : key

    const current = groups.get(key) || {
      key,
      label,
      requests_total: 0,
      requests_success: 0,
      requests_error: 0,
      requests_stream: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_creation_input_tokens: 0,
      cache_read_input_tokens: 0,
      total_tokens: 0,
      total_cost_usd: 0,
      actual_total_cost_usd: 0,
      total_response_time_ms: 0,
      response_samples: 0,
    }

    current.requests_total += 1
    current.requests_success += record.status === 'failed' ? 0 : 1
    current.requests_error += record.status === 'failed' ? 1 : 0
    current.requests_stream += record.is_stream ? 1 : 0
    current.input_tokens += record.input_tokens || 0
    current.output_tokens += record.output_tokens || 0
    current.cache_creation_input_tokens += record.cache_creation_input_tokens || 0
    current.cache_read_input_tokens += record.cache_read_input_tokens || 0
    current.total_tokens += record.total_tokens || 0
    current.total_cost_usd += record.cost || 0
    current.actual_total_cost_usd += record.actual_cost || 0

    if (typeof record.response_time_ms === 'number') {
      current.total_response_time_ms += record.response_time_ms
      current.response_samples += 1
    }

    groups.set(key, current)
  }

  const rows = Array.from(groups.values()).map(group => {
    const inputContextTokens = group.input_tokens + group.cache_read_input_tokens
    const successRate = group.requests_total > 0
      ? Number(((group.requests_success / group.requests_total) * 100).toFixed(2))
      : 0
    const avgResponseTimeMs = group.response_samples > 0
      ? Math.round(group.total_response_time_ms / group.response_samples)
      : 0

    return {
      key: group.key,
      label: group.label,
      requests_total: group.requests_total,
      requests_success: group.requests_success,
      requests_error: group.requests_error,
      requests_stream: group.requests_stream,
      success_rate: successRate,
      input_tokens: group.input_tokens,
      output_tokens: group.output_tokens,
      input_output_total_tokens: group.input_tokens + group.output_tokens,
      cache_creation_input_tokens: group.cache_creation_input_tokens,
      cache_read_input_tokens: group.cache_read_input_tokens,
      input_context_tokens: inputContextTokens,
      total_tokens: group.total_tokens,
      cache_hit_rate: inputContextTokens > 0
        ? Number(((group.cache_read_input_tokens / inputContextTokens) * 100).toFixed(2))
        : 0,
      input_cost_usd: 0,
      output_cost_usd: 0,
      cache_creation_cost_usd: 0,
      cache_read_cost_usd: 0,
      cache_cost_usd: 0,
      request_cost_usd: Number(group.total_cost_usd.toFixed(6)),
      total_cost_usd: Number(group.total_cost_usd.toFixed(6)),
      actual_total_cost_usd: Number(group.actual_total_cost_usd.toFixed(6)),
      actual_cache_cost_usd: 0,
      avg_response_time_ms: avgResponseTimeMs,
      avg_first_byte_time_ms: 0,
      format_conversion_count: 0,
      models_used_count: 1,
      share_of_total_cost: 0,
      share_of_total_tokens: 0,
      share_of_selected_metric: 0,
    }
  })

  const totalCost = rows.reduce((sum, row) => sum + row.total_cost_usd, 0)
  const totalTokens = rows.reduce((sum, row) => sum + row.total_tokens, 0)

  return rows.map(row => ({
    ...row,
    share_of_total_cost: totalCost > 0
      ? Number(((row.total_cost_usd / totalCost) * 100).toFixed(2))
      : 0,
    share_of_total_tokens: totalTokens > 0
      ? Number(((row.total_tokens / totalTokens) * 100).toFixed(2))
      : 0,
  }))
}

function buildMockActiveRequests(
  ids: string[] = [],
  includeAdminFields: boolean = false,
  records: ReturnType<typeof getUsageRecords> = getUsageRecords(),
) {
  const activeRecords = records.filter(record =>
    (record.status === 'streaming' || record.status === 'pending') &&
    (ids.length === 0 || ids.includes(record.id))
  )

  return activeRecords.map(record => {
    const item: Record<string, unknown> = {
      id: record.id,
      status: record.status,
      input_tokens: record.input_tokens,
      output_tokens: record.output_tokens,
      cache_creation_input_tokens: record.cache_creation_input_tokens,
      cache_read_input_tokens: record.cache_read_input_tokens,
      cost: record.cost,
      actual_cost: record.actual_cost,
      rate_multiplier: record.rate_multiplier,
      response_time_ms: record.response_time_ms ?? null,
      first_byte_time_ms: record.first_byte_time_ms ?? null,
      api_format: record.api_format,
      endpoint_api_format: record.endpoint_api_format,
      has_format_conversion: record.has_format_conversion,
      target_model: record.target_model,
    }
    if (includeAdminFields) {
      item.provider = record.provider
      item.api_key_name = record.api_key_name
    }
    return item
  })
}

type MockUsageRecord = ReturnType<typeof getUsageRecords>[number]

interface MockAnalyticsPayload {
  scope?: {
    kind?: 'global' | 'me' | 'user' | 'api_key'
    user_id?: string | null
    api_key_id?: string | null
  }
  time_range?: {
    start_date?: string
    end_date?: string
    preset?: string
    granularity?: 'hour' | 'day' | 'week' | 'month'
  }
  filters?: {
    user_ids?: string[]
    provider_names?: string[]
    models?: string[]
    target_models?: string[]
    api_key_ids?: string[]
    api_formats?: string[]
    request_types?: string[]
    statuses?: string[]
    error_categories?: string[]
    is_stream?: boolean | null
    has_format_conversion?: boolean | null
  }
  search?: {
    text?: string | null
    request_id?: string | null
  }
  pagination?: {
    limit?: number
    offset?: number
  }
  dimension?: 'model' | 'provider' | 'api_format' | 'api_key' | 'user'
  metric?: 'requests_total' | 'total_tokens' | 'total_cost_usd' | 'actual_total_cost_usd'
  entity?: 'user' | 'api_key'
  limit?: number
  user_id?: string | null
  api_key_id?: string | null
  ids?: string[]
  hours?: number
  include_user_info?: boolean
}

function parseMockAnalyticsPayload(config: AxiosRequestConfig): MockAnalyticsPayload {
  if (!config.data) return {}
  try {
    return JSON.parse(config.data) as MockAnalyticsPayload
  } catch {
    return {}
  }
}

function getMockRecordRequestType(record: MockUsageRecord): string {
  return record.is_stream ? 'stream' : 'standard'
}

const MOCK_ERROR_CATEGORY_LABELS: Record<string, string> = {
  rate_limit: '频率限制',
  auth: '认证失败',
  invalid_request: '请求无效',
  not_found: '资源不存在',
  content_filter: '内容过滤',
  context_length: '上下文过长',
  server_error: '服务端错误',
  timeout: '请求超时',
  network: '网络错误',
  cancelled: '已取消',
  unknown: '未知错误',
}

function getMockErrorCategory(record: MockUsageRecord): string | null {
  if (record.status !== 'failed') return null
  const message = (record.error_message || '').toLowerCase()
  if (message.includes('rate')) return 'rate_limit'
  if ((record.status_code || 0) >= 500) return 'server_error'
  if ((record.status_code || 0) >= 400) return 'invalid_request'
  return 'unknown'
}

function getMockErrorCategoryLabel(category: string): string {
  return MOCK_ERROR_CATEGORY_LABELS[category] || category
}

function requireAnalyticsScope(scopeKind?: 'global' | 'me' | 'user' | 'api_key') {
  if (!isCurrentUserAdmin() && scopeKind && scopeKind !== 'me') {
    throw { response: createMockResponse({ detail: 'Only admin can access non-personal analytics scope' }, 403) }
  }
}

function resolveMockTimeRangeBounds(timeRange?: MockAnalyticsPayload['time_range']) {
  const now = new Date()
  let start: Date | null = null
  let endExclusive: Date | null = null

  const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate())
  const addDays = (date: Date, days: number) => {
    const next = new Date(date)
    next.setDate(next.getDate() + days)
    return next
  }

  switch (timeRange?.preset) {
    case 'today': {
      const dayStart = startOfDay(now)
      start = dayStart
      endExclusive = addDays(dayStart, 1)
      break
    }
    case 'last7days': {
      const dayStart = startOfDay(now)
      start = addDays(dayStart, -6)
      endExclusive = addDays(dayStart, 1)
      break
    }
    case 'last30days': {
      const dayStart = startOfDay(now)
      start = addDays(dayStart, -29)
      endExclusive = addDays(dayStart, 1)
      break
    }
    case 'last180days': {
      const dayStart = startOfDay(now)
      start = addDays(dayStart, -179)
      endExclusive = addDays(dayStart, 1)
      break
    }
    case 'last1year': {
      const dayStart = startOfDay(now)
      start = addDays(dayStart, -364)
      endExclusive = addDays(dayStart, 1)
      break
    }
    default: {
      if (timeRange?.start_date) {
        start = new Date(`${timeRange.start_date}T00:00:00`)
      }
      if (timeRange?.end_date) {
        endExclusive = addDays(new Date(`${timeRange.end_date}T00:00:00`), 1)
      }
      break
    }
  }

  return {
    start,
    endExclusive,
    granularity: timeRange?.granularity || 'day',
  }
}

function applyMockAnalyticsScope(records: MockUsageRecord[], payload: MockAnalyticsPayload): MockUsageRecord[] {
  const scope = payload.scope || { kind: 'me' as const }
  requireAnalyticsScope(scope.kind)

  switch (scope.kind) {
    case 'global':
      return records
    case 'user':
      return records.filter(record => record.user_id === scope.user_id)
    case 'api_key': {
      const apiKeyId = scope.api_key_id
      return apiKeyId
        ? records.filter(record => record.api_key?.id === apiKeyId)
        : []
    }
    case 'me':
    default:
      return records.filter(record => record.user_id === getCurrentUser().id)
  }
}

function applyMockAnalyticsTimeRange(records: MockUsageRecord[], payload: MockAnalyticsPayload): MockUsageRecord[] {
  const { start, endExclusive } = resolveMockTimeRangeBounds(payload.time_range)
  return records.filter(record => {
    const createdAt = new Date(record.created_at)
    if (start && createdAt < start) return false
    if (endExclusive && createdAt >= endExclusive) return false
    return true
  })
}

function applyMockAnalyticsFilters(records: MockUsageRecord[], payload: MockAnalyticsPayload): MockUsageRecord[] {
  const filters = payload.filters || {}
  return records.filter(record => {
    if (filters.user_ids?.length && !filters.user_ids.includes(record.user_id)) return false
    if (filters.provider_names?.length && !filters.provider_names.includes(record.provider)) return false
    if (filters.models?.length && !filters.models.includes(record.model)) return false
    if (filters.target_models?.length && !filters.target_models.includes(record.target_model || '')) return false
    if (filters.api_key_ids?.length && !filters.api_key_ids.includes(record.api_key?.id || '')) return false
    if (filters.api_formats?.length && !filters.api_formats.includes(record.api_format || '')) return false
    if (filters.request_types?.length && !filters.request_types.includes(getMockRecordRequestType(record))) return false
    if (filters.statuses?.length && !filters.statuses.includes(record.status)) return false
    if (filters.error_categories?.length && !filters.error_categories.includes(getMockErrorCategory(record) || '')) return false
    if (typeof filters.is_stream === 'boolean' && record.is_stream !== filters.is_stream) return false
    if (typeof filters.has_format_conversion === 'boolean') {
      const hasFormatConversion = record.has_format_conversion === true
      if (hasFormatConversion !== filters.has_format_conversion) return false
    }
    return true
  })
}

function applyMockAnalyticsSearch(records: MockUsageRecord[], payload: MockAnalyticsPayload): MockUsageRecord[] {
  const search = payload.search
  if (!search?.text && !search?.request_id) return records

  return records.filter(record => {
    if (search.request_id) {
      const normalized = search.request_id.trim()
      if (normalized && normalized !== record.id && normalized !== `req_${record.id}`) {
        return false
      }
    }

    if (!search.text?.trim()) return true
    const keywords = search.text.toLowerCase().split(/\s+/).filter(Boolean)
    const haystacks = [
      record.id,
      `req_${record.id}`,
      record.username,
      record.user_email,
      record.api_key?.name,
      record.provider,
      record.api_key_name,
      record.model,
      record.target_model,
      record.api_format,
      record.error_message,
    ]
      .filter(Boolean)
      .map(value => String(value).toLowerCase())

    return keywords.every(keyword => haystacks.some(field => field.includes(keyword)))
  })
}

function getMockAnalyticsRecords(payload: MockAnalyticsPayload): MockUsageRecord[] {
  let records = getUsageRecords()
  records = applyMockAnalyticsScope(records, payload)
  records = applyMockAnalyticsTimeRange(records, payload)
  records = applyMockAnalyticsFilters(records, payload)
  records = applyMockAnalyticsSearch(records, payload)
  return records
}

function roundNumber(value: number, digits: number = 6): number {
  return Number(value.toFixed(digits))
}

function percentile(values: number[], p: number): number | null {
  if (!values.length) return null
  const sorted = [...values].sort((a, b) => a - b)
  const index = (sorted.length - 1) * p
  const lower = Math.floor(index)
  const upper = Math.ceil(index)
  if (lower === upper) return Math.round(sorted[lower] * 100) / 100
  const weight = index - lower
  return Math.round((sorted[lower] * (1 - weight) + sorted[upper] * weight) * 100) / 100
}

function buildMockAnalyticsSummary(records: MockUsageRecord[]) {
  const requestsTotal = records.length
  const requestsSuccess = records.filter(record => record.status !== 'failed').length
  const requestsError = records.filter(record => record.status === 'failed').length
  const requestsStream = records.filter(record => record.is_stream).length
  const inputTokens = records.reduce((sum, record) => sum + (record.input_tokens || 0), 0)
  const outputTokens = records.reduce((sum, record) => sum + (record.output_tokens || 0), 0)
  const cacheCreationTokens = records.reduce((sum, record) => sum + (record.cache_creation_input_tokens || 0), 0)
  const cacheReadTokens = records.reduce((sum, record) => sum + (record.cache_read_input_tokens || 0), 0)
  const totalTokens = records.reduce((sum, record) => sum + (record.total_tokens || 0), 0)
  const totalCost = records.reduce((sum, record) => sum + (record.cost || 0), 0)
  const actualTotalCost = records.reduce((sum, record) => sum + (record.actual_cost || 0), 0)
  const responseTimes = records
    .map(record => typeof record.response_time_ms === 'number' ? record.response_time_ms : null)
    .filter((value): value is number => value !== null)
  const firstByteTimes = responseTimes.map(value => Math.round(value * 0.18))
  const inputContextTokens = inputTokens + cacheReadTokens

  return {
    requests_total: requestsTotal,
    requests_success: requestsSuccess,
    requests_error: requestsError,
    requests_stream: requestsStream,
    success_rate: requestsTotal > 0 ? roundNumber((requestsSuccess / requestsTotal) * 100, 2) : 0,
    input_tokens: inputTokens,
    output_tokens: outputTokens,
    input_output_total_tokens: inputTokens + outputTokens,
    cache_creation_input_tokens: cacheCreationTokens,
    cache_read_input_tokens: cacheReadTokens,
    input_context_tokens: inputContextTokens,
    total_tokens: totalTokens,
    cache_hit_rate: inputContextTokens > 0 ? roundNumber((cacheReadTokens / inputContextTokens) * 100, 2) : 0,
    input_cost_usd: 0,
    output_cost_usd: 0,
    cache_creation_cost_usd: 0,
    cache_read_cost_usd: 0,
    cache_cost_usd: 0,
    request_cost_usd: roundNumber(totalCost),
    total_cost_usd: roundNumber(totalCost),
    actual_total_cost_usd: roundNumber(actualTotalCost),
    actual_cache_cost_usd: 0,
    avg_response_time_ms: responseTimes.length
      ? roundNumber(responseTimes.reduce((sum, value) => sum + value, 0) / responseTimes.length, 2)
      : 0,
    avg_first_byte_time_ms: firstByteTimes.length
      ? roundNumber(firstByteTimes.reduce((sum, value) => sum + value, 0) / firstByteTimes.length, 2)
      : 0,
    format_conversion_count: records.filter(record => record.has_format_conversion === true).length,
    models_used_count: new Set(records.map(record => record.model)).size,
  }
}

function buildMockAnalyticsRecord(record: MockUsageRecord, includeAdminFields: boolean) {
  return {
    id: record.id,
    request_id: `req_${record.id}`,
    created_at: record.created_at,
    user_id: record.user_id,
    username: record.username,
    api_key_id: record.api_key?.id || null,
    api_key_name: record.api_key?.name || null,
    provider_api_key_name: includeAdminFields ? record.api_key_name || null : null,
    provider_name: includeAdminFields ? record.provider : null,
    model: record.model,
    target_model: record.target_model || null,
    api_format: record.api_format || null,
    request_type: getMockRecordRequestType(record),
    status: record.status,
    billing_status: record.status === 'failed' ? 'failed' : 'billed',
    is_stream: record.is_stream,
    has_format_conversion: record.has_format_conversion ?? false,
    has_fallback: record.has_fallback ?? false,
    has_retry: false,
    status_code: record.status_code ?? null,
    error_message: record.error_message || null,
    error_category: getMockErrorCategory(record),
    response_time_ms: record.response_time_ms ?? null,
    first_byte_time_ms: typeof record.response_time_ms === 'number' ? Math.round(record.response_time_ms * 0.18) : null,
    input_tokens: record.input_tokens,
    output_tokens: record.output_tokens,
    input_output_total_tokens: record.input_tokens + record.output_tokens,
    cache_creation_input_tokens: record.cache_creation_input_tokens || 0,
    cache_ttl_minutes: record.cache_creation_input_tokens || record.cache_read_input_tokens ? 5 : null,
    cache_read_input_tokens: record.cache_read_input_tokens || 0,
    input_context_tokens: record.input_tokens + (record.cache_read_input_tokens || 0),
    total_tokens: record.total_tokens,
    input_cost_usd: 0,
    output_cost_usd: 0,
    cache_creation_cost_usd: 0,
    cache_read_cost_usd: 0,
    cache_cost_usd: 0,
    request_cost_usd: roundNumber(record.cost || 0),
    total_cost_usd: roundNumber(record.cost || 0),
    actual_total_cost_usd: includeAdminFields ? roundNumber(record.actual_cost || 0) : 0,
    actual_cache_cost_usd: 0,
    rate_multiplier: includeAdminFields ? (record.rate_multiplier || 1) : 1,
  }
}

function buildMockComposition(summary: ReturnType<typeof buildMockAnalyticsSummary>) {
  const tokenTotal = summary.total_tokens || 1
  const costTotal = summary.total_cost_usd || 1
  return {
    token_segments: [
      { key: 'input', value: summary.input_tokens, percentage: roundNumber((summary.input_tokens / tokenTotal) * 100, 2) },
      { key: 'output', value: summary.output_tokens, percentage: roundNumber((summary.output_tokens / tokenTotal) * 100, 2) },
      { key: 'cache_creation', value: summary.cache_creation_input_tokens, percentage: roundNumber((summary.cache_creation_input_tokens / tokenTotal) * 100, 2) },
      { key: 'cache_read', value: summary.cache_read_input_tokens, percentage: roundNumber((summary.cache_read_input_tokens / tokenTotal) * 100, 2) },
    ].filter(segment => segment.value > 0),
    cost_segments: [
      { key: 'request', value: summary.request_cost_usd, percentage: roundNumber((summary.request_cost_usd / costTotal) * 100, 2) },
      { key: 'cache', value: summary.cache_cost_usd, percentage: roundNumber((summary.cache_cost_usd / costTotal) * 100, 2) },
    ].filter(segment => segment.value > 0),
  }
}

function toMockBucketStart(date: Date, granularity: 'hour' | 'day' | 'week' | 'month'): Date {
  const next = new Date(date)
  if (granularity === 'hour') {
    next.setUTCMinutes(0, 0, 0)
    return next
  }
  if (granularity === 'week') {
    next.setUTCHours(0, 0, 0, 0)
    const day = next.getUTCDay()
    const diff = day === 0 ? -6 : 1 - day
    next.setUTCDate(next.getUTCDate() + diff)
    return next
  }
  if (granularity === 'month') {
    next.setUTCDate(1)
    next.setUTCHours(0, 0, 0, 0)
    return next
  }
  next.setUTCHours(0, 0, 0, 0)
  return next
}

function addMockBucketStep(date: Date, granularity: 'hour' | 'day' | 'week' | 'month'): Date {
  const next = new Date(date)
  if (granularity === 'hour') {
    next.setUTCHours(next.getUTCHours() + 1)
    return next
  }
  if (granularity === 'week') {
    next.setUTCDate(next.getUTCDate() + 7)
    return next
  }
  if (granularity === 'month') {
    next.setUTCMonth(next.getUTCMonth() + 1)
    return next
  }
  next.setUTCDate(next.getUTCDate() + 1)
  return next
}

function buildMockTimeseriesBuckets(records: MockUsageRecord[], granularity: 'hour' | 'day' | 'week' | 'month') {
  const groups = new Map<string, MockUsageRecord[]>()

  for (const record of records) {
    const bucketStart = toMockBucketStart(new Date(record.created_at), granularity)
    const key = bucketStart.toISOString()
    const items = groups.get(key) || []
    items.push(record)
    groups.set(key, items)
  }

  return Array.from(groups.entries())
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([bucketStart, items]) => {
      const summary = buildMockAnalyticsSummary(items)
      const start = new Date(bucketStart)
      return {
        bucket_start: bucketStart,
        bucket_end: addMockBucketStep(start, granularity).toISOString(),
        ...summary,
      }
    })
}

function buildMockAnalyticsFilterOptions(records: MockUsageRecord[], includeAdminFields: boolean) {
  const toOptions = (values: Array<{ value: string; label?: string }>) => values.map(item => ({
    value: item.value,
    label: item.label || item.value,
  }))
  const unique = (values: string[]) => Array.from(new Set(values)).sort()

  return {
    providers: includeAdminFields
      ? toOptions(unique(records.map(record => record.provider).filter(Boolean)).map(value => ({ value, label: value })))
      : [],
    models: toOptions(unique(records.map(record => record.model).filter(Boolean)).map(value => ({ value, label: value }))),
    target_models: toOptions(unique(records.map(record => record.target_model).filter(Boolean) as string[]).map(value => ({ value, label: value }))),
    api_formats: toOptions(unique(records.map(record => record.api_format).filter(Boolean) as string[]).map(value => ({ value, label: value }))),
    request_types: toOptions(unique(records.map(record => getMockRecordRequestType(record))).map(value => ({ value, label: value }))),
    error_categories: toOptions(unique(records.map(record => getMockErrorCategory(record)).filter(Boolean) as string[]).map(value => ({
      value,
      label: getMockErrorCategoryLabel(value),
    }))),
    statuses: toOptions(unique(records.map(record => record.status)).map(value => ({ value, label: value }))),
    users: includeAdminFields
      ? toOptions(
        unique(records.map(record => record.user_id).filter(Boolean) as string[])
          .map(value => ({
            value,
            label: records.find(record => record.user_id === value)?.username || value,
          })),
      )
      : undefined,
    api_keys: toOptions(
      unique(records.map(record => record.api_key?.id).filter(Boolean) as string[])
        .map(value => ({
          value,
          label: records.find(record => record.api_key?.id === value)?.api_key?.name || value,
        })),
    ),
  }
}

function buildMockAnalyticsLeaderboardItems(
  records: MockUsageRecord[],
  entity: 'user' | 'api_key',
  metric: 'requests_total' | 'total_tokens' | 'total_cost_usd' | 'actual_total_cost_usd',
  includeAdminFields: boolean,
) {
  const groups = new Map<string, { id: string; label: string; records: MockUsageRecord[] }>()

  for (const record of records) {
    const id = entity === 'user' ? record.user_id : (record.api_key?.id || '')
    if (!id) continue
    const label = entity === 'user'
      ? (record.username || record.user_email || id)
      : (record.api_key?.name || id)
    const current = groups.get(id) || { id, label, records: [] }
    current.records.push(record)
    groups.set(id, current)
  }

  return Array.from(groups.values())
    .map(group => {
      const summary = buildMockAnalyticsSummary(group.records)
      const metricValue = summary[metric]
      return {
        id: group.id,
        label: group.label,
        requests_total: summary.requests_total,
        total_tokens: summary.total_tokens,
        total_cost_usd: summary.total_cost_usd,
        actual_total_cost_usd: includeAdminFields ? summary.actual_total_cost_usd : 0,
        metric_value: typeof metricValue === 'number' ? metricValue : 0,
      }
    })
    .sort((left, right) => right.metric_value - left.metric_value)
    .map((item, index) => ({
      rank: index + 1,
      ...item,
    }))
}

// Mock 映射数据
const MOCK_ALIASES = [
  { id: 'alias-001', source_model: 'claude-4-sonnet', target_global_model_id: 'gm-003', target_global_model_name: 'claude-sonnet-4-5-20250929', target_global_model_display_name: 'Claude Sonnet 4.5', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-002', source_model: 'claude-4-opus', target_global_model_id: 'gm-002', target_global_model_name: 'claude-opus-4-5-20251101', target_global_model_display_name: 'Claude Opus 4.5', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-003', source_model: 'gpt5', target_global_model_id: 'gm-006', target_global_model_name: 'gpt-5.1', target_global_model_display_name: 'GPT-5.1', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-004', source_model: 'gemini-pro', target_global_model_id: 'gm-005', target_global_model_name: 'gemini-3-pro-preview', target_global_model_display_name: 'Gemini 3 Pro Preview', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' }
]

function normalizeApiFormat(apiFormat: string): string {
  return apiFormat.toLowerCase().replace(/_/g, ':')
}

function getMockEndpointExtras(apiFormat: string) {
  const normalizedFormat = normalizeApiFormat(apiFormat)
  const extras: Record<string, unknown> = {}

  if (normalizedFormat === 'claude:chat') {
    extras.header_rules = [
      { action: 'set', key: 'x-app-id', value: 'demo-app' },
      { action: 'rename', from: 'x-client-id', to: 'x-client' },
      { action: 'drop', key: 'x-debug' }
    ]
    extras.body_rules = [
      { action: 'set', path: 'metadata.user_id', value: 'demo-user' },
      { action: 'insert', path: 'messages', index: 0, value: { role: 'system', content: 'You are a helpful assistant.' } },
      { action: 'regex_replace', path: 'messages[0].content', pattern: '\\s+', replacement: ' ', flags: 'm', condition: { path: 'metadata.source', op: 'eq', value: 'internal' } }
    ]
  } else if (normalizedFormat === 'openai:chat') {
    extras.custom_path = '/v1/chat/completions'
    extras.header_rules = [
      { action: 'set', key: 'x-client', value: 'demo' }
    ]
    extras.format_acceptance_config = {
      enabled: true,
      accept_formats: ['openai:chat', 'claude:chat']
    }
    extras.config = { upstream_stream_policy: 'force_stream' }
  } else if (normalizedFormat === 'openai:cli') {
    extras.config = { upstream_stream_policy: 'force_non_stream' }
  } else if (normalizedFormat === 'gemini:chat') {
    extras.custom_path = '/v1beta/models/gemini-3-pro-preview:generateContent'
    extras.body_rules = [
      { action: 'drop', path: 'metadata.debug' }
    ]
  }

  return extras
}


// Mock Endpoint Keys
const MOCK_ENDPOINT_KEYS = [
  { id: 'ekey-001', provider_id: 'provider-001', api_formats: ['claude:chat'], api_key_masked: 'sk-ant...abc1', auth_type: 'api_key', name: 'Primary Key', rate_multiplier: 1.0, internal_priority: 1, health_score: 0.98, consecutive_failures: 0, request_count: 5000, success_count: 4950, error_count: 50, success_rate: 0.99, avg_response_time_ms: 1200, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ekey-002', provider_id: 'provider-001', api_formats: ['claude:chat'], api_key_masked: 'sk-ant...def2', auth_type: 'api_key', name: 'Backup Key', rate_multiplier: 1.0, internal_priority: 2, health_score: 0.95, consecutive_failures: 1, request_count: 2000, success_count: 1950, error_count: 50, success_rate: 0.975, avg_response_time_ms: 1350, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-02-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ekey-003', provider_id: 'provider-002', api_formats: ['openai:chat'], api_key_masked: 'sk-oai...ghi3', auth_type: 'oauth', name: 'OpenAI OAuth', oauth_email: 'oauth-demo@aether.dev', oauth_expires_at: Math.floor(Date.now() / 1000) + 6 * 3600, oauth_plan_type: 'pro', oauth_account_id: 'acct-demo-002', rate_multiplier: 1.0, internal_priority: 1, health_score: 0.97, consecutive_failures: 0, request_count: 3500, success_count: 3450, error_count: 50, success_rate: 0.986, avg_response_time_ms: 900, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-01-15T00:00:00Z', updated_at: new Date().toISOString() }
]

// Mock Endpoints
const MOCK_ENDPOINTS = [
  { id: 'ep-001', provider_id: 'provider-001', provider_name: 'anthropic', api_format: 'claude:chat', base_url: 'https://api.anthropic.com', max_retries: 2, is_active: true, total_keys: 2, active_keys: 2, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString(), ...getMockEndpointExtras('claude:chat') },
  { id: 'ep-002', provider_id: 'provider-002', provider_name: 'openai', api_format: 'openai:chat', base_url: 'https://api.openai.com', max_retries: 2, is_active: true, total_keys: 1, active_keys: 1, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString(), ...getMockEndpointExtras('openai:chat') },
  { id: 'ep-003', provider_id: 'provider-003', provider_name: 'google', api_format: 'gemini:chat', base_url: 'https://generativelanguage.googleapis.com', max_retries: 2, is_active: true, total_keys: 1, active_keys: 1, created_at: '2024-01-15T00:00:00Z', updated_at: new Date().toISOString(), ...getMockEndpointExtras('gemini:chat') }
]

// Mock 能力定义
const MOCK_CAPABILITIES = [
  { name: 'cache_1h', display_name: '1小时缓存', description: '支持1小时prompt缓存', match_mode: 'exclusive', short_name: '1h' },
  { name: 'context_1m', display_name: '1M上下文', description: '支持1M上下文窗口', match_mode: 'compatible', short_name: '1M' }
]

/**
 * Mock API 路由处理器
 */
const mockHandlers: Record<string, (config: AxiosRequestConfig) => Promise<AxiosResponse<unknown>>> = {
  // ========== 认证相关 ==========
  'POST /api/auth/login': async (config) => {
    await delay()
    const body = JSON.parse(config.data || '{}')
    const { email, password } = body

    if (email === DEMO_ACCOUNTS.admin.email && password === DEMO_ACCOUNTS.admin.password) {
      currentUserToken = 'demo-access-token-admin'
      return createMockResponse(MOCK_LOGIN_RESPONSE_ADMIN)
    }

    if (email === DEMO_ACCOUNTS.user.email && password === DEMO_ACCOUNTS.user.password) {
      currentUserToken = 'demo-access-token-user'
      return createMockResponse(MOCK_LOGIN_RESPONSE_USER)
    }

    throw { response: createMockResponse({ detail: '邮箱或密码错误' }, 401) }
  },

  'POST /api/auth/logout': async () => {
    await delay(100)
    currentUserToken = null
    return createMockResponse({ message: '已登出' })
  },

  'POST /api/auth/refresh': async () => {
    await delay(100)
    if (isCurrentUserAdmin()) {
      return createMockResponse(MOCK_LOGIN_RESPONSE_ADMIN)
    }
    return createMockResponse(MOCK_LOGIN_RESPONSE_USER)
  },

  // ========== 用户信息 ==========
  'GET /api/users/me': async () => {
    await delay()
    return createMockResponse(getCurrentUser())
  },

  'PUT /api/users/me': async () => {
    await delay()
    return createMockResponse({ message: '更新成功（演示模式）' })
  },

  'PATCH /api/users/me/password': async () => {
    await delay()
    return createMockResponse({ message: '密码修改成功（演示模式）' })
  },

  'GET /api/users/me/sessions': async () => {
    await delay()
    return createMockResponse([
      {
        id: 'session-current',
        device_label: 'Chrome / macOS',
        device_type: 'desktop',
        browser_name: 'Chrome',
        browser_version: '134.0',
        os_name: 'macOS',
        os_version: '15.3',
        device_model: null,
        ip_address: '192.168.1.100',
        last_seen_at: new Date().toISOString(),
        created_at: new Date(Date.now() - 2 * 24 * 3600 * 1000).toISOString(),
        is_current: true,
        revoked_at: null,
        revoke_reason: null
      },
      {
        id: 'session-other',
        device_label: 'Safari / iPhone',
        device_type: 'mobile',
        browser_name: 'Safari',
        browser_version: '18.0',
        os_name: 'iOS',
        os_version: '18.3',
        device_model: 'iPhone',
        ip_address: '10.0.0.12',
        last_seen_at: new Date(Date.now() - 3 * 3600 * 1000).toISOString(),
        created_at: new Date(Date.now() - 5 * 24 * 3600 * 1000).toISOString(),
        is_current: false,
        revoked_at: null,
        revoke_reason: null
      }
    ])
  },

  'DELETE /api/users/me/sessions/others': async () => {
    await delay()
    return createMockResponse({ message: '其他设备已退出登录（演示模式）', revoked_count: 1 })
  },

  'PATCH /api/users/me/sessions/:sessionId': async (config) => {
    await delay()
    const sessionId = config.url?.split('/').pop() || 'session'
    const body = JSON.parse(config.data || '{}')
    return createMockResponse({
      id: sessionId,
      device_label: body.device_label || '已重命名设备',
      device_type: 'desktop',
      browser_name: 'Chrome',
      browser_version: '134.0',
      os_name: 'macOS',
      os_version: '15.3',
      device_model: null,
      ip_address: '192.168.1.100',
      last_seen_at: new Date().toISOString(),
      created_at: new Date(Date.now() - 2 * 24 * 3600 * 1000).toISOString(),
      is_current: sessionId === 'session-current',
      revoked_at: null,
      revoke_reason: null
    })
  },

  'DELETE /api/users/me/sessions/:sessionId': async () => {
    await delay()
    return createMockResponse({ message: '设备已退出登录（演示模式）' })
  },

  'GET /api/users/me/api-keys': async () => {
    await delay()
    return createMockResponse(MOCK_USER_API_KEYS)
  },

  'POST /api/users/me/api-keys': async (config) => {
    await delay()
    const body = JSON.parse(config.data || '{}')
    const newKey = {
      id: `key-demo-${Date.now()}`,
      key: `sk-aether-demo-${Math.random().toString(36).substring(2, 15)}`,
      key_display: 'sk-ae...demo',
      name: body.name || '新密钥（演示）',
      created_at: new Date().toISOString(),
      is_active: true,
      is_standalone: false,
      total_requests: 0,
      total_cost_usd: 0
    }
    return createMockResponse(newKey)
  },

  'GET /api/users/me/providers': async () => createMockResponse({ detail: 'Not Found' }, 404),

  'GET /api/users/me/endpoint-status': async () => {
    await delay()
    return createMockResponse(MOCK_ENDPOINTS.map(e => ({
      api_format: e.api_format,
      health_score: e.health_score,
      is_active: e.is_active
    })))
  },

  'GET /api/users/me/preferences': async () => {
    await delay()
    return createMockResponse(getCurrentProfile().preferences || { theme: 'auto', language: 'zh-CN' })
  },

  'PUT /api/users/me/preferences': async () => {
    await delay()
    return createMockResponse({ message: '偏好设置已更新（演示模式）' })
  },

  'GET /api/users/me/model-capabilities': async () => {
    await delay()
    return createMockResponse({ model_capability_settings: {} })
  },

  'PUT /api/users/me/model-capabilities': async () => {
    await delay()
    return createMockResponse({ message: '已更新', model_capability_settings: {} })
  },

  // ========== 公告 ==========
  'GET /api/announcements': async () => {
    await delay()
    return createMockResponse({ items: MOCK_ANNOUNCEMENTS, total: MOCK_ANNOUNCEMENTS.length, unread_count: 1 })
  },

  'GET /api/announcements/active': async () => {
    await delay()
    return createMockResponse({ items: MOCK_ANNOUNCEMENTS.filter(a => a.is_active), total: MOCK_ANNOUNCEMENTS.filter(a => a.is_active).length, unread_count: 1 })
  },

  'GET /api/announcements/users/me/unread-count': async () => {
    await delay()
    return createMockResponse({ unread_count: 1 })
  },

  'PATCH /api/announcements': async () => {
    await delay()
    return createMockResponse({ message: '已标记为已读' })
  },

  'POST /api/announcements': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    return createMockResponse({ id: `ann-demo-${Date.now()}`, title: body.title, message: '公告已创建（演示模式）' })
  },

  'POST /api/announcements/read-all': async () => {
    await delay()
    return createMockResponse({ message: '已全部标记为已读' })
  },

  // ========== Admin: 用户管理 ==========
  'GET /api/admin/users': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ALL_USERS)
  },

  'GET /api/admin/users/groups': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_USER_GROUPS)
  },

  'POST /api/admin/users/groups': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const defaultModelGroup = MOCK_MODEL_GROUPS.find(group => group.is_default)
    const newGroup = {
      id: `group-demo-${Date.now()}`,
      name: body.name,
      description: body.description ?? null,
      is_default: false,
      allowed_api_formats: body.allowed_api_formats ?? null,
      model_group_bindings: Array.isArray(body.model_group_bindings) && body.model_group_bindings.length > 0
        ? normalizeMockUserGroupBindings(body.model_group_bindings)
        : (defaultModelGroup
            ? [{
              model_group_id: defaultModelGroup.id,
              priority: 10,
              is_active: true,
              model_group_name: defaultModelGroup.name,
              model_group_display_name: defaultModelGroup.display_name,
              model_group_is_default: defaultModelGroup.is_default,
            }]
            : []),
      rate_limit: body.rate_limit ?? null,
      user_count: 0,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString()
    }
    MOCK_USER_GROUPS.push(newGroup)
    syncMockUserGroupsAndUsers()
    return createMockResponse(newGroup)
  },

  'POST /api/admin/users': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const defaultGroup = getDefaultMockUserGroup()
    const now = new Date().toISOString()
    const newUser = {
      id: `user-demo-${Date.now()}`,
      username: body.username,
      email: body.email ?? '',
      role: body.role || 'user',
      unlimited: Boolean(body.unlimited),
      is_active: true,
      group_id: body.group_id ?? defaultGroup?.id ?? null,
      group_name: null,
      effective_allowed_providers: null,
      effective_allowed_api_formats: null,
      effective_allowed_models: null,
      effective_rate_limit: null,
      created_at: now,
      updated_at: now
    }
    MOCK_ALL_USERS.push(newUser)
    syncMockUserGroupsAndUsers()
    return createMockResponse(newUser)
  },

  'POST /api/admin/users/groups/bindings/batch': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const action = body.action
    const normalizedUserIds = Array.from(
      new Set(
        (Array.isArray(body.user_ids) ? body.user_ids : [])
          .map((userId: unknown) => String(userId ?? '').trim())
          .filter(Boolean)
      )
    )

    if (!normalizedUserIds.length) {
      throw { response: createMockResponse({ detail: '至少需要提供一个用户 ID' }, 400) }
    }
    if (action !== 'bind' && action !== 'unbind') {
      throw { response: createMockResponse({ detail: '不支持的批量分组操作' }, 400) }
    }

    const targetGroup = action === 'bind'
      ? getMockUserGroupById(body.group_id ?? null)
      : null
    const defaultGroup = getDefaultMockUserGroup()

    if (action === 'bind') {
      if (!body.group_id) {
        throw { response: createMockResponse({ detail: '绑定用户到分组时必须提供 group_id' }, 400) }
      }
      if (!targetGroup) {
        throw { response: createMockResponse({ detail: '用户分组不存在' }, 404) }
      }
    }

    const updatedUsers: (typeof MOCK_ALL_USERS)[number][] = []
    let skippedCount = 0
    const now = new Date().toISOString()

    for (const userId of normalizedUserIds) {
      const user = MOCK_ALL_USERS.find(item => item.id === userId)
      if (!user) {
        skippedCount += 1
        continue
      }

      let changed = false

      if (action === 'bind' && targetGroup) {
        if (user.group_id !== targetGroup.id) {
          user.group_id = targetGroup.id
          changed = true
        }
      } else {
        if (body.source_group_id && user.group_id !== body.source_group_id) {
          skippedCount += 1
          continue
        }

        if (!defaultGroup || user.group_id === defaultGroup.id) {
          skippedCount += 1
          continue
        }

        if (user.group_id != null) {
          user.group_id = defaultGroup.id
          changed = true
        }
      }

      if (!changed) {
        skippedCount += 1
        continue
      }

      user.updated_at = now
      updatedUsers.push(user)
    }

    syncMockUserGroupsAndUsers()

    return createMockResponse({
      action,
      group_id: action === 'bind' ? body.group_id ?? null : defaultGroup?.id ?? null,
      source_group_id: body.source_group_id ?? null,
      updated_count: updatedUsers.length,
      skipped_count: skippedCount,
      users: updatedUsers,
    })
  },

  // ========== Admin: API Keys ==========
  'GET /api/admin/api-keys': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ADMIN_API_KEYS)
  },

  'POST /api/admin/api-keys': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const newKey = {
      id: `standalone-demo-${Date.now()}`,
      key: `sk-sa-demo-${Math.random().toString(36).substring(2, 15)}`,
      user_id: 'demo-user-uuid-0002',
      name: body.name || '新独立 Key（演示）',
      key_display: 'sk-sa...demo',
      is_active: true,
      is_standalone: true,
      total_requests: 0,
      created_at: new Date().toISOString()
    }
    return createMockResponse(newKey)
  },

  // ========== Admin: Providers ==========
  'GET /api/admin/providers/summary': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_PROVIDERS)
  },

  'GET /api/admin/providers': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_PROVIDERS)
  },

  'POST /api/admin/providers': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    return createMockResponse({ ...body, id: `provider-demo-${Date.now()}`, created_at: new Date().toISOString() })
  },

  // ========== Admin: Endpoints ==========
  'GET /api/admin/endpoints/providers': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ENDPOINTS)
  },

  'GET /api/admin/endpoints': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ENDPOINTS)
  },

  'GET /api/admin/endpoints/health/summary': async () => {
    await delay()
    requireAdmin()
    return createMockResponse({
      endpoints: { total: 6, active: 5, unhealthy: 1 },
      keys: { total: 15, active: 12, unhealthy: 3 }
    })
  },

  'GET /api/admin/endpoints/health/api-formats': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ENDPOINT_STATUS)
  },

  'GET /api/admin/monitoring/system-status': async () => {
    await delay()
    requireAdmin()

    const records = getUsageRecords()
    const now = new Date()
    const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    const oneHourAgo = new Date(now.getTime() - 60 * 60 * 1000)
    const todayRecords = records.filter(record => new Date(record.created_at) >= startOfToday)
    const recentErrors = records.filter(record => (
      record.status === 'failed' && new Date(record.created_at) >= oneHourAgo
    )).length

    const totalApiKeys = MOCK_ADMIN_API_KEYS.api_keys.length + MOCK_USER_API_KEYS.length
    const activeApiKeys = [
      ...MOCK_ADMIN_API_KEYS.api_keys,
      ...MOCK_USER_API_KEYS,
    ].filter(key => key.is_active).length
    const cpuUsage = Math.min(92, 22 + (todayRecords.length % 38))
    const memoryPercent = Math.min(94, 46 + (todayRecords.length % 28))
    const totalMemory = 32 * 1024 * 1024 * 1024
    const usedMemory = Math.round(totalMemory * (memoryPercent / 100))
    const redisLatency = recentErrors > 0 ? 24 + recentErrors * 6 : 12
    const redisMemoryCeiling = 2 * 1024 * 1024 * 1024
    const redisUsedMemory = Math.round(redisMemoryCeiling * (0.34 + (todayRecords.length % 18) / 100))
    const redisMemoryPercent = roundNumber((redisUsedMemory / redisMemoryCeiling) * 100, 1)
    const postgresUsage = Math.min(96, 28 + ((activeApiKeys + recentErrors) % 6) * 10)
    const postgresStorageTotal = 512 * 1024 * 1024 * 1024
    const postgresStorageFree = Math.round(postgresStorageTotal * (0.18 + ((activeApiKeys + 1) % 5) * 0.05))
    const postgresStorageFreePercent = roundNumber((postgresStorageFree / postgresStorageTotal) * 100, 1)
    const postgresDatabaseSize = Math.round(postgresStorageTotal * 0.29)

    const metricStatus = (value: number, warning: number, danger: number) => (
      value >= danger ? ['danger', '紧张'] : value >= warning ? ['warning', '偏高'] : ['ok', '正常']
    ) as const
    const remainingStatus = (value: number, warning: number, danger: number) => (
      value <= danger ? ['danger', '紧张'] : value <= warning ? ['warning', '偏高'] : ['ok', '正常']
    ) as const

    const [cpuStatus, cpuLabel] = metricStatus(cpuUsage, 70, 85)
    const [memoryStatus, memoryLabel] = metricStatus(memoryPercent, 75, 90)
    const [redisMemoryStatus, redisMemoryLabel] = metricStatus(redisMemoryPercent, 75, 90)
    const [postgresStatus, postgresLabel] = metricStatus(postgresUsage, 70, 90)
    const [postgresStorageStatus, postgresStorageLabel] = remainingStatus(postgresStorageFreePercent, 20, 10)
    const redisStatus = redisLatency >= 80 ? 'warning' : 'ok'
    const redisLabel = redisLatency >= 80 ? '偏高' : '正常'

    return createMockResponse({
      timestamp: now.toISOString(),
      users: {
        total: MOCK_ALL_USERS.length,
        active: MOCK_ALL_USERS.filter(user => user.is_active).length,
      },
      providers: {
        total: MOCK_PROVIDERS.length,
        active: MOCK_PROVIDERS.filter(provider => provider.is_active).length,
      },
      api_keys: {
        total: totalApiKeys,
        active: activeApiKeys,
      },
      today_stats: {
        requests: todayRecords.length,
        tokens: todayRecords.reduce((sum, record) => sum + (record.total_tokens || 0), 0),
        cost_usd: roundNumber(todayRecords.reduce((sum, record) => sum + (record.total_cost_usd || 0), 0), 4),
      },
      recent_errors: recentErrors,
      system_metrics: {
        cpu: {
          status: cpuStatus,
          label: cpuLabel,
          usage_percent: cpuUsage,
          load_percent: roundNumber(Math.max(0, cpuUsage - 8), 1),
          core_count: 8,
        },
        memory: {
          status: memoryStatus,
          label: memoryLabel,
          used_percent: memoryPercent,
          used_bytes: usedMemory,
          available_bytes: totalMemory - usedMemory,
          total_bytes: totalMemory,
        },
        redis: {
          status: redisStatus,
          label: redisLabel,
          latency_ms: redisLatency,
          memory_status: redisMemoryStatus,
          memory_label: redisMemoryLabel,
          used_memory_bytes: redisUsedMemory,
          peak_memory_bytes: Math.round(redisUsedMemory * 1.12),
          maxmemory_bytes: redisMemoryCeiling,
          memory_ceiling_bytes: redisMemoryCeiling,
          available_memory_bytes: redisMemoryCeiling - redisUsedMemory,
          memory_percent: redisMemoryPercent,
          message: redisLatency >= 80 ? '缓存响应偏慢' : null,
        },
        postgres: {
          status: postgresStatus,
          label: postgresLabel,
          usage_percent: postgresUsage,
          checked_out: Math.max(1, Math.round((postgresUsage / 100) * 20)),
          pool_size: 12,
          overflow: Math.max(0, Math.round((postgresUsage - 60) / 12)),
          max_capacity: 20,
          pool_timeout: 30,
          storage_status: postgresStorageStatus,
          storage_label: postgresStorageLabel,
          storage_total_bytes: postgresStorageTotal,
          storage_free_bytes: postgresStorageFree,
          storage_free_percent: postgresStorageFreePercent,
          database_size_bytes: postgresDatabaseSize,
          message: null,
        },
      },
    })
  },

  'GET /api/admin/endpoints/keys': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ENDPOINT_KEYS)
  },

  // ========== Admin: Global Models ==========
  'GET /api/admin/models/groups': async () => {
    await delay()
    requireAdmin()
    return createMockResponse({ model_groups: getMockModelGroupsList(), total: MOCK_MODEL_GROUPS.length })
  },

  'POST /api/admin/models/groups': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const now = new Date().toISOString()
    const newGroup = {
      id: `mg-demo-${Date.now()}`,
      name: body.name,
      display_name: body.display_name || body.name,
      description: body.description ?? null,
      default_user_billing_multiplier: body.default_user_billing_multiplier ?? 1,
      routing_mode: body.routing_mode ?? 'inherit',
      is_default: false,
      is_active: body.is_active !== false,
      sort_order: body.sort_order ?? 100,
      model_count: 0,
      user_group_count: 0,
      models: [],
      routes: Array.isArray(body.routes)
        ? body.routes.map((route: Record<string, unknown>, index: number) => ({
          id: `mgr-demo-${Date.now()}-${index}`,
          provider_id: String(route.provider_id ?? ''),
          provider_name: MOCK_PROVIDERS.find(provider => provider.id === route.provider_id)?.name ?? null,
          provider_api_key_id: route.provider_api_key_id ? String(route.provider_api_key_id) : null,
          provider_api_key_name: route.provider_api_key_id
            ? MOCK_ENDPOINT_KEYS.find(key => key.id === route.provider_api_key_id)?.name ?? null
            : null,
          priority: Number(route.priority ?? 50),
          user_billing_multiplier_override: route.user_billing_multiplier_override == null ? null : Number(route.user_billing_multiplier_override),
          is_active: route.is_active !== false,
          notes: route.notes ? String(route.notes) : null,
        }))
        : [],
      user_groups: [],
      created_at: now,
      updated_at: now,
    }
    MOCK_MODEL_GROUPS.push(newGroup)
    replaceMockModelGroupMemberships(newGroup.id, Array.isArray(body.model_ids) ? body.model_ids : [])
    syncMockUserGroupsAndUsers()
    return createMockResponse(getMockModelGroupDetail(newGroup.id))
  },

  'GET /api/admin/models/global': async () => {
    await delay()
    requireAdmin()
    syncMockGlobalModelGroups()
    return createMockResponse({ models: MOCK_GLOBAL_MODELS, total: MOCK_GLOBAL_MODELS.length })
  },

  'POST /api/admin/models/global': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const now = new Date().toISOString()
    const newModel = {
      ...body,
      id: `gm-demo-${Date.now()}`,
      model_group_ids: Array.isArray(body.model_group_ids) ? body.model_group_ids : [],
      model_groups: [],
      created_at: now,
      updated_at: now,
    }
    MOCK_GLOBAL_MODELS.unshift(newModel)
    syncMockGlobalModelGroups()
    return createMockResponse(newModel)
  },

  // ========== Admin: Model Mappings / Aliases ==========
  'GET /api/admin/models/mappings': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ALIASES)
  },

  'POST /api/admin/models/mappings': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    return createMockResponse({ ...body, id: `alias-demo-${Date.now()}`, created_at: new Date().toISOString(), updated_at: new Date().toISOString() })
  },

  'POST /api/analytics/overview': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const records = getMockAnalyticsRecords(body)
    const includeAdminFields = isCurrentUserAdmin()
    const summary = buildMockAnalyticsSummary(records)
    const responseSummary = {
      ...summary,
      actual_total_cost_usd: includeAdminFields ? summary.actual_total_cost_usd : 0,
      actual_cache_cost_usd: 0,
    }

    return createMockResponse({
      query_context: {
        scope: body.scope || { kind: 'me' },
        time_range: body.time_range || { preset: 'last30days' },
      },
      summary: responseSummary,
      composition: buildMockComposition(responseSummary),
    })
  },

  'POST /api/analytics/timeseries': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const records = getMockAnalyticsRecords(body)
    const includeAdminFields = isCurrentUserAdmin()
    const { granularity } = resolveMockTimeRangeBounds(body.time_range)
    const buckets = buildMockTimeseriesBuckets(records, granularity)
      .map(bucket => ({
        ...bucket,
        actual_total_cost_usd: includeAdminFields ? bucket.actual_total_cost_usd : 0,
        actual_cache_cost_usd: 0,
      }))

    return createMockResponse({ buckets })
  },

  'POST /api/analytics/breakdown': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const dimension = body.dimension || 'model'
    const limit = Number(body.limit) || 50

    if (!isCurrentUserAdmin() && dimension === 'provider') {
      throw { response: createMockResponse({ detail: 'Only admin can access provider breakdown' }, 403) }
    }
    if (!isCurrentUserAdmin() && body.metric === 'actual_total_cost_usd') {
      throw { response: createMockResponse({ detail: 'Only admin can access actual cost breakdown' }, 403) }
    }

    if (!['model', 'provider', 'api_format', 'api_key', 'user'].includes(dimension)) {
      return createMockResponse({ dimension, metric: body.metric || 'total_cost_usd', rows: [] })
    }

    const metric = body.metric || 'total_cost_usd'
    const rows = buildMockAnalyticsBreakdownRows(
      dimension as 'model' | 'provider' | 'api_format' | 'api_key' | 'user',
      getMockAnalyticsRecords(body),
    )
      .sort((a, b) => {
        const left = typeof a[metric as keyof typeof a] === 'number' ? a[metric as keyof typeof a] as number : 0
        const right = typeof b[metric as keyof typeof b] === 'number' ? b[metric as keyof typeof b] as number : 0
        return right - left
      })
      .slice(0, limit)
      .map(row => ({
        ...row,
        actual_total_cost_usd: isCurrentUserAdmin() ? row.actual_total_cost_usd : 0,
        actual_cache_cost_usd: 0,
      }))

    return createMockResponse({
      dimension,
      metric,
      rows,
    })
  },

  'POST /api/analytics/records': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const includeAdminFields = isCurrentUserAdmin()
    const records = getMockAnalyticsRecords(body)
      .sort((left, right) => new Date(right.created_at).getTime() - new Date(left.created_at).getTime())
    const limit = Number(body.pagination?.limit) || 100
    const offset = Number(body.pagination?.offset) || 0

    return createMockResponse({
      total: records.length,
      limit,
      offset,
      records: records
        .slice(offset, offset + limit)
        .map(record => buildMockAnalyticsRecord(record, includeAdminFields)),
    })
  },

  'POST /api/analytics/filter-options': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const records = getMockAnalyticsRecords(body)
    return createMockResponse(buildMockAnalyticsFilterOptions(records, isCurrentUserAdmin()))
  },

  'POST /api/analytics/heatmap': async () => {
    await delay()
    const includeAdminFields = isCurrentUserAdmin()
    const heatmap = getActivityHeatmap()
    return createMockResponse({
      ...heatmap,
      days: heatmap.days.map(day => ({
        ...day,
        actual_total_cost: includeAdminFields ? day.actual_total_cost : 0,
      })),
    })
  },

  'POST /api/analytics/active-requests': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const ids = Array.isArray(body.ids)
      ? body.ids.filter((id: unknown): id is string => typeof id === 'string')
      : []

    return createMockResponse({
      requests: buildMockActiveRequests(
        ids,
        isCurrentUserAdmin(),
        applyMockAnalyticsScope(getUsageRecords(), body),
      )
    })
  },

  'POST /api/analytics/interval-timeline': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const hours = Number(body.hours) || 24
    const limit = Number(body.limit) || 5000
    const includeUserInfo = body.include_user_info === true && body.scope?.kind === 'global'
    return createMockResponse(generateIntervalTimelineData(hours, limit, includeUserInfo))
  },

  'POST /api/analytics/leaderboard': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const entity = body.entity || 'user'
    const metric = body.metric || 'total_cost_usd'
    const limit = Number(body.limit) || 20
    const items = buildMockAnalyticsLeaderboardItems(
      getMockAnalyticsRecords(body),
      entity,
      metric,
      isCurrentUserAdmin(),
    ).slice(0, limit)

    return createMockResponse({
      entity,
      metric,
      items,
    })
  },

  'POST /api/analytics/performance': async (config) => {
    await delay()
    const body = parseMockAnalyticsPayload(config)
    const records = getMockAnalyticsRecords(body)
    const responseTimes = records
      .map(record => typeof record.response_time_ms === 'number' ? record.response_time_ms : null)
      .filter((value): value is number => value !== null)
    const firstByteTimes = responseTimes.map(value => Math.round(value * 0.18))
    const dailyBuckets = buildMockTimeseriesBuckets(records, 'day')

    const errorRecords = records.filter(record => record.status === 'failed')
    const errorCategories = Array.from(
      errorRecords.reduce((map, record) => {
        const category = getMockErrorCategory(record) || 'unknown'
        map.set(category, (map.get(category) || 0) + 1)
        return map
      }, new Map<string, number>()),
    ).map(([category, count]) => ({
      category,
      label: getMockErrorCategoryLabel(category),
      count,
    }))

    const errorTrend = dailyBuckets.map(bucket => ({
      date: bucket.bucket_start.slice(0, 10),
      total: records.filter(record =>
        record.status === 'failed' &&
        record.created_at >= bucket.bucket_start &&
        record.created_at < bucket.bucket_end,
      ).length,
    }))

    const providerHealth = isCurrentUserAdmin()
      ? buildMockAnalyticsBreakdownRows('provider', records).map(row => ({
        provider_name: row.label,
        requests_total: row.requests_total,
        success_rate: row.success_rate,
        error_rate: row.requests_total > 0 ? roundNumber((row.requests_error / row.requests_total) * 100, 2) : 0,
        avg_response_time_ms: row.avg_response_time_ms,
        avg_first_byte_time_ms: row.avg_first_byte_time_ms,
      }))
      : []

    return createMockResponse({
      latency: {
        response_time_ms: {
          avg: responseTimes.length
            ? roundNumber(responseTimes.reduce((sum, value) => sum + value, 0) / responseTimes.length, 2)
            : 0,
          p50: percentile(responseTimes, 0.5),
          p90: percentile(responseTimes, 0.9),
          p99: percentile(responseTimes, 0.99),
        },
        first_byte_time_ms: {
          avg: firstByteTimes.length
            ? roundNumber(firstByteTimes.reduce((sum, value) => sum + value, 0) / firstByteTimes.length, 2)
            : 0,
          p50: percentile(firstByteTimes, 0.5),
          p90: percentile(firstByteTimes, 0.9),
          p99: percentile(firstByteTimes, 0.99),
        },
      },
      percentiles: dailyBuckets.map(bucket => ({
        date: bucket.bucket_start.slice(0, 10),
        p50_response_time_ms: bucket.avg_response_time_ms || null,
        p90_response_time_ms: bucket.avg_response_time_ms ? roundNumber(bucket.avg_response_time_ms * 1.35, 2) : null,
        p99_response_time_ms: bucket.avg_response_time_ms ? roundNumber(bucket.avg_response_time_ms * 1.6, 2) : null,
        p50_first_byte_time_ms: bucket.avg_first_byte_time_ms || null,
        p90_first_byte_time_ms: bucket.avg_first_byte_time_ms ? roundNumber(bucket.avg_first_byte_time_ms * 1.35, 2) : null,
        p99_first_byte_time_ms: bucket.avg_first_byte_time_ms ? roundNumber(bucket.avg_first_byte_time_ms * 1.6, 2) : null,
      })),
      errors: {
        total: errorRecords.length,
        rate: records.length > 0 ? roundNumber((errorRecords.length / records.length) * 100, 2) : 0,
        categories: errorCategories,
        trend: errorTrend,
      },
      provider_health: providerHealth,
    })
  },

  'POST /api/analytics/cache-affinity/ttl-analysis': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const hours = Number(body.hours) || 168
    return createMockResponse(generateTTLAnalysisData(hours))
  },

  'POST /api/analytics/cache-affinity/hit-analysis': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const hours = Number(body.hours) || 168
    return createMockResponse(generateCacheHitAnalysisData(hours))
  },

  // ========== Admin: System ==========
  'GET /api/admin/system/configs': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_SYSTEM_CONFIGS)
  },

  'GET /api/admin/system/api-formats': async () => {
    await delay()
    return createMockResponse(MOCK_API_FORMATS)
  },

  'GET /api/admin/system/stats': async () => {
    await delay()
    requireAdmin()
    return createMockResponse({
      total_requests_today: 1234,
      total_requests_month: 45678,
      total_users: 156,
      active_users_today: 28,
      total_cost_today: 45.67,
      total_cost_month: 1234.56,
      uptime_hours: 720,
      cache_hit_rate: 0.35
    })
  },

  // ========== 能力接口 ==========
  'GET /api/capabilities': async () => {
    await delay()
    return createMockResponse({ capabilities: MOCK_CAPABILITIES })
  },

  'GET /api/capabilities/user-configurable': async () => {
    await delay()
    return createMockResponse({ capabilities: MOCK_CAPABILITIES.filter(c => c.match_mode === 'exclusive') })
  },

  // ========== 公开接口 ==========
  'GET /api/public/global-models': async () => {
    await delay()
    return createMockResponse({
      models: MOCK_GLOBAL_MODELS.map(m => ({
        id: m.id,
        name: m.name,
        display_name: m.display_name,
        is_active: m.is_active,
        default_tiered_pricing: m.default_tiered_pricing,
        default_price_per_request: m.default_price_per_request,
        supported_capabilities: m.supported_capabilities,
        config: sanitizePublicModelConfig(m.config)
      })),
      total: MOCK_GLOBAL_MODELS.length
    })
  },

  'GET /api/public/models': async () => {
    await delay()
    return createMockResponse({
      models: MOCK_GLOBAL_MODELS.map(m => ({
        name: m.name,
        display_name: m.display_name,
        description: m.description
      }))
    })
  },

  'GET /api/public/health': async () => {
    await delay(50)
    return createMockResponse({ status: 'healthy', demo_mode: true })
  },

  'GET /api/public/health/api-formats': async () => {
    await delay()
    return createMockResponse({
      generated_at: new Date().toISOString(),
      formats: MOCK_ENDPOINT_STATUS.formats.map(f => ({
        api_format: f.api_format,
        api_path: f.api_path,
        total_attempts: f.total_attempts,
        success_count: f.success_count,
        failed_count: f.failed_count,
        skipped_count: f.skipped_count,
        success_rate: f.success_rate,
        last_event_at: f.last_event_at,
        events: f.events.slice(0, 10)
      }))
    })
  }
}

// 动态路由匹配器 - 支持 :id 形式的参数
interface RouteMatch {
  handler: (config: AxiosRequestConfig, params: Record<string, string>) => Promise<AxiosResponse<unknown>>
  params: Record<string, string>
}

type DynamicHandler = (config: AxiosRequestConfig, params: Record<string, string>) => Promise<AxiosResponse<unknown>>

// 动态路由注册表
const dynamicRoutes: Array<{
  method: string
  pattern: RegExp
  paramNames: string[]
  handler: DynamicHandler
}> = []

/**
 * 注册动态路由
 */
function registerDynamicRoute(
  method: string,
  path: string,
  handler: DynamicHandler
) {
  // 将 :param 形式转换为正则
  const paramNames: string[] = []
  const regexStr = path.replace(/:([^/]+)/g, (_, paramName) => {
    paramNames.push(paramName)
    return '([^/]+)'
  })
  dynamicRoutes.push({
    method: method.toUpperCase(),
    pattern: new RegExp(`^${regexStr}$`),
    paramNames,
    handler
  })
}

/**
 * 匹配动态路由
 */
function matchDynamicRoute(method: string, url: string): RouteMatch | null {
  const cleanUrl = url.split('?')[0]
  const upperMethod = method.toUpperCase()

  for (const route of dynamicRoutes) {
    if (route.method !== upperMethod) continue
    const match = cleanUrl.match(route.pattern)
    if (match) {
      const params: Record<string, string> = {}
      route.paramNames.forEach((name, index) => {
        params[name] = match[index + 1]
      })
      return { handler: route.handler, params }
    }
  }
  return null
}

/**
 * 匹配请求到 handler
 */
function matchHandler(method: string, url: string): ((config: AxiosRequestConfig) => Promise<AxiosResponse<unknown>>) | null {
  // 移除查询参数
  const cleanUrl = url.split('?')[0]
  const upperMethod = method.toUpperCase()

  // 精确匹配
  const exactKey = `${upperMethod} ${cleanUrl}`
  if (mockHandlers[exactKey]) {
    return mockHandlers[exactKey]
  }

  // 动态路由匹配
  const dynamicMatch = matchDynamicRoute(method, url)
  if (dynamicMatch) {
    return (config) => dynamicMatch.handler(config, dynamicMatch.params)
  }

  // 路径前缀匹配（按优先级排序）
  const sortedPatterns = Object.keys(mockHandlers).sort((a, b) => b.length - a.length)

  for (const pattern of sortedPatterns) {
    const [patternMethod, patternPath] = pattern.split(' ')
    if (patternMethod !== upperMethod) continue

    // 检查是否为前缀匹配（用于处理带 ID 的路由）
    if (cleanUrl.startsWith(patternPath) || patternPath === cleanUrl) {
      return mockHandlers[pattern]
    }
  }

  return null
}

/**
 * 处理 Mock 请求
 */
export async function handleMockRequest(config: AxiosRequestConfig): Promise<AxiosResponse<unknown> | null> {
  if (!isDemoMode()) {
    return null
  }

  const method = config.method?.toUpperCase() || 'GET'
  const url = config.url || ''

  // 尝试匹配 handler
  const handler = matchHandler(method, url)

  if (handler) {
    try {
      return await handler(config)
    } catch (error: unknown) {
      if ((error as Record<string, unknown>)?.response) {
        throw error
      }
      // eslint-disable-next-line no-console
      console.error('[Mock] Handler error:', error)
      throw { response: createMockResponse({ detail: '模拟请求处理失败' }, 500) }
    }
  }

  // 未匹配的请求返回默认响应
  // eslint-disable-next-line no-console
  console.warn(`[Mock] Unhandled request: ${method} ${url}`)
  return createMockResponse({ message: '演示模式：该接口暂未模拟', demo_mode: true })
}

/**
 * 设置当前用户 token（供 client 初始化使用）
 */
export function setMockUserToken(token: string | null): void {
  currentUserToken = token
}

/**
 * 获取当前 mock token
 */
export function getMockUserToken(): string | null {
  return currentUserToken
}

// ========== Mock Provider Endpoints 数据 ==========
// 为每个 provider 生成对应的 endpoints
function generateMockEndpointsForProvider(providerId: string) {
  const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
  if (!provider || provider.api_formats.length === 0) return []

  return provider.api_formats.map((format, index) => {
    const normalizedFormat = normalizeApiFormat(format)
    const healthDetail = provider.endpoint_health_details.find(h => h.api_format === format)
    const baseUrl = normalizedFormat.includes('claude') ? 'https://api.anthropic.com' :
      normalizedFormat.includes('openai') ? 'https://api.openai.com' :
        'https://generativelanguage.googleapis.com'
    return {
      id: `ep-${providerId}-${index + 1}`,
      provider_id: providerId,
      provider_name: provider.name,
      api_format: format,
      base_url: baseUrl,
      max_retries: 2,
      is_active: healthDetail?.is_active ?? true,
      total_keys: Math.ceil(Math.random() * 3) + 1,
      active_keys: Math.ceil(Math.random() * 2) + 1,
      created_at: provider.created_at,
      updated_at: new Date().toISOString(),
      ...getMockEndpointExtras(normalizedFormat)
    }
  })
}

// 为 provider 生成 keys（Key 归属 Provider，通过 api_formats 关联）
const PROVIDER_KEYS_CACHE: Record<string, Record<string, unknown>[]> = {}
function generateMockKeysForProvider(providerId: string, count: number = 2) {
  const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
  const formats = provider?.api_formats || []
  const nowSec = Math.floor(Date.now() / 1000)

  return Array.from({ length: count }, (_, i) => {
    const isOAuth = i === 1
    const markInvalid = isOAuth && providerId.endsWith('3')
    const oauthFields = isOAuth ? {
      auth_type: 'oauth',
      oauth_email: 'oauth-demo@aether.dev',
      oauth_expires_at: markInvalid ? null : nowSec + 6 * 3600,
      oauth_invalid_at: markInvalid ? nowSec - 3600 : null,
      oauth_invalid_reason: markInvalid ? '[ACCOUNT_BLOCK] Demo verification required' : null,
      status_snapshot: {
        oauth: { code: 'valid', label: '有效', reason: null, expires_at: nowSec + 6 * 3600, invalid_at: null, requires_reauth: false, expiring_soon: false },
        account: markInvalid
          ? { code: 'account_verification', label: '需要验证', reason: 'Demo verification required', blocked: true, source: 'oauth_invalid', recoverable: false }
          : { code: 'ok', label: null, reason: null, blocked: false, source: null, recoverable: false },
        quota: { code: 'unknown', label: null, reason: null, exhausted: false, usage_ratio: null, updated_at: null, reset_seconds: null, plan_type: null }
      },
      oauth_plan_type: 'pro',
      oauth_account_id: `acct-${providerId}`
    } : { auth_type: 'api_key' }

    return {
      id: `key-${providerId}-${i + 1}`,
      provider_id: providerId,
      api_formats: i === 0 ? formats : formats.slice(0, 1),
      api_key_masked: `sk-***...${Math.random().toString(36).substring(2, 6)}`,
      name: i === 0 ? 'Primary Key' : `Backup Key ${i}`,
      ...oauthFields,
      rate_multiplier: 1.0,
      internal_priority: i + 1,
      health_score: 0.90 + Math.random() * 0.10,
      consecutive_failures: Math.random() > 0.8 ? 1 : 0,
      request_count: 1000 + Math.floor(Math.random() * 5000),
      success_count: 950 + Math.floor(Math.random() * 4800),
      error_count: Math.floor(Math.random() * 100),
      success_rate: 0.95 + Math.random() * 0.04,
      avg_response_time_ms: 800 + Math.floor(Math.random() * 600),
      cache_ttl_minutes: 5,
      max_probe_interval_minutes: 32,
      is_active: true,
      created_at: '2024-01-01T00:00:00Z',
      updated_at: new Date().toISOString()
    }
  })
}

// 为 provider 生成 models
function generateMockModelsForProvider(providerId: string) {
  const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
  if (!provider) return []

  // 基于 provider 的 api_formats 选择合适的模型
  const hasClaude = provider.api_formats.some(f => f.includes('claude'))
  const hasOpenAI = provider.api_formats.some(f => f.includes('openai'))
  const hasGemini = provider.api_formats.some(f => f.includes('gemini'))

  const models: Record<string, unknown>[] = []
  const now = new Date().toISOString()

  if (hasClaude) {
    models.push(
      {
        id: `pm-${providerId}-claude-1`,
        provider_id: providerId,
        global_model_id: 'gm-003',
        provider_model_name: 'claude-sonnet-4-5-20250929',
        global_model_name: 'claude-sonnet-4-5-20250929',
        global_model_display_name: 'claude-sonnet-4-5',
        effective_input_price: 3.0,
        effective_output_price: 15.0,
        effective_supports_vision: true,
        effective_supports_function_calling: true,
        effective_supports_streaming: true,
        effective_supports_extended_thinking: true,
        is_active: true,
        is_available: true,
        created_at: provider.created_at,
        updated_at: now
      },
      {
        id: `pm-${providerId}-claude-2`,
        provider_id: providerId,
        global_model_id: 'gm-001',
        provider_model_name: 'claude-haiku-4-5-20251001',
        global_model_name: 'claude-haiku-4-5-20251001',
        global_model_display_name: 'claude-haiku-4-5',
        effective_input_price: 1.0,
        effective_output_price: 5.0,
        effective_supports_vision: true,
        effective_supports_function_calling: true,
        effective_supports_streaming: true,
        effective_supports_extended_thinking: true,
        is_active: true,
        is_available: true,
        created_at: provider.created_at,
        updated_at: now
      }
    )
  }
  if (hasOpenAI) {
    models.push(
      {
        id: `pm-${providerId}-openai-1`,
        provider_id: providerId,
        global_model_id: 'gm-006',
        provider_model_name: 'gpt-5.1',
        global_model_name: 'gpt-5.1',
        global_model_display_name: 'gpt-5.1',
        effective_input_price: 1.25,
        effective_output_price: 10.0,
        effective_supports_vision: true,
        effective_supports_function_calling: true,
        effective_supports_streaming: true,
        effective_supports_extended_thinking: true,
        is_active: true,
        is_available: true,
        created_at: provider.created_at,
        updated_at: now
      },
      {
        id: `pm-${providerId}-openai-2`,
        provider_id: providerId,
        global_model_id: 'gm-007',
        provider_model_name: 'gpt-5.1-codex',
        global_model_name: 'gpt-5.1-codex',
        global_model_display_name: 'gpt-5.1-codex',
        effective_input_price: 1.25,
        effective_output_price: 10.0,
        effective_supports_vision: true,
        effective_supports_function_calling: true,
        effective_supports_streaming: true,
        effective_supports_extended_thinking: true,
        is_active: true,
        is_available: true,
        created_at: provider.created_at,
        updated_at: now
      }
    )
  }
  if (hasGemini) {
    models.push(
      {
        id: `pm-${providerId}-gemini-1`,
        provider_id: providerId,
        global_model_id: 'gm-005',
        provider_model_name: 'gemini-3-pro-preview',
        global_model_name: 'gemini-3-pro-preview',
        global_model_display_name: 'gemini-3-pro-preview',
        effective_input_price: 2.0,
        effective_output_price: 12.0,
        effective_supports_vision: true,
        effective_supports_function_calling: true,
        effective_supports_streaming: true,
        effective_supports_extended_thinking: true,
        is_active: true,
        is_available: true,
        created_at: provider.created_at,
        updated_at: now
      }
    )
  }

  return models
}

// ========== 注册动态路由 ==========

// Provider 详情
registerDynamicRoute('GET', '/api/admin/providers/:providerId/summary', async (_config, params) => {
  await delay()
  requireAdmin()
  const provider = MOCK_PROVIDERS.find(p => p.id === params.providerId)
  if (!provider) {
    throw { response: createMockResponse({ detail: '提供商不存在' }, 404) }
  }
  return createMockResponse(provider)
})

// Provider 更新
registerDynamicRoute('PATCH', '/api/admin/providers/:providerId', async (config, params) => {
  await delay()
  requireAdmin()
  const provider = MOCK_PROVIDERS.find(p => p.id === params.providerId)
  if (!provider) {
    throw { response: createMockResponse({ detail: '提供商不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ ...provider, ...body, updated_at: new Date().toISOString() })
})

// Provider 删除
registerDynamicRoute('DELETE', '/api/admin/providers/:providerId', async (_config, params) => {
  await delay()
  requireAdmin()
  const provider = MOCK_PROVIDERS.find(p => p.id === params.providerId)
  if (!provider) {
    throw { response: createMockResponse({ detail: '提供商不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// Provider Endpoints 列表
registerDynamicRoute('GET', '/api/admin/endpoints/providers/:providerId/endpoints', async (_config, params) => {
  await delay()
  requireAdmin()
  const endpoints = generateMockEndpointsForProvider(params.providerId)
  return createMockResponse(endpoints)
})

// 创建 Endpoint
registerDynamicRoute('POST', '/api/admin/endpoints/providers/:providerId/endpoints', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({
    id: `ep-demo-${Date.now()}`,
    provider_id: params.providerId,
    ...body,
    created_at: new Date().toISOString()
  })
})

// Endpoint 详情
registerDynamicRoute('GET', '/api/admin/endpoints/:endpointId', async (_config, params) => {
  await delay()
  requireAdmin()
  // 从所有 providers 的 endpoints 中查找
  for (const provider of MOCK_PROVIDERS) {
    const endpoints = generateMockEndpointsForProvider(provider.id)
    const endpoint = endpoints.find(e => e.id === params.endpointId)
    if (endpoint) {
      return createMockResponse(endpoint)
    }
  }
  throw { response: createMockResponse({ detail: '端点不存在' }, 404) }
})

// Endpoint 更新
registerDynamicRoute('PUT', '/api/admin/endpoints/:endpointId', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ id: params.endpointId, ...body, updated_at: new Date().toISOString() })
})

// Endpoint 删除
registerDynamicRoute('DELETE', '/api/admin/endpoints/:endpointId', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: '删除成功（演示模式）', affected_keys_count: 0 })
})

// Provider Keys 列表
registerDynamicRoute('GET', '/api/admin/endpoints/providers/:providerId/keys', async (_config, params) => {
  await delay()
  requireAdmin()
  if (!PROVIDER_KEYS_CACHE[params.providerId]) {
    PROVIDER_KEYS_CACHE[params.providerId] = generateMockKeysForProvider(params.providerId, 2)
  }
  return createMockResponse(PROVIDER_KEYS_CACHE[params.providerId])
})

// 为 Provider 创建 Key
registerDynamicRoute('POST', '/api/admin/endpoints/providers/:providerId/keys', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const apiKeyPlain = body.api_key || 'sk-demo'
  const masked = apiKeyPlain.length >= 12
    ? `${apiKeyPlain.slice(0, 8)}***${apiKeyPlain.slice(-4)}`
    : 'sk-***...demo'

  const newKey = {
    id: `key-demo-${Date.now()}`,
    provider_id: params.providerId,
    api_formats: body.api_formats || [],
    api_key_masked: masked,
    api_key_plain: null,
    auth_type: body.auth_type || 'api_key',
    name: body.name || 'New Key',
    note: body.note,
    rate_multiplier: body.rate_multiplier ?? 1.0,
    rate_multipliers: body.rate_multipliers ?? null,
    internal_priority: body.internal_priority ?? 50,
    global_priority: body.global_priority ?? null,
    rpm_limit: body.rpm_limit ?? null,
    allowed_models: body.allowed_models ?? null,
    capabilities: body.capabilities ?? null,
    cache_ttl_minutes: body.cache_ttl_minutes ?? 5,
    max_probe_interval_minutes: body.max_probe_interval_minutes ?? 32,
    health_score: 1.0,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 0.0,
    avg_response_time_ms: 0.0,
    is_active: true,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  }

  if (!PROVIDER_KEYS_CACHE[params.providerId]) {
    PROVIDER_KEYS_CACHE[params.providerId] = []
  }
  PROVIDER_KEYS_CACHE[params.providerId].push(newKey)
  return createMockResponse(newKey)
})

registerDynamicRoute('POST', '/api/admin/endpoints/providers/:providerId/refresh-quota', async (_config, params) => {
  await delay()
  requireAdmin()
  if (!PROVIDER_KEYS_CACHE[params.providerId]) {
    PROVIDER_KEYS_CACHE[params.providerId] = generateMockKeysForProvider(params.providerId, 2)
  }
  const keys = PROVIDER_KEYS_CACHE[params.providerId] || []
  const results = keys.map(key => ({
    key_id: key.id,
    key_name: key.name || key.id.slice(0, 8),
    status: 'success',
    metadata: { updated_at: new Date().toISOString() }
  }))
  return createMockResponse({
    success: results.length,
    failed: 0,
    total: results.length,
    results
  })
})

registerDynamicRoute('POST', '/api/admin/provider-oauth/keys/:keyId/refresh', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    provider_type: 'codex',
    expires_at: Math.floor(Date.now() / 1000) + 6 * 3600,
    has_refresh_token: true,
    email: 'oauth-demo@aether.dev',
    key_id: params.keyId
  })
})

registerDynamicRoute('POST', '/api/admin/provider-oauth/providers/:providerId/start', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    authorization_url: `https://example.com/oauth/authorize?provider=${params.providerId}`,
    redirect_uri: 'https://aether.local/oauth/callback',
    provider_type: 'codex',
    instructions: 'Open the authorization URL and paste the callback URL here.'
  })
})

registerDynamicRoute('POST', '/api/admin/provider-oauth/providers/:providerId/complete', async (config, _params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({
    key_id: `key-oauth-${Date.now()}`,
    provider_type: 'codex',
    expires_at: Math.floor(Date.now() / 1000) + 24 * 3600,
    has_refresh_token: true,
    email: body.name ? `${body.name}@demo.dev` : 'oauth-demo@aether.dev'
  })
})

registerDynamicRoute('POST', '/api/admin/provider-oauth/providers/:providerId/import-refresh-token', async (config, _params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({
    key_id: `key-oauth-${Date.now()}`,
    provider_type: 'codex',
    expires_at: Math.floor(Date.now() / 1000) + 24 * 3600,
    has_refresh_token: true,
    email: body.name ? `${body.name}@demo.dev` : 'oauth-demo@aether.dev'
  })
})

registerDynamicRoute('POST', '/api/admin/provider-oauth/providers/:providerId/batch-import', async (config, _params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const raw = typeof body.credentials === 'string' ? body.credentials.trim() : ''
  const lines = raw ? raw.split('\n').filter(line => line.trim() && !line.trim().startsWith('#')) : []
  const total = Math.max(Math.min(lines.length, 5), 2)
  const results = []
  for (let index = 0; index < total; index++) {
    results.push({
      index,
      status: 'success',
      key_id: `key-oauth-${Date.now()}-${index}`,
      key_name: `Imported OAuth ${index + 1}`,
      auth_method: 'oauth'
    })
  }
  return createMockResponse({
    total,
    success: results.length,
    failed: 0,
    results
  })
})


// Key 更新
registerDynamicRoute('PUT', '/api/admin/endpoints/keys/:keyId', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ id: params.keyId, ...body, updated_at: new Date().toISOString() })
})

// Key 删除
registerDynamicRoute('DELETE', '/api/admin/endpoints/keys/:keyId', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// Key Reveal
registerDynamicRoute('GET', '/api/admin/endpoints/keys/:keyId/reveal', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ api_key: 'sk-demo-reveal' })
})

registerDynamicRoute('GET', '/api/admin/endpoints/keys/:keyId/export', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    key_id: params.keyId,
    provider_type: 'codex',
    auth_method: 'oauth',
    refresh_token: 'rt-demo',
    email: 'oauth-demo@aether.dev',
    exported_at: new Date().toISOString()
  })
})

registerDynamicRoute('POST', '/api/admin/endpoints/keys/:keyId/clear-oauth-invalid', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: 'OAuth invalid cleared (demo)', key_id: params.keyId })
})

registerDynamicRoute('GET', '/api/admin/system/configs/:key', async (_config, params) => {
  await delay()
  requireAdmin()
  const config = MOCK_SYSTEM_CONFIGS.find(item => item.key === params.key)
  return createMockResponse({
    key: params.key,
    value: config?.value ?? null,
  })
})

registerDynamicRoute('PUT', '/api/admin/system/configs/:key', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const existing = MOCK_SYSTEM_CONFIGS.find(item => item.key === params.key)
  if (existing) {
    existing.value = body.value
    existing.description = body.description ?? existing.description
    return createMockResponse({
      ...existing,
      updated_at: new Date().toISOString(),
    })
  }

  MOCK_SYSTEM_CONFIGS.push({
    key: params.key,
    value: body.value ?? null,
    description: body.description ?? '',
  })

  const created = {
    key: params.key,
    value: body.value ?? null,
    description: body.description ?? '',
    updated_at: new Date().toISOString(),
  }
  return createMockResponse(created)
})


// Keys grouped by format
mockHandlers['GET /api/admin/endpoints/keys/grouped-by-format'] = async () => {
  await delay()
  requireAdmin()

  // 确保每个 provider 都有 key 数据
  for (const provider of MOCK_PROVIDERS) {
    if (!PROVIDER_KEYS_CACHE[provider.id]) {
      PROVIDER_KEYS_CACHE[provider.id] = generateMockKeysForProvider(provider.id, 2)
    }
  }

  const grouped: Record<string, Record<string, unknown>[]> = {}
  for (const provider of MOCK_PROVIDERS) {
    const keys = PROVIDER_KEYS_CACHE[provider.id] || []
    for (const key of keys) {
      const formats: string[] = key.api_formats || []
      for (const fmt of formats) {
        if (!grouped[fmt]) grouped[fmt] = []
        grouped[fmt].push({
          ...key,
          api_format: fmt,
          provider_name: provider.name,
          pool_enabled: Boolean((provider.config as Record<string, unknown> | undefined)?.pool_advanced),
          global_priority: key.global_priority ?? null,
          circuit_breaker_open: false,
          capabilities: [],
        })
      }
    }
  }

  return createMockResponse(grouped)
}

// Provider Models 列表
registerDynamicRoute('GET', '/api/admin/providers/:providerId/models', async (_config, params) => {
  await delay()
  requireAdmin()
  const models = generateMockModelsForProvider(params.providerId)
  return createMockResponse(models)
})

// Provider Model 详情
registerDynamicRoute('GET', '/api/admin/providers/:providerId/models/:modelId', async (_config, params) => {
  await delay()
  requireAdmin()
  const models = generateMockModelsForProvider(params.providerId)
  const model = models.find(m => m.id === params.modelId)
  if (!model) {
    throw { response: createMockResponse({ detail: '模型不存在' }, 404) }
  }
  return createMockResponse(model)
})

// 创建 Provider Model
registerDynamicRoute('POST', '/api/admin/providers/:providerId/models', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({
    id: `pm-demo-${Date.now()}`,
    provider_id: params.providerId,
    ...body,
    created_at: new Date().toISOString()
  })
})

// 更新 Provider Model
registerDynamicRoute('PATCH', '/api/admin/providers/:providerId/models/:modelId', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ id: params.modelId, provider_id: params.providerId, ...body, updated_at: new Date().toISOString() })
})

// 删除 Provider Model
registerDynamicRoute('DELETE', '/api/admin/providers/:providerId/models/:modelId', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 批量创建 Provider Models
registerDynamicRoute('POST', '/api/admin/providers/:providerId/models/batch', async (config, params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const models = ((body.models || []) as Record<string, unknown>[]).map((m: Record<string, unknown>, i: number) => ({
    id: `pm-demo-${Date.now()}-${i}`,
    provider_id: params.providerId,
    ...m,
    created_at: new Date().toISOString()
  }))
  return createMockResponse({ models, created_count: models.length })
})

// Provider 可用源模型
registerDynamicRoute('GET', '/api/admin/providers/:providerId/available-source-models', async (_config, params) => {
  await delay()
  requireAdmin()
  const provider = MOCK_PROVIDERS.find(p => p.id === params.providerId)
  if (!provider) {
    throw { response: createMockResponse({ detail: '提供商不存在' }, 404) }
  }
  // 返回一些可用的源模型
  const availableModels = [
    'claude-sonnet-4-5-20250929',
    'claude-haiku-4-5-20251001',
    'claude-opus-4-5-20251101',
    'gpt-5.1',
    'gpt-5.1-codex',
    'gemini-3-pro-preview'
  ]
  return createMockResponse({ models: availableModels })
})

// 分配 GlobalModels 到 Provider
registerDynamicRoute('POST', '/api/admin/providers/:providerId/assign-global-models', async (config, _params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const result = {
    success: (body.global_model_ids || []).map((id: string) => ({
      global_model_id: id,
      provider_model_id: `pm-demo-${Date.now()}-${id}`
    })),
    errors: []
  }
  return createMockResponse(result)
})

// GlobalModel 详情
registerDynamicRoute('GET', '/api/admin/models/global/:modelId', async (_config, params) => {
  await delay()
  requireAdmin()
  syncMockGlobalModelGroups()
  const model = MOCK_GLOBAL_MODELS.find(m => m.id === params.modelId)
  if (!model) {
    throw { response: createMockResponse({ detail: '模型不存在' }, 404) }
  }
  return createMockResponse(model)
})

// GlobalModel 更新
registerDynamicRoute('PATCH', '/api/admin/models/global/:modelId', async (config, params) => {
  await delay()
  requireAdmin()
  const model = MOCK_GLOBAL_MODELS.find(m => m.id === params.modelId)
  if (!model) {
    throw { response: createMockResponse({ detail: '模型不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  Object.assign(model, {
    ...body,
    model_group_ids: Array.isArray(body.model_group_ids) ? body.model_group_ids : model.model_group_ids,
    updated_at: new Date().toISOString(),
  })
  syncMockGlobalModelGroups()
  return createMockResponse(model)
})

// GlobalModel 删除
registerDynamicRoute('DELETE', '/api/admin/models/global/:modelId', async (_config, params) => {
  await delay()
  requireAdmin()
  const modelIndex = MOCK_GLOBAL_MODELS.findIndex(m => m.id === params.modelId)
  if (modelIndex === -1) {
    throw { response: createMockResponse({ detail: '模型不存在' }, 404) }
  }
  MOCK_GLOBAL_MODELS.splice(modelIndex, 1)
  syncMockGlobalModelGroups()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

registerDynamicRoute('GET', '/api/admin/models/groups/:groupId', async (_config, params) => {
  await delay()
  requireAdmin()
  const group = getMockModelGroupDetail(params.groupId)
  if (!group) {
    throw { response: createMockResponse({ detail: '模型分组不存在' }, 404) }
  }
  return createMockResponse(group)
})

registerDynamicRoute('PATCH', '/api/admin/models/groups/:groupId', async (config, params) => {
  await delay()
  requireAdmin()
  const group = getMockModelGroupDetail(params.groupId)
  if (!group) {
    throw { response: createMockResponse({ detail: '模型分组不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  if ('name' in body && body.name != null) group.name = body.name
  if ('display_name' in body && body.display_name != null) group.display_name = body.display_name
  if ('description' in body) group.description = body.description ?? null
  if ('default_user_billing_multiplier' in body && body.default_user_billing_multiplier != null) {
    group.default_user_billing_multiplier = Number(body.default_user_billing_multiplier)
  }
  if ('routing_mode' in body && body.routing_mode != null) group.routing_mode = body.routing_mode
  if ('is_active' in body && body.is_active != null) group.is_active = Boolean(body.is_active)
  if ('sort_order' in body && body.sort_order != null) group.sort_order = Number(body.sort_order)
  if ('routes' in body) {
    group.routes = Array.isArray(body.routes)
      ? body.routes.map((route: Record<string, unknown>, index: number) => ({
        id: String(route.id ?? `mgr-demo-${Date.now()}-${index}`),
        provider_id: String(route.provider_id ?? ''),
        provider_name: MOCK_PROVIDERS.find(provider => provider.id === route.provider_id)?.name ?? null,
        provider_api_key_id: route.provider_api_key_id ? String(route.provider_api_key_id) : null,
        provider_api_key_name: route.provider_api_key_id
          ? MOCK_ENDPOINT_KEYS.find(key => key.id === route.provider_api_key_id)?.name ?? null
          : null,
        priority: Number(route.priority ?? 50),
        user_billing_multiplier_override: route.user_billing_multiplier_override == null ? null : Number(route.user_billing_multiplier_override),
        is_active: route.is_active !== false,
        notes: route.notes ? String(route.notes) : null,
      }))
      : []
  }
  if ('model_ids' in body) {
    replaceMockModelGroupMemberships(group.id, Array.isArray(body.model_ids) ? body.model_ids : [])
  } else {
    syncMockGlobalModelGroups()
  }
  group.updated_at = new Date().toISOString()
  syncMockUserGroupsAndUsers()
  return createMockResponse(getMockModelGroupDetail(group.id))
})

registerDynamicRoute('DELETE', '/api/admin/models/groups/:groupId', async (_config, params) => {
  await delay()
  requireAdmin()
  const groupIndex = MOCK_MODEL_GROUPS.findIndex(group => group.id === params.groupId)
  if (groupIndex === -1) {
    throw { response: createMockResponse({ detail: '模型分组不存在' }, 404) }
  }
  if (MOCK_MODEL_GROUPS[groupIndex]?.is_default) {
    throw { response: createMockResponse({ detail: '默认模型分组不能删除' }, 400) }
  }
  if (MOCK_USER_GROUPS.some(group => (group.model_group_bindings || []).some(binding => binding.model_group_id === params.groupId))) {
    throw { response: createMockResponse({ detail: '该模型分组仍有关联的用户分组，请先移除绑定' }, 400) }
  }
  for (const model of MOCK_GLOBAL_MODELS) {
    model.model_group_ids = (model.model_group_ids || []).filter(id => id !== params.groupId)
  }
  MOCK_MODEL_GROUPS.splice(groupIndex, 1)
  syncMockGlobalModelGroups()
  syncMockUserGroupsAndUsers()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// GlobalModel 批量分配到 Providers
registerDynamicRoute('POST', '/api/admin/models/global/:modelId/assign-to-providers', async (config, _params) => {
  await delay()
  requireAdmin()
  const body = JSON.parse(config.data || '{}')
  const result = {
    success: (body.provider_ids || []).map((providerId: string) => {
      const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
      return {
        provider_id: providerId,
        provider_name: provider?.name || 'unknown',
        model_id: `pm-demo-${Date.now()}-${providerId}`
      }
    }),
    errors: []
  }
  return createMockResponse(result)
})

// Endpoint Health 详情
registerDynamicRoute('GET', '/api/admin/endpoints/health/endpoint/:endpointId', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    endpoint_id: params.endpointId,
    health_score: 0.95,
    total_requests: 5000,
    success_count: 4750,
    failed_count: 250,
    success_rate: 0.95,
    avg_response_time_ms: 1200,
    last_success_at: new Date().toISOString(),
    last_failure_at: new Date(Date.now() - 3600000).toISOString()
  })
})

// Key Health 详情
registerDynamicRoute('GET', '/api/admin/endpoints/health/key/:keyId', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    key_id: params.keyId,
    health_score: 0.92,
    total_requests: 2000,
    success_count: 1840,
    failed_count: 160,
    success_rate: 0.92,
    avg_response_time_ms: 1100,
    last_success_at: new Date().toISOString(),
    last_failure_at: new Date(Date.now() - 7200000).toISOString()
  })
})

registerDynamicRoute('PATCH', '/api/admin/endpoints/health/keys', async () => {
  await delay()
  requireAdmin()
  return createMockResponse({
    message: 'All key health recovered (demo)',
    recovered_count: 2,
    recovered_keys: [
      { key_id: 'key-demo-1', key_name: 'Primary Key', endpoint_id: 'ep-demo-1' },
      { key_id: 'key-demo-2', key_name: 'Backup Key', endpoint_id: 'ep-demo-2' }
    ]
  })
})

// 重置 Key Health
registerDynamicRoute('PATCH', '/api/admin/endpoints/health/keys/:keyId', async (_config, params) => {
  await delay()
  requireAdmin()
  return createMockResponse({
    key_id: params.keyId,
    message: '健康状态已重置（演示模式）'
  })
})

// Alias/Mapping 详情
registerDynamicRoute('GET', '/api/admin/models/mappings/:mappingId', async (_config, params) => {
  await delay()
  requireAdmin()
  const alias = MOCK_ALIASES.find(a => a.id === params.mappingId)
  if (!alias) {
    throw { response: createMockResponse({ detail: '映射不存在' }, 404) }
  }
  return createMockResponse(alias)
})

// Alias/Mapping 更新
registerDynamicRoute('PATCH', '/api/admin/models/mappings/:mappingId', async (config, params) => {
  await delay()
  requireAdmin()
  const alias = MOCK_ALIASES.find(a => a.id === params.mappingId)
  if (!alias) {
    throw { response: createMockResponse({ detail: '映射不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ ...alias, ...body, updated_at: new Date().toISOString() })
})

// Alias/Mapping 删除
registerDynamicRoute('DELETE', '/api/admin/models/mappings/:mappingId', async (_config, params) => {
  await delay()
  requireAdmin()
  const alias = MOCK_ALIASES.find(a => a.id === params.mappingId)
  if (!alias) {
    throw { response: createMockResponse({ detail: '映射不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 公告详情
registerDynamicRoute('GET', '/api/announcements/:announcementId', async (_config, params) => {
  await delay()
  const announcement = MOCK_ANNOUNCEMENTS.find(a => a.id === params.announcementId)
  if (!announcement) {
    throw { response: createMockResponse({ detail: '公告不存在' }, 404) }
  }
  return createMockResponse(announcement)
})

// 公告更新
registerDynamicRoute('PATCH', '/api/announcements/:announcementId', async (config, params) => {
  await delay()
  requireAdmin()
  const announcement = MOCK_ANNOUNCEMENTS.find(a => a.id === params.announcementId)
  if (!announcement) {
    throw { response: createMockResponse({ detail: '公告不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ ...announcement, ...body, updated_at: new Date().toISOString() })
})

// 公告删除
registerDynamicRoute('DELETE', '/api/announcements/:announcementId', async (_config, params) => {
  await delay()
  requireAdmin()
  const announcement = MOCK_ANNOUNCEMENTS.find(a => a.id === params.announcementId)
  if (!announcement) {
    throw { response: createMockResponse({ detail: '公告不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 用户详情
registerDynamicRoute('GET', '/api/admin/users/:userId', async (_config, params) => {
  await delay()
  requireAdmin()
  const user = MOCK_ALL_USERS.find(u => u.id === params.userId)
  if (!user) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  return createMockResponse(user)
})

registerDynamicRoute('PUT', '/api/admin/users/groups/:groupId', async (config, params) => {
  await delay()
  requireAdmin()
  const group = MOCK_USER_GROUPS.find(g => g.id === params.groupId)
  if (!group) {
    throw { response: createMockResponse({ detail: '用户分组不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  if ('name' in body && body.name != null) {
    group.name = body.name
  }
  if ('description' in body) {
    group.description = body.description ?? null
  }
  if ('allowed_api_formats' in body) {
    group.allowed_api_formats = body.allowed_api_formats ?? null
  }
  if ('model_group_bindings' in body) {
    group.model_group_bindings = normalizeMockUserGroupBindings(body.model_group_bindings)
  }
  if ('rate_limit' in body) {
    group.rate_limit = body.rate_limit ?? null
  }
  group.updated_at = new Date().toISOString()
  syncMockUserGroupsAndUsers()
  return createMockResponse(group)
})

registerDynamicRoute('DELETE', '/api/admin/users/groups/:groupId', async (_config, params) => {
  await delay()
  requireAdmin()
  const groupIndex = MOCK_USER_GROUPS.findIndex(g => g.id === params.groupId)
  if (groupIndex === -1) {
    throw { response: createMockResponse({ detail: '用户分组不存在' }, 404) }
  }
  if (MOCK_USER_GROUPS[groupIndex]?.is_default) {
    throw { response: createMockResponse({ detail: '默认分组不能删除' }, 400) }
  }
  if (MOCK_ALL_USERS.some(user => user.group_id === params.groupId)) {
    throw { response: createMockResponse({ detail: '该分组仍有关联用户，请先移除分组成员' }, 400) }
  }
  MOCK_USER_GROUPS.splice(groupIndex, 1)
  syncMockUserGroupsAndUsers()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 用户更新
registerDynamicRoute('PATCH', '/api/admin/users/:userId', async (config, params) => {
  await delay()
  requireAdmin()
  const user = MOCK_ALL_USERS.find(u => u.id === params.userId)
  if (!user) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  const defaultGroup = getDefaultMockUserGroup()
  Object.assign(user, body, {
    group_id: body.group_id ?? defaultGroup?.id ?? user.group_id ?? null,
    updated_at: new Date().toISOString()
  })
  syncMockUserGroupsAndUsers()
  return createMockResponse(user)
})

registerDynamicRoute('PUT', '/api/admin/users/:userId', async (config, params) => {
  await delay()
  requireAdmin()
  const user = MOCK_ALL_USERS.find(u => u.id === params.userId)
  if (!user) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  const defaultGroup = getDefaultMockUserGroup()
  Object.assign(user, {
    ...body,
    group_id: body.group_id ?? defaultGroup?.id ?? null,
    updated_at: new Date().toISOString()
  })
  syncMockUserGroupsAndUsers()
  return createMockResponse(user)
})

// 用户删除
registerDynamicRoute('DELETE', '/api/admin/users/:userId', async (_config, params) => {
  await delay()
  requireAdmin()
  const userIndex = MOCK_ALL_USERS.findIndex(u => u.id === params.userId)
  if (userIndex === -1) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  MOCK_ALL_USERS.splice(userIndex, 1)
  syncMockUserGroupsAndUsers()
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 用户 API Keys
registerDynamicRoute('GET', '/api/admin/users/:userId/api-keys', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse(MOCK_USER_API_KEYS)
})

// 管理员 - 用户会话列表
registerDynamicRoute('GET', '/api/admin/users/:userId/sessions', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse([
    {
      id: 'admin-session-1',
      device_label: 'Chrome / macOS',
      device_type: 'desktop',
      browser_name: 'Chrome',
      browser_version: '134.0',
      os_name: 'macOS',
      os_version: '15.3',
      device_model: null,
      ip_address: '192.168.1.100',
      last_seen_at: new Date().toISOString(),
      created_at: new Date(Date.now() - 2 * 24 * 3600 * 1000).toISOString(),
      is_current: false,
      revoked_at: null,
      revoke_reason: null
    }
  ])
})

// 管理员 - 撤销用户单个会话
registerDynamicRoute('DELETE', '/api/admin/users/:userId/sessions/:sessionId', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: '会话已撤销（演示模式）' })
})

// 管理员 - 撤销用户全部会话
registerDynamicRoute('DELETE', '/api/admin/users/:userId/sessions', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse({ message: '全部会话已撤销（演示模式）', revoked_count: 1 })
})

// API Key 详情
registerDynamicRoute('GET', '/api/admin/api-keys/:keyId', async (_config, params) => {
  await delay()
  requireAdmin()
  const key = MOCK_ADMIN_API_KEYS.api_keys.find(k => k.id === params.keyId)
  if (!key) {
    throw { response: createMockResponse({ detail: 'API Key 不存在' }, 404) }
  }
  return createMockResponse(key)
})

// API Key 更新
registerDynamicRoute('PATCH', '/api/admin/api-keys/:keyId', async (config, params) => {
  await delay()
  requireAdmin()
  const key = MOCK_ADMIN_API_KEYS.api_keys.find(k => k.id === params.keyId)
  if (!key) {
    throw { response: createMockResponse({ detail: 'API Key 不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ ...key, ...body })
})

// API Key 删除
registerDynamicRoute('DELETE', '/api/admin/api-keys/:keyId', async (_config, params) => {
  await delay()
  requireAdmin()
  const key = MOCK_ADMIN_API_KEYS.api_keys.find(k => k.id === params.keyId)
  if (!key) {
    throw { response: createMockResponse({ detail: 'API Key 不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 用户 API Key 删除
registerDynamicRoute('DELETE', '/api/users/me/api-keys/:keyId', async (_config, params) => {
  await delay()
  const key = MOCK_USER_API_KEYS.find(k => k.id === params.keyId)
  if (!key) {
    throw { response: createMockResponse({ detail: 'API Key 不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

function resolveUsageRecordOrThrow(requestId: string) {
  const records = getUsageRecords()
  const record = records.find(r => r.id === requestId || `req_${r.id}` === requestId)

  if (!record) {
    throw { response: createMockResponse({ detail: '请求记录不存在' }, 404) }
  }

  return record
}

// 使用记录详情 - /api/admin/usage/:requestId
registerDynamicRoute('GET', '/api/admin/usage/:requestId', async (_config, params) => {
  await delay()
  requireAdmin()

  const record = resolveUsageRecordOrThrow(params.requestId)

  // 生成详细的请求信息
  const users = [
    { id: 'demo-admin-uuid-0001', username: 'Demo Admin', email: 'admin@demo.aether.ai' },
    { id: 'demo-user-uuid-0002', username: 'Demo User', email: 'user@demo.aether.ai' },
    { id: 'demo-user-uuid-0003', username: 'Alice Chen', email: 'alice@demo.aether.ai' },
    { id: 'demo-user-uuid-0004', username: 'Bob Zhang', email: 'bob@demo.aether.ai' }
  ]
  const user = users.find(u => u.id === record.user_id) || users[0]

  // 生成模拟的请求/响应数据
  const mockRequestBody = {
    model: record.model,
    max_tokens: 4096,
    messages: [
      {
        role: 'user',
        content: 'Hello! Can you help me understand how AI gateways work?'
      }
    ],
    stream: record.is_stream
  }

  const mockResponseBody = record.status === 'failed' ? {
    error: {
      type: 'api_error',
      message: record.error_message || 'An error occurred'
    }
  } : {
    id: `msg_${record.id}`,
    type: 'message',
    role: 'assistant',
    content: [
      {
        type: 'text',
        text: 'AI gateways are middleware services that sit between clients and backend services. They handle routing, authentication, rate limiting, and more...'
      }
    ],
    model: record.model,
    stop_reason: 'end_turn',
    usage: {
      input_tokens: record.input_tokens,
      output_tokens: record.output_tokens
    }
  }

  // 计算费用明细
  const inputPricePer1M = record.model.includes('opus') ? 15 : record.model.includes('haiku') ? 1 : 3
  const outputPricePer1M = record.model.includes('opus') ? 75 : record.model.includes('haiku') ? 5 : 15
  const inputCost = (record.input_tokens / 1000000) * inputPricePer1M
  const outputCost = (record.output_tokens / 1000000) * outputPricePer1M
  const cacheCreationCost = (record.cache_creation_input_tokens / 1000000) * (inputPricePer1M * 1.25)
  const cacheReadCost = (record.cache_read_input_tokens / 1000000) * (inputPricePer1M * 0.1)

  const detail = {
    id: record.id,
    request_id: `req_${record.id}`,
    user: {
      id: user.id,
      username: user.username,
      email: user.email
    },
    api_key: {
      id: `key-${record.api_key_name}`,
      name: record.api_key_name,
      display: `sk-***${record.api_key_name.slice(-4)}`
    },
    provider: record.provider,
    api_format: record.api_format,
    model: record.model,
    target_model: record.target_model,
    tokens: {
      input: record.input_tokens,
      output: record.output_tokens,
      total: record.total_tokens
    },
    cost: {
      input: inputCost,
      output: outputCost,
      total: record.cost
    },
    input_tokens: record.input_tokens,
    output_tokens: record.output_tokens,
    total_tokens: record.total_tokens,
    cache_creation_input_tokens: record.cache_creation_input_tokens,
    cache_read_input_tokens: record.cache_read_input_tokens,
    input_cost: inputCost,
    output_cost: outputCost,
    total_cost: record.cost,
    cache_creation_cost: cacheCreationCost,
    cache_read_cost: cacheReadCost,
    input_price_per_1m: inputPricePer1M,
    output_price_per_1m: outputPricePer1M,
    cache_creation_price_per_1m: inputPricePer1M * 1.25,
    cache_read_price_per_1m: inputPricePer1M * 0.1,
    request_type: record.is_stream ? 'stream' : 'standard',
    is_stream: record.is_stream,
    status_code: record.status_code,
    error_message: record.error_message,
    response_time_ms: record.response_time_ms,
    created_at: record.created_at,
    request_headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer sk-aether-***',
      'X-Api-Key': 'sk-***',
      'User-Agent': 'Aether-Client/1.0',
      'Accept': 'application/json',
      'X-Request-ID': `req_${record.id}`
    },
    request_body: mockRequestBody,
    provider_request_headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer sk-${record.provider}-***`,
      'anthropic-version': '2024-01-01',
      'X-Request-ID': `req_${record.id}`
    },
    response_headers: {
      'Content-Type': 'application/json',
      'X-Request-ID': `req_${record.id}`,
      'X-RateLimit-Limit': '1000',
      'X-RateLimit-Remaining': '999',
      'X-RateLimit-Reset': new Date(Date.now() + 60000).toISOString()
    },
    response_body: mockResponseBody,
    metadata: {
      client_ip: '192.168.1.100',
      user_agent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)',
      request_path: `/v1/messages`,
      provider_endpoint: `https://api.${record.provider}.com/v1/messages`,
      gateway_version: '1.0.0',
      processing_time_ms: Math.floor((record.response_time_ms || 1000) * 0.1)
    },
    tiered_pricing: {
      total_input_context: record.input_tokens + record.cache_creation_input_tokens + record.cache_read_input_tokens,
      tier_index: 0,
      source: 'provider',
      tiers: [
        {
          up_to: 200000,
          input_price_per_1m: inputPricePer1M,
          output_price_per_1m: outputPricePer1M,
          cache_creation_price_per_1m: inputPricePer1M * 1.25,
          cache_read_price_per_1m: inputPricePer1M * 0.1
        },
        {
          up_to: null,
          input_price_per_1m: inputPricePer1M * 0.5,
          output_price_per_1m: outputPricePer1M * 0.5,
          cache_creation_price_per_1m: inputPricePer1M * 0.625,
          cache_read_price_per_1m: inputPricePer1M * 0.05
        }
      ]
    }
  }

  return createMockResponse(detail)
})

registerDynamicRoute('GET', '/api/admin/usage/:requestId/curl', async (_config, params) => {
  await delay()
  requireAdmin()

  const record = resolveUsageRecordOrThrow(params.requestId)
  const url = `https://api.${record.provider}.com/v1/messages`
  const body = {
    model: record.target_model || record.model,
    max_tokens: 4096,
    messages: [
      {
        role: 'user',
        content: 'Hello! Can you help me understand how AI gateways work?'
      }
    ],
    stream: record.is_stream
  }
  const headers = {
    'Content-Type': 'application/json',
    'Authorization': `Bearer sk-${record.provider}-demo-key`,
    'X-Request-ID': `req_${record.id}`
  }

  return createMockResponse({
    url,
    method: 'POST',
    headers,
    body,
    curl: [
      `curl -X POST '${url}'`,
      `-H 'Content-Type: ${headers['Content-Type']}'`,
      `-H 'Authorization: ${headers.Authorization}'`,
      `-H 'X-Request-ID: ${headers['X-Request-ID']}'`,
      `-d '${JSON.stringify(body)}'`,
    ].join(' \\\n  '),
  })
})

registerDynamicRoute('POST', '/api/admin/usage/:requestId/replay', async (config, params) => {
  await delay()
  requireAdmin()

  const record = resolveUsageRecordOrThrow(params.requestId)
  const payload = parseMockAnalyticsPayload(config) as {
    provider_id?: string
    api_key_id?: string
  }
  const provider = payload.provider_id || record.provider || 'openai'

  return createMockResponse({
    url: `https://api.${provider}.com/v1/messages`,
    provider,
    status_code: 200,
    response_headers: {
      'Content-Type': 'application/json',
      'X-Request-ID': `replay_${record.id}`,
    },
    response_body: {
      id: `replay_${record.id}`,
      type: 'message',
      role: 'assistant',
      content: [
        {
          type: 'text',
          text: payload.api_key_id
            ? 'Replay completed successfully with the selected provider key.'
            : 'Replay completed successfully.'
        }
      ],
      model: record.target_model || record.model,
    },
    response_time_ms: Math.max(120, Math.round((record.response_time_ms || 1000) * 0.85)),
  })
})

// 请求链路追踪 - /api/admin/monitoring/trace/:requestId
registerDynamicRoute('GET', '/api/admin/monitoring/trace/:requestId', async (_config, params) => {
  await delay()
  requireAdmin()

  const requestId = params.requestId
  // 从 usage-xxxx 格式中提取记录
  const records = getUsageRecords()
  const recordId = requestId.startsWith('req_') ? requestId.replace('req_', '') : requestId
  const record = records.find(r => r.id === recordId)

  if (!record) {
    throw { response: createMockResponse({ detail: '请求记录不存在' }, 404) }
  }

  // 生成候选记录
  const now = new Date(record.created_at)
  const baseLatency = record.response_time_ms || 1000

  // 根据请求状态生成不同的候选链路
  const candidates = []
  const providerNames = ['AlphaAI', 'BetaClaude', 'GammaCode', 'DeltaAPI']

  if (record.status === 'completed') {
    // 成功请求：可能有1-2个跳过的候选，最后一个成功
    const skipCount = Math.random() > 0.5 ? 1 : 0

    for (let i = 0; i < skipCount; i++) {
      const skipStarted = new Date(now.getTime() + i * 50)
      candidates.push({
        id: `candidate-${requestId}-${i}`,
        request_id: requestId,
        candidate_index: i,
        retry_index: 0,
        provider_id: `provider-${i + 1}`,
        provider_name: providerNames[i % providerNames.length],
        provider_website: `https://${providerNames[i % providerNames.length].toLowerCase()}.com`,
        endpoint_id: `endpoint-${i + 1}`,
        endpoint_name: record.api_format,
        key_id: `key-${i + 1}`,
        key_name: `${record.provider}-key-${i + 1}`,
        key_preview: `sk-***${Math.random().toString(36).substring(2, 6)}`,
        key_capabilities: { 'cache_1h': true, 'vision': true },
        required_capabilities: { 'cache_1h': record.cache_read_input_tokens > 0 },
        status: 'skipped',
        skip_reason: ['并发限制已满', '健康分数过低', '倍率不匹配'][i % 3],
        is_cached: false,
        latency_ms: 10 + Math.floor(Math.random() * 20),
        created_at: skipStarted.toISOString(),
        started_at: skipStarted.toISOString(),
        finished_at: new Date(skipStarted.getTime() + 10).toISOString()
      })
    }

    // 成功的候选
    const successStarted = new Date(now.getTime() + skipCount * 50)
    candidates.push({
      id: `candidate-${requestId}-success`,
      request_id: requestId,
      candidate_index: skipCount,
      retry_index: 0,
      provider_id: `provider-${record.provider}`,
      provider_name: record.provider === 'anthropic' ? 'AlphaAI' : record.provider === 'openai' ? 'BetaClaude' : 'GammaCode',
      provider_website: `https://api.${record.provider}.com`,
      endpoint_id: `endpoint-${record.provider}`,
      endpoint_name: record.api_format,
      key_id: `key-${record.api_key_name}`,
      key_name: record.api_key_name,
      key_preview: `sk-***${Math.random().toString(36).substring(2, 6)}`,
      key_capabilities: { 'cache_1h': true, 'vision': true, 'extended_thinking': true },
      required_capabilities: {
        'cache_1h': record.cache_read_input_tokens > 0,
        'vision': false,
        'extended_thinking': false
      },
      status: 'success',
      is_cached: record.cache_read_input_tokens > 0,
      status_code: 200,
      latency_ms: baseLatency,
      created_at: successStarted.toISOString(),
      started_at: successStarted.toISOString(),
      finished_at: new Date(successStarted.getTime() + baseLatency).toISOString()
    })
  } else if (record.status === 'failed') {
    // 失败请求：多个候选都失败
    const attemptCount = 2 + Math.floor(Math.random() * 2)

    for (let i = 0; i < attemptCount; i++) {
      const attemptStarted = new Date(now.getTime() + i * 200)
      const attemptLatency = 100 + Math.floor(Math.random() * 500)
      candidates.push({
        id: `candidate-${requestId}-${i}`,
        request_id: requestId,
        candidate_index: i,
        retry_index: 0,
        provider_id: `provider-${i + 1}`,
        provider_name: providerNames[i % providerNames.length],
        provider_website: `https://${providerNames[i % providerNames.length].toLowerCase()}.com`,
        endpoint_id: `endpoint-${i + 1}`,
        endpoint_name: record.api_format,
        key_id: `key-${i + 1}`,
        key_name: `${record.provider}-key-${i + 1}`,
        key_preview: `sk-***${Math.random().toString(36).substring(2, 6)}`,
        key_capabilities: { 'cache_1h': true },
        required_capabilities: {},
        status: 'failed',
        is_cached: false,
        status_code: record.status_code,
        error_type: ['rate_limit_error', 'api_error', 'timeout_error'][i % 3],
        error_message: record.error_message || 'Request failed',
        latency_ms: attemptLatency,
        created_at: attemptStarted.toISOString(),
        started_at: attemptStarted.toISOString(),
        finished_at: new Date(attemptStarted.getTime() + attemptLatency).toISOString()
      })
    }
  } else {
    // 进行中的请求
    candidates.push({
      id: `candidate-${requestId}-0`,
      request_id: requestId,
      candidate_index: 0,
      retry_index: 0,
      provider_id: `provider-${record.provider}`,
      provider_name: record.provider === 'anthropic' ? 'AlphaAI' : record.provider === 'openai' ? 'BetaClaude' : 'GammaCode',
      provider_website: `https://api.${record.provider}.com`,
      endpoint_id: `endpoint-${record.provider}`,
      endpoint_name: record.api_format,
      key_id: `key-${record.api_key_name}`,
      key_name: record.api_key_name,
      key_preview: `sk-***${Math.random().toString(36).substring(2, 6)}`,
      key_capabilities: { 'cache_1h': true, 'vision': true },
      required_capabilities: {},
      status: 'streaming',
      is_cached: false,
      latency_ms: undefined,
      created_at: now.toISOString(),
      started_at: now.toISOString(),
      finished_at: undefined
    })
  }

  const totalLatency = candidates.reduce((sum, c) => sum + (c.latency_ms || 0), 0)

  return createMockResponse({
    request_id: requestId,
    total_candidates: candidates.length,
    final_status: record.status === 'completed' ? 'success' : record.status === 'failed' ? 'failed' : 'streaming',
    total_latency_ms: totalLatency,
    candidates
  })
})

// ========== 请求间隔时间线 Mock 数据 ==========

// 生成请求间隔时间线数据（用于散点图）
function generateIntervalTimelineData(
  hours: number = 24,
  limit: number = 5000,
  includeUserInfo: boolean = false
) {
  const now = Date.now()
  const startTime = now - hours * 60 * 60 * 1000
  const points: Array<{ x: string; y: number; user_id?: string; model?: string }> = []

  // 用户列表（用于管理员视图）
  const users = [
    { id: 'demo-admin-uuid-0001', username: 'Demo Admin' },
    { id: 'demo-user-uuid-0002', username: 'Demo User' },
    { id: 'demo-user-uuid-0003', username: 'Alice Chen' },
    { id: 'demo-user-uuid-0004', username: 'Bob Zhang' }
  ]

  // 模型列表（用于按模型区分颜色）
  const models = [
    'claude-sonnet-4-5-20250929',
    'claude-haiku-4-5-20251001',
    'claude-opus-4-5-20251101',
    'gpt-5.1'
  ]

  // 生成模拟的请求间隔数据
  // 间隔时间分布：大部分在 0-10 分钟，少量在 10-60 分钟，极少数在 60-120 分钟
  const pointCount = Math.min(limit, Math.floor(hours * 80)) // 每小时约 80 个数据点

  let currentTime = startTime + Math.random() * 60 * 1000 // 从起始时间后随机开始

  for (let i = 0; i < pointCount && currentTime < now; i++) {
    // 生成间隔时间（分钟），使用指数分布模拟真实场景
    let interval: number
    const rand = Math.random()
    if (rand < 0.7) {
      // 70% 的请求间隔在 0-5 分钟
      interval = Math.random() * 5
    } else if (rand < 0.9) {
      // 20% 的请求间隔在 5-30 分钟
      interval = 5 + Math.random() * 25
    } else if (rand < 0.98) {
      // 8% 的请求间隔在 30-90 分钟
      interval = 30 + Math.random() * 60
    } else {
      // 2% 的请求间隔在 90-120 分钟
      interval = 90 + Math.random() * 30
    }

    // 添加一些工作时间的模式（工作时间间隔更短）
    const hour = new Date(currentTime).getHours()
    if (hour >= 9 && hour <= 18) {
      interval *= 0.6 // 工作时间间隔更短
    } else if (hour >= 22 || hour <= 6) {
      interval *= 1.5 // 夜间间隔更长
    }

    // 确保间隔不超过 120 分钟
    interval = Math.min(interval, 120)

    const point: { x: string; y: number; user_id?: string; model?: string } = {
      x: new Date(currentTime).toISOString(),
      y: Math.round(interval * 100) / 100,
      model: models[Math.floor(Math.random() * models.length)]
    }

    if (includeUserInfo) {
      // 管理员视图：添加用户信息
      const user = users[Math.floor(Math.random() * users.length)]
      point.user_id = user.id
    }

    points.push(point)

    // 下一个请求时间 = 当前时间 + 间隔 + 一些随机抖动
    currentTime += interval * 60 * 1000 + Math.random() * 30 * 1000
  }

  // 按时间排序
  points.sort((a, b) => new Date(a.x).getTime() - new Date(b.x).getTime())

  // 收集出现的模型
  const usedModels = [...new Set(points.map(p => p.model).filter(Boolean))] as string[]

  const response: {
    analysis_period_hours: number
    total_points: number
    points: typeof points
    users?: Record<string, string>
    models?: string[]
  } = {
    analysis_period_hours: hours,
    total_points: points.length,
    points,
    models: usedModels
  }

  if (includeUserInfo) {
    response.users = Object.fromEntries(users.map(u => [u.id, u.username]))
  }

  return response
}

// ========== TTL 分析 Mock 数据 ==========

// 生成 TTL 分析数据
function generateTTLAnalysisData(hours: number = 168) {
  const users = [
    { id: 'demo-admin-uuid-0001', username: 'Demo Admin', email: 'admin@demo.aether.io' },
    { id: 'demo-user-uuid-0002', username: 'Demo User', email: 'user@demo.aether.io' },
    { id: 'demo-user-uuid-0003', username: 'Alice Chen', email: 'alice@demo.aether.io' },
    { id: 'demo-user-uuid-0004', username: 'Bob Zhang', email: 'bob@demo.aether.io' }
  ]

  const usersAnalysis = users.map(user => {
    // 为每个用户生成不同的使用模式
    const requestCount = 50 + Math.floor(Math.random() * 500)

    // 根据用户特性生成不同的间隔分布
    let within5min, within15min, within30min, within60min, over60min
    let p50, p75, p90
    let recommendedTtl: number
    let recommendationReason: string

    const userType = Math.random()
    if (userType < 0.3) {
      // 高频用户 (30%)
      within5min = Math.floor(requestCount * (0.6 + Math.random() * 0.2))
      within15min = Math.floor(requestCount * (0.1 + Math.random() * 0.1))
      within30min = Math.floor(requestCount * (0.05 + Math.random() * 0.05))
      within60min = Math.floor(requestCount * (0.02 + Math.random() * 0.03))
      over60min = requestCount - within5min - within15min - within30min - within60min
      p50 = 1.5 + Math.random() * 2
      p75 = 3 + Math.random() * 3
      p90 = 4 + Math.random() * 2
      recommendedTtl = 5
      recommendationReason = `高频用户：90% 的请求间隔在 ${p90.toFixed(1)} 分钟内`
    } else if (userType < 0.6) {
      // 中频用户 (30%)
      within5min = Math.floor(requestCount * (0.3 + Math.random() * 0.15))
      within15min = Math.floor(requestCount * (0.25 + Math.random() * 0.15))
      within30min = Math.floor(requestCount * (0.15 + Math.random() * 0.1))
      within60min = Math.floor(requestCount * (0.1 + Math.random() * 0.05))
      over60min = requestCount - within5min - within15min - within30min - within60min
      p50 = 5 + Math.random() * 5
      p75 = 10 + Math.random() * 8
      p90 = 18 + Math.random() * 10
      recommendedTtl = 15
      recommendationReason = `中高频用户：75% 的请求间隔在 ${p75.toFixed(1)} 分钟内`
    } else if (userType < 0.85) {
      // 中低频用户 (25%)
      within5min = Math.floor(requestCount * (0.15 + Math.random() * 0.1))
      within15min = Math.floor(requestCount * (0.2 + Math.random() * 0.1))
      within30min = Math.floor(requestCount * (0.25 + Math.random() * 0.1))
      within60min = Math.floor(requestCount * (0.15 + Math.random() * 0.1))
      over60min = requestCount - within5min - within15min - within30min - within60min
      p50 = 12 + Math.random() * 8
      p75 = 22 + Math.random() * 10
      p90 = 35 + Math.random() * 15
      recommendedTtl = 30
      recommendationReason = `中频用户：75% 的请求间隔在 ${p75.toFixed(1)} 分钟内`
    } else {
      // 低频用户 (15%)
      within5min = Math.floor(requestCount * (0.05 + Math.random() * 0.1))
      within15min = Math.floor(requestCount * (0.1 + Math.random() * 0.1))
      within30min = Math.floor(requestCount * (0.15 + Math.random() * 0.1))
      within60min = Math.floor(requestCount * (0.25 + Math.random() * 0.1))
      over60min = requestCount - within5min - within15min - within30min - within60min
      p50 = 25 + Math.random() * 15
      p75 = 45 + Math.random() * 20
      p90 = 70 + Math.random() * 30
      recommendedTtl = 60
      recommendationReason = `低频用户：75% 的请求间隔为 ${p75.toFixed(1)} 分钟，建议使用长 TTL`
    }

    // 确保没有负数
    over60min = Math.max(0, over60min)

    const avgInterval = (within5min * 2.5 + within15min * 10 + within30min * 22 + within60min * 45 + over60min * 80) / requestCount

    return {
      group_id: user.id,
      username: user.username,
      email: user.email,
      request_count: requestCount,
      interval_distribution: {
        within_5min: within5min,
        within_15min: within15min,
        within_30min: within30min,
        within_60min: within60min,
        over_60min: over60min
      },
      interval_percentages: {
        within_5min: Math.round(within5min / requestCount * 1000) / 10,
        within_15min: Math.round(within15min / requestCount * 1000) / 10,
        within_30min: Math.round(within30min / requestCount * 1000) / 10,
        within_60min: Math.round(within60min / requestCount * 1000) / 10,
        over_60min: Math.round(over60min / requestCount * 1000) / 10
      },
      percentiles: {
        p50: Math.round(p50 * 100) / 100,
        p75: Math.round(p75 * 100) / 100,
        p90: Math.round(p90 * 100) / 100
      },
      avg_interval_minutes: Math.round(avgInterval * 100) / 100,
      min_interval_minutes: Math.round((0.1 + Math.random() * 0.5) * 100) / 100,
      max_interval_minutes: Math.round((80 + Math.random() * 40) * 100) / 100,
      recommended_ttl_minutes: recommendedTtl,
      recommendation_reason: recommendationReason
    }
  })

  // 汇总 TTL 分布
  const ttlDistribution = {
    '5min': usersAnalysis.filter(u => u.recommended_ttl_minutes === 5).length,
    '15min': usersAnalysis.filter(u => u.recommended_ttl_minutes === 15).length,
    '30min': usersAnalysis.filter(u => u.recommended_ttl_minutes === 30).length,
    '60min': usersAnalysis.filter(u => u.recommended_ttl_minutes === 60).length
  }

  return {
    analysis_period_hours: hours,
    total_users_analyzed: usersAnalysis.length,
    ttl_distribution: ttlDistribution,
    users: usersAnalysis
  }
}

// 生成缓存命中分析数据
function generateCacheHitAnalysisData(hours: number = 168) {
  const totalRequests = 5000 + Math.floor(Math.random() * 10000)
  const requestsWithCacheHit = Math.floor(totalRequests * (0.25 + Math.random() * 0.35))
  const totalInputTokens = totalRequests * (2000 + Math.floor(Math.random() * 3000))
  const totalCacheReadTokens = Math.floor(totalInputTokens * (0.15 + Math.random() * 0.25))
  const totalCacheCreationTokens = Math.floor(totalInputTokens * (0.05 + Math.random() * 0.1))

  // 缓存读取成本：按每百万 token $0.30 计算
  const cacheReadCostPer1M = 0.30
  const cacheCreationCostPer1M = 3.75
  const totalCacheReadCost = (totalCacheReadTokens / 1000000) * cacheReadCostPer1M
  const totalCacheCreationCost = (totalCacheCreationTokens / 1000000) * cacheCreationCostPer1M

  // 缓存读取节省了 90% 的成本
  const estimatedSavings = totalCacheReadCost * 9

  const tokenCacheHitRate = totalCacheReadTokens / (totalInputTokens + totalCacheReadTokens) * 100

  return {
    analysis_period_hours: hours,
    total_requests: totalRequests,
    requests_with_cache_hit: requestsWithCacheHit,
    request_cache_hit_rate: Math.round(requestsWithCacheHit / totalRequests * 10000) / 100,
    total_input_tokens: totalInputTokens,
    total_cache_read_tokens: totalCacheReadTokens,
    total_cache_creation_tokens: totalCacheCreationTokens,
    token_cache_hit_rate: Math.round(tokenCacheHitRate * 100) / 100,
    total_cache_read_cost_usd: Math.round(totalCacheReadCost * 10000) / 10000,
    total_cache_creation_cost_usd: Math.round(totalCacheCreationCost * 10000) / 10000,
    estimated_savings_usd: Math.round(estimatedSavings * 10000) / 10000
  }
}
