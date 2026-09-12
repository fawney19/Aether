import apiClient from './client'

export interface ReferralSummary {
  total_invites: number
  effective_invites: number
  paid_reward_usd: number
  pending_reward_usd: number
  reversed_reward_usd: number
}

export interface ReferralDashboardResponse {
  invite_code: string
  invitation_link: string
  summary: ReferralSummary
}

export interface ReferralRelationshipRecord {
  id: string
  inviter_user_id: string
  inviter_username?: string | null
  invitee_user_id: string
  invitee_username?: string | null
  invite_code_snapshot: string
  first_paid_order_id?: string | null
  first_paid_at_unix_secs?: number | null
  source?: Record<string, unknown> | null
  created_at_unix_secs: number
}

export interface ReferralRewardRecord {
  id: string
  referral_id: string
  inviter_user_id: string
  invitee_user_id: string
  reward_type: string
  source_order_id?: string | null
  trigger_point: string
  amount_usd: number
  status: string
  wallet_transaction_id?: string | null
  idempotency_key: string
  reversed_amount_usd: number
  pending_reversal_amount_usd: number
  admin_operator_id?: string | null
  admin_note?: string | null
  created_at_unix_secs: number
  updated_at_unix_secs: number
}

export interface ReferralListResponse<T> {
  items: T[]
  total: number
  limit: number
  offset: number
  stats: ReferralSummary
}

export interface ReferralRelationshipQuery {
  inviter?: string
  invitee?: string
  invite_code?: string
  first_paid?: boolean | null
  limit?: number
  offset?: number
}

export interface ReferralRewardQuery {
  order_id?: string
  reward_type?: string
  status?: string
  limit?: number
  offset?: number
}

function cleanParams<T extends Record<string, unknown>>(params: T): Partial<T> {
  return Object.fromEntries(
    Object.entries(params).filter(([, value]) => value !== undefined && value !== null && value !== '')
  ) as Partial<T>
}

export const referralApi = {
  async getMyReferral(): Promise<ReferralDashboardResponse> {
    const response = await apiClient.get<ReferralDashboardResponse>('/api/users/me/referral')
    return response.data
  },

  async getAdminReferrals(
    params: ReferralRelationshipQuery = {}
  ): Promise<ReferralListResponse<ReferralRelationshipRecord>> {
    const response = await apiClient.get<ReferralListResponse<ReferralRelationshipRecord>>('/api/admin/referrals', {
      params: cleanParams(params as Record<string, unknown>)
    })
    return response.data
  },

  async getAdminReferralRewards(
    params: ReferralRewardQuery = {}
  ): Promise<ReferralListResponse<ReferralRewardRecord>> {
    const response = await apiClient.get<ReferralListResponse<ReferralRewardRecord>>('/api/admin/referral-rewards', {
      params: cleanParams(params as Record<string, unknown>)
    })
    return response.data
  },

  async retryReferralReward(id: string, note?: string): Promise<{ reward: ReferralRewardRecord }> {
    const response = await apiClient.post<{ reward: ReferralRewardRecord }>(`/api/admin/referral-rewards/${id}/retry`, { note })
    return response.data
  },

  async voidReferralReward(id: string, note?: string): Promise<{ reward: ReferralRewardRecord }> {
    const response = await apiClient.post<{ reward: ReferralRewardRecord }>(`/api/admin/referral-rewards/${id}/void`, { note })
    return response.data
  }
}
