import apiClient from './client'
import type { RefundRequest, WalletSummary, WalletTransaction } from './wallet'

export interface AdminWallet extends WalletSummary {
  user_id: string | null
  api_key_id: string | null
  owner_type: 'user' | 'api_key'
  owner_name: string | null
  owner_email?: string | null
  created_at: string
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
  owner_email?: string | null
  wallet_status?: string | null
}

export interface AdminGlobalRefund extends RefundRequest {
  wallet_id: string
  owner_type: 'user' | 'api_key'
  owner_name: string | null
  owner_email?: string | null
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

export interface RefundCompleteRequest {
  gateway_refund_id?: string
  payout_reference?: string
  payout_proof?: Record<string, unknown>
}

export const adminWalletApi = {
  async listWallets(params?: {
    status?: string
    limit?: number
    offset?: number
  }): Promise<AdminWalletListResponse> {
    const response = await apiClient.get<AdminWalletListResponse>('/api/admin/wallets', { params })
    return response.data
  },

  async listAllWallets(params?: {
    status?: string
  }): Promise<AdminWallet[]> {
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

  async getWalletDetail(walletId: string): Promise<AdminWalletDetailResponse> {
    const response = await apiClient.get<AdminWalletDetailResponse>(`/api/admin/wallets/${walletId}`)
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
    params?: { limit?: number; offset?: number }
  ): Promise<AdminWalletTransactionsResponse> {
    const response = await apiClient.get<AdminWalletTransactionsResponse>(
      `/api/admin/wallets/${walletId}/transactions`,
      { params }
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
    const response = await apiClient.post(`/api/admin/wallets/${walletId}/recharge`, payload)
    return response.data
  },

  async adjustWallet(walletId: string, payload: WalletAdjustRequest): Promise<{
    wallet: AdminWallet
    transaction: WalletTransaction
  }> {
    const response = await apiClient.post(`/api/admin/wallets/${walletId}/adjust`, payload)
    return response.data
  },

  async processRefund(walletId: string, refundId: string): Promise<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction
  }> {
    const response = await apiClient.post(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/process`,
      {}
    )
    return response.data
  },

  async failRefund(walletId: string, refundId: string, payload: RefundFailRequest): Promise<{
    wallet: AdminWallet
    refund: RefundRequest
    transaction: WalletTransaction | null
  }> {
    const response = await apiClient.post(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/fail`,
      payload
    )
    return response.data
  },

  async completeRefund(
    walletId: string,
    refundId: string,
    payload: RefundCompleteRequest
  ): Promise<{ refund: RefundRequest }> {
    const response = await apiClient.post(
      `/api/admin/wallets/${walletId}/refunds/${refundId}/complete`,
      payload
    )
    return response.data
  },
}
