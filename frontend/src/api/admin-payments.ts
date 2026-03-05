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
}
