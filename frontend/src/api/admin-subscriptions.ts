import apiClient from './client'
import type { PaymentCallbackRecord } from './admin-payments'
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
  active_subscription_count: number
  created_at: string
  updated_at: string
}

export interface SubscriptionProduct {
  id: string
  code: string
  name: string
  description: string | null
  user_group_id: string
  user_group_name: string | null
  plan_level: number
  overage_policy: 'block' | 'use_wallet_balance'
  is_active: boolean
  active_subscription_count: number
  variant_count: number
  variants: SubscriptionVariant[]
  created_at: string
  updated_at: string
}

export interface UserSubscription {
  id: string
  user_id: string
  username: string | null
  email: string | null
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
  user_group_id: string | null
  user_group_name: string | null
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

export interface SubscriptionOrder extends PaymentOrder {
  username: string | null
  email: string | null
  subscription_status: string | null
  product_id: string | null
  product_name: string | null
  plan_id: string | null
  plan_name: string | null
  variant_name: string | null
  purchased_months: number | null
  upgraded_from_subscription_id: string | null
}

export interface SubscriptionCallbackListResponse {
  items: PaymentCallbackRecord[]
  total: number
  limit: number
  offset: number
}

export interface SubscriptionVariantRequest {
  id?: string
  code: string
  name: string
  description?: string | null
  monthly_price_usd: number
  monthly_quota_usd: number
  variant_rank: number
  term_discounts_json: SubscriptionTermDiscount[]
  is_active?: boolean
  is_default_variant?: boolean
}

export interface CreateSubscriptionProductRequest {
  code: string
  name: string
  description?: string | null
  user_group_id: string
  plan_level: number
  overage_policy: 'block' | 'use_wallet_balance'
  is_active?: boolean
  variants: SubscriptionVariantRequest[]
}

export interface UpdateSubscriptionProductRequest {
  code?: string
  name?: string
  description?: string | null
  user_group_id?: string
  plan_level?: number
  overage_policy?: 'block' | 'use_wallet_balance'
  is_active?: boolean
  variants?: SubscriptionVariantRequest[]
}

export interface CreateUserSubscriptionRequest {
  plan_id: string
  purchased_months: number
  started_at?: string | null
}

export interface CancelUserSubscriptionRequest {
  immediate?: boolean
}

export interface UpgradeUserSubscriptionRequest {
  new_plan_id: string
  purchased_months: number
}

export const adminSubscriptionsApi = {
  async listProducts(): Promise<{ products: SubscriptionProduct[]; total: number }> {
    const response = await apiClient.get<{ products: SubscriptionProduct[]; total: number }>(
      '/api/admin/subscriptions/products'
    )
    return response.data
  },

  async createProduct(payload: CreateSubscriptionProductRequest): Promise<SubscriptionProduct> {
    const response = await apiClient.post<SubscriptionProduct>(
      '/api/admin/subscriptions/products',
      payload
    )
    return response.data
  },

  async updateProduct(
    productId: string,
    payload: UpdateSubscriptionProductRequest
  ): Promise<SubscriptionProduct> {
    const response = await apiClient.patch<SubscriptionProduct>(
      `/api/admin/subscriptions/products/${productId}`,
      payload
    )
    return response.data
  },

  async deleteProduct(productId: string): Promise<void> {
    await apiClient.delete(`/api/admin/subscriptions/products/${productId}`)
  },

  async listUserSubscriptions(params?: {
    status?: string
    user_id?: string
    plan_id?: string
    product_id?: string
  }): Promise<{ subscriptions: UserSubscription[]; total: number }> {
    const response = await apiClient.get<{ subscriptions: UserSubscription[]; total: number }>(
      '/api/admin/subscriptions',
      { params }
    )
    return response.data
  },

  async listOrders(params?: {
    status?: string
    payment_method?: string
    user_id?: string
  }): Promise<{ orders: SubscriptionOrder[]; total: number }> {
    const response = await apiClient.get<{ orders: SubscriptionOrder[]; total: number }>(
      '/api/admin/subscriptions/orders',
      { params }
    )
    return response.data
  },

  async listCallbacks(params?: {
    payment_method?: string
    limit?: number
    offset?: number
  }): Promise<SubscriptionCallbackListResponse> {
    const response = await apiClient.get<SubscriptionCallbackListResponse>(
      '/api/admin/subscriptions/callbacks',
      { params }
    )
    return response.data
  },

  async approveOrder(orderId: string): Promise<{ order: SubscriptionOrder }> {
    const response = await apiClient.post<{ order: SubscriptionOrder }>(
      `/api/admin/subscriptions/orders/${orderId}/approve`
    )
    return response.data
  },

  async rejectOrder(orderId: string): Promise<{ order: SubscriptionOrder }> {
    const response = await apiClient.post<{ order: SubscriptionOrder }>(
      `/api/admin/subscriptions/orders/${orderId}/reject`
    )
    return response.data
  },

  async getCurrentUserSubscription(userId: string): Promise<UserSubscription | null> {
    const response = await apiClient.get<UserSubscription | null>(
      `/api/admin/subscriptions/users/${userId}/current`
    )
    return response.data
  },

  async createUserSubscription(
    userId: string,
    payload: CreateUserSubscriptionRequest
  ): Promise<UserSubscription> {
    const response = await apiClient.post<UserSubscription>(
      `/api/admin/subscriptions/users/${userId}`,
      payload
    )
    return response.data
  },

  async cancelUserSubscription(
    subscriptionId: string,
    payload: CancelUserSubscriptionRequest
  ): Promise<UserSubscription> {
    const response = await apiClient.post<UserSubscription>(
      `/api/admin/subscriptions/${subscriptionId}/cancel`,
      payload
    )
    return response.data
  },

  async upgradeUserSubscription(
    subscriptionId: string,
    payload: UpgradeUserSubscriptionRequest
  ): Promise<UserSubscription> {
    const response = await apiClient.post<UserSubscription>(
      `/api/admin/subscriptions/${subscriptionId}/upgrade`,
      payload
    )
    return response.data
  },
}
