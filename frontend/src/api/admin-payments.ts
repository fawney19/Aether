import apiClient from './client'
import type { PaymentOrder } from './wallet'

export interface PaymentCallbackRecord {
  id: string
  payment_order_id: string | null
  payment_method: string
  callback_key: string
  order_no: string | null
  gateway_order_id: string | null
  payload_hash: string | null
  signature_valid: boolean
  status: string
  payload: Record<string, unknown> | null
  error_message: string | null
  created_at: string
  processed_at: string | null
}

export interface AdminPaymentOrderListResponse {
  items: PaymentOrder[]
  total: number
  limit: number
  offset: number
}

export interface AdminPaymentCallbacksResponse {
  items: PaymentCallbackRecord[]
  total: number
  limit: number
  offset: number
}

export interface AdminPaymentCreditRequest {
  gateway_order_id?: string
  pay_amount?: number
  pay_currency?: string
  exchange_rate?: number
  gateway_response?: Record<string, unknown>
}

export interface RedeemCodeBatch {
  id: string
  name: string
  amount_usd: number
  currency: string
  balance_bucket: string
  total_count: number
  redeemed_count: number
  active_count: number
  status: string
  description?: string | null
  created_by?: string | null
  expires_at?: string | null
  created_at: string | null
  updated_at: string | null
}

export interface RedeemCodeRecord {
  id: string
  batch_id: string
  batch_name?: string | null
  code_prefix: string
  code_suffix: string
  masked_code: string
  status: string
  redeemed_by_user_id?: string | null
  redeemed_by_user_name?: string | null
  redeemed_wallet_id?: string | null
  redeemed_payment_order_id?: string | null
  redeemed_order_no?: string | null
  redeemed_at?: string | null
  disabled_by?: string | null
  expires_at?: string | null
  created_at: string | null
  updated_at: string | null
}

export interface CreateRedeemCodeBatchRequest {
  name: string
  amount_usd: number
  total_count: number
  expires_at?: string
  description?: string
}

export interface CreateRedeemCodeBatchResponse {
  batch: RedeemCodeBatch
  codes: Array<{
    id: string
    code: string
    masked_code: string
  }>
}

export interface RedeemCodeBatchListResponse {
  items: RedeemCodeBatch[]
  total: number
  limit: number
  offset: number
}

export interface RedeemCodeListResponse {
  batch: RedeemCodeBatch
  items: RedeemCodeRecord[]
  total: number
  limit: number
  offset: number
}

export const adminPaymentsApi = {
  async listOrders(params?: {
    status?: string
    payment_method?: string
    limit?: number
    offset?: number
  }): Promise<AdminPaymentOrderListResponse> {
    const response = await apiClient.get<AdminPaymentOrderListResponse>('/api/admin/payments/orders', { params })
    return response.data
  },

  async getOrder(orderId: string): Promise<{ order: PaymentOrder }> {
    const response = await apiClient.get<{ order: PaymentOrder }>(`/api/admin/payments/orders/${orderId}`)
    return response.data
  },

  async expireOrder(orderId: string): Promise<{ order: PaymentOrder; expired: boolean }> {
    const response = await apiClient.post<{ order: PaymentOrder; expired: boolean }>(
      `/api/admin/payments/orders/${orderId}/expire`,
      {}
    )
    return response.data
  },

  async failOrder(orderId: string): Promise<{ order: PaymentOrder }> {
    const response = await apiClient.post<{ order: PaymentOrder }>(
      `/api/admin/payments/orders/${orderId}/fail`,
      {}
    )
    return response.data
  },

  async creditOrder(
    orderId: string,
    payload: AdminPaymentCreditRequest
  ): Promise<{ order: PaymentOrder; credited: boolean }> {
    const response = await apiClient.post<{ order: PaymentOrder; credited: boolean }>(
      `/api/admin/payments/orders/${orderId}/credit`,
      payload
    )
    return response.data
  },

  async listCallbacks(params?: {
    payment_method?: string
    limit?: number
    offset?: number
  }): Promise<AdminPaymentCallbacksResponse> {
    const response = await apiClient.get<AdminPaymentCallbacksResponse>('/api/admin/payments/callbacks', { params })
    return response.data
  },

  async listRedeemCodeBatches(params?: {
    status?: string
    limit?: number
    offset?: number
  }): Promise<RedeemCodeBatchListResponse> {
    const response = await apiClient.get<RedeemCodeBatchListResponse>(
      '/api/admin/payments/redeem-codes/batches',
      { params }
    )
    return response.data
  },

  async createRedeemCodeBatch(
    payload: CreateRedeemCodeBatchRequest
  ): Promise<CreateRedeemCodeBatchResponse> {
    const response = await apiClient.post<CreateRedeemCodeBatchResponse>(
      '/api/admin/payments/redeem-codes/batches',
      payload
    )
    return response.data
  },

  async getRedeemCodeBatch(batchId: string): Promise<{ batch: RedeemCodeBatch }> {
    const response = await apiClient.get<{ batch: RedeemCodeBatch }>(
      `/api/admin/payments/redeem-codes/batches/${batchId}`
    )
    return response.data
  },

  async listRedeemCodes(
    batchId: string,
    params?: {
      status?: string
      limit?: number
      offset?: number
    }
  ): Promise<RedeemCodeListResponse> {
    const response = await apiClient.get<RedeemCodeListResponse>(
      `/api/admin/payments/redeem-codes/batches/${batchId}/codes`,
      { params }
    )
    return response.data
  },

  async disableRedeemCodeBatch(batchId: string): Promise<{ batch: RedeemCodeBatch }> {
    const response = await apiClient.post<{ batch: RedeemCodeBatch }>(
      `/api/admin/payments/redeem-codes/batches/${batchId}/disable`,
      {}
    )
    return response.data
  },

  async deleteRedeemCodeBatch(batchId: string): Promise<{ batch: RedeemCodeBatch }> {
    const response = await apiClient.post<{ batch: RedeemCodeBatch }>(
      `/api/admin/payments/redeem-codes/batches/${batchId}/delete`,
      {}
    )
    return response.data
  },

  async disableRedeemCode(codeId: string): Promise<{ code: RedeemCodeRecord }> {
    const response = await apiClient.post<{ code: RedeemCodeRecord }>(
      `/api/admin/payments/redeem-codes/codes/${codeId}/disable`,
      {}
    )
    return response.data
  },
}
