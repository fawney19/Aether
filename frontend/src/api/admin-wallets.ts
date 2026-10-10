import apiClient from './client'
import { buildCacheKey, cachedRequest } from '@/utils/cache'
import type { RefundRequest, WalletDailyQuotaSummary, WalletSummary, WalletTransaction } from './wallet'

export interface AdminWallet extends WalletSummary {
  user_id: string | null
  api_key_id: string | null
  owner_type: 'user' | 'api_key'
  owner_name: string | null
  created_at: string
  wallet_balance?: number | null
  package_balance?: number | null
  total_available_balance?: number | null
  daily_quota?: WalletDailyQuotaSummary | null
  deduction_order?: string[]
}

export interface AdminWalletListResponse {
  items: AdminWallet[]
  total: number
  limit: number
  offset: number
}

export interface AdminWalletDetailResponse extends AdminWallet {
  pending_refund_count: number
}

export interface AdminWalletTransactionsResponse {
  wallet: AdminWallet
  items: WalletTransaction[]
  total: number
  limit: number
  offset: number
}

export interface AdminWalletRefundsResponse {
  wallet: AdminWallet
  items: RefundRequest[]
  total: number
  limit: number
  offset: number
}

export interface AdminLedgerTransaction extends WalletTransaction {
  wallet_id: string
  owner_type: 'user' | 'api_key'
  owner_name: string | null
  wallet_status?: string | null
}

export interface AdminGlobalRefund extends RefundRequest {
  wallet_id: string
  owner_type: 'user' | 'api_key'
  owner_name: string | null
  wallet_status?: string | null
}

export interface AdminLedgerResponse {
  items: AdminLedgerTransaction[]
  total: number
  limit: number
  offset: number
}

export interface AdminGlobalRefundsListResponse {
  items: AdminGlobalRefund[]
  total: number
  limit: number
  offset: number
}

export interface ManualRechargeRequest {
  amount_usd: number
  payment_method?: string
  description?: string
}

export interface WalletAdjustRequest {
  amount_usd: number
  balance_type?: 'recharge' | 'gift'
  description?: string
}

export interface RefundFailRequest {
  reason: string
}

export interface ReferralRefundPreview {
  refund_id: string
  applicable: boolean
  stage: 'process' | 'complete'
  rewards: Array<{
    inviter_user_id: string
    inviter_username: string | null
    expected_reversal_usd: number
    available_gift_usd: number
    deductible_usd: number
    shortfall_usd: number
  }>
  total_expected_reversal_usd: number
  total_deductible_usd: number
  total_shortfall_usd: number
  confirmation_token: string | null
}

export interface ReferralRefundConfirmation {
  referral_shortfall_confirmation?: string
}

export interface ReferralRefundReversal {
  reversed_amount_usd: number
  pending_reversal_amount_usd: number
}

export interface RefundCompleteRequest extends ReferralRefundConfirmation {
  gateway_refund_id?: string
  payout_reference?: string
  payout_proof?: Record<string, unknown>
}

export const adminWalletApi = {
  async listWallets(params?: {
    user_id?: string
    status?: string
    owner_type?: 'user' | 'api_key'
    limit?: number
    offset?: number
  }, signal?: AbortSignal): Promise<AdminWalletListResponse> {
    const response = await apiClient.get<AdminWalletListResponse>('/api/admin/wallets', { params, signal })
    return response.data
  },

  async listAllWallets(params?: {
    status?: string
    owner_type?: 'user' | 'api_key'
  }, options: { cacheTtlMs?: number } = {}): Promise<AdminWallet[]> {
    const cacheKey = buildCacheKey(
      'admin:wallets:list-all',
      params as Record<string, unknown> | undefined,
    )
    return cachedRequest(
      cacheKey,
      async () => {
        const items: AdminWallet[] = []
        const limit = 200
        const maxPages = 200
        let offset = 0
        let page = 0

        while (page < maxPages) {
          const response = await apiClient.get<AdminWalletListResponse>('/api/admin/wallets', {
            params: {
              ...params,
              limit,
              offset,
            },
          })
          const data = response.data
          items.push(...data.items)

          if (items.length >= data.total || data.items.length < limit) {
            break
          }

          const nextOffset = offset + data.items.length
          if (nextOffset <= offset) {
            throw new Error('分页游标未前进，终止全量钱包拉取以避免死循环')
          }
          offset = nextOffset
          page += 1
        }

        if (page >= maxPages) {
          throw new Error(`钱包列表分页超过最大页数 ${maxPages}，已中止请求`)
        }

        return items
      },
      options.cacheTtlMs ?? 0,
    )
  },

  async getWalletDetail(walletId: string, signal?: AbortSignal): Promise<AdminWalletDetailResponse> {
    const response = await apiClient.get<AdminWalletDetailResponse>(`/api/admin/wallets/${walletId}`, { signal })
    return response.data
  },

  async getRefundDetail(walletId: string, refundId: string, signal?: AbortSignal): Promise<{ refund: AdminGlobalRefund }> {
    const response = await apiClient.get<{ refund: AdminGlobalRefund }>(
      `/api/admin/wallets/${walletId}/refunds/${refundId}`, { signal },
    )
    return response.data
  },

  async getTransactionDetail(walletId: string, transactionId: string, signal?: AbortSignal): Promise<{ transaction: AdminLedgerTransaction }> {
    const response = await apiClient.get<{ transaction: AdminLedgerTransaction }>(
      `/api/admin/wallets/${walletId}/transactions/${transactionId}`, { signal },
    )
    return response.data
  },

  async listLedger(params?: {
    category?: string
    reason_code?: string
    owner_type?: string
    limit?: number
    offset?: number
  }): Promise<AdminLedgerResponse> {
    const response = await apiClient.get<AdminLedgerResponse>('/api/admin/wallets/ledger', { params })
    return response.data
  },

  async listGlobalRefunds(params?: {
    status?: string
    owner_type?: string
    limit?: number
    offset?: number
  }): Promise<AdminGlobalRefundsListResponse> {
    const response = await apiClient.get<AdminGlobalRefundsListResponse>('/api/admin/wallets/refund-requests', {
      params,
    })
    return response.data
  },

  async getWalletTransactions(
    walletId: string,
    params?: { limit?: number; offset?: number },
    signal?: AbortSignal,
  ): Promise<AdminWalletTransactionsResponse> {
    const response = await apiClient.get<AdminWalletTransactionsResponse>(
      `/api/admin/wallets/${walletId}/transactions`,
      { params, signal }
    )
    return response.data
  },

  async getWalletRefunds(
    walletId: string,
    params?: { limit?: number; offset?: number }
  ): Promise<AdminWalletRefundsResponse> {
    const response = await apiClient.get<AdminWalletRefundsResponse>(
      `/api/admin/wallets/${walletId}/refunds`,
      { params }
    )
    return response.data
  },

  async rechargeWallet(walletId: string, payload: ManualRechargeRequest): Promise<{
    wallet: AdminWallet
    payment_order: {
      id: string
      order_no: string
      amount_usd: number
      payment_method: string
      status: string
      created_at: string
      credited_at: string | null
    }
  }> {
    const response = await apiClient.post<{
    wallet: AdminWallet
    payment_order: {
      id: string
      order_no: string
      amount_usd: number
      payment_method: string
      status: string
      created_at: string
      credited_at: string | null
    }
  }>(`/api/admin/wallets/${walletId}/recharge`, payload)
    return response.data
  },

  async adjustWallet(walletId: string, payload: WalletAdjustRequest): Promise<{
    wallet: AdminWallet
    transaction: WalletTransaction
  }> {
    const response = await apiClient.post<{
    wallet: AdminWallet
    transaction: WalletTransaction
  }>(`/api/admin/wallets/${walletId}/adjust`, payload)
    return response.data
  },

  async getReferralRefundPreview(walletId: string, refundId: string, stage: 'process' | 'complete', signal?: AbortSignal): Promise<ReferralRefundPreview> {
    const response = await apiClient.get<ReferralRefundPreview>(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/referral-preview`, { params: { stage }, ...(signal ? { signal } : {}) }
    )
    return response.data
  },

  async processRefund(walletId: string, refundId: string, payload: ReferralRefundConfirmation = {}): Promise<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction
  }> {
    const response = await apiClient.post<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction
  }>(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/process`,
      payload
    )
    return response.data
  },

  async failRefund(walletId: string, refundId: string, payload: RefundFailRequest): Promise<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction | null
  }> {
    const response = await apiClient.post<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction | null
  }>(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/fail`,
      payload
    )
    return response.data
  },

  async completeRefund(
    walletId: string,
    refundId: string,
    payload: RefundCompleteRequest
  ): Promise<{ refund: RefundRequest; referral_reversal?: ReferralRefundReversal }> {
    const response = await apiClient.post<{ refund: RefundRequest; referral_reversal?: ReferralRefundReversal }>(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/complete`,
      payload
    )
    return response.data
  },
}
