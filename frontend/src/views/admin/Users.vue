<template>
  <div class="space-y-6 pb-8">
    <!-- 用户表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <UserManagementHeader
        :search-query="searchQuery"
        :filter-role="filterRole"
        :filter-group="filterGroup"
        :filter-status="filterStatus"
        :sort-option="sortOption"
        :user-groups="userGroups"
        :role-options="userRoleFilterOptions"
        :status-options="userStatusFilterOptions"
        :sort-options="userSortOptions"
        :loading="usersStore.loading"
        :can-operate-admin="authStore.canOperateAdmin"
        @update:search-query="searchQuery = $event"
        @update:filter-role="filterRole = $event"
        @update:filter-group="filterGroup = $event"
        @update:filter-status="filterStatus = $event"
        @update:sort-option="sortOption = $event"
        @open-groups="showUserGroupsDialog = true"
        @create-user="openCreateDialog"
        @refresh="refreshUsers"
      />

      <UserSelectionToolbar
        :is-all-filtered-selected="isAllFilteredSelected"
        :is-partially-filtered-selected="isPartiallyFilteredSelected"
        :filtered-user-count="filteredUserCount"
        :current-page-count="paginatedUsers.length"
        :selected-count="selectedCount"
        :is-current-page-fully-selected="isCurrentPageFullySelected"
        :can-clear-selection="canClearSelection"
        :select-all-filtered="selectAllFiltered"
        :loading="usersStore.loading"
        :can-operate-admin="authStore.canOperateAdmin"
        :group-count="userGroups.length"
        @toggle-select-filtered="toggleSelectFiltered"
        @toggle-select-current-page="toggleSelectCurrentPage"
        @clear-selection="clearSelection"
        @open-batch-dialog="openUserBatchDialog"
      />

      <UserManagementList
        :rows="userRows"
        :selected-id-set="selectedIdSet"
        :select-all-filtered="selectAllFiltered"
        :is-all-filtered-selected="isAllFilteredSelected"
        :is-partially-filtered-selected="isPartiallyFilteredSelected"
        :is-current-page-fully-selected="isCurrentPageFullySelected"
        :selection-disabled="selectAllFiltered || usersStore.loading"
        :loading="usersStore.loading"
        :can-operate-admin="authStore.canOperateAdmin"
        :has-filters="hasUserFilters"
        :sort-by="sortBy"
        :sort-order="sortOrder"
        @toggle-selected="toggleOne"
        @toggle-select-current-page="toggleSelectCurrentPage"
        @edit="editUser"
        @wallet="openWalletActionDialog"
        @plans="manageUserPlans"
        @api-keys="manageApiKeys"
        @sessions="manageUserSessions"
        @toggle-status="toggleUserStatus"
        @delete="deleteUser"
        @sort="handleTableSort"
      />

      <!-- 分页控件 -->
      <Pagination
        :current="currentPage"
        :total="filteredUserCount"
        :page-size="pageSize"
        cache-key="users-page-size"
        @update:current="handlePageChange"
        @update:page-size="handlePageSizeChange"
      />
    </Card>

    <!-- 用户表单对话框（创建/编辑共用） -->
    <UserFormDialog
      ref="userFormDialogRef"
      :open="showUserFormDialog"
      :user="editingUser"
      :groups="userGroups"
      @close="closeUserFormDialog"
      @submit="handleUserFormSubmit"
    />

    <UserBatchActionDialog
      :open="showUserBatchDialog"
      :selected-ids="selectedIds"
      :select-all-filtered="selectAllFiltered"
      :selected-count="selectedCount"
      :filters="batchSelectionFilters"
      :groups="userGroups"
      @close="showUserBatchDialog = false"
      @completed="handleUserBatchCompleted"
    />

    <UserGroupsDialog
      :open="showUserGroupsDialog"
      :users-version="userOptionsVersion"
      @close="showUserGroupsDialog = false"
      @changed="handleUserGroupsChanged"
    />

    <UserPlanDialog
      :open="showUserPlansDialog"
      :user-id="selectedUser?.id || null"
      :user-name="selectedUser?.username || ''"
      :entitlements="userPlanEntitlements"
      :plans="grantableBillingPlans"
      :selected-plan-id="selectedGrantPlanId"
      :grant-reason="grantReason"
      :loading-entitlements="loadingUserPlans"
      :loading-plans="loadingBillingPlans"
      :granting="grantingUserPlan"
      :format-date-time="formatDateTime"
      :format-plan-price="formatPlanPrice"
      :format-plan-duration="formatPlanDuration"
      :entitlement-labels="entitlementLabels"
      @close="showUserPlansDialog = false"
      @update:selected-plan-id="selectedGrantPlanId = $event"
      @update:grant-reason="grantReason = $event"
      @refresh-entitlements="loadUserPlanEntitlements"
      @grant="grantPlanToSelectedUser"
    />

    <UserApiKeysDialog
      :open="showApiKeysDialog"
      :api-keys="userApiKeys"
      :creating="creatingApiKey"
      :format-rate-limit="formatRateLimitSimple"
      :format-concurrent-limit="formatConcurrentLimitSimple"
      :format-ip-rules="formatIpRules"
      @close="showApiKeysDialog = false"
      @create-key="openCreateUserApiKeyDialog"
      @edit-key="openEditUserApiKeyDialog"
      @toggle-lock="toggleLockApiKey"
      @delete-key="deleteApiKey"
      @copy-full-key="copyFullKey"
    />

    <UserApiKeyFormDialog
      :open="showUserApiKeyFormDialog"
      :form="userApiKeyForm"
      :is-editing="Boolean(editingUserApiKey)"
      :creating="creatingApiKey"
      @close="closeUserApiKeyFormDialog"
      @update:form="userApiKeyForm = $event"
      @submit="submitUserApiKeyForm"
    />

    <UserSessionsDialog
      :open="showUserSessionsDialog"
      :sessions="userSessions"
      :loading="loadingUserSessions"
      :action-loading="sessionDialogActionLoading"
      :format-date="formatDate"
      :format-session-meta="formatSessionMeta"
      @close="showUserSessionsDialog = false"
      @revoke-session="revokeSelectedUserSession"
      @revoke-all="revokeAllSelectedUserSessions"
    />

    <WalletOpsDrawer
      :open="showWalletActionDialogState"
      :wallet="walletActionTarget?.wallet || null"
      :owner-name="walletActionTarget?.user.username || ''"
      :owner-subtitle="walletActionTarget?.user.email || legacyT('未设置邮箱')"
      :context-label="legacyT('用户钱包')"
      accent="emerald"
      @close="closeWalletActionDrawer"
      @changed="handleWalletDrawerChanged"
    />

    <NewApiKeyDialog
      :open="showNewApiKeyDialog"
      :api-key="newApiKey"
      @close="closeNewApiKeyDialog"
      @copy="copyApiKey"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useUsersStore } from '@/stores/users'
import { useAuthStore } from '@/stores/auth'
import {
  usersApi,
  type User,
  type ApiKey,
  type UserSession,
  type UserBatchActionResponse,
  type UserBatchSelectionFilters,
  type UserGroup,
  type AdminUserPlanEntitlement,
  type AdminUserSortBy,
  type AdminUserSortOrder,
} from '@/api/users'
import { formatSessionMeta } from '@/types/session'
import { adminWalletApi, type AdminWallet } from '@/api/admin-wallets'
import { adminBillingPlansApi, type BillingEntitlement, type BillingPlan } from '@/api/billing'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { adminApi } from '@/api/admin'
import { walletStatusBadge, walletStatusLabel } from '@/utils/walletDisplay'

// UI 组件
import {
  Card,
  Pagination,
} from '@/components/ui'

// 功能组件
import NewApiKeyDialog from '@/features/users/components/NewApiKeyDialog.vue'
import UserApiKeyFormDialog, { type UserApiKeyFormState } from '@/features/users/components/UserApiKeyFormDialog.vue'
import UserApiKeysDialog from '@/features/users/components/UserApiKeysDialog.vue'
import UserFormDialog, { type UserFormData } from '@/features/users/components/UserFormDialog.vue'
import UserBatchActionDialog from '@/features/users/components/UserBatchActionDialog.vue'
import UserGroupsDialog from '@/features/users/components/UserGroupsDialog.vue'
import UserManagementHeader from '@/features/users/components/UserManagementHeader.vue'
import UserManagementList from '@/features/users/components/UserManagementList.vue'
import UserPlanDialog from '@/features/users/components/UserPlanDialog.vue'
import UserSelectionToolbar from '@/features/users/components/UserSelectionToolbar.vue'
import UserSessionsDialog from '@/features/users/components/UserSessionsDialog.vue'
import type { UserManagementRow } from '@/features/users/components/user-management-types'
import {
  USER_ROLE_FILTER_OPTIONS,
  USER_SORT_OPTIONS,
  USER_STATUS_FILTER_OPTIONS,
  formatUserRoleLabel,
  userRoleBadgeVariant,
} from '@/features/users/components/user-management-config'
import WalletOpsDrawer from '@/features/wallet/components/WalletOpsDrawer.vue'
import { parseApiError } from '@/utils/errorParser'
import { formatTokens, formatRateLimitInheritable, formatRateLimitSimple, isRateLimitInherited, isRateLimitUnlimited } from '@/utils/format'
import {
  mergeChatPiiRedactionFeatureSettings,
  readChatPiiRedactionFeatureSettings,
} from '@/utils/featureSettings'
import { log } from '@/utils/logger'
import { useBatchSelection } from '@/composables/useBatchSelection'
import { useI18n } from '@/i18n'

const { success, error } = useToast()
const { confirmDanger } = useConfirm()
const { copyToClipboard } = useClipboard()
const { legacyT, locale } = useI18n()
const usersStore = useUsersStore()
const authStore = useAuthStore()

function localizedApiError(err: unknown, fallback: string): string {
  return legacyT(parseApiError(err, fallback))
}

// 用户表单对话框状态
const showUserFormDialog = ref(false)
const editingUser = ref<UserFormData | null>(null)
const userFormDialogRef = ref<InstanceType<typeof UserFormDialog>>()

// API Keys 对话框状态
const showApiKeysDialog = ref(false)
const showUserSessionsDialog = ref(false)
const showUserPlansDialog = ref(false)
const showNewApiKeyDialog = ref(false)
const showUserApiKeyFormDialog = ref(false)
const selectedUser = ref<User | null>(null)
const userApiKeys = ref<ApiKey[]>([])
const userSessions = ref<UserSession[]>([])
const userPlanEntitlements = ref<AdminUserPlanEntitlement[]>([])
const availableBillingPlans = ref<BillingPlan[]>([])
const selectedGrantPlanId = ref('')
const grantReason = ref('')
const newApiKey = ref('')
const creatingApiKey = ref(false)
const loadingUserSessions = ref(false)
const loadingUserPlans = ref(false)
const loadingBillingPlans = ref(false)
const grantingUserPlan = ref(false)
const sessionDialogActionLoading = ref<string | null>(null)
const editingUserApiKey = ref<ApiKey | null>(null)
const userApiKeyForm = ref<UserApiKeyFormState>({
  name: '',
  rate_limit: undefined,
  concurrent_limit: undefined,
  ip_rules_text: '',
  chat_pii_redaction_enabled: false,
  chat_pii_redaction_placeholder_notice: true,
})

// 用户统计
const userWalletMap = ref<Record<string, AdminWallet>>({})

const showWalletActionDialogState = ref(false)
const walletActionTarget = ref<{ user: User; wallet: AdminWallet } | null>(null)
const showUserBatchDialog = ref(false)
const showUserGroupsDialog = ref(false)
const userOptionsVersion = ref(0)

const searchQuery = ref('')
const filterRole = ref<'all' | User['role']>('all')
const filterStatus = ref<'all' | 'active' | 'inactive'>('all')
const filterGroup = ref('all')
const sortOption = ref<'default' | 'created_at_desc' | 'created_at_asc'>('default')
const userGroups = ref<UserGroup[]>([])
const userRoleFilterOptions = USER_ROLE_FILTER_OPTIONS
const userStatusFilterOptions = USER_STATUS_FILTER_OPTIONS
const userSortOptions = USER_SORT_OPTIONS
const sortBy = computed<AdminUserSortBy | null>(() =>
  sortOption.value === 'default' ? null : 'created_at'
)
const sortOrder = computed<AdminUserSortOrder>(() =>
  sortOption.value === 'created_at_asc' ? 'asc' : 'desc'
)

const currentPage = ref(1)
const pageSize = ref(20)
const USERS_PAGE_CACHE_TTL_MS = 10 * 1000
const USER_WALLETS_CACHE_TTL_MS = 10 * 1000
let userWalletsRequestId = 0

const filteredUsers = computed(() => usersStore.users)

const paginatedUsers = computed(() => filteredUsers.value)

const filteredUserCount = computed(() => usersStore.total)
const {
  selectedIds,
  selectAllFiltered,
  selectedIdSet,
  selectedCount,
  isAllFilteredSelected,
  isPartiallyFilteredSelected,
  isCurrentPageFullySelected,
  canClearSelection,
  rememberItems: rememberBatchPageUsers,
  resetSelection: resetBatchSelection,
  toggleOne,
  toggleSelectFiltered,
  toggleSelectCurrentPage,
  clearSelection,
} = useBatchSelection<User>({
  pageItems: paginatedUsers,
  filteredTotal: filteredUserCount,
  getItemId: (user) => user.id,
})

const batchSelectionFilters = computed<UserBatchSelectionFilters>(() => {
  const filters: UserBatchSelectionFilters = {}
  const search = searchQuery.value.trim()
  if (search) filters.search = search
  if (filterRole.value === 'admin' || filterRole.value === 'audit_admin' || filterRole.value === 'user') filters.role = filterRole.value
  if (filterStatus.value === 'active') filters.is_active = true
  if (filterStatus.value === 'inactive') filters.is_active = false
  if (filterGroup.value !== 'all') filters.group_id = filterGroup.value
  return filters
})

const grantableBillingPlans = computed(() =>
  availableBillingPlans.value.filter((plan) => hasPackageEntitlement(plan.entitlements))
)

const hasUserFilters = computed(() =>
  Boolean(searchQuery.value.trim())
  || filterRole.value !== 'all'
  || filterStatus.value !== 'all'
  || filterGroup.value !== 'all'
)

const userRows = computed<UserManagementRow[]>(() =>
  paginatedUsers.value.map((user) => {
    const totalBalance = getUserWalletTotalBalance(user)
    const walletStatus = getUserWalletStatus(user.id)
    return {
      user,
      roleLabel: legacyT(formatUserRoleLabel(user.role)),
      roleBadgeVariant: userRoleBadgeVariant(user.role),
      isUnlimited: isUserUnlimited(user),
      hasWallet: Boolean(getUserWallet(user.id)),
      totalBalanceLabel: formatCurrencyValue(totalBalance, '-'),
      packageBalanceLabel: formatCurrencyValue(getUserPackageBalance(user), '$0.00'),
      walletBalanceLabel: formatCurrencyValue(getUserWalletBalance(user), '$0.00'),
      consumedLabel: `$${getUserWalletConsumed(user).toFixed(2)}`,
      isNegativeBalance: isNegativeWalletValue(totalBalance),
      walletStatusLabel: walletStatusLabel(walletStatus),
      walletStatusVariant: walletStatusBadge(walletStatus),
      requestCountLabel: formatNumber(user.request_count),
      tokensLabel: formatTokens(user.total_tokens ?? 0),
      rateLimitLabel: formatRateLimitInheritable(user.rate_limit),
      rateLimitSource: formatUserEffectiveRateLimitSource(user),
      rateLimitAsBadge: isRateLimitInherited(user.rate_limit) || isRateLimitUnlimited(user.rate_limit),
      createdAtLabel: formatDate(user.created_at),
      statusLabel: legacyT(user.is_active ? '活跃' : '禁用'),
      statusVariant: user.is_active ? 'success' : 'destructive',
    }
  })
)

// Watch filter changes and reset to first page
watch([searchQuery, filterRole, filterStatus, filterGroup, sortOption], () => {
  currentPage.value = 1
  resetBatchSelection()
  void refreshUsers()
})

watch(paginatedUsers, (users) => rememberBatchPageUsers(users), { immediate: true })

onMounted(() => {
  void refreshUsers({ preferCache: true })
})

async function refreshUsers(options: { preferCache?: boolean } = {}) {
  const cacheTtlMs = options.preferCache ? USERS_PAGE_CACHE_TTL_MS : 0
  const search = searchQuery.value.trim()
  await Promise.all([
    usersStore.fetchUsers({
      cacheTtlMs,
      search: search || undefined,
      role: filterRole.value === 'all' ? undefined : filterRole.value,
      is_active: filterStatus.value === 'all' ? undefined : filterStatus.value === 'active',
      group_id: filterGroup.value === 'all' ? undefined : filterGroup.value,
      sort_by: sortBy.value ?? undefined,
      sort_order: sortBy.value ? sortOrder.value : undefined,
      skip: (currentPage.value - 1) * pageSize.value,
      limit: pageSize.value,
    }),
    loadUserGroups(),
  ])
  void loadUserWallets({
    cacheTtlMs: options.preferCache ? USER_WALLETS_CACHE_TTL_MS : 0,
  })
}

function handleTableSort(payload: { key: string, direction: AdminUserSortOrder }): void {
  if (payload.key !== 'created_at') return
  sortOption.value = payload.direction === 'asc' ? 'created_at_asc' : 'created_at_desc'
}

function handlePageChange(page: number): void {
  currentPage.value = page
  void refreshUsers({ preferCache: true })
}

function handlePageSizeChange(size: number): void {
  pageSize.value = size
  currentPage.value = 1
  resetBatchSelection()
  void refreshUsers()
}

async function loadUserGroups(): Promise<void> {
  try {
    const response = await usersStore.listUserGroups()
    userGroups.value = response.items
    if (filterGroup.value !== 'all' && !userGroups.value.some((group) => group.id === filterGroup.value)) {
      filterGroup.value = 'all'
    }
  } catch (err) {
    log.error('加载用户分组失败:', err)
  }
}

async function handleUserGroupsChanged(): Promise<void> {
  await refreshUsers()
}

function openUserBatchDialog(): void {
  if (selectedCount.value === 0 && userGroups.value.length === 0) return
  showUserBatchDialog.value = true
}

async function handleUserBatchCompleted(_result: UserBatchActionResponse): Promise<void> {
  await refreshUsers()
  resetBatchSelection(true)
}

function invalidateUserOptions(): void {
  userOptionsVersion.value += 1
}

function formatDate(dateString: string) {
  return new Date(dateString).toLocaleDateString(locale.value)
}

function formatDateTime(value?: string | null): string {
  if (!value) return '-'
  return new Date(value).toLocaleString(locale.value, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function formatPlanPrice(plan: BillingPlan): string {
  return `${Number(plan.price_amount || 0).toFixed(2)} ${plan.price_currency || 'CNY'}`
}

function formatPlanDuration(plan: BillingPlan): string {
  const labels: Record<string, string> = {
    day: legacyT('天'),
    month: legacyT('个月'),
    year: legacyT('年'),
    custom: legacyT('天'),
  }
  const unit = labels[plan.duration_unit] || legacyT('天')
  return `${Number(plan.duration_value || 1)}${unit}`
}

function entitlementLabels(items: BillingEntitlement[] | undefined): string[] {
  return (items || []).map((item) => {
    if (item.type === 'wallet_credit') {
      return `${legacyT('附赠余额')} $${Number(item.amount_usd || 0).toFixed(2)}`
    }
    if (item.type === 'daily_quota') {
      return `${legacyT('每日额度')} $${Number(item.daily_quota_usd || 0).toFixed(2)}`
    }
    if (item.type === 'membership_group') {
      return legacyT('会员权益')
    }
    return item.type
  })
}

function hasPackageEntitlement(items: BillingEntitlement[] | undefined): boolean {
  return (items || []).some((item) => item.type === 'daily_quota' || item.type === 'membership_group')
}

async function loadUserWallets(options: { cacheTtlMs?: number } = {}) {
  const requestId = ++userWalletsRequestId
  try {
    const wallets = await adminWalletApi.listAllWallets(
      { owner_type: 'user' },
      { cacheTtlMs: options.cacheTtlMs ?? 0 },
    )
    if (requestId !== userWalletsRequestId) return
    userWalletMap.value = wallets
      .filter((wallet) => !!wallet.user_id)
      .reduce<Record<string, AdminWallet>>((acc, wallet) => {
        acc[wallet.user_id as string] = wallet
        return acc
      }, {})
  } catch (err) {
    if (requestId !== userWalletsRequestId) return
    log.error('加载用户钱包失败:', err)
  }
}

function formatNumber(value?: number | null): string {
  const numericValue = typeof value === 'number' && Number.isFinite(value) ? value : 0
  return numericValue.toLocaleString()
}

function getUserWallet(userId: string): AdminWallet | null {
  return userWalletMap.value[userId] || null
}

function isUserUnlimited(user: User): boolean {
  const wallet = getUserWallet(user.id)
  if (wallet?.limit_mode === 'unlimited' || wallet?.unlimited === true) {
    return true
  }
  return Boolean(user.unlimited)
}

function getUserWalletTotalBalance(user: User): number | null {
  if (isUserUnlimited(user)) {
    return null
  }
  const wallet = getUserWallet(user.id)
  if (!wallet) {
    return null
  }
  if (typeof wallet.total_available_balance === 'number' && Number.isFinite(wallet.total_available_balance)) {
    return wallet.total_available_balance
  }
  return getUserWalletBalance(user) + getUserPackageBalance(user)
}

function getUserWalletBalance(user: User): number {
  const wallet = getUserWallet(user.id)
  const value = wallet?.wallet_balance ?? wallet?.balance ?? 0
  return Number.isFinite(value) ? value : 0
}

function getUserPackageBalance(user: User): number {
  const value = getUserWallet(user.id)?.package_balance ?? 0
  return Number.isFinite(value) ? value : 0
}

function getUserWalletConsumed(user: User): number {
  return getUserWallet(user.id)?.total_consumed ?? 0
}

function getUserWalletStatus(userId: string): string | null {
  return getUserWallet(userId)?.status ?? null
}

function formatCurrencyValue(value: number | null, nullLabel = '-'): string {
  if (value == null) {
    return nullLabel
  }
  return `$${value.toFixed(2)}`
}

function formatConcurrentLimitSimple(concurrentLimit?: number | null): string {
  if (concurrentLimit == null || concurrentLimit === 0) {
    return legacyT('不限并发')
  }
  return locale.value === 'en-US' ? `${concurrentLimit} concurrent` : `${concurrentLimit} 并发`
}

function formatIpRules(ipRules?: string[] | null): string {
  return ipRules && ipRules.length > 0 ? ipRules.join(', ') : legacyT('不限制')
}

function parseIpRulesInput(value: string): string[] | null {
  const items = value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
  return items.length > 0 ? items : null
}

function formatUserEffectiveRateLimitSource(user: User): string {
  const source = user.effective_policy?.rate_limit
  if (!source) return ''
  if (source.source === 'group' && source.group_name) {
    return `${legacyT('继承自分组：')}${source.group_name}`
  }
  if (source.source === 'combined') {
    const groupNames = Array.isArray(source.group_names) ? source.group_names.join(locale.value === 'en-US' ? ', ' : '、') : ''
    return groupNames ? `${legacyT('用户额外限制与分组叠加：')}${groupNames}` : legacyT('用户额外限制与分组叠加')
  }
  if (source.source === 'user') {
    return legacyT('用户单独配置')
  }
  return legacyT('系统默认')
}

function isNegativeWalletValue(value: number | null): boolean {
  return typeof value === 'number' && value < 0
}

async function toggleUserStatus(user: User) {
  const action = user.is_active ? '禁用' : '启用'
  const localizedAction = legacyT(action)
  const confirmed = await confirmDanger(
    locale.value === 'en-US'
      ? `${localizedAction} user ${user.username}?`
      : `确定要${action}用户 ${user.username} 吗？`,
    locale.value === 'en-US' ? `${localizedAction} user` : `${action}用户`,
    localizedAction
  )

  if (!confirmed) return

  try {
    await usersStore.updateUser(user.id, { is_active: !user.is_active })
    invalidateUserOptions()
    await refreshUsers()
    success(legacyT(`用户已${action}`))
  } catch (err: unknown) {
    error(localizedApiError(err, '未知错误'), legacyT(`${action}用户失败`))
  }
}

// ========== 用户表单对话框方法 ==========

function openCreateDialog() {
  editingUser.value = null
  showUserFormDialog.value = true
}

function editUser(user: User) {
  // 创建数组副本，避免与 store 数据共享引用
  editingUser.value = {
    id: user.id,
    username: user.username,
    email: user.email,
    unlimited: user.unlimited,
    role: user.role,
    is_active: user.is_active,
    group_ids: (user.groups || []).map(group => group.id),
    feature_settings: user.feature_settings ?? null,
  }
  showUserFormDialog.value = true
}

function closeUserFormDialog() {
  showUserFormDialog.value = false
  editingUser.value = null
}

async function handleUserFormSubmit(data: UserFormData & { password?: string; unlimited?: boolean }) {
  userFormDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      // 更新用户
      const updateData: Record<string, unknown> = {
        username: data.username,
        email: data.email || undefined,
        unlimited: data.unlimited,
        role: data.role,
        group_ids: data.group_ids ?? [],
        feature_settings: data.feature_settings ?? null,
      }
      if (data.password) {
        updateData.password = data.password
      }
      await usersStore.updateUser(data.id, updateData)
      invalidateUserOptions()
      success(legacyT('用户信息已更新'))
    } else {
      // 创建用户
      const newUser = await usersStore.createUser({
        username: data.username,
        password: data.password ?? '',
        email: data.email || undefined,
        initial_gift_usd: data.initial_gift_usd,
        unlimited: data.unlimited,
        role: data.role,
        group_ids: data.group_ids ?? [],
        feature_settings: data.feature_settings ?? null,
      })
      // 如果创建时指定为禁用，则更新状态
      if (data.is_active === false && newUser) {
        await usersStore.updateUser(newUser.id, { is_active: false })
      }
      invalidateUserOptions()
      success(legacyT('用户创建成功'))
    }
    closeUserFormDialog()
    await refreshUsers()
  } catch (err: unknown) {
    const title = data.id ? '更新用户失败' : '创建用户失败'
    error(localizedApiError(err, '未知错误'), legacyT(title))
  } finally {
    userFormDialogRef.value?.setSaving(false)
  }
}

async function manageApiKeys(user: User) {
  selectedUser.value = user
  showApiKeysDialog.value = true
  await loadUserApiKeys(user.id)
}

async function manageUserSessions(user: User) {
  selectedUser.value = user
  showUserSessionsDialog.value = true
  loadingUserSessions.value = true
  try {
    userSessions.value = await usersStore.getUserSessions(user.id)
  } catch (err) {
    error(localizedApiError(err, '加载用户设备会话失败'), legacyT('加载用户设备会话失败'))
  } finally {
    loadingUserSessions.value = false
  }
}

async function manageUserPlans(user: User) {
  selectedUser.value = user
  showUserPlansDialog.value = true
  selectedGrantPlanId.value = ''
  grantReason.value = ''
  await Promise.all([
    loadUserPlanEntitlements(user.id),
    loadAvailableBillingPlans(),
  ])
  if (!selectedGrantPlanId.value && grantableBillingPlans.value.length > 0) {
    selectedGrantPlanId.value = grantableBillingPlans.value[0].id
  }
}

async function loadUserPlanEntitlements(userId: string) {
  loadingUserPlans.value = true
  try {
    const response = await usersApi.listUserPlanEntitlements(userId)
    userPlanEntitlements.value = response.items
  } catch (err) {
    error(localizedApiError(err, '加载用户套餐失败'), legacyT('加载用户套餐失败'))
    userPlanEntitlements.value = []
  } finally {
    loadingUserPlans.value = false
  }
}

async function loadAvailableBillingPlans() {
  loadingBillingPlans.value = true
  try {
    const response = await adminBillingPlansApi.list()
    availableBillingPlans.value = response.items
    if (
      selectedGrantPlanId.value
      && !response.items.some((plan) => plan.id === selectedGrantPlanId.value)
    ) {
      selectedGrantPlanId.value = ''
    }
  } catch (err) {
    error(localizedApiError(err, '加载套餐列表失败'), legacyT('加载套餐列表失败'))
    availableBillingPlans.value = []
  } finally {
    loadingBillingPlans.value = false
  }
}

async function grantPlanToSelectedUser() {
  if (!selectedUser.value || !selectedGrantPlanId.value) return
  grantingUserPlan.value = true
  try {
    const response = await usersApi.grantUserPlan(selectedUser.value.id, {
      plan_id: selectedGrantPlanId.value,
      reason: grantReason.value.trim() || null,
    })
    userPlanEntitlements.value = response.items
    grantReason.value = ''
    success(legacyT('套餐已发放'))
  } catch (err) {
    error(localizedApiError(err, '发放套餐失败'), legacyT('发放套餐失败'))
  } finally {
    grantingUserPlan.value = false
  }
}

async function loadUserApiKeys(userId: string) {
  try {
    userApiKeys.value = await usersStore.getUserApiKeys(userId)
  } catch (err) {
    log.error('加载API Keys失败:', err)
    userApiKeys.value = []
  }
}

function openCreateUserApiKeyDialog() {
  const redactionFeature = readChatPiiRedactionFeatureSettings(null)
  userApiKeyForm.value = {
    name: `Key-${new Date().toISOString().split('T')[0]}`,
    rate_limit: undefined,
    concurrent_limit: undefined,
    ip_rules_text: '',
    chat_pii_redaction_enabled: redactionFeature.enabled,
    chat_pii_redaction_placeholder_notice: redactionFeature.inject_model_instruction,
  }
  editingUserApiKey.value = null
  showUserApiKeyFormDialog.value = true
}

function openEditUserApiKeyDialog(apiKey: ApiKey) {
  const redactionFeature = readChatPiiRedactionFeatureSettings(apiKey.feature_settings)
  editingUserApiKey.value = apiKey
  userApiKeyForm.value = {
    name: apiKey.name || '',
    rate_limit: apiKey.rate_limit ?? undefined,
    concurrent_limit: apiKey.concurrent_limit ?? undefined,
    ip_rules_text: apiKey.ip_rules?.join(', ') ?? '',
    chat_pii_redaction_enabled: redactionFeature.enabled,
    chat_pii_redaction_placeholder_notice: redactionFeature.inject_model_instruction,
  }
  showUserApiKeyFormDialog.value = true
}

function closeUserApiKeyFormDialog() {
  showUserApiKeyFormDialog.value = false
  editingUserApiKey.value = null
  userApiKeyForm.value = {
    name: '',
    rate_limit: undefined,
    concurrent_limit: undefined,
    ip_rules_text: '',
    chat_pii_redaction_enabled: false,
    chat_pii_redaction_placeholder_notice: true,
  }
}

async function submitUserApiKeyForm() {
  if (!selectedUser.value) return
  if (!userApiKeyForm.value.name.trim()) {
    error(legacyT('请输入密钥名称'), legacyT(editingUserApiKey.value ? '更新 API Key 失败' : '创建 API Key 失败'))
    return
  }

  creatingApiKey.value = true
  try {
    const ipRules = parseIpRulesInput(userApiKeyForm.value.ip_rules_text)
    if (editingUserApiKey.value) {
      await usersStore.updateApiKey(selectedUser.value.id, editingUserApiKey.value.id, {
        name: userApiKeyForm.value.name,
        rate_limit: userApiKeyForm.value.rate_limit ?? 0,
        concurrent_limit: userApiKeyForm.value.concurrent_limit,
        ip_rules: ipRules,
        feature_settings: mergeChatPiiRedactionFeatureSettings(editingUserApiKey.value.feature_settings, {
          enabled: userApiKeyForm.value.chat_pii_redaction_enabled,
          inject_model_instruction: userApiKeyForm.value.chat_pii_redaction_placeholder_notice,
        }),
      })
      success(legacyT('API Key已更新'))
    } else {
      const response = await usersStore.createApiKey(selectedUser.value.id, {
        name: userApiKeyForm.value.name,
        rate_limit: userApiKeyForm.value.rate_limit ?? 0,
        concurrent_limit: userApiKeyForm.value.concurrent_limit,
        ip_rules: ipRules,
        feature_settings: mergeChatPiiRedactionFeatureSettings(null, {
          enabled: userApiKeyForm.value.chat_pii_redaction_enabled,
          inject_model_instruction: userApiKeyForm.value.chat_pii_redaction_placeholder_notice,
        }),
      })
      newApiKey.value = response.key || ''
      showNewApiKeyDialog.value = true
      success(legacyT('API Key创建成功'))
    }
    await loadUserApiKeys(selectedUser.value.id)
    closeUserApiKeyFormDialog()
  } catch (err: unknown) {
    error(localizedApiError(err, '未知错误'), legacyT(editingUserApiKey.value ? '更新 API Key 失败' : '创建 API Key 失败'))
  } finally {
    creatingApiKey.value = false
  }
}

async function revokeSelectedUserSession(sessionId: string) {
  if (!selectedUser.value) return
  sessionDialogActionLoading.value = sessionId
  try {
    await usersStore.revokeUserSession(selectedUser.value.id, sessionId)
    userSessions.value = userSessions.value.filter((session) => session.id !== sessionId)
    success(legacyT('设备已强制下线'))
  } catch (err) {
    error(localizedApiError(err, '强制下线失败'), legacyT('强制下线失败'))
  } finally {
    sessionDialogActionLoading.value = null
  }
}

async function revokeAllSelectedUserSessions() {
  if (!selectedUser.value) return
  sessionDialogActionLoading.value = 'all'
  try {
    const result = await usersStore.revokeAllUserSessions(selectedUser.value.id)
    userSessions.value = []
    success(result.revoked_count > 0
      ? legacyT(`已强制下线 ${result.revoked_count} 个设备`)
      : legacyT('没有可下线的设备'))
  } catch (err) {
    error(localizedApiError(err, '强制下线全部设备失败'), legacyT('强制下线全部设备失败'))
  } finally {
    sessionDialogActionLoading.value = null
  }
}

async function copyApiKey() {
  await copyToClipboard(newApiKey.value)
}

async function closeNewApiKeyDialog() {
  showNewApiKeyDialog.value = false
  newApiKey.value = ''
}

async function deleteApiKey(apiKey: ApiKey) {
  const confirmed = await confirmDanger(
    locale.value === 'en-US'
      ? `Delete this API key?\n\n${apiKey.key_display || '****'}\n\nThis action cannot be undone.`
      : `确定要删除这个API Key吗？\n\n${apiKey.key_display || '****'}\n\n此操作无法撤销。`,
    legacyT('删除 API Key')
  )

  if (!confirmed) return

  try {
    await usersStore.deleteApiKey(selectedUser.value.id, apiKey.id)
    await loadUserApiKeys(selectedUser.value.id)
    success(legacyT('API Key已删除'))
  } catch (err: unknown) {
    error(localizedApiError(err, '未知错误'), legacyT('删除 API Key 失败'))
  }
}

async function toggleLockApiKey(apiKey: ApiKey) {
  if (!selectedUser.value) return
  try {
    const response = await adminApi.toggleUserApiKeyLock(selectedUser.value.id, apiKey.id)
    // 更新本地状态
    const index = userApiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      userApiKeys.value[index].is_locked = response.is_locked
    }
    success(legacyT(response.message))
  } catch (err: unknown) {
    log.error('切换密钥锁定状态失败:', err)
    error(localizedApiError(err, '操作失败'), legacyT('锁定/解锁失败'))
  }
}

async function copyFullKey(apiKey: ApiKey) {
  if (!selectedUser.value) return
  try {
    const response = await usersStore.getFullApiKey(selectedUser.value.id, apiKey.id)
    await copyToClipboard(response.key)
  } catch (err: unknown) {
    log.error('复制密钥失败:', err)
    error(localizedApiError(err, '未知错误'), legacyT('复制密钥失败'))
  }
}

function openWalletActionDialog(user: User) {
  const wallet = getUserWallet(user.id)
  if (!wallet) {
    error(legacyT('该用户的钱包尚未初始化，暂时无法进行资金操作'))
    return
  }

  walletActionTarget.value = {
    user,
    wallet,
  }
  showWalletActionDialogState.value = true
}

function closeWalletActionDrawer() {
  showWalletActionDialogState.value = false
}

async function handleWalletDrawerChanged() {
  await loadUserWallets()
  if (!walletActionTarget.value) return
  const latestWallet = getUserWallet(walletActionTarget.value.user.id)
  if (latestWallet) {
    walletActionTarget.value.wallet = latestWallet
  }
}

async function deleteUser(user: User) {
  const confirmed = await confirmDanger(
    locale.value === 'en-US'
      ? `Delete user ${user.username}?\n\nThis will delete:\n- User account\n- All API keys\n- All usage records\n\nThis action cannot be undone.`
      : `确定要删除用户 ${user.username} 吗？\n\n此操作将删除：\n• 用户账户\n• 所有API密钥\n• 所有使用记录\n\n此操作无法撤销！`,
    legacyT('删除用户')
  )

  if (!confirmed) return

  try {
    await usersStore.deleteUser(user.id)
    invalidateUserOptions()
    if (usersStore.users.length === 0 && currentPage.value > 1) {
      currentPage.value -= 1
    }
    await refreshUsers()
    success(legacyT('用户已删除'))
  } catch (err: unknown) {
    error(localizedApiError(err, '未知错误'), legacyT('删除用户失败'))
  }
}
</script>
