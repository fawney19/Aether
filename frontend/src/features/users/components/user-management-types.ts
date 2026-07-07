import type { ListPolicyMode, RateLimitPolicyMode, User, UserRole } from '@/api/users'

export type UserFilterRole = 'all' | UserRole
export type UserFilterStatus = 'all' | 'active' | 'inactive'
export type UserSortOption = 'default' | 'created_at_desc' | 'created_at_asc'
export type UserBatchQuotaMode = 'skip' | 'wallet' | 'unlimited'
export type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'

export interface UserFilterOption<TValue extends string = string> {
  value: TValue
  label: string
}

export interface UserSelectOption {
  value: string
  label: string
}

export interface UserGroupFormState {
  name: string
  allowed_providers_mode: ListPolicyMode
  allowed_api_formats_mode: ListPolicyMode
  allowed_models_mode: ListPolicyMode
  allowed_providers: string[]
  allowed_api_formats: string[]
  allowed_models: string[]
  rate_limit_mode: RateLimitPolicyMode
  rate_limit: number | undefined
}

export interface UserManagementRow {
  user: User
  roleLabel: string
  roleBadgeVariant: BadgeVariant
  isUnlimited: boolean
  hasWallet: boolean
  totalBalanceLabel: string
  packageBalanceLabel: string
  walletBalanceLabel: string
  consumedLabel: string
  isNegativeBalance: boolean
  walletStatusLabel: string
  walletStatusVariant: BadgeVariant
  requestCountLabel: string
  tokensLabel: string
  rateLimitLabel: string
  rateLimitSource: string
  rateLimitAsBadge: boolean
  createdAtLabel: string
  statusLabel: string
  statusVariant: BadgeVariant
}
