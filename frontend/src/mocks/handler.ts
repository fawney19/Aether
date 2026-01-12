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
  MOCK_DASHBOARD_STATS,
  MOCK_RECENT_REQUESTS,
  MOCK_PROVIDER_STATUS,
  MOCK_DAILY_STATS,
  MOCK_ALL_USERS,
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
    config: {} as any
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
      api_format: 'CLAUDE',
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
      api_format: 'CLAUDE_CLI',
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
      api_format: 'GEMINI',
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
      api_format: 'GEMINI_CLI',
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
      api_format: 'OPENAI',
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
      api_format: 'OPENAI_CLI',
      api_path: '/responses',
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

  const apiFormats = ['CLAUDE', 'CLAUDE_CLI', 'OPENAI', 'OPENAI_CLI', 'GEMINI', 'GEMINI_CLI']
  const statusOptions: Array<'completed' | 'failed' | 'streaming'> = ['completed', 'completed', 'completed', 'completed', 'failed', 'streaming']

  for (let i = 0; i < count; i++) {
    const model = models[Math.floor(Math.random() * models.length)]
    const user = users[Math.floor(Math.random() * users.length)]
    const status = statusOptions[Math.floor(Math.random() * statusOptions.length)]

    // 根据模型类型选择 API 格式
    let apiFormat = apiFormats[0]
    if (model.provider === 'anthropic') {
      apiFormat = Math.random() > 0.3 ? 'CLAUDE_CLI' : 'CLAUDE'
    } else if (model.provider === 'openai') {
      apiFormat = Math.random() > 0.3 ? 'OPENAI_CLI' : 'OPENAI'
    } else {
      apiFormat = Math.random() > 0.3 ? 'GEMINI_CLI' : 'GEMINI'
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
      is_stream: apiFormat.includes('CLI'),
      status_code: status === 'failed' ? [500, 502, 429, 400][Math.floor(Math.random() * 4)] : 200,
      error_message: status === 'failed' ? ['Rate limit exceeded', 'Internal server error', 'Model overloaded'][Math.floor(Math.random() * 3)] : undefined,
      status,
      created_at: createdAt.toISOString(),
      has_fallback: Math.random() > 0.9,
      request_metadata: model.provider === 'google' ? { model_version: 'gemini-3-pro-preview-2025-01' } : undefined
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

// Mock 映射数据
const MOCK_ALIASES = [
  { id: 'alias-001', source_model: 'claude-4-sonnet', target_global_model_id: 'gm-003', target_global_model_name: 'claude-sonnet-4-5-20250929', target_global_model_display_name: 'Claude Sonnet 4.5', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-002', source_model: 'claude-4-opus', target_global_model_id: 'gm-002', target_global_model_name: 'claude-opus-4-5-20251101', target_global_model_display_name: 'Claude Opus 4.5', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-003', source_model: 'gpt5', target_global_model_id: 'gm-006', target_global_model_name: 'gpt-5.1', target_global_model_display_name: 'GPT-5.1', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' },
  { id: 'alias-004', source_model: 'gemini-pro', target_global_model_id: 'gm-005', target_global_model_name: 'gemini-3-pro-preview', target_global_model_display_name: 'Gemini 3 Pro Preview', provider_id: null, provider_name: null, scope: 'global', mapping_type: 'alias', is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: '2024-01-01T00:00:00Z' }
]

// Mock Endpoint Keys
const MOCK_ENDPOINT_KEYS = [
  { id: 'ekey-001', provider_id: 'provider-001', api_formats: ['CLAUDE'], api_key_masked: 'sk-ant...abc1', name: 'Primary Key', rate_multiplier: 1.0, internal_priority: 1, health_score: 0.98, consecutive_failures: 0, request_count: 5000, success_count: 4950, error_count: 50, success_rate: 0.99, avg_response_time_ms: 1200, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ekey-002', provider_id: 'provider-001', api_formats: ['CLAUDE'], api_key_masked: 'sk-ant...def2', name: 'Backup Key', rate_multiplier: 1.0, internal_priority: 2, health_score: 0.95, consecutive_failures: 1, request_count: 2000, success_count: 1950, error_count: 50, success_rate: 0.975, avg_response_time_ms: 1350, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-02-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ekey-003', provider_id: 'provider-002', api_formats: ['OPENAI'], api_key_masked: 'sk-oai...ghi3', name: 'OpenAI Main', rate_multiplier: 1.0, internal_priority: 1, health_score: 0.97, consecutive_failures: 0, request_count: 3500, success_count: 3450, error_count: 50, success_rate: 0.986, avg_response_time_ms: 900, cache_ttl_minutes: 5, max_probe_interval_minutes: 32, is_active: true, created_at: '2024-01-15T00:00:00Z', updated_at: new Date().toISOString() }
]

// Mock Endpoints
const MOCK_ENDPOINTS = [
  { id: 'ep-001', provider_id: 'provider-001', provider_name: 'anthropic', api_format: 'CLAUDE', base_url: 'https://api.anthropic.com', timeout: 300, max_retries: 2, is_active: true, total_keys: 2, active_keys: 2, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ep-002', provider_id: 'provider-002', provider_name: 'openai', api_format: 'OPENAI', base_url: 'https://api.openai.com', timeout: 60, max_retries: 2, is_active: true, total_keys: 1, active_keys: 1, created_at: '2024-01-01T00:00:00Z', updated_at: new Date().toISOString() },
  { id: 'ep-003', provider_id: 'provider-003', provider_name: 'google', api_format: 'GEMINI', base_url: 'https://generativelanguage.googleapis.com', timeout: 60, max_retries: 2, is_active: true, total_keys: 1, active_keys: 1, created_at: '2024-01-15T00:00:00Z', updated_at: new Date().toISOString() }
]

// Mock 能力定义
const MOCK_CAPABILITIES = [
  { name: 'cache_1h', display_name: '1小时缓存', description: '支持1小时prompt缓存', match_mode: 'exclusive', short_name: '1h' },
  { name: 'context_1m', display_name: '1M上下文', description: '支持1M上下文窗口', match_mode: 'compatible', short_name: '1M' }
]

/**
 * Mock API 路由处理器
 */
const mockHandlers: Record<string, (config: AxiosRequestConfig) => Promise<AxiosResponse<any>>> = {
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

  'GET /api/users/me/usage': async () => {
    await delay()
    const heatmap = getActivityHeatmap()
    const records = getUsageRecords()
    // 只返回当前用户的数据
    const userRecords = records.filter(r => r.user_id === getCurrentUser().id)
    const totalRequests = userRecords.length
    const totalTokens = userRecords.reduce((sum, r) => sum + r.total_tokens, 0)
    const totalInputTokens = userRecords.reduce((sum, r) => sum + r.input_tokens, 0)
    const totalOutputTokens = userRecords.reduce((sum, r) => sum + r.output_tokens, 0)
    const totalCost = userRecords.reduce((sum, r) => sum + r.cost, 0)
    const totalActualCost = userRecords.reduce((sum, r) => sum + (r.actual_cost || 0), 0)
    const avgResponseTime = userRecords.filter(r => r.response_time_ms).reduce((sum, r) => sum + (r.response_time_ms || 0), 0) / userRecords.filter(r => r.response_time_ms).length / 1000

    // 按模型聚合
    const modelStats = new Map<string, { requests: number; input_tokens: number; output_tokens: number; total_tokens: number; total_cost_usd: number; actual_total_cost_usd: number }>()
    for (const r of userRecords) {
      const existing = modelStats.get(r.model) || { requests: 0, input_tokens: 0, output_tokens: 0, total_tokens: 0, total_cost_usd: 0, actual_total_cost_usd: 0 }
      existing.requests++
      existing.input_tokens += r.input_tokens
      existing.output_tokens += r.output_tokens
      existing.total_tokens += r.total_tokens
      existing.total_cost_usd += r.cost
      existing.actual_total_cost_usd += r.actual_cost || 0
      modelStats.set(r.model, existing)
    }

    return createMockResponse({
      total_requests: totalRequests * 20,
      total_input_tokens: totalInputTokens * 20,
      total_output_tokens: totalOutputTokens * 20,
      total_tokens: totalTokens * 20,
      total_cost: Number((totalCost * 20).toFixed(2)),
      total_actual_cost: Number((totalActualCost * 20).toFixed(2)),
      avg_response_time: Number(avgResponseTime.toFixed(2)) || 1.23,
      quota_usd: 100,
      used_usd: Number((totalCost * 20).toFixed(2)),
      activity_heatmap: heatmap,
      summary_by_model: Array.from(modelStats.entries()).map(([model, stats]) => ({
        model,
        requests: stats.requests * 20,
        input_tokens: stats.input_tokens * 20,
        output_tokens: stats.output_tokens * 20,
        total_tokens: stats.total_tokens * 20,
        total_cost_usd: Number((stats.total_cost_usd * 20).toFixed(2)),
        actual_total_cost_usd: Number((stats.actual_total_cost_usd * 20).toFixed(2))
      })),
      records: userRecords.slice(0, 10).map(r => ({
        id: r.id,
        provider: r.provider,
        model: r.model,
        input_tokens: r.input_tokens,
        output_tokens: r.output_tokens,
        total_tokens: r.total_tokens,
        cost: r.cost,
        response_time_ms: r.response_time_ms,
        is_stream: r.is_stream,
        created_at: r.created_at,
        status_code: r.status_code,
        input_price_per_1m: 3,
        output_price_per_1m: 15
      }))
    })
  },

  'GET /api/users/me/providers': async () => {
    await delay()
    return createMockResponse(MOCK_PROVIDERS.map(p => ({
      id: p.id,
      name: p.name,
      is_active: p.is_active
    })))
  },

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

  // ========== Dashboard ==========
  'GET /api/dashboard/stats': async () => {
    await delay()
    return createMockResponse(MOCK_DASHBOARD_STATS)
  },

  'GET /api/dashboard/recent-requests': async () => {
    await delay()
    return createMockResponse({ requests: MOCK_RECENT_REQUESTS })
  },

  'GET /api/dashboard/provider-status': async () => {
    await delay()
    return createMockResponse({ providers: MOCK_PROVIDER_STATUS })
  },

  'GET /api/dashboard/daily-stats': async () => {
    await delay()
    return createMockResponse(MOCK_DAILY_STATS)
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

  'POST /api/admin/users': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    const newUser = {
      id: `user-demo-${Date.now()}`,
      username: body.username,
      email: body.email,
      role: body.role || 'user',
      is_active: true,
      quota_usd: body.quota_usd || null,
      used_usd: 0,
      total_usd: 0,
      allowed_providers: null,
      allowed_api_formats: null,
      allowed_models: null,
      created_at: new Date().toISOString()
    }
    return createMockResponse(newUser)
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
      balance_used_usd: 0,
      current_balance_usd: body.initial_balance_usd || 100,
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

  'GET /api/admin/endpoints/keys': async () => {
    await delay()
    requireAdmin()
    return createMockResponse(MOCK_ENDPOINT_KEYS)
  },

  // ========== Admin: Global Models ==========
  'GET /api/admin/models/global': async () => {
    await delay()
    requireAdmin()
    return createMockResponse({ models: MOCK_GLOBAL_MODELS, total: MOCK_GLOBAL_MODELS.length })
  },

  'POST /api/admin/models/global': async (config) => {
    await delay()
    requireAdmin()
    const body = JSON.parse(config.data || '{}')
    return createMockResponse({ ...body, id: `gm-demo-${Date.now()}`, created_at: new Date().toISOString() })
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

  // ========== Admin: Usage ==========
  'GET /api/admin/usage/stats': async () => {
    await delay()
    requireAdmin()
    const heatmap = getActivityHeatmap()
    const records = getUsageRecords()
    const totalRequests = records.length
    const totalTokens = records.reduce((sum, r) => sum + r.total_tokens, 0)
    const totalCost = records.reduce((sum, r) => sum + r.cost, 0)
    const totalActualCost = records.reduce((sum, r) => sum + (r.actual_cost || 0), 0)
    const avgResponseTime = records.filter(r => r.response_time_ms).reduce((sum, r) => sum + (r.response_time_ms || 0), 0) / records.filter(r => r.response_time_ms).length / 1000

    // 今日数据
    const today = new Date().toISOString().split('T')[0]
    const todayRecords = records.filter(r => r.created_at.startsWith(today))

    return createMockResponse({
      total_requests: totalRequests * 100, // 放大显示
      total_tokens: totalTokens * 100,
      total_cost: Number((totalCost * 100).toFixed(2)),
      total_actual_cost: Number((totalActualCost * 100).toFixed(2)),
      avg_response_time: Number(avgResponseTime.toFixed(2)),
      today: {
        requests: todayRecords.length * 10,
        tokens: todayRecords.reduce((sum, r) => sum + r.total_tokens, 0) * 10,
        cost: Number((todayRecords.reduce((sum, r) => sum + r.cost, 0) * 10).toFixed(2))
      },
      activity_heatmap: heatmap
    })
  },

  'GET /api/admin/usage/records': async (config) => {
    await delay()
    requireAdmin()
    let records = getUsageRecords()
    const params = config.params || {}
    const limit = parseInt(params.limit) || 20
    const offset = parseInt(params.offset) || 0

    // 通用搜索：用户名、密钥名、模型名、提供商名
    // 支持空格分隔的组合搜索，多个关键词之间是 AND 关系
    if (typeof params.search === 'string' && params.search.trim()) {
      const keywords = params.search.trim().toLowerCase().split(/\s+/)
      records = records.filter(r => {
        // 每个关键词都要匹配至少一个字段
        return keywords.every((keyword: string) =>
          (r.username || '').toLowerCase().includes(keyword) ||
          (r.api_key?.name || '').toLowerCase().includes(keyword) ||
          (r.model || '').toLowerCase().includes(keyword) ||
          (r.provider || '').toLowerCase().includes(keyword)
        )
      })
    }

    return createMockResponse({
      records: records.slice(offset, offset + limit),
      total: records.length,
      limit,
      offset
    })
  },

  'GET /api/admin/usage/aggregation/stats': async (config) => {
    await delay()
    requireAdmin()
    const params = config.params || {}
    const groupBy = params.group_by || 'model'
    const records = getUsageRecords()

    if (groupBy === 'model') {
      // 按模型聚合
      const modelStats = new Map<string, { request_count: number; total_tokens: number; total_cost: number }>()
      for (const r of records) {
        const existing = modelStats.get(r.model) || { request_count: 0, total_tokens: 0, total_cost: 0 }
        existing.request_count++
        existing.total_tokens += r.total_tokens
        existing.total_cost += r.cost
        modelStats.set(r.model, existing)
      }
      return createMockResponse(
        Array.from(modelStats.entries()).map(([model, stats]) => ({
          model,
          request_count: stats.request_count * 50,
          total_tokens: stats.total_tokens * 50,
          total_cost: Number((stats.total_cost * 50).toFixed(2))
        }))
      )
    }

    if (groupBy === 'provider') {
      const providerStats = new Map<string, { request_count: number; total_tokens: number; total_cost: number; actual_cost: number; response_times: number[]; errors: number }>()
      for (const r of records) {
        const existing = providerStats.get(r.provider) || { request_count: 0, total_tokens: 0, total_cost: 0, actual_cost: 0, response_times: [], errors: 0 }
        existing.request_count++
        existing.total_tokens += r.total_tokens
        existing.total_cost += r.cost
        existing.actual_cost += r.actual_cost || 0
        if (r.response_time_ms) existing.response_times.push(r.response_time_ms)
        if (r.status === 'failed') existing.errors++
        providerStats.set(r.provider, existing)
      }
      return createMockResponse(
        Array.from(providerStats.entries()).map(([provider, stats]) => ({
          provider_id: `provider-${provider}`,
          provider,
          request_count: stats.request_count * 50,
          total_tokens: stats.total_tokens * 50,
          total_cost: Number((stats.total_cost * 50).toFixed(2)),
          actual_cost: Number((stats.actual_cost * 50).toFixed(2)),
          avg_response_time_ms: Math.round(stats.response_times.reduce((a, b) => a + b, 0) / stats.response_times.length || 0),
          success_rate: (stats.request_count - stats.errors) / stats.request_count,
          error_count: stats.errors * 50
        }))
      )
    }

    if (groupBy === 'user') {
      const userStats = new Map<string, { user_id: string; username: string; email: string; request_count: number; total_tokens: number; total_cost: number }>()
      for (const r of records) {
        const existing = userStats.get(r.user_id) || { user_id: r.user_id, username: r.username, email: r.user_email || '', request_count: 0, total_tokens: 0, total_cost: 0 }
        existing.request_count++
        existing.total_tokens += r.total_tokens
        existing.total_cost += r.cost
        userStats.set(r.user_id, existing)
      }
      return createMockResponse(
        Array.from(userStats.values()).map(stats => ({
          user_id: stats.user_id,
          email: stats.email,
          username: stats.username,
          request_count: stats.request_count * 50,
          total_tokens: stats.total_tokens * 50,
          total_cost: Number((stats.total_cost * 50).toFixed(2))
        }))
      )
    }

    if (groupBy === 'api_format') {
      const formatStats = new Map<string, { request_count: number; total_tokens: number; total_cost: number; actual_cost: number; response_times: number[] }>()
      for (const r of records) {
        const existing = formatStats.get(r.api_format) || { request_count: 0, total_tokens: 0, total_cost: 0, actual_cost: 0, response_times: [] }
        existing.request_count++
        existing.total_tokens += r.total_tokens
        existing.total_cost += r.cost
        existing.actual_cost += r.actual_cost || 0
        if (r.response_time_ms) existing.response_times.push(r.response_time_ms)
        formatStats.set(r.api_format, existing)
      }
      return createMockResponse(
        Array.from(formatStats.entries()).map(([api_format, stats]) => ({
          api_format,
          request_count: stats.request_count * 50,
          total_tokens: stats.total_tokens * 50,
          total_cost: Number((stats.total_cost * 50).toFixed(2)),
          actual_cost: Number((stats.actual_cost * 50).toFixed(2)),
          avg_response_time_ms: Math.round(stats.response_times.reduce((a, b) => a + b, 0) / stats.response_times.length || 0)
        }))
      )
    }

    return createMockResponse([])
  },

  'GET /api/admin/usage/active': async () => {
    await delay()
    requireAdmin()
    return createMockResponse({ requests: [] })
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
        config: m.config
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
  handler: (config: AxiosRequestConfig, params: Record<string, string>) => Promise<AxiosResponse<any>>
  params: Record<string, string>
}

type DynamicHandler = (config: AxiosRequestConfig, params: Record<string, string>) => Promise<AxiosResponse<any>>

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
function matchHandler(method: string, url: string): ((config: AxiosRequestConfig) => Promise<AxiosResponse<any>>) | null {
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
export async function handleMockRequest(config: AxiosRequestConfig): Promise<AxiosResponse<any> | null> {
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
    } catch (error: any) {
      if (error.response) {
        throw error
      }
      console.error('[Mock] Handler error:', error)
      throw { response: createMockResponse({ detail: '模拟请求处理失败' }, 500) }
    }
  }

  // 未匹配的请求返回默认响应
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
    const healthDetail = provider.endpoint_health_details.find(h => h.api_format === format)
    return {
      id: `ep-${providerId}-${index + 1}`,
      provider_id: providerId,
      provider_name: provider.name,
      api_format: format,
      base_url: format.includes('CLAUDE') ? 'https://api.anthropic.com' :
                format.includes('OPENAI') ? 'https://api.openai.com' :
                'https://generativelanguage.googleapis.com',
      timeout: 300,
      max_retries: 2,
      is_active: healthDetail?.is_active ?? true,
      total_keys: Math.ceil(Math.random() * 3) + 1,
      active_keys: Math.ceil(Math.random() * 2) + 1,
      created_at: provider.created_at,
      updated_at: new Date().toISOString()
    }
  })
}

// 为 provider 生成 keys（Key 归属 Provider，通过 api_formats 关联）
const PROVIDER_KEYS_CACHE: Record<string, any[]> = {}
function generateMockKeysForProvider(providerId: string, count: number = 2) {
  const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
  const formats = provider?.api_formats || []

  return Array.from({ length: count }, (_, i) => ({
    id: `key-${providerId}-${i + 1}`,
    provider_id: providerId,
    api_formats: i === 0 ? formats : formats.slice(0, 1),
    api_key_masked: `sk-***...${Math.random().toString(36).substring(2, 6)}`,
    name: i === 0 ? 'Primary Key' : `Backup Key ${i}`,
    rate_multiplier: 1.0,
    internal_priority: i + 1,
    health_score: 0.90 + Math.random() * 0.10,  // 0.90-1.00
    consecutive_failures: Math.random() > 0.8 ? 1 : 0,
    request_count: 1000 + Math.floor(Math.random() * 5000),
    success_count: 950 + Math.floor(Math.random() * 4800),
    error_count: Math.floor(Math.random() * 100),
    success_rate: 0.95 + Math.random() * 0.04,  // 0.95-0.99
    avg_response_time_ms: 800 + Math.floor(Math.random() * 600),
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    is_active: true,
    created_at: '2024-01-01T00:00:00Z',
    updated_at: new Date().toISOString()
  }))
}

// 为 provider 生成 models
function generateMockModelsForProvider(providerId: string) {
  const provider = MOCK_PROVIDERS.find(p => p.id === providerId)
  if (!provider) return []

  // 基于 provider 的 api_formats 选择合适的模型
  const hasClaude = provider.api_formats.some(f => f.includes('CLAUDE'))
  const hasOpenAI = provider.api_formats.some(f => f.includes('OPENAI'))
  const hasGemini = provider.api_formats.some(f => f.includes('GEMINI'))

  const models: any[] = []
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

  const grouped: Record<string, any[]> = {}
  for (const provider of MOCK_PROVIDERS) {
    const endpoints = generateMockEndpointsForProvider(provider.id)
    const baseUrlByFormat = Object.fromEntries(endpoints.map(e => [e.api_format, e.base_url]))
    const keys = PROVIDER_KEYS_CACHE[provider.id] || []
    for (const key of keys) {
      const formats: string[] = key.api_formats || []
      for (const fmt of formats) {
        if (!grouped[fmt]) grouped[fmt] = []
        grouped[fmt].push({
          ...key,
          api_format: fmt,
          provider_name: provider.name,
          endpoint_base_url: baseUrlByFormat[fmt],
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
  const models = (body.models || []).map((m: any, i: number) => ({
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
  return createMockResponse({ ...model, ...body, updated_at: new Date().toISOString() })
})

// GlobalModel 删除
registerDynamicRoute('DELETE', '/api/admin/models/global/:modelId', async (_config, params) => {
  await delay()
  requireAdmin()
  const model = MOCK_GLOBAL_MODELS.find(m => m.id === params.modelId)
  if (!model) {
    throw { response: createMockResponse({ detail: '模型不存在' }, 404) }
  }
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

// 用户更新
registerDynamicRoute('PATCH', '/api/admin/users/:userId', async (config, params) => {
  await delay()
  requireAdmin()
  const user = MOCK_ALL_USERS.find(u => u.id === params.userId)
  if (!user) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  const body = JSON.parse(config.data || '{}')
  return createMockResponse({ ...user, ...body })
})

// 用户删除
registerDynamicRoute('DELETE', '/api/admin/users/:userId', async (_config, params) => {
  await delay()
  requireAdmin()
  const user = MOCK_ALL_USERS.find(u => u.id === params.userId)
  if (!user) {
    throw { response: createMockResponse({ detail: '用户不存在' }, 404) }
  }
  return createMockResponse({ message: '删除成功（演示模式）' })
})

// 用户 API Keys
registerDynamicRoute('GET', '/api/admin/users/:userId/api-keys', async (_config, _params) => {
  await delay()
  requireAdmin()
  return createMockResponse(MOCK_USER_API_KEYS)
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

// 使用记录详情 - /api/admin/usage/:requestId
registerDynamicRoute('GET', '/api/admin/usage/:requestId', async (_config, params) => {
  await delay()
  requireAdmin()

  const records = getUsageRecords()
  const record = records.find(r => r.id === params.requestId)

  if (!record) {
    throw { response: createMockResponse({ detail: '请求记录不存在' }, 404) }
  }

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
        content: 'Hello! Can you help me understand how API gateways work?'
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
        text: 'API gateways are middleware services that sit between clients and backend services. They handle routing, authentication, rate limiting, and more...'
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

// 用户 interval-timeline 接口
mockHandlers['GET /api/users/me/usage/interval-timeline'] = async (config) => {
  await delay()
  const params = config.params || {}
  const hours = parseInt(params.hours) || 24
  const limit = parseInt(params.limit) || 5000
  const data = generateIntervalTimelineData(hours, limit, false)
  return createMockResponse(data)
}

// 管理员 interval-timeline 接口
mockHandlers['GET /api/admin/usage/cache-affinity/interval-timeline'] = async (config) => {
  await delay()
  requireAdmin()
  const params = config.params || {}
  const hours = parseInt(params.hours) || 24
  const limit = parseInt(params.limit) || 10000
  const userId = params.user_id
  const includeUserInfo = params.include_user_info === 'true' || params.include_user_info === true

  // 如果指定了 user_id，则不包含用户信息
  const data = generateIntervalTimelineData(hours, limit, includeUserInfo && !userId)
  return createMockResponse(data)
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

// TTL 分析接口
mockHandlers['GET /api/admin/usage/cache-affinity/ttl-analysis'] = async (config) => {
  await delay()
  requireAdmin()
  const params = config.params || {}
  const hours = parseInt(params.hours) || 168
  const data = generateTTLAnalysisData(hours)
  return createMockResponse(data)
}

// 缓存命中分析接口
mockHandlers['GET /api/admin/usage/cache-affinity/hit-analysis'] = async (config) => {
  await delay()
  requireAdmin()
  const params = config.params || {}
  const hours = parseInt(params.hours) || 168
  const data = generateCacheHitAnalysisData(hours)
  return createMockResponse(data)
}
