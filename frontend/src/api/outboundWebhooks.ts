import apiClient from './client'

export type WebhookEventType =
  | 'user.registered'
  | 'wallet.recharged'
  | 'api_key.created'
  | 'risk_control.hit'
  | 'provider.error'
  | 'balance.low'
  | 'webhook.test'

export type WebhookEventCategory = 'account' | 'billing' | 'security' | 'infrastructure' | 'diagnostic'

export interface WebhookEventDefinition {
  type: WebhookEventType
  name: string
  description: string
  category: WebhookEventCategory
  high_priority?: boolean
}

export type WebhookDeliveryStatus =
  | 'pending'
  | 'delivering'
  | 'succeeded'
  | 'failed'
  | 'retrying'
  | 'dead'
  | 'cancelled'

export interface WebhookEndpoint {
  id: string
  name: string
  url: string
  enabled: boolean
  subscribed_events: WebhookEventType[]
  secret_set: boolean
  timeout_ms: number
  max_retries: number
  created_at: string
  updated_at: string
  last_delivery_at: string | null
  last_delivery_status: WebhookDeliveryStatus | null
  failure_count: number
}

export interface WebhookEndpointUpsertRequest {
  name: string
  url: string
  enabled: boolean
  subscribed_events: WebhookEventType[]
  timeout_ms: number
  max_retries: number
  secret?: string | null
}

export interface WebhookDeliveryLog {
  id: string
  endpoint_id: string
  endpoint_name: string
  event_type: WebhookEventType
  status: WebhookDeliveryStatus
  attempt_count: number
  max_attempts: number
  status_code: number | null
  duration_ms: number | null
  created_at: string
  next_retry_at: string | null
  delivered_at: string | null
  last_error: string | null
  response_excerpt: string | null
  request_id: string | null
}

export interface WebhookDeliveryListParams {
  endpoint_id?: string
  status?: WebhookDeliveryStatus | 'all'
  event_type?: WebhookEventType | 'all'
  limit?: number
}

export interface WebhookDeliveryListResponse {
  items: WebhookDeliveryLog[]
  total: number
}

export interface WebhookTestRequest {
  event_type: WebhookEventType
  payload?: Record<string, unknown>
}

export const WEBHOOK_EVENT_DEFINITIONS: WebhookEventDefinition[] = [
  {
    type: 'user.registered',
    name: '用户注册',
    description: '新用户完成注册时触发',
    category: 'account',
  },
  {
    type: 'wallet.recharged',
    name: '充值成功',
    description: '钱包充值订单完成入账时触发',
    category: 'billing',
  },
  {
    type: 'api_key.created',
    name: 'Key 创建',
    description: '用户或管理员创建 API Key 时触发',
    category: 'account',
  },
  {
    type: 'risk_control.hit',
    name: '风控命中',
    description: '请求命中风控策略并生成处置结果时触发',
    category: 'security',
    high_priority: true,
  },
  {
    type: 'provider.error',
    name: 'Provider 异常',
    description: '上游 Provider 或 Key 出现连续失败时触发',
    category: 'infrastructure',
    high_priority: true,
  },
  {
    type: 'balance.low',
    name: '余额不足',
    description: '用户余额低于提醒阈值时触发',
    category: 'billing',
    high_priority: true,
  },
  {
    type: 'webhook.test',
    name: '测试事件',
    description: '管理员手动发送的 Webhook 连通性测试',
    category: 'diagnostic',
  },
]

const WEBHOOK_ENDPOINT_URL_PREFIX = '/api/admin/system/webhooks/outbound'

export function validateWebhookEndpointUrl(rawUrl: string): string | null {
  const url = rawUrl.trim()
  if (!url) return '请填写 Webhook URL'
  if (url.length > 2048) return 'Webhook URL 不能超过 2048 个字符'
  if (url.startsWith('//')) return '请填写完整的 https 地址'
  try {
    const parsed = new URL(url)
    if (parsed.username || parsed.password) return 'Webhook URL 不能包含用户名或密码'
    if (parsed.protocol === 'https:') return null
    return '仅允许 HTTPS Webhook 地址'
  } catch {
    return '请填写有效的完整 URL'
  }
}

export const outboundWebhooksApi = {
  async listEndpoints(): Promise<WebhookEndpoint[]> {
    const response = await apiClient.get<WebhookEndpoint[] | { items: WebhookEndpoint[] }>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/endpoints`
    )
    return Array.isArray(response.data) ? response.data : response.data.items
  },

  async createEndpoint(payload: WebhookEndpointUpsertRequest): Promise<WebhookEndpoint> {
    const response = await apiClient.post<WebhookEndpoint>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/endpoints`,
      payload
    )
    return response.data
  },

  async updateEndpoint(id: string, payload: Partial<WebhookEndpointUpsertRequest>): Promise<WebhookEndpoint> {
    const response = await apiClient.put<WebhookEndpoint>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/endpoints/${encodeURIComponent(id)}`,
      payload
    )
    return response.data
  },

  async deleteEndpoint(id: string): Promise<void> {
    await apiClient.delete(`${WEBHOOK_ENDPOINT_URL_PREFIX}/endpoints/${encodeURIComponent(id)}`)
  },

  async listDeliveries(params: WebhookDeliveryListParams = {}): Promise<WebhookDeliveryListResponse> {
    const response = await apiClient.get<WebhookDeliveryListResponse>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/deliveries`,
      { params }
    )
    return response.data
  },

  async retryDelivery(id: string): Promise<WebhookDeliveryLog> {
    const response = await apiClient.post<WebhookDeliveryLog>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/deliveries/${encodeURIComponent(id)}/retry`
    )
    return response.data
  },

  async sendTestDelivery(endpointId: string, payload: WebhookTestRequest): Promise<WebhookDeliveryLog> {
    const response = await apiClient.post<WebhookDeliveryLog>(
      `${WEBHOOK_ENDPOINT_URL_PREFIX}/endpoints/${encodeURIComponent(endpointId)}/test`,
      payload
    )
    return response.data
  },
}
