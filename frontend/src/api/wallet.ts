import apiClient from './client'

export interface WalletSummary {
  id: string
  // balance = 总可用余额（充值余额 + 赠款余额）
  balance: number
  recharge_balance: number
  gift_balance: number
  refundable_balance: number
  currency: string
  status: string
  limit_mode?: 'finite' | 'unlimited'
  unlimited?: boolean
  total_recharged: number
  total_consumed: number
  total_refunded: number
  total_adjusted: number
  updated_at: string
}

export interface WalletBalanceResponse {
  wallet: WalletSummary | null
  unlimited: boolean
  limit_mode: 'finite' | 'unlimited'
  // balance = 总可用余额（充值余额 + 赠款余额）
  balance: number | null
  recharge_balance?: number | null
  gift_balance?: number | null
  refundable_balance?: number | null
  currency: string
  pending_refund_count?: number
}

export interface WalletTransaction {
  id: string
  category: string
  reason_code: string
  amount: number
  // 总可用余额（充值+赠款）快照
  balance_before: number
  balance_after: number
  // 分账户快照
  recharge_balance_before: number
  recharge_balance_after: number
  gift_balance_before: number
  gift_balance_after: number
  link_type?: string | null
  link_id?: string | null
  operator_id?: string | null
  operator_name?: string | null
  operator_email?: string | null
  description?: string | null
  created_at: string
}

export interface WalletTransactionsResponse extends WalletBalanceResponse {
  items: WalletTransaction[]
  total: number
  limit: number
  offset: number
}

export interface DailyUsageRecord {
  id?: string | null
  date: string | null
  timezone?: string | null
  total_cost: number
  total_requests: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  first_finalized_at?: string | null
  last_finalized_at?: string | null
  aggregated_at?: string | null
  is_today: boolean
}

export type FlowItem =
  | { type: 'transaction'; data: WalletTransaction }
  | { type: 'daily_usage'; data: DailyUsageRecord }

export interface WalletFlowResponse extends WalletBalanceResponse {
  today_entry: DailyUsageRecord | null
  items: FlowItem[]
  total: number
  limit: number
  offset: number
}

export type TodayCostResponse = DailyUsageRecord

export interface PaymentOrder {
  id: string
  order_no: string
  wallet_id: string
  user_id: string | null
  amount_usd: number
  pay_amount: number | null
  pay_currency: string | null
  exchange_rate: number | null
  refunded_amount_usd: number
  refundable_amount_usd: number
  payment_method: string
  gateway_order_id: string | null
  gateway_response: Record<string, unknown> | null
  status: string
  created_at: string
  paid_at: string | null
  credited_at: string | null
  expires_at: string | null
}

export interface RefundRequest {
  id: string
  refund_no: string
  payment_order_id: string | null
  source_type: string
  source_id: string | null
  refund_mode: string
  amount_usd: number
  status: string
  reason: string | null
  failure_reason: string | null
  gateway_refund_id: string | null
  payout_method: string | null
  payout_reference: string | null
  payout_proof: Record<string, unknown> | null
  created_at: string
  updated_at: string
  processed_at: string | null
  completed_at: string | null
}

export interface WalletRechargeCreateRequest {
  amount_usd: number
  payment_method: string
  pay_amount?: number
  pay_currency?: string
  exchange_rate?: number
}

export interface WalletRefundCreateRequest {
  amount_usd: number
  payment_order_id?: string
  source_type?: string
  source_id?: string
  refund_mode?: string
  reason?: string
  idempotency_key?: string
}

export const walletApi = {
  async getBalance(): Promise<WalletBalanceResponse> {
    const response = await apiClient.get<WalletBalanceResponse>('/api/wallet/balance')
    return response.data
  },

  async getTransactions(params?: { limit?: number; offset?: number }): Promise<WalletTransactionsResponse> {
    const response = await apiClient.get<WalletTransactionsResponse>('/api/wallet/transactions', { params })
    return response.data
  },

  async getFlow(params?: { limit?: number; offset?: number }): Promise<WalletFlowResponse> {
    const response = await apiClient.get<WalletFlowResponse>('/api/wallet/flow', { params })
    return response.data
  },

  async getTodayCost(): Promise<TodayCostResponse> {
    const response = await apiClient.get<TodayCostResponse>('/api/wallet/today-cost')
    return response.data
  },

  async createRechargeOrder(payload: WalletRechargeCreateRequest): Promise<{
    order: PaymentOrder
    payment_instructions: Record<string, unknown>
  }> {
    const response = await apiClient.post('/api/wallet/recharge', payload)
    return response.data
  },

  async listRechargeOrders(params?: { limit?: number; offset?: number }): Promise<{
    items: PaymentOrder[]
    total: number
    limit: number
    offset: number
  }> {
    const response = await apiClient.get('/api/wallet/recharge', { params })
    return response.data
  },

  async getRechargeOrder(orderId: string): Promise<{ order: PaymentOrder }> {
    const response = await apiClient.get(`/api/wallet/recharge/${orderId}`)
    return response.data
  },

  async listRefunds(params?: { limit?: number; offset?: number }): Promise<{
    items: RefundRequest[]
    total: number
    limit: number
    offset: number
  }> {
    const response = await apiClient.get('/api/wallet/refunds', { params })
    return response.data
  },

  async getRefund(refundId: string): Promise<RefundRequest> {
    const response = await apiClient.get<RefundRequest>(`/api/wallet/refunds/${refundId}`)
    return response.data
  },

  async createRefund(payload: WalletRefundCreateRequest): Promise<RefundRequest> {
    const response = await apiClient.post<RefundRequest>('/api/wallet/refunds', payload)
    return response.data
  },
}
