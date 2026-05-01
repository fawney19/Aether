import apiClient from './client'
import type { PaymentOrder } from './wallet'

export interface SubscriptionTermDiscount {
  months: number
  discount_factor: number
}

export interface SubscriptionVariant {
  id: string
  product_id: string
  code: string
  name: string
  description: string | null
  monthly_price_usd: number
  monthly_quota_usd: number
  variant_rank: number
  term_discounts_json: SubscriptionTermDiscount[]
  is_active: boolean
  is_default_variant: boolean
  created_at: string
  updated_at: string
}

export interface SubscriptionProduct {
  id: string
  code: string
  name: string
  description: string | null
  plan_level: number
  overage_policy: 'block' | 'use_wallet_balance'
  is_active: boolean
  variant_count: number
  available_model_names: string[]
  variants: SubscriptionVariant[]
  created_at: string
  updated_at: string
}

export interface UserSubscription {
  id: string
  product_id: string | null
  product_code: string | null
  product_name: string | null
  plan_id: string
  plan_code: string | null
  plan_name: string | null
  variant_id: string | null
  variant_code: string | null
  variant_name: string | null
  variant_rank: number | null
  status: 'pending_payment' | 'active' | 'canceled' | 'expired'
  end_reason: string | null
  purchased_months: number
  discount_factor: number
  monthly_price_usd_snapshot: number
  total_price_usd: number
  started_at: string
  ends_at: string
  current_cycle_start: string
  current_cycle_end: string
  cycle_quota_usd: number
  cycle_used_usd: number
  remaining_quota_usd: number
  cancel_at_period_end: boolean
  canceled_at: string | null
  ended_at: string | null
  upgraded_from_subscription_id: string | null
  created_at: string
  updated_at: string
}

export interface SubscriptionDashboardResponse {
  current_subscription: UserSubscription | null
}

export interface SubscriptionCheckoutRequest {
  plan_id: string
  purchased_months: number
  payment_method: string
}

export interface SubscriptionUpgradeRequest {
  new_plan_id: string
  purchased_months: number
  payment_method: string
}

export interface SubscriptionCheckoutResponse {
  subscription: UserSubscription
  payable_amount_usd: number
  order: PaymentOrder | null
  payment_instructions: Record<string, unknown>
}

export interface SubscriptionOrder extends PaymentOrder {
  subscription_status: string | null
  product_id: string | null
  product_name: string | null
  plan_id: string | null
  plan_name: string | null
  variant_name: string | null
  purchased_months: number | null
  upgraded_from_subscription_id: string | null
}

export const subscriptionsApi = {
  async getDashboard(): Promise<SubscriptionDashboardResponse> {
    const response = await apiClient.get<SubscriptionDashboardResponse>('/api/subscriptions/dashboard')
    return response.data
  },

  async listProducts(): Promise<{ products: SubscriptionProduct[]; total: number }> {
    const response = await apiClient.get<{ products: SubscriptionProduct[]; total: number }>(
      '/api/subscriptions/products'
    )
    return response.data
  },

  async listOrders(params?: { limit?: number; offset?: number }): Promise<{
    items: SubscriptionOrder[]
    total: number
    limit: number
    offset: number
  }> {
    const response = await apiClient.get<{
      items: SubscriptionOrder[]
      total: number
      limit: number
      offset: number
    }>('/api/subscriptions/orders', { params })
    return response.data
  },

  async cancelOrder(orderId: string): Promise<{ order: SubscriptionOrder }> {
    const response = await apiClient.post<{ order: SubscriptionOrder }>(`/api/subscriptions/orders/${orderId}/cancel`, {})
    return response.data
  },

  async purchase(payload: SubscriptionCheckoutRequest): Promise<SubscriptionCheckoutResponse> {
    const response = await apiClient.post<SubscriptionCheckoutResponse>(
      '/api/subscriptions/purchase',
      payload
    )
    return response.data
  },

  async upgrade(
    subscriptionId: string,
    payload: SubscriptionUpgradeRequest
  ): Promise<SubscriptionCheckoutResponse> {
    const response = await apiClient.post<SubscriptionCheckoutResponse>(
      `/api/subscriptions/${subscriptionId}/upgrade`,
      payload
    )
    return response.data
  },
}
