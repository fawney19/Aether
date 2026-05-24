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
import type { ModuleStatus } from '@/api/modules'

// ========== 用户数据 ==========

const MOCK_ADMIN_BILLING = {
  id: 'wallet-demo-admin',
  balance: 0,
  recharge_balance: 0,
  gift_balance: 0,
  refundable_balance: 0,
  currency: 'USD',
  status: 'active',
  limit_mode: 'unlimited' as const,
  unlimited: true,
  total_recharged: 0,
  total_consumed: 1234.56,
  total_refunded: 0,
  total_adjusted: 0,
  updated_at: new Date().toISOString(),
}

const MOCK_USER_BILLING = {
  id: 'wallet-demo-user',
  balance: 54.68,
  recharge_balance: 40,
  gift_balance: 14.68,
  refundable_balance: 40,
  currency: 'USD',
  status: 'active',
  limit_mode: 'finite' as const,
  unlimited: false,
  total_recharged: 100,
  total_consumed: 45.32,
  total_refunded: 0,
  total_adjusted: 0,
  updated_at: new Date().toISOString(),
}

export const MOCK_ADMIN_USER: User = {
  id: 'demo-admin-uuid-0001',
  username: 'Demo Admin',
  email: 'admin@demo.aether.io',
  role: 'admin',
  is_active: true,
  billing: MOCK_ADMIN_BILLING,
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
  billing: MOCK_USER_BILLING,
  allowed_providers: null,
  allowed_api_formats: null,
  allowed_models: null,
  created_at: '2024-06-01T00:00:00Z',
  last_login_at: new Date().toISOString()
}

export const MOCK_LOGIN_RESPONSE_ADMIN: LoginResponse = {
  access_token: 'demo-access-token-admin',
  token_type: 'bearer',
  expires_in: 3600,
  user_id: MOCK_ADMIN_USER.id,
  email: MOCK_ADMIN_USER.email,
  username: MOCK_ADMIN_USER.username,
  role: 'admin'
}

export const MOCK_LOGIN_RESPONSE_USER: LoginResponse = {
  access_token: 'demo-access-token-user',
  token_type: 'bearer',
  expires_in: 3600,
  user_id: MOCK_NORMAL_USER.id,
  email: MOCK_NORMAL_USER.email,
  username: MOCK_NORMAL_USER.username,
  role: 'user'
}

// ========== Profile 数据 ==========

export const MOCK_ADMIN_PROFILE: Profile = {
  id: MOCK_ADMIN_USER.id ?? '',
  email: MOCK_ADMIN_USER.email ?? '',
  username: MOCK_ADMIN_USER.username,
  role: 'admin',
  is_active: true,
  billing: MOCK_ADMIN_BILLING,
  auth_source: 'local',
  has_password: true,
  created_at: '2024-01-01T00:00:00Z',
  updated_at: new Date().toISOString(),
  last_login_at: new Date().toISOString(),
  preferences: {
    theme: 'auto',
    language: 'zh-CN'
  }
}

export const MOCK_USER_PROFILE: Profile = {
  id: MOCK_NORMAL_USER.id ?? '',
  email: MOCK_NORMAL_USER.email ?? '',
  username: MOCK_NORMAL_USER.username,
  role: 'user',
  is_active: true,
  billing: MOCK_USER_BILLING,
  auth_source: 'local',
  has_password: true,
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
    cache_hit_rate: 35.0,
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
    unlimited: true,
    is_active: true,
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
    unlimited: false,
    is_active: true,
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
    unlimited: false,
    is_active: true,
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
    unlimited: false,
    is_active: true,
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
    unlimited: false,
    is_active: false,
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
    api_formats: ['CLAUDE_MESSAGES', 'GEMINI_GENERATE_CONTENT', 'OPENAI_RESPONSES'],
    endpoint_health_details: [
      { api_format: 'CLAUDE_MESSAGES', health_score: 0.73, is_active: true, active_keys: 1 },
      { api_format: 'GEMINI_GENERATE_CONTENT', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'OPENAI_RESPONSES', health_score: 1.0, is_active: true, active_keys: 1 }
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
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 3,
    active_keys: 3,
    total_models: 3,
    active_models: 1,
    avg_health_score: 0.825,
    unhealthy_endpoints: 0,
    api_formats: ['claude:messages'],
    endpoint_health_details: [
      { api_format: 'claude:messages', health_score: 1.0, is_active: true, active_keys: 2 }
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
    api_formats: ['claude:messages', 'openai:responses'],
    endpoint_health_details: [
      { api_format: 'claude:messages', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'openai:responses', health_score: 1.0, is_active: true, active_keys: 1 }
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
    total_endpoints: 3,
    active_endpoints: 3,
    total_keys: 3,
    active_keys: 3,
    total_models: 7,
    active_models: 7,
    avg_health_score: 1.0,
    unhealthy_endpoints: 0,
    api_formats: ['claude:messages', 'gemini:generate_content', 'openai:responses'],
    endpoint_health_details: [
      { api_format: 'claude:messages', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'gemini:generate_content', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'openai:responses', health_score: 1.0, is_active: true, active_keys: 1 }
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
    total_endpoints: 5,
    active_endpoints: 5,
    total_keys: 11,
    active_keys: 11,
    total_models: 9,
    active_models: 9,
    avg_health_score: 0.863,
    unhealthy_endpoints: 1,
    api_formats: ['claude:messages', 'gemini:generate_content', 'openai:chat', 'openai:responses', 'openai:embedding'],
    endpoint_health_details: [
      { api_format: 'claude:messages', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'gemini:generate_content', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'openai:chat', health_score: 0.85, is_active: true, active_keys: 2 },
      { api_format: 'openai:responses', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'openai:embedding', health_score: 0.98, is_active: true, active_keys: 1 }
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
    api_formats: ['gemini:generate_content'],
    endpoint_health_details: [
      { api_format: 'gemini:generate_content', health_score: 1.0, is_active: true, active_keys: 1 }
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
  },
  {
    id: 'gm-010',
    name: 'text-embedding-3-small',
    display_name: 'text-embedding-3-small',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 0.02, output_price_per_1m: 0 }]
    },
    supported_capabilities: ['embedding'],
    supports_embedding: true,
    config: {
      streaming: false,
      embedding: true,
      model_type: 'embedding',
      api_formats: ['openai:embedding'],
      dimensions: 1536,
      description: 'OpenAI 文本向量嵌入模型'
    },
    provider_count: 1,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'gm-rerank-001',
    name: 'bge-reranker-base',
    display_name: 'bge-reranker-base',
    is_active: true,
    default_tiered_pricing: {
      tiers: [{ up_to: null, input_price_per_1m: 0.05, output_price_per_1m: 0 }]
    },
    supported_capabilities: ['rerank'],
    config: {
      streaming: false,
      rerank: true,
      model_type: 'rerank',
      api_formats: ['openai:rerank'],
      description: '文本重排序模型'
    },
    provider_count: 1,
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
  billing: MOCK_USER_BILLING,
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

export const MOCK_SYSTEM_CONFIGS: Array<{ key: string; value: unknown; description?: string }> = [
  { key: 'rate_limit_enabled', value: true, description: '是否启用速率限制' },
  { key: 'default_rate_limit', value: 60, description: '默认速率限制（请求/分钟）' },
  { key: 'cache_enabled', value: true, description: '是否启用缓存' },
  { key: 'default_cache_ttl', value: 3600, description: '默认缓存 TTL（秒）' },
  { key: 'fallback_enabled', value: true, description: '是否启用故障转移' },
  { key: 'max_fallback_attempts', value: 3, description: '最大故障转移次数' },
  { key: 'module.important_notification.enabled', value: false, description: '通知服务总开关' },
  { key: 'module.important_notification.email_enabled', value: false, description: '通知服务邮件推送开关' },
  { key: 'module.important_notification.email_recipients', value: '', description: '通知服务管理员收件人' },
  { key: 'module.important_notification.default_channel', value: 'all', description: '通知服务全局推送服务' },
  {
    key: 'module.important_notification.items',
    value: [
      {
        key: 'provider_quota_alert',
        name: '号池额度不足',
        enabled: true,
        channel: 'global',
        title_template: '',
        markdown_template: '',
        text_template: '',
        user_email_enabled: false,
        system: true,
      },
      {
        key: 'provider_pool_abnormal',
        name: '号池异常',
        enabled: true,
        channel: 'global',
        title_template: '号池异常：{provider_name}',
        markdown_template: '号池 `{provider_name}` 出现异常，请检查服务状态。',
        text_template: '号池 {provider_name} 出现异常，请检查服务状态。',
        user_email_enabled: false,
        system: true,
      },
      {
        key: 'user_balance_low',
        name: '用户余额不足',
        enabled: true,
        channel: 'email',
        title_template: '余额不足提醒',
        markdown_template: '你的账户余额已低于提醒阈值，请及时处理。',
        text_template: '你的账户余额已低于提醒阈值，请及时处理。',
        user_email_enabled: true,
        system: true,
      },
    ],
    description: '通知服务通知项和模板',
  },
  { key: 'module.server_chan_push.enabled', value: false, description: 'Server 酱推送开关' },
  { key: 'module.server_chan_push.send_key', value: null, description: 'Server 酱 SendKey' },
  { key: 'module.server_chan_push.template', value: '', description: 'Server 酱推送模板' },
  { key: 'module.bark_push.enabled', value: false, description: 'Bark 推送开关' },
  { key: 'module.bark_push.device_key', value: null, description: 'Bark Device Key' },
  { key: 'module.bark_push.server_url', value: 'https://api.day.app', description: 'Bark 服务器地址' },
  { key: 'module.bark_push.template', value: '', description: 'Bark 推送模板' },
  { key: 'proxy_node_metrics_1m_retention_days', value: 30, description: '代理节点 1m 指标保留天数' },
  { key: 'proxy_node_metrics_1h_retention_days', value: 180, description: '代理节点 1h 指标保留天数' },
  { key: 'proxy_node_metrics_cleanup_batch_size', value: 5000, description: '代理节点指标每批次清理条数' }
]

const MOCK_MODULE_DEFINITIONS: Array<Omit<ModuleStatus, 'active' | 'health'> & { health?: ModuleStatus['health'] }> = [
  {
    name: 'management_tokens',
    display_name: '访问令牌',
    description: '管理 API 访问令牌，支持细粒度权限控制和 IP 限制',
    category: 'security',
    available: true,
    enabled: true,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/management-tokens',
    admin_menu_icon: null,
    admin_menu_group: null,
    admin_menu_order: 0,
  },
  {
    name: 'ldap',
    display_name: 'LDAP 认证',
    description: '支持通过 LDAP/Active Directory 进行用户认证',
    category: 'auth',
    available: true,
    enabled: false,
    config_validated: false,
    config_error: '请先配置 LDAP 连接信息',
    admin_route: '/admin/ldap',
    admin_menu_icon: 'Users',
    admin_menu_group: 'system',
    admin_menu_order: 50,
  },
  {
    name: 'oauth',
    display_name: 'OAuth 登录',
    description: '支持通过第三方 OAuth Provider 登录/绑定账号',
    category: 'auth',
    available: true,
    enabled: true,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/oauth',
    admin_menu_icon: 'Key',
    admin_menu_group: null,
    admin_menu_order: 55,
  },
  {
    name: 'important_notification',
    display_name: '通知服务',
    description: '统一管理通知项、模板和推送服务选择，供后台任务和用户通知使用',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: false,
    config_error: '请先完成通知服务推送渠道配置',
    admin_route: '/admin/notification-service',
    admin_menu_icon: 'BellRing',
    admin_menu_group: null,
    admin_menu_order: 58,
  },
  {
    name: 'server_chan_push',
    display_name: 'Server 酱推送',
    description: '第三方推送服务，配置 Server 酱 Turbo SendKey 并测试微信推送',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: false,
    config_error: '请先配置 Server 酱 SendKey',
    admin_route: '/admin/modules/server-chan',
    admin_menu_icon: 'Send',
    admin_menu_group: 'system',
    admin_menu_order: 59,
  },
  {
    name: 'bark_push',
    display_name: 'Bark 推送',
    description: '第三方推送服务，配置 Bark Device Key 并测试 iOS 推送',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: false,
    config_error: '请先配置 Bark Device Key',
    admin_route: '/admin/modules/bark',
    admin_menu_icon: 'Send',
    admin_menu_group: 'system',
    admin_menu_order: 59,
  },
  {
    name: 'chat_pii_redaction',
    display_name: '敏感信息保护',
    description: '发送给供应商前将聊天消息中的敏感信息替换为占位符，返回客户端前自动还原。',
    category: 'security',
    available: true,
    enabled: false,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/modules/chat-pii-redaction',
    admin_menu_icon: 'ShieldCheck',
    admin_menu_group: 'system',
    admin_menu_order: 59,
  },
  {
    name: 'model_directives',
    display_name: '模型后缀参数',
    description: '允许通过模型名后缀覆盖推理参数',
    category: 'integration',
    available: true,
    enabled: true,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/model-directives',
    admin_menu_icon: 'SlidersHorizontal',
    admin_menu_group: null,
    admin_menu_order: 59,
  },
  {
    name: 'gemini_files',
    display_name: '文件缓存',
    description: '管理 Gemini Files API 上传的文件，支持文件上传、查看和删除',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: false,
    config_error: '至少启用一个具有「Gemini 文件 API」能力的 Key',
    admin_route: '/admin/gemini-files',
    admin_menu_icon: 'FileUp',
    admin_menu_group: 'system',
    admin_menu_order: 60,
    health: 'degraded',
  },
  {
    name: 'proxy_nodes',
    display_name: '代理节点',
    description: '添加Http/Socket代理节点, 或使用Aether-Proxy自动连接代理节点.',
    category: 'integration',
    available: true,
    enabled: true,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/proxy-nodes',
    admin_menu_icon: 'Server',
    admin_menu_group: 'system',
    admin_menu_order: 60,
  },
  {
    name: 'payment_gateways',
    display_name: '支付配置',
    description: '配置易支付、支付宝官方、微信支付官方和 Stripe 等支付网关',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/payment-gateways',
    admin_menu_icon: 'CreditCard',
    admin_menu_group: null,
    admin_menu_order: 70,
  },
  {
    name: 'referral',
    display_name: '邀请返利',
    description: '管理用户邀请关系与返利记录，支持比例返利和人头返利',
    category: 'integration',
    available: true,
    enabled: false,
    config_validated: true,
    config_error: null,
    admin_route: '/admin/referrals',
    admin_menu_icon: 'Gift',
    admin_menu_group: 'management',
    admin_menu_order: 75,
  },
]

export const MOCK_MODULE_STATUSES: Record<string, ModuleStatus> = Object.fromEntries(
  MOCK_MODULE_DEFINITIONS.map(module => [
    module.name,
    {
      ...module,
      active: module.available && module.enabled && module.config_validated,
      health: module.health ?? 'healthy',
    },
  ])
) as Record<string, ModuleStatus>

// ========== API 格式 ==========

export const MOCK_API_FORMATS = {
  formats: [
    { value: 'claude:messages', label: 'Claude Messages', default_path: '/v1/messages', aliases: [] },
    { value: 'openai:chat', label: 'OpenAI Chat', default_path: '/v1/chat/completions', aliases: [] },
    { value: 'openai:responses', label: 'OpenAI Responses', default_path: '/v1/responses', aliases: [] },
    { value: 'openai:responses:compact', label: 'OpenAI Responses Compact', default_path: '/v1/responses/compact', aliases: [] },
    { value: 'openai:embedding', label: 'OpenAI Embedding', default_path: '/v1/embeddings', aliases: [] },
    { value: 'openai:rerank', label: 'OpenAI Rerank', default_path: '/v1/rerank', aliases: [] },
    { value: 'openai:image', label: 'OpenAI Image', default_path: '/v1/images/generations', aliases: [] },
    { value: 'openai:video', label: 'OpenAI Video', default_path: '/v1/videos', aliases: [] },
    { value: 'gemini:generate_content', label: 'Gemini Generate Content', default_path: '/v1beta/models/{model}:{action}', aliases: [] },
    { value: 'gemini:embedding', label: 'Gemini Embedding', default_path: '/v1beta/models/{model}:embedContent', aliases: [] },
    { value: 'gemini:video', label: 'Gemini Video', default_path: '/v1beta/models/{model}:predictLongRunning', aliases: [] },
    { value: 'jina:embedding', label: 'Jina Embedding', default_path: '/v1/embeddings', aliases: [] },
    { value: 'jina:rerank', label: 'Jina Rerank', default_path: '/v1/rerank', aliases: [] },
    { value: 'doubao:embedding', label: 'Doubao Embedding', default_path: '/embeddings/multimodal', aliases: [] }
  ]
}
