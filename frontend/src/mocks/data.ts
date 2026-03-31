/**
 * Demo Mode Mock Data
 * 演示模式的模拟数据
 */

import type { User, LoginResponse } from '@/api/auth'
import type { User as AdminUser, UserGroup } from '@/api/users'
import type { AdminApiKeysResponse } from '@/api/admin'
import type { Profile } from '@/api/me'
import type { ProviderWithEndpointsSummary, GlobalModelResponse } from '@/api/endpoints/types'
import type { ModelGroupDetail } from '@/api/model-groups'

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
  created_at: '2024-06-01T00:00:00Z',
  updated_at: new Date().toISOString(),
  last_login_at: new Date().toISOString(),
  preferences: {
    theme: 'auto',
    language: 'zh-CN'
  }
}

// ========== 用户管理数据 ==========

export const MOCK_MODEL_GROUPS: ModelGroupDetail[] = [
  {
    id: 'mg-default-demo',
    name: 'default',
    display_name: '默认模型分组',
    description: '系统默认模型分组，供默认用户组与基础场景使用。',
    default_user_billing_multiplier: 1,
    routing_mode: 'inherit',
    is_default: true,
    is_active: true,
    sort_order: 0,
    model_count: 2,
    user_group_count: 1,
    models: [],
    routes: [],
    user_groups: [],
    created_at: '2024-01-05T00:00:00Z',
    updated_at: '2024-01-05T00:00:00Z',
  },
  {
    id: 'mg-internal-demo',
    name: 'internal',
    display_name: '内部全量模型组',
    description: '内部用户使用，继承全局路由并开放更多统一模型。',
    default_user_billing_multiplier: 1,
    routing_mode: 'inherit',
    is_default: false,
    is_active: true,
    sort_order: 10,
    model_count: 9,
    user_group_count: 1,
    models: [],
    routes: [],
    user_groups: [],
    created_at: '2024-01-10T00:00:00Z',
    updated_at: '2024-01-10T00:00:00Z',
  },
  {
    id: 'mg-vendor-demo',
    name: 'vendor',
    display_name: '供应商受限模型组',
    description: '合作方使用，只开放指定 OpenAI 渠道并应用单独计费倍率。',
    default_user_billing_multiplier: 1.15,
    routing_mode: 'custom',
    is_default: false,
    is_active: true,
    sort_order: 20,
    model_count: 2,
    user_group_count: 1,
    models: [],
    routes: [
      {
        id: 'mgr-vendor-openai',
        provider_id: 'provider-demo-openai',
        provider_name: 'OpenAI 官方',
        provider_api_key_id: 'ekey-003',
        provider_api_key_name: 'OpenAI OAuth',
        priority: 10,
        user_billing_multiplier_override: 1.15,
        is_active: true,
        notes: '合作方默认优先走 OpenAI OAuth Key',
      },
    ],
    user_groups: [],
    created_at: '2024-02-10T00:00:00Z',
    updated_at: '2024-02-10T00:00:00Z',
  },
]

export const MOCK_USER_GROUPS: UserGroup[] = [
  {
    id: 'group-default-demo',
    name: '默认分组',
    description: '系统默认分组，所有未显式指定分组的用户都会归入此组。',
    is_default: true,
    allowed_api_formats: null,
    model_group_bindings: [
      {
        model_group_id: 'mg-default-demo',
        priority: 10,
        is_active: true,
        model_group_name: 'default',
        model_group_display_name: '默认模型分组',
        model_group_is_default: true,
      },
    ],
    rate_limit: null,
    user_count: 0,
    created_at: '2024-01-05T00:00:00Z',
    updated_at: '2024-01-05T00:00:00Z',
  },
  {
    id: 'group-internal-demo',
    name: '内部研发',
    description: '内部员工默认不限制访问范围',
    is_default: false,
    allowed_api_formats: null,
    model_group_bindings: [
      {
        model_group_id: 'mg-internal-demo',
        priority: 10,
        is_active: true,
        model_group_name: 'internal',
        model_group_display_name: '内部全量模型组',
        model_group_is_default: false,
      },
      {
        model_group_id: 'mg-default-demo',
        priority: 20,
        is_active: true,
        model_group_name: 'default',
        model_group_display_name: '默认模型分组',
        model_group_is_default: true,
      },
    ],
    rate_limit: null,
    user_count: 1,
    created_at: '2024-01-10T00:00:00Z',
    updated_at: '2024-01-10T00:00:00Z',
  },
  {
    id: 'group-vendor-demo',
    name: '供应商',
    description: '外部合作方默认只开放受限资源',
    is_default: false,
    allowed_api_formats: ['openai:chat'],
    model_group_bindings: [
      {
        model_group_id: 'mg-vendor-demo',
        priority: 10,
        is_active: true,
        model_group_name: 'vendor',
        model_group_display_name: '供应商受限模型组',
        model_group_is_default: false,
      },
    ],
    rate_limit: 30,
    user_count: 2,
    created_at: '2024-02-10T00:00:00Z',
    updated_at: '2024-02-10T00:00:00Z',
  },
]

export const MOCK_ALL_USERS: AdminUser[] = [
  {
    id: 'demo-admin-uuid-0001',
    username: 'Demo Admin',
    email: 'admin@demo.aether.io',
    role: 'admin',
    unlimited: true,
    is_active: true,
    group_id: 'group-default-demo',
    group_name: '默认分组',
    effective_allowed_providers: null,
    effective_allowed_api_formats: null,
    effective_allowed_models: null,
    effective_rate_limit: null,
    created_at: '2024-01-01T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0002',
    username: 'Demo User',
    email: 'user@demo.aether.io',
    role: 'user',
    unlimited: false,
    is_active: true,
    group_id: 'group-internal-demo',
    group_name: '内部研发',
    effective_allowed_providers: null,
    effective_allowed_api_formats: null,
    effective_allowed_models: null,
    effective_rate_limit: null,
    created_at: '2024-06-01T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0003',
    username: 'Alice Wang',
    email: 'alice@example.com',
    role: 'user',
    unlimited: false,
    is_active: true,
    group_id: 'group-vendor-demo',
    group_name: '供应商',
    effective_allowed_providers: ['provider-demo-openai'],
    effective_allowed_api_formats: ['openai:chat'],
    effective_allowed_models: ['gpt-4o-mini'],
    effective_rate_limit: 30,
    created_at: '2024-03-15T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0004',
    username: 'Bob Zhang',
    email: 'bob@example.com',
    role: 'user',
    unlimited: false,
    is_active: true,
    group_id: 'group-vendor-demo',
    group_name: '供应商',
    effective_allowed_providers: ['provider-demo-openai'],
    effective_allowed_api_formats: ['openai:chat'],
    effective_allowed_models: ['gpt-4o-mini'],
    effective_rate_limit: 30,
    created_at: '2024-02-20T00:00:00Z'
  },
  {
    id: 'demo-user-uuid-0005',
    username: 'Charlie Li',
    email: 'charlie@example.com',
    role: 'user',
    unlimited: false,
    is_active: false,
    group_id: 'group-default-demo',
    group_name: '默认分组',
    effective_allowed_providers: null,
    effective_allowed_api_formats: null,
    effective_allowed_models: null,
    effective_rate_limit: null,
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
    api_formats: ['claude:chat', 'claude:cli'],
    endpoint_health_details: [
      { api_format: 'claude:chat', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'claude:cli', health_score: 0.65, is_active: true, active_keys: 1 }
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
    api_formats: ['claude:cli', 'openai:cli'],
    endpoint_health_details: [
      { api_format: 'claude:cli', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'openai:cli', health_score: 1.0, is_active: true, active_keys: 1 }
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
    api_formats: ['claude:cli', 'gemini:chat', 'gemini:cli', 'openai:cli'],
    endpoint_health_details: [
      { api_format: 'claude:cli', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'gemini:chat', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'gemini:cli', health_score: 1.0, is_active: true, active_keys: 1 },
      { api_format: 'openai:cli', health_score: 1.0, is_active: true, active_keys: 1 }
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
    api_formats: ['claude:chat', 'claude:cli', 'gemini:chat', 'gemini:cli', 'openai:chat', 'openai:cli'],
    endpoint_health_details: [
      { api_format: 'claude:chat', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'claude:cli', health_score: 0.48, is_active: true, active_keys: 2 },
      { api_format: 'gemini:chat', health_score: 1.0, is_active: true, active_keys: 2 },
      { api_format: 'gemini:cli', health_score: 0.85, is_active: true, active_keys: 2 },
      { api_format: 'openai:chat', health_score: 0.85, is_active: true, active_keys: 2 },
      { api_format: 'openai:cli', health_score: 1.0, is_active: true, active_keys: 1 }
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
    api_formats: ['gemini:chat'],
    endpoint_health_details: [
      { api_format: 'gemini:chat', health_score: 1.0, is_active: true, active_keys: 1 }
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
    model_group_ids: ['mg-default-demo', 'mg-internal-demo'],
    model_groups: [
      { id: 'mg-default-demo', name: 'default', display_name: '默认模型分组', is_default: true },
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-default-demo', 'mg-internal-demo'],
    model_groups: [
      { id: 'mg-default-demo', name: 'default', display_name: '默认模型分组', is_default: true },
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo', 'mg-vendor-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
      { id: 'mg-vendor-demo', name: 'vendor', display_name: '供应商受限模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo', 'mg-vendor-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
      { id: 'mg-vendor-demo', name: 'vendor', display_name: '供应商受限模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
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
    model_group_ids: ['mg-internal-demo'],
    model_groups: [
      { id: 'mg-internal-demo', name: 'internal', display_name: '内部全量模型组', is_default: false },
    ],
    created_at: '2024-01-01T00:00:00Z'
  }
]

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
    { value: 'claude:chat', label: 'Claude Chat', default_path: '/v1/messages', aliases: [] },
    { value: 'claude:cli', label: 'Claude CLI', default_path: '/v1/messages', aliases: [] },
    { value: 'openai:chat', label: 'OpenAI Chat', default_path: '/v1/chat/completions', aliases: [] },
    { value: 'openai:cli', label: 'OpenAI CLI', default_path: '/v1/responses', aliases: [] },
    { value: 'openai:video', label: 'OpenAI Video', default_path: '/v1/videos', aliases: [] },
    { value: 'gemini:chat', label: 'Gemini Chat', default_path: '/v1beta/models/{model}:{action}', aliases: [] },
    { value: 'gemini:cli', label: 'Gemini CLI', default_path: '/v1beta/models/{model}:{action}', aliases: [] },
    { value: 'gemini:video', label: 'Gemini Video', default_path: '/v1beta/models/{model}:predictLongRunning', aliases: [] }
  ]
}
