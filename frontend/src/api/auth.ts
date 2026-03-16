import apiClient from './client'
import { log } from '@/utils/logger'

export interface LoginRequest {
  email: string
  password: string
  auth_type?: 'local' | 'ldap'
}

export interface LoginResponse {
  access_token: string
  token_type?: string
  expires_in?: number
  user_id?: string // UUID
  email?: string
  username?: string
  role?: string
}

export interface UserPreferences {
  theme?: 'light' | 'dark' | 'auto'
  language?: string
  notifications_enabled?: boolean
  [key: string]: unknown // 允许扩展其他偏好设置
}

export interface UserStats {
  total_requests?: number
  total_cost?: number
  last_request_at?: string
  [key: string]: unknown // 允许扩展其他统计数据
}

export interface SendVerificationCodeRequest {
  email: string
}

export interface SendVerificationCodeResponse {
  message: string
  success: boolean
  expire_minutes?: number
}

export interface VerifyEmailRequest {
  email: string
  code: string
}

export interface VerifyEmailResponse {
  message: string
  success: boolean
}

export interface VerificationStatusRequest {
  email: string
}

export interface VerificationStatusResponse {
  email: string
  has_pending_code: boolean
  is_verified: boolean
  cooldown_remaining: number | null
  code_expires_in: number | null
}

export interface RegisterRequest {
  email?: string
  username: string
  password: string
}

export interface RegisterResponse {
  user_id: string
  email?: string
  username: string
  message: string
}

export interface RegistrationSettingsResponse {
  enable_registration: boolean
  require_email_verification: boolean
  email_configured: boolean
  password_policy_level: string
}

export interface AuthSettingsResponse {
  local_enabled: boolean
  ldap_enabled: boolean
  ldap_exclusive: boolean
}

export interface BillingSummary {
  id?: string | null
  balance: number
  recharge_balance: number
  gift_balance: number
  refundable_balance: number
  currency: string
  status: string
  limit_mode: 'finite' | 'unlimited'
  unlimited: boolean
  total_recharged: number
  total_consumed: number
  total_refunded: number
  total_adjusted: number
  updated_at?: string | null
}

export interface User {
  id: string // UUID
  username: string
  email?: string
  role: string  // 'admin' or 'user'
  is_active: boolean
  billing?: BillingSummary
  allowed_providers?: string[] | null  // 允许使用的提供商 ID 列表
  allowed_api_formats?: string[] | null  // 允许使用的 API 格式列表
  allowed_models?: string[] | null  // 允许使用的模型名称列表
  created_at: string
  last_login_at?: string
  preferences?: UserPreferences
  stats?: UserStats
}

export const authApi = {
  async login(credentials: LoginRequest): Promise<LoginResponse> {
    const response = await apiClient.post<LoginResponse>('/api/auth/login', credentials)
    apiClient.setToken(response.data.access_token)
    return response.data
  },

  async logout() {
    try {
      // 调用后端登出接口，将 Token 加入黑名单
      await apiClient.post('/api/auth/logout', {})
    } catch (error) {
      // 即使后端登出失败，也要清除本地认证信息
      log.warn('后端登出失败，仅清除本地认证信息:', error)
    } finally {
      // 清除本地认证信息
      apiClient.clearAuth()
    }
  },

  async getCurrentUser(): Promise<User> {
    const response = await apiClient.get<User>('/api/users/me')
    return response.data
  },

  async refreshToken(): Promise<LoginResponse> {
    const response = await apiClient.post<LoginResponse>('/api/auth/refresh', {})
    apiClient.setToken(response.data.access_token)
    return response.data
  },

  async sendVerificationCode(email: string): Promise<SendVerificationCodeResponse> {
    const response = await apiClient.post<SendVerificationCodeResponse>(
      '/api/auth/send-verification-code',
      { email }
    )
    return response.data
  },

  async verifyEmail(email: string, code: string): Promise<VerifyEmailResponse> {
    const response = await apiClient.post<VerifyEmailResponse>(
      '/api/auth/verify-email',
      { email, code }
    )
    return response.data
  },

  async register(data: RegisterRequest): Promise<RegisterResponse> {
    const response = await apiClient.post<RegisterResponse>('/api/auth/register', data)
    return response.data
  },

  async getRegistrationSettings(): Promise<RegistrationSettingsResponse> {
    const response = await apiClient.get<RegistrationSettingsResponse>(
      '/api/auth/registration-settings'
    )
    return response.data
  },

  async getVerificationStatus(email: string): Promise<VerificationStatusResponse> {
    const response = await apiClient.post<VerificationStatusResponse>(
      '/api/auth/verification-status',
      { email }
    )
    return response.data
  },

  async getAuthSettings(): Promise<AuthSettingsResponse> {
    const response = await apiClient.get<AuthSettingsResponse>('/api/auth/settings')
    return response.data
  }
}
