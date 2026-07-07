import {
  Ban,
  CheckCircle2,
  ShieldCheck,
  UserCog,
} from 'lucide-vue-next'
import type { Component } from 'vue'
import type { UserBatchAction, UserRole } from '@/api/users'
import type {
  UserFilterOption,
  UserFilterRole,
  UserFilterStatus,
  UserSortOption,
} from './user-management-types'

export interface UserBatchActionOption {
  value: UserBatchAction
  label: string
  description: string
  icon: Component
}

export const USER_ROLE_FILTER_OPTIONS: UserFilterOption<UserFilterRole>[] = [
  { value: 'all', label: '全部角色' },
  { value: 'admin', label: '管理员' },
  { value: 'audit_admin', label: '审计管理员' },
  { value: 'user', label: '普通用户' },
]

export const USER_STATUS_FILTER_OPTIONS: UserFilterOption<UserFilterStatus>[] = [
  { value: 'all', label: '全部状态' },
  { value: 'active', label: '活跃' },
  { value: 'inactive', label: '禁用' },
]

export const USER_SORT_OPTIONS: UserFilterOption<UserSortOption>[] = [
  { value: 'default', label: '默认排序' },
  { value: 'created_at_desc', label: '创建时间 新到旧' },
  { value: 'created_at_asc', label: '创建时间 旧到新' },
]

export const USER_BATCH_ACTION_OPTIONS: UserBatchActionOption[] = [
  {
    value: 'enable',
    label: '启用',
    description: '恢复用户登录与调用',
    icon: CheckCircle2,
  },
  {
    value: 'disable',
    label: '禁用',
    description: '暂停用户访问权限',
    icon: Ban,
  },
  {
    value: 'update_access_control',
    label: '额度',
    description: '批量调整用户额度模式',
    icon: ShieldCheck,
  },
  {
    value: 'update_role',
    label: '修改角色',
    description: '批量设为普通用户或管理员',
    icon: UserCog,
  },
]

export function formatUserRoleLabel(role: UserRole | string): string {
  if (role === 'admin') return '管理员'
  if (role === 'audit_admin') return '审计管理员'
  return '普通用户'
}

export function userRoleBadgeVariant(role: UserRole | string): 'default' | 'secondary' {
  return role === 'admin' ? 'default' : 'secondary'
}
