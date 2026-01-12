/**
 * Demo Mode Mock Data
 * 演示模式的模拟数据
 */

import type { User, LoginResponse } from '@/api/auth'
import type { DashboardStatsResponse, RecentRequest, ProviderStatus, DailyStatsResponse } from '@/api/dashboard'
import type { User as AdminUser } from '@/api/users'
import type { AdminApiKeysResponse } from '@/api/admin'
import type { Profile, UsageResponse } from '@/api/me'
import type { ProviderWithEndpointsSummary, GlobalModelResponse } from '@/api/endpoints/types'

// ========== 用户数据 ==========

export const MOCK_ADMIN_USER: User = {
  id: 'demo-admin-uuid-0001',
  username: 'Demo Admin',
  email: 'admin@demo.aether.io',
  role: 'admin',
  is_active: true,
  quota_usd: null,
  used_usd: 156.78,
  total_usd: 1234.56,
  allowed_providers: null,
  allowed_api_formats: null,
  allowed_models: null,
  created_at: '2024-01-01T00:00:00Z',
  last_login_at: new Date().toISOString()
}

export const MOCK_NORMAL_USER: User = {
  id: 'demo-user-uuid-0002',
  username: 'Demo User',
  email: 'user@demo.aether.io',
  role: 'user',
  is_active: true,
  quota_usd: 100,
  used_usd: 45.32,
  total_usd: 245.32,
  allowed_providers: null,
  allowed_api_formats: null,
  allowed_models: null,
  created_at: '2024-06-01T00:00:00Z',
  last_login_at: new Date().toISOString()
}

export const MOCK_LOGIN_RESPONSE_ADMIN: LoginResponse = {
  access_token: 'demo-access-token-admin',
  refresh_token: 'demo-refresh-token-admin',
  token_type: 'bearer',
  expires_in: 3600,
  user_id: MOCK_ADMIN_USER.id,
  email: MOCK_ADMIN_USER.email,
  username: MOCK_ADMIN_USER.username,
  role: 'admin'
}

export const MOCK_LOGIN_RESPONSE_USER: LoginResponse = {
  access_token: 'demo-access-token-user',
  refresh_token: 'demo-refresh-token-user',
  token_type: 'bearer',
  expires_in: 3600,
  user_id: MOCK_NORMAL_USER.id,
  email: MOCK_NORMAL_USER.email,
  username: MOCK_NORMAL_USER.username,
  role: 'user'
}

// ========== Profile 数据 ==========

export const MOCK_ADMIN_PROFILE: Profile = {
  id: MOCK_ADMIN_USER.id!,
  email: MOCK_ADMIN_USER.email!,
  username: MOCK_ADMIN_USER.username,
  role: 'admin',
  is_active: true,
  quota_usd: null,
  used_usd: 156.78,
  total_usd: 1234.56,
  created_at: '2024-01-01T00:00:00Z',
  updated_at: new Date().toISOString(),
  last_login_at: new Date().toISOString(),
  preferences: {
    theme: 'auto',
    language: 'zh-CN'
  }
}

export const MOCK_USER_PROFILE: Profile = {
  id: MOCK_NORMAL_USER.id!,
  email: MOCK_NORMAL_USER.email!,
  username: MOCK_NORMAL_USER.username,
  role: 'user',
  is_active: true,
  quota_usd: 100,
  used_usd: 45.32,
  total_usd: 245.32,
  created_at: '2024-06-01T00:00:00Z',
  updated_at: new Date().toISOString(),
  last_login_at: new Date().toISOString(),
  preferences: {
    theme: 'auto',
    language: 'zh-CN'
  }
}

// ========== Dashboard 数据 ==========

export const MOCK_DASHBOARD_STATS: DashboardStatsResponse = {
  stats: [
    {
      name: '今日请求',
      value: '1,234',
      subValue: '成功率 99.2%',
      change: '+12.5%',
      changeType: 'increase',
      icon: 'Activity'
    },
    {
      name: '今日 Token',
      value: '2.5M',
      subValue: '输入 1.8M / 输出 0.7M',
      change: '+8.3%',
      changeType: 'increase',
      icon: 'Zap'
    },
    {
      name: '今日费用',
      value: '$45.67',
      subValue: '节省 $12.34 (21%)',
      change: '-5.2%',
      changeType: 'decrease',
      icon: 'DollarSign'
    },
    {
      name: '活跃用户',
      value: '28',
      subValue: '总用户 156',
      change: '+3',
      changeType: 'increase',
      icon: 'Users'
    }
  ],
  today: {
    requests: 1234,
    tokens: 2500000,
    cost: 45.67,
    actual_cost: 33.33,
    cache_creation_tokens: 50000,
    cache_read_tokens: 200000
  },
  api_keys: {
    total: 45,
    active: 38
  },
  tokens: {
    month: 75000000
  },
  system_health: {
    avg_response_time: 1.23,
    error_rate: 0.8,
    error_requests: 10,
    fallback_count: 5,
    total_requests: 1234
  },
  cost_stats: {
    total_cost: 45.67,
    total_actual_cost: 33.33,
    cost_savings: 12.34
  },
  cache_stats: {
    cache_creation_tokens: 50000,
    cache_read_tokens: 200000,
    cache_creation_cost: 0.25,
    cache_read_cost: 0.10,
    cache_hit_rate: 0.35,
    total_cache_tokens: 250000
  },
  users: {
    total: 156,
    active: 28
  },
  token_breakdown: {
    input: 1800000,
    output: 700000,
    cache_creation: 50000,
    cache_read: 200000
  },
  // 普通用户专用字段
  monthly_cost: 45.67
}

export const MOCK_RECENT_REQUESTS: RecentRequest[] = [
  { id: 'req-001', user: 'alice', model: 'claude-sonnet-4-5-20250929', tokens: 15234, time: '2 分钟前' },
  { id: 'req-002', user: 'bob', model: 'gpt-5.1', tokens: 8765, time: '5 分钟前' },
  { id: 'req-003', user: 'charlie', model: 'claude-opus-4-5-20251101', tokens: 32100, time: '8 分钟前' },
  { id: 'req-004', user: 'diana', model: 'gemini-3-pro-preview', tokens: 4521, time: '12 分钟前' },
  { id: 'req-005', user: 'eve', model: 'claude-sonnet-4-5-20250929', tokens: 9876, time: '15 分钟前' },
  { id: 'req-006', user: 'frank', model: 'gpt-5.1-codex-mini', tokens: 2345, time: '18 分钟前' },
  { id: 'req-007', user: 'grace', model: 'claude-haiku-4-5-20251001', tokens: 6789, time: '22 分钟前' },
  { id: 'req-008', user: 'henry', model: 'gemini-3-pro-preview', tokens: 12345, time: '25 分钟前' }
]

export const MOCK_PROVIDER_STATUS: ProviderStatus[] = [
  { name: 'Anthropic Official', status: 'active', requests: 456 },
  { name: 'OpenAI Official', status: 'active', requests: 389 },
  { name: 'Google AI', status: 'active', requests: 234 },
  { name: 'AWS Bedrock', status: 'active', requests: 89 },
  { name: 'Azure OpenAI', status: 'inactive', requests: 0 },
  { name: 'Vertex AI', status: 'active', requests: 66 }
]

// 生成过去7天的每日统计数据
function generateDailyStats(): DailyStatsResponse {
  const dailyStats = []
  const now = new Date()

  for (let i = 6; i >= 0; i--) {
    const date = new Date(now)
    date.setDate(date.getDate() - i)
    const dateStr = date.toISOString().split('T')[0]

    const baseRequests = 800 + Math.floor(Math.random() * 600)
    const baseTokens = 1500000 + Math.floor(Math.random() * 1500000)
    const baseCost = 30 + Math.random() * 30

    dailyStats.push({
      date: dateStr,
      requests: baseRequests,
      tokens: baseTokens,
      cost: Number(baseCost.toFixed(2)),
      avg_response_time: 0.8 + Math.random() * 0.8,
      unique_models: 8 + Math.floor(Math.random() * 5),
      unique_providers: 4 + Math.floor(Math.random() * 3),
      model_breakdown: [
        { model: 'claude-sonnet-4-5-20250929', requests: Math.floor(baseRequests * 0.35), tokens: Math.floor(baseTokens * 0.35), cost: Number((baseCost * 0.35).toFixed(2)) },
        { model: 'gpt-5.1', requests: Math.floor(baseRequests * 0.25), tokens: Math.floor(baseTokens * 0.25), cost: Number((baseCost * 0.25).toFixed(2)) },
        { model: 'claude-opus-4-5-20251101', requests: Math.floor(baseRequests * 0.15), tokens: Math.floor(baseTokens * 0.15), cost: Number((baseCost * 0.20).toFixed(2)) },
        { model: 'gemini-3-pro-preview', requests: Math.floor(baseRequests * 0.15), tokens: Math.floor(baseTokens * 0.15), cost: Number((baseCost * 0.10).toFixed(2)) },
        { model: 'claude-haiku-4-5-20251001', requests: Math.floor(baseRequests * 0.10), tokens: Math.floor(baseTokens * 0.10), cost: Number((baseCost * 0.10).toFixed(2)) }
      ]
    })
  }

  return {
    daily_stats: dailyStats,
    model_summary: [
      { model: 'claude-sonnet-4-5-20250929', requests: 2456, tokens: 8500000, cost: 125.45, avg_response_time: 1.2, cost_per_request: 0.051, tokens_per_request: 3461 },
      { model: 'gpt-5.1', requests: 1823, tokens: 6200000, cost: 98.32, avg_response_time: 0.9, cost_per_request: 0.054, tokens_per_request: 3401 },
      { model: 'claude-opus-4-5-20251101', requests: 987, tokens: 4100000, cost: 156.78, avg_response_time: 2.1, cost_per_request: 0.159, tokens_per_request: 4154 },
      { model: 'gemini-3-pro-preview', requests: 1234, tokens: 3800000, cost: 28.56, avg_response_time: 0.6, cost_per_request: 0.023, tokens_per_request: 3079 },
      { model: 'claude-haiku-4-5-20251001', requests: 2100, tokens: 5200000, cost: 32.10, avg_response_time: 0.5, cost_per_request: 0.015, tokens_per_request: 2476 }
    ],
    period: {
      start_date: dailyStats[0].date,
      end_date: dailyStats[dailyStats.length - 1].date,
      days: 7
    }
  }
}

export const MOCK_DAILY_STATS = generateDailyStats()

// ========== 用户管理数据 ==========

export const MOCK_ALL_USERS: AdminUser[] = [
  {
    id: 'demo-admin-uuid-0001',
    username: 'Demo Admin',
    email: 'admin@demo.aether.io',
    role: 'admin',
    is_active: true,
    quota_usd: null,
    used_usd: 156.78,
    total_usd: 1234.56,
    allowed_providers: null,
    allowed_api_formats: null,
    allowed_models: null,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0002',
    username: 'Demo User',
    email: 'user@demo.aether.io',
    role: 'user',
    is_active: true,
    quota_usd: 100,
    used_usd: 45.32,
    total_usd: 245.32,
    allowed_providers: null,
    allowed_api_formats: null,
    allowed_models: null,
    created_at: '2024-06-01T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0003',
    username: 'Alice Wang',
    email: 'alice@example.com',
    role: 'user',
    is_active: true,
    quota_usd: 50,
    used_usd: 23.45,
    total_usd: 123.45,
    allowed_providers: null,
    allowed_api_formats: null,
    allowed_models: null,
    created_at: '2024-03-15T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0004',
    username: 'Bob Zhang',
    email: 'bob@example.com',
    role: 'user',
    is_active: true,
    quota_usd: 200,
    used_usd: 89.12,
    total_usd: 589.12,
    allowed_providers: null,
    allowed_api_formats: null,
    allowed_models: null,
    created_at: '2024-02-20T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0005',
    username: 'Charlie Li',
    email: 'charlie@example.com',
    role: 'user',
    is_active: false,
    quota_usd: 30,
    used_usd: 30.00,
    total_usd: 30.00,
    allowed_providers: null,
    allowed_api_formats: null,
    allowed_models: null,
    created_at: '2024-04-10T00:00:00Z'
  }
]

// ========== API Key 数据 ==========

export const MOCK_USER_API_KEYS = [
  {
    id: 'key-uuid-001',
    key_display: 'sk-ae...x7f9',
    name: '开发环境',
    created_at: '2024-06-15T00:00:00Z',
    last_used_at: new Date().toISOString(),
    is_active: true,
    is_standalone: false,
    total_requests: 1234,
    total_cost_usd: 45.67,
    force_capabilities: null
  },
  {
    id: 'key-uuid-002',
    key_display: 'sk-ae...m2k8',
    name: '生产环境',
    created_at: '2024-07-01T00:00:00Z',
    last_used_at: new Date().toISOString(),
    is_active: true,
    is_standalone: false,
    total_requests: 5678,
    total_cost_usd: 123.45,
    force_capabilities: { cache_1h: true }
  },
  {
    id: 'key-uuid-003',
    key_display: 'sk-ae...p9q1',
    name: '测试用途',
    created_at: '2024-08-01T00:00:00Z',
    is_active: false,
    is_standalone: false,
    total_requests: 100,
    total_cost_usd: 2.34,
    force_capabilities: null
  }
]

export const MOCK_ADMIN_API_KEYS: AdminApiKeysResponse = {
  api_keys: [
    {
      id: 'standalone-key-001',
      user_id: 'demo-user-uuid-0002',
      user_email: 'user@demo.aether.io',
      username: 'Demo User',
      name: '独立余额 Key #1',
      key_display: 'sk-sa...abc1',
      is_active: true,
      is_standalone: true,
      balance_used_usd: 25.50,
      current_balance_usd: 74.50,
      total_requests: 500,
      total_tokens: 1500000,
      total_cost_usd: 25.50,
      created_at: '2024-09-01T00:00:00Z',
      last_used_at: new Date().toISOString()
    },
    {
      id: 'standalone-key-002',
      user_id: 'demo-user-uuid-0003',
      user_email: 'alice@example.com',
      username: 'Alice Wang',
      name: '独立余额 Key #2',
      key_display: 'sk-sa...def2',
      is_active: true,
      is_standalone: true,
      balance_used_usd: 45.00,
      current_balance_usd: 55.00,
      total_requests: 800,
      total_tokens: 2400000,
      total_cost_usd: 45.00,
      rate_limit: 60,
      created_at: '2024-08-15T00:00:00Z',
      last_used_at: new Date().toISOString()
    }
  ],
  total: 2,
  limit: 20,
  skip: 0
}

// ========== Provider 数据 ==========

export const MOCK_PROVIDERS: ProviderWithEndpointsSummary[] = [
  {
    id: 'provider-001',
    name: 'DuckCodingFree',
    description: '',
    website: 'https://duckcoding.com',
    provider_priority: 1,
    billing_type: 'free_tier',
    monthly_used_usd: 0.0,
    is_active: true,
    total_endpoints: 3,
    active_endpoints: 3,
    total_keys: 3,
    active_keys: 3,
    total_models: 7,
    active_models: 7,
    avg_health_score: 0.91,
    unhealthy_endpoints: 0,
    api_formats: ['CLAUDE_CLI', 'GEMINI_CLI', 'OPENAI_CLI'],
    endpoint_health_details: [
      { api_format: 'CLAUDE_CLI', health_score: 0.73, is_active: true, active_keys: 1 },
      { api_format: 'GEMINI_CLI', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'OPENAI_CLI', health_score: 1.0, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-09T14:10:36.446217+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-002',
    name: 'OpenClaudeCode',
    description: '',
    website: 'https://www.openclaudecode.cn',
    provider_priority: 2,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 545.18,
    is_active: true,
    total_endpoints: 2,
    active_endpoints: 2,
    total_keys: 3,
    active_keys: 3,
    total_models: 3,
    active_models: 1,
    avg_health_score: 0.825,
    unhealthy_endpoints: 0,
    api_formats: ['CLAUDE', 'CLAUDE_CLI'],
    endpoint_health_details: [
      { api_format: 'CLAUDE', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'CLAUDE_CLI', health_score: 0.65, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-07T22:58:15.044538+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-003',
    name: '88Code',
    description: '',
    website: 'https://www.88code.org/',
    provider_priority: 3,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 33.36,
    is_active: true,
    total_endpoints: 2,
    active_endpoints: 2,
    total_keys: 2,
    active_keys: 2,
    total_models: 5,
    active_models: 5,
    avg_health_score: 1.0,
    unhealthy_endpoints: 0,
    api_formats: ['CLAUDE_CLI', 'OPENAI_CLI'],
    endpoint_health_details: [
      { api_format: 'CLAUDE_CLI', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'OPENAI_CLI', health_score: 1.0, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-07T22:56:46.361092+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-004',
    name: 'IKunCode',
    description: '',
    website: 'https://api.ikuncode.cc',
    provider_priority: 4,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 268.65,
    is_active: true,
    total_endpoints: 4,
    active_endpoints: 4,
    total_keys: 3,
    active_keys: 3,
    total_models: 7,
    active_models: 7,
    avg_health_score: 1.0,
    unhealthy_endpoints: 0,
    api_formats: ['CLAUDE_CLI', 'GEMINI', 'GEMINI_CLI', 'OPENAI_CLI'],
    endpoint_health_details: [
      { api_format: 'CLAUDE_CLI', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'GEMINI', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'GEMINI_CLI', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'OPENAI_CLI', health_score: 1.0, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-07T15:16:55.807595+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-005',
    name: 'DuckCoding',
    description: '',
    website: 'https://duckcoding.com',
    provider_priority: 5,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 5.29,
    is_active: true,
    total_endpoints: 6,
    active_endpoints: 6,
    total_keys: 11,
    active_keys: 11,
    total_models: 8,
    active_models: 8,
    avg_health_score: 0.863,
    unhealthy_endpoints: 1,
    api_formats: ['CLAUDE', 'CLAUDE_CLI', 'GEMINI', 'GEMINI_CLI', 'OPENAI', 'OPENAI_CLI'],
    endpoint_health_details: [
      { api_format: 'CLAUDE', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'CLAUDE_CLI', health_score: 0.48, is_active: true, active_keys: 2 },
      { api_format: 'GEMINI', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'GEMINI_CLI', health_score: 0.85, is_active: true, active_keys: 2 },
      { api_format: 'OPENAI', health_score: 0.85, is_active: true, active_keys: 2 },
      { api_format: 'OPENAI_CLI', health_score: 1.0, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-07T22:56:09.712806+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-006',
    name: 'Privnode',
    description: '',
    website: 'https://privnode.com',
    provider_priority: 6,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 0.0,
    is_active: true,
    total_endpoints: 0,
    active_endpoints: 0,
    total_keys: 0,
    active_keys: 0,
    total_models: 6,
    active_models: 6,
    avg_health_score: 1.0,
    unhealthy_endpoints: 0,
    api_formats: [],
    endpoint_health_details: [],
    created_at: '2024-12-07T22:57:18.069024+08:00',
    updated_at: new Date().toISOString()
  },
  {
    id: 'provider-007',
    name: 'UndyingAPI',
    description: '',
    website: 'https://vip.undyingapi.com',
    provider_priority: 7,
    billing_type: 'pay_as_you_go',
    monthly_used_usd: 6.6,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 1,
    active_models: 1,
    avg_health_score: 1.0,
    unhealthy_endpoints: 0,
    api_formats: ['GEMINI'],
    endpoint_health_details: [
      { api_format: 'GEMINI', health_score: 1.0, is_active: true, active_keys: 1 }
    ],
    created_at: '2024-12-07T23:00:42.559105+08:00',
    updated_at: new Date().toISOString()
  }
]

// ========== GlobalModel 数据 ==========

export const MOCK_GLOBAL_MODELS: GlobalModelResponse[] = [
  {
    id: 'gm-001',
    name: 'claude-haiku-4-5-20251001',
    display_name: 'claude-haiku-4-5',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 1.00, output_price_per_1m: 5.00, cache_creation_price_per_1m: 1.25, cache_read_price_per_1m: 0.1 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'Anthropic 最快速的 Claude 4 系列模型'
    },
    provider_count: 3,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-002',
    name: 'claude-opus-4-5-20251101',
    display_name: 'claude-opus-4-5',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 5.00, output_price_per_1m: 25.00, cache_creation_price_per_1m: 6.25, cache_read_price_per_1m: 0.5 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'Anthropic 最强大的模型'
    },
    provider_count: 2,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-003',
    name: 'claude-sonnet-4-5-20250929',
    display_name: 'claude-sonnet-4-5',
    is_active: true,
    default_tiered_pricing: {
      tiers: [
        {
          "up_to": 200000,
          "input_price_per_1m": 3,
          "output_price_per_1m": 15,
          "cache_creation_price_per_1m": 3.75,
          "cache_read_price_per_1m": 0.3,
          "cache_ttl_pricing": [
            {
              "ttl_minutes": 60,
              "cache_creation_price_per_1m": 6
            }
          ]
        },
        {
          "up_to": null,
          "input_price_per_1m": 6,
          "output_price_per_1m": 22.5,
          "cache_creation_price_per_1m": 7.5,
          "cache_read_price_per_1m": 0.6,
          "cache_ttl_pricing": [
            {
              "ttl_minutes": 60,
              "cache_creation_price_per_1m": 12
            }
          ]
        }
      ]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'Anthropic 平衡型模型，支持 1h 缓存和 CLI 1M 上下文'
    },
    supported_capabilities: ['cache_1h', 'cli_1m'],
    provider_count: 3,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-004',
    name: 'gemini-3-pro-image-preview',
    display_name: 'gemini-3-pro-image-preview',
    is_active: true,
    default_price_per_request: 0.300,
    default_tiered_pricing: {
      tiers: []
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: false,
      image_generation: true,
      description: 'Google Gemini 3 Pro 图像生成预览版'
    },
    provider_count: 1,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-005',
    name: 'gemini-3-pro-preview',
    display_name: 'gemini-3-pro-preview',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 2.00, output_price_per_1m: 12.00 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'Google Gemini 3 Pro 预览版'
    },
    provider_count: 1,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-006',
    name: 'gpt-5.1',
    display_name: 'gpt-5.1',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 1.25, output_price_per_1m: 10.00 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'OpenAI GPT-5.1 模型'
    },
    provider_count: 2,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-007',
    name: 'gpt-5.1-codex',
    display_name: 'gpt-5.1-codex',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 1.25, output_price_per_1m: 10.00 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'OpenAI GPT-5.1 Codex 代码专用模型'
    },
    provider_count: 2,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-008',
    name: 'gpt-5.1-codex-max',
    display_name: 'gpt-5.1-codex-max',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 1.25, output_price_per_1m: 10.00 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'OpenAI GPT-5.1 Codex Max 代码专用增强版'
    },
    provider_count: 2,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-009',
    name: 'gpt-5.1-codex-mini',
    display_name: 'gpt-5.1-codex-mini',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 1.25, output_price_per_1m: 10.00 }]
    },
    config: {
      streaming: true,
      vision: true,
      function_calling: true,
      extended_thinking: true,
      description: 'OpenAI GPT-5.1 Codex Mini 轻量代码模型'
    },
    provider_count: 2,
    created_at: '2024-01-01T00:00:00Z'
  }
]

// ========== Usage 数据 ==========

export const MOCK_USAGE_RESPONSE: UsageResponse = {
  total_requests: 1234,
  total_input_tokens: 1800000,
  total_output_tokens: 700000,
  total_tokens: 2500000,
  total_cost: 45.67,
  total_actual_cost: 33.33,
  avg_response_time: 1.23,
  quota_usd: 100,
  used_usd: 45.32,
  summary_by_model: [
    { model: 'claude-sonnet-4-5-20250929', requests: 456, input_tokens: 650000, output_tokens: 250000, total_tokens: 900000, total_cost_usd: 18.50, actual_total_cost_usd: 13.50 },
    { model: 'gpt-5.1', requests: 312, input_tokens: 480000, output_tokens: 180000, total_tokens: 660000, total_cost_usd: 12.30, actual_total_cost_usd: 9.20 },
    { model: 'claude-haiku-4-5-20251001', requests: 289, input_tokens: 420000, output_tokens: 170000, total_tokens: 590000, total_cost_usd: 8.50, actual_total_cost_usd: 6.30 },
    { model: 'gemini-3-pro-preview', requests: 177, input_tokens: 250000, output_tokens: 100000, total_tokens: 350000, total_cost_usd: 6.37, actual_total_cost_usd: 4.33 }
  ],
  records: [
    {
      id: 'usage-001',
      provider: 'anthropic',
      model: 'claude-sonnet-4-5-20250929',
      input_tokens: 1500,
      output_tokens: 800,
      total_tokens: 2300,
      cost: 0.0165,
      response_time_ms: 1234,
      is_stream: true,
      created_at: new Date().toISOString(),
      status_code: 200,
      input_price_per_1m: 3,
      output_price_per_1m: 15
    },
    {
      id: 'usage-002',
      provider: 'openai',
      model: 'gpt-5.1',
      input_tokens: 2000,
      output_tokens: 500,
      total_tokens: 2500,
      cost: 0.01,
      response_time_ms: 890,
      is_stream: false,
      created_at: new Date(Date.now() - 300000).toISOString(),
      status_code: 200,
      input_price_per_1m: 2.5,
      output_price_per_1m: 10
    }
  ]
}

// ========== 系统配置 ==========

export const MOCK_SYSTEM_CONFIGS = [
  { key: 'rate_limit_enabled', value: true, description: '是否启用速率限制' },
  { key: 'default_rate_limit', value: 60, description: '默认速率限制（请求/分钟）' },
  { key: 'cache_enabled', value: true, description: '是否启用缓存' },
  { key: 'default_cache_ttl', value: 3600, description: '默认缓存 TTL（秒）' },
  { key: 'fallback_enabled', value: true, description: '是否启用故障转移' },
  { key: 'max_fallback_attempts', value: 3, description: '最大故障转移次数' }
]

// ========== API 格式 ==========

export const MOCK_API_FORMATS = {
  formats: [
    { value: 'claude', label: 'Claude API', default_path: '/v1/messages', aliases: [] },
    { value: 'claude_cli', label: 'Claude CLI', default_path: '/v1/messages', aliases: [] },
    { value: 'openai', label: 'OpenAI API', default_path: '/v1/chat/completions', aliases: [] },
    { value: 'openai_cli', label: 'OpenAI Responses API', default_path: '/v1/responses', aliases: [] },
    { value: 'gemini', label: 'Gemini API', default_path: '/v1beta/models', aliases: [] },
    { value: 'gemini_cli', label: 'Gemini CLI', default_path: '/v1beta/models', aliases: [] }
  ]
}
