<template>
  <div class="space-y-4 pb-6 sm:space-y-5 sm:pb-8">
    <div class="grid gap-4 lg:grid-cols-[264px_minmax(0,1fr)] lg:gap-5 xl:grid-cols-[280px_minmax(0,1fr)] xl:gap-6">
      <Card
        variant="default"
        class="overflow-hidden"
      >
        <div class="border-b border-border/60 px-3.5 py-3 sm:px-4 sm:py-4">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h3 class="text-base font-semibold">
                用户分组
              </h3>
              <p class="mt-0.5 hidden text-[11px] text-muted-foreground sm:block">
                独立维护分组默认限制，并统一管理分组成员。
              </p>
            </div>
            <Button
              class="h-8 shrink-0 px-3"
              @click="openCreateGroupDialog"
            >
              <Plus class="mr-1.5 h-3.5 w-3.5" />
              新增
            </Button>
          </div>

          <div class="relative mt-3">
            <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              v-model="groupSearchQuery"
              type="text"
              placeholder="搜索分组名称..."
              class="h-9 pl-8 text-sm"
            />
          </div>
        </div>

        <div class="p-2.5 sm:p-3">
          <div
            v-if="filteredGroups.length === 0"
            class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
          >
            {{ groupSearchQuery ? '没有匹配的分组' : '还没有用户分组' }}
          </div>

          <div
            v-else
            class="flex gap-2 overflow-x-auto pb-1 sm:max-h-[68vh] sm:flex-col sm:overflow-y-auto sm:pb-0"
          >
            <button
              v-for="group in filteredGroups"
              :key="group.id"
              type="button"
              class="relative w-full min-w-[220px] shrink-0 overflow-hidden rounded-xl border px-3 py-2.5 text-left transition-all sm:min-w-0 sm:px-4 sm:py-3"
              :class="selectedGroupId === group.id
                ? 'border-border bg-muted/35 shadow-sm'
                : 'border-border/60 bg-card hover:border-primary/30 hover:bg-muted/30'"
              @click="selectedGroupId = group.id"
            >
              <span
                v-if="selectedGroupId === group.id"
                class="absolute inset-y-2 left-0 w-1 rounded-r-full bg-primary"
              />
              <div class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="truncate text-sm font-semibold text-foreground">
                      {{ group.name }}
                    </span>
                    <Badge
                      v-if="group.is_default"
                      variant="outline"
                      class="h-5 shrink-0 px-1.5 py-0 text-[10px] font-medium"
                    >
                      默认
                    </Badge>
                    <Badge
                      variant="secondary"
                      class="h-5 shrink-0 px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ group.user_count }} 人
                    </Badge>
                  </div>
                  <p class="mt-1 line-clamp-1 text-[11px] text-muted-foreground sm:line-clamp-2 sm:text-xs">
                    {{ group.description || '未填写分组说明' }}
                  </p>
                </div>
              </div>
            </button>
          </div>
        </div>
      </Card>

      <div class="space-y-4 sm:space-y-5 lg:space-y-6">
        <Card
          variant="default"
          class="overflow-hidden"
        >
          <div
            v-if="selectedGroup"
            class="space-y-4 px-3.5 py-3.5 sm:px-5 sm:py-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <h2 class="truncate text-lg font-semibold text-foreground">
                    {{ selectedGroup.name }}
                  </h2>
                  <Badge
                    v-if="selectedGroup.is_default"
                    variant="outline"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    默认分组
                  </Badge>
                  <Badge
                    variant="secondary"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    {{ selectedGroup.user_count }} 人
                  </Badge>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">
                  {{ selectedGroup.description || '未填写分组说明' }}
                </p>
              </div>

              <div class="grid grid-cols-2 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  @click="editUserGroup(selectedGroup)"
                >
                  <SquarePen class="mr-1.5 h-3.5 w-3.5" />
                  编辑分组
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full border-rose-200 px-3 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40 sm:w-auto"
                  :disabled="selectedGroup.is_default"
                  @click="removeUserGroup(selectedGroup)"
                >
                  <Trash2 class="mr-1.5 h-3.5 w-3.5" />
                  删除分组
                </Button>
              </div>
            </div>

            <div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-5">
              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <UsersIcon class="h-4 w-4" />
                  <span class="text-xs font-medium">成员数量</span>
                </div>
                <div class="mt-2 text-xl font-semibold text-foreground sm:text-2xl">
                  {{ selectedGroup.user_count }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Layers3 class="h-4 w-4" />
                  <span class="text-xs font-medium">模型分组</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ formatModelGroupBindingsSummary(selectedGroup.model_group_bindings) }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Braces class="h-4 w-4" />
                  <span class="text-xs font-medium">API 格式</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ formatRestrictionSummary(selectedGroup.allowed_api_formats, '个格式') }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Shield class="h-4 w-4" />
                  <span class="text-xs font-medium">调度策略</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ formatSchedulingMode(selectedGroup.scheduling_mode) }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Gauge class="h-4 w-4" />
                  <span class="text-xs font-medium">分组速率</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ formatRateLimitInheritable(selectedGroup.rate_limit) }}
                </div>
              </div>
            </div>

            <div class="rounded-xl border border-border/60 bg-card px-3 py-3">
              <div class="flex items-center gap-2 text-muted-foreground">
                <Shield class="h-4 w-4" />
                <span class="text-xs font-medium">绑定顺序</span>
              </div>
              <div
                v-if="selectedGroup.model_group_bindings.length === 0"
                class="mt-2 text-sm text-muted-foreground"
              >
                未显式绑定，保存时会自动落到默认模型分组。
              </div>
              <div
                v-else
                class="mt-3 flex flex-wrap gap-2"
              >
                <Badge
                  v-for="(binding, index) in selectedGroup.model_group_bindings"
                  :key="`${binding.model_group_id}-${index}`"
                  variant="outline"
                  class="h-7 gap-1.5 px-2.5 text-xs"
                >
                  <span>{{ index + 1 }}</span>
                  <span>{{ binding.model_group_display_name || binding.model_group_name || binding.model_group_id }}</span>
                  <span
                    v-if="!binding.is_active"
                    class="text-muted-foreground"
                  >停用</span>
                </Badge>
              </div>
            </div>
          </div>

          <div
            v-else
            class="px-5 py-9 text-center sm:px-6 sm:py-12"
          >
            <div class="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-primary/10">
              <UsersIcon class="h-5 w-5 text-primary" />
            </div>
            <h3 class="mt-4 text-base font-semibold text-foreground">
              选择一个分组
            </h3>
            <p class="mt-2 text-sm text-muted-foreground">
              左侧选择现有分组，或先创建一个新分组来配置默认访问限制。
            </p>
          </div>
        </Card>

        <Card
          variant="default"
          class="overflow-hidden"
        >
          <div class="border-b border-border/60 px-3.5 py-3.5 sm:px-5 sm:py-4">
            <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
              <div>
                <h3 class="text-base font-semibold">
                  批量成员管理
                </h3>
                <p class="mt-0.5 text-[11px] text-muted-foreground sm:text-xs">
                  选择用户后，可批量绑定到当前分组，或从当前分组移回默认分组。
                </p>
              </div>

              <div class="grid grid-cols-2 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Badge
                  variant="secondary"
                  class="col-span-2 h-6 justify-center px-2 text-xs sm:col-span-1 sm:justify-start"
                >
                  已选 {{ selectedUserIds.length }} 人
                </Badge>
                <Button
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  :disabled="!selectedGroup || selectedUserIds.length === 0 || batchSubmitting"
                  @click="handleBatchBind"
                >
                  <Link2 class="mr-1.5 h-3.5 w-3.5" />
                  {{ batchSubmitting ? '处理中...' : '批量绑定' }}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  :disabled="!selectedGroup || selectedGroup.is_default || selectedCurrentGroupUserCount === 0 || batchSubmitting"
                  @click="handleBatchUnbind"
                >
                  <Link2Off class="mr-1.5 h-3.5 w-3.5" />
                  移回默认
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  :disabled="selectedUserIds.length === 0"
                  @click="selectedUserIds = []"
                >
                  清空选择
                </Button>
              </div>
            </div>

            <div class="mt-3 grid gap-2 sm:grid-cols-[minmax(0,1fr)_148px] lg:grid-cols-[minmax(0,1fr)_160px]">
              <div class="relative">
                <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  v-model="userSearchQuery"
                  type="text"
                  placeholder="搜索用户名、邮箱或分组..."
                  class="h-9 pl-8 text-sm"
                />
              </div>

              <Select v-model="membershipFilter">
                <SelectTrigger class="h-9 text-sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部用户
                  </SelectItem>
                  <SelectItem
                    value="current"
                    :disabled="!selectedGroup"
                  >
                    当前分组成员
                  </SelectItem>
                  <SelectItem
                    value="default"
                    :disabled="!defaultGroup"
                  >
                    默认分组成员
                  </SelectItem>
                  <SelectItem
                    value="other"
                    :disabled="!selectedGroup"
                  >
                    其他分组成员
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div class="hidden overflow-x-auto xl:block">
            <Table>
              <TableHeader>
                <TableRow class="border-b border-border/60 hover:bg-transparent">
                  <TableHead class="w-12">
                    <Checkbox
                      :checked="allPageSelected"
                      @update:checked="togglePageSelection"
                    />
                  </TableHead>
                  <TableHead class="w-[240px]">
                    用户
                  </TableHead>
                  <TableHead class="w-[148px] whitespace-nowrap">
                    当前分组
                  </TableHead>
                  <TableHead class="w-[170px]">
                    限速
                  </TableHead>
                  <TableHead class="w-[180px] text-right">
                    快捷操作
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                <TableRow
                  v-for="user in paginatedUsers"
                  :key="user.id"
                  class="border-b border-border/40"
                >
                  <TableCell>
                    <Checkbox
                      :checked="selectedUserIdSet.has(user.id)"
                      @update:checked="(checked) => toggleUserSelection(user.id, checked)"
                    />
                  </TableCell>
                  <TableCell>
                    <div class="min-w-0">
                      <div class="truncate text-sm font-medium text-foreground">
                        {{ user.username }}
                      </div>
                      <div class="truncate text-xs text-muted-foreground">
                        {{ user.email || '-' }}
                      </div>
                    </div>
                  </TableCell>
                  <TableCell class="whitespace-nowrap">
                    <Badge
                      variant="outline"
                      class="h-5 whitespace-nowrap px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ formatUserGroupName(user.group_name) }}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <span class="text-sm text-foreground">
                      {{ formatRateLimitInheritable(user.effective_rate_limit) }}
                    </span>
                  </TableCell>
                  <TableCell class="text-right">
                    <div class="flex justify-end gap-2">
                      <Button
                        size="sm"
                        class="h-8 px-3 text-xs"
                        :disabled="!selectedGroup || batchSubmitting"
                        @click="handleSingleBind(user.id)"
                      >
                        绑定
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        class="h-8 px-3 text-xs"
                        :disabled="!selectedGroup || selectedGroup.is_default || user.group_id !== selectedGroup.id || batchSubmitting"
                        @click="handleSingleUnbind(user.id)"
                      >
                        移回默认
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              </TableBody>
            </Table>
          </div>

          <div class="space-y-2.5 p-2.5 sm:p-3 xl:hidden">
            <div
              v-if="paginatedUsers.length === 0"
              class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
            >
              没有匹配的用户
            </div>

            <div
              v-for="user in paginatedUsers"
              :key="user.id"
              class="rounded-xl border border-border/60 bg-card p-3"
            >
              <div class="flex items-start gap-3">
                <Checkbox
                  :checked="selectedUserIdSet.has(user.id)"
                  class="mt-0.5"
                  @update:checked="(checked) => toggleUserSelection(user.id, checked)"
                />
                <div class="min-w-0 flex-1 space-y-2">
                  <div class="flex items-center gap-2">
                    <div class="truncate text-sm font-semibold text-foreground">
                      {{ user.username }}
                    </div>
                    <Badge
                      variant="outline"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ formatUserGroupName(user.group_name) }}
                    </Badge>
                  </div>
                  <div class="mt-1 truncate text-xs text-muted-foreground">
                    {{ user.email || '-' }}
                  </div>
                  <div class="grid grid-cols-2 gap-2 text-[11px]">
                    <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                      <div class="text-muted-foreground">
                        当前分组
                      </div>
                      <div class="mt-1 truncate font-medium text-foreground">
                        {{ formatUserGroupName(user.group_name) }}
                      </div>
                    </div>
                    <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                      <div class="text-muted-foreground">
                        限速
                      </div>
                      <div class="mt-1 truncate font-medium text-foreground">
                        {{ formatRateLimitInheritable(user.effective_rate_limit) }}
                      </div>
                    </div>
                  </div>
                  <div class="grid grid-cols-2 gap-2">
                    <Button
                      size="sm"
                      class="h-8 w-full px-3 text-xs"
                      :disabled="!selectedGroup || batchSubmitting"
                      @click="handleSingleBind(user.id)"
                    >
                      绑定
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      class="h-8 w-full px-3 text-xs"
                      :disabled="!selectedGroup || selectedGroup.is_default || user.group_id !== selectedGroup.id || batchSubmitting"
                      @click="handleSingleUnbind(user.id)"
                    >
                      移回默认
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <Pagination
            :current="currentPage"
            :total="filteredUsers.length"
            :page-size="pageSize"
            cache-key="user-groups-users-page-size"
            @update:current="currentPage = $event"
            @update:page-size="pageSize = $event"
          />
        </Card>
      </div>
    </div>

    <UserGroupFormDialog
      ref="userGroupFormDialogRef"
      :open="showUserGroupFormDialog"
      :group="editingUserGroup"
      @close="closeUserGroupFormDialog"
      @submit="handleUserGroupFormSubmit"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import {
  Braces,
  Gauge,
  Layers3,
  Link2,
  Link2Off,
  Plus,
  Search,
  Shield,
  SquarePen,
  Trash2,
  Users as UsersIcon,
} from 'lucide-vue-next'
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Input,
  Pagination,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui'
import { useUsersStore } from '@/stores/users'
import {
  usersApi,
  type BatchUserGroupBindingRequest,
  type UserGroup,
  type UserGroupSchedulingMode,
} from '@/api/users'
import UserGroupFormDialog, {
  type UserGroupFormData,
} from '@/features/users/components/UserGroupFormDialog.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatRateLimitInheritable } from '@/utils/format'

type MembershipFilter = 'all' | 'current' | 'default' | 'other'

const usersStore = useUsersStore()
const { success, error } = useToast()
const { confirmDanger, confirmWarning } = useConfirm()

const userGroups = ref<UserGroup[]>([])
const selectedGroupId = ref<string | null>(null)
const showUserGroupFormDialog = ref(false)
const editingUserGroup = ref<UserGroupFormData | null>(null)
const userGroupFormDialogRef = ref<InstanceType<typeof UserGroupFormDialog>>()

const groupSearchQuery = ref('')
const userSearchQuery = ref('')
const membershipFilter = ref<MembershipFilter>('all')
const batchSubmitting = ref(false)

const selectedUserIds = ref<string[]>([])
const currentPage = ref(1)
const pageSize = ref(20)

const filteredGroups = computed(() => {
  const keyword = groupSearchQuery.value.trim().toLowerCase()
  if (!keyword) return userGroups.value
  return userGroups.value.filter((group) => {
    const searchable = `${group.name} ${group.description || ''}`.toLowerCase()
    return searchable.includes(keyword)
  })
})

const defaultGroup = computed(() => {
  return userGroups.value.find((group) => group.is_default) || userGroups.value[0] || null
})

const selectedGroup = computed(() => {
  return userGroups.value.find((group) => group.id === selectedGroupId.value) || null
})

const filteredUsers = computed(() => {
  let filtered = [...usersStore.users]

  const keyword = userSearchQuery.value.trim().toLowerCase()
  if (keyword) {
    filtered = filtered.filter((user) => {
      const searchable = `${user.username} ${user.email || ''} ${user.group_name || ''}`.toLowerCase()
      return searchable.includes(keyword)
    })
  }

  if (membershipFilter.value === 'current' && selectedGroup.value) {
    filtered = filtered.filter((user) => user.group_id === selectedGroup.value?.id)
  } else if (membershipFilter.value === 'default' && defaultGroup.value) {
    filtered = filtered.filter((user) => user.group_id === defaultGroup.value?.id)
  } else if (membershipFilter.value === 'other' && selectedGroup.value) {
    filtered = filtered.filter((user) => user.group_id && user.group_id !== selectedGroup.value?.id)
  }

  filtered.sort((a, b) => {
    const aInCurrentGroup = selectedGroup.value && a.group_id === selectedGroup.value.id ? 1 : 0
    const bInCurrentGroup = selectedGroup.value && b.group_id === selectedGroup.value.id ? 1 : 0
    if (aInCurrentGroup !== bInCurrentGroup) {
      return bInCurrentGroup - aInCurrentGroup
    }
    return a.username.localeCompare(b.username, 'zh-CN')
  })

  return filtered
})

const paginatedUsers = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredUsers.value.slice(start, start + pageSize.value)
})

const selectedUserIdSet = computed(() => new Set(selectedUserIds.value))

const allPageSelected = computed(() => {
  return paginatedUsers.value.length > 0 && paginatedUsers.value.every((user) => selectedUserIdSet.value.has(user.id))
})

const selectedCurrentGroupUserCount = computed(() => {
  if (!selectedGroup.value) return 0
  return selectedUserIds.value.filter((userId) => {
    const user = usersStore.users.find((item) => item.id === userId)
    return user?.group_id === selectedGroup.value?.id
  }).length
})

function formatModelGroupBindingsSummary(bindings: UserGroup['model_group_bindings'] | null | undefined): string {
  if (!bindings || bindings.length === 0) return '默认模型分组'
  const activeBindings = bindings.filter((binding) => binding.is_active)
  if (activeBindings.length === 0) return '全部停用'
  if (activeBindings.length === 1) {
    const first = activeBindings[0]
    return first.model_group_display_name || first.model_group_name || first.model_group_id
  }
  return `${activeBindings.length} 个模型分组`
}

function formatRestrictionSummary(
  list: string[] | null | undefined,
  unitLabel: string,
): string {
  if (list == null) return '不限制'
  if (list.length === 0) return '未开放'
  if (list.length <= 2) return list.join('、')
  return `${list.length} ${unitLabel}`
}

function formatUserGroupName(groupName: string | null | undefined): string {
  return groupName || defaultGroup.value?.name || '默认分组'
}

function formatSchedulingMode(mode: UserGroupSchedulingMode | string | null | undefined): string {
  switch (mode) {
    case 'fixed_order':
      return '固定顺序'
    case 'load_balance':
      return '负载均衡'
    case 'cache_affinity':
    default:
      return '缓存亲和'
  }
}

function toggleUserSelection(userId: string, checked: boolean) {
  if (checked) {
    if (!selectedUserIdSet.value.has(userId)) {
      selectedUserIds.value = [...selectedUserIds.value, userId]
    }
    return
  }
  selectedUserIds.value = selectedUserIds.value.filter((item) => item !== userId)
}

function togglePageSelection(checked: boolean) {
  const pageIds = paginatedUsers.value.map((user) => user.id)
  if (checked) {
    selectedUserIds.value = Array.from(new Set([...selectedUserIds.value, ...pageIds]))
    return
  }
  selectedUserIds.value = selectedUserIds.value.filter((id) => !pageIds.includes(id))
}

async function refreshData() {
  await Promise.all([
    loadUserGroups(),
    usersStore.fetchUsers(),
  ])
}

async function loadUserGroups() {
  const groups = await usersApi.getAllUserGroups()
  userGroups.value = groups

  if (!groups.length) {
    selectedGroupId.value = null
    return
  }

  if (!selectedGroupId.value || !groups.some((group) => group.id === selectedGroupId.value)) {
    selectedGroupId.value = groups[0].id
  }
}

function openCreateGroupDialog() {
  editingUserGroup.value = null
  showUserGroupFormDialog.value = true
}

function editUserGroup(group: UserGroup) {
  editingUserGroup.value = {
    id: group.id,
    name: group.name,
    description: group.description ?? null,
    scheduling_mode: group.scheduling_mode,
    allowed_api_formats: group.allowed_api_formats == null ? null : [...group.allowed_api_formats],
    model_group_bindings: group.model_group_bindings.map((binding) => ({
      model_group_id: binding.model_group_id,
      priority: binding.priority,
      is_active: binding.is_active,
      model_group_name: binding.model_group_name ?? null,
      model_group_display_name: binding.model_group_display_name ?? null,
      model_group_is_default: binding.model_group_is_default,
    })),
    rate_limit: group.rate_limit ?? null,
  }
  showUserGroupFormDialog.value = true
}

function closeUserGroupFormDialog() {
  showUserGroupFormDialog.value = false
  editingUserGroup.value = null
}

async function handleUserGroupFormSubmit(data: UserGroupFormData) {
  userGroupFormDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      const updated = await usersApi.updateUserGroup(data.id, {
        name: data.name,
        description: data.description ?? null,
        scheduling_mode: data.scheduling_mode,
        allowed_api_formats: data.allowed_api_formats,
        model_group_bindings: data.model_group_bindings?.map((binding) => ({
          model_group_id: binding.model_group_id,
          priority: binding.priority,
          is_active: binding.is_active,
        })) ?? [],
        rate_limit: data.rate_limit ?? null,
      })
      success('用户分组已更新')
      selectedGroupId.value = updated.id
    } else {
      const created = await usersApi.createUserGroup({
        name: data.name,
        description: data.description ?? null,
        scheduling_mode: data.scheduling_mode,
        allowed_api_formats: data.allowed_api_formats,
        model_group_bindings: data.model_group_bindings?.map((binding) => ({
          model_group_id: binding.model_group_id,
          priority: binding.priority,
          is_active: binding.is_active,
        })) ?? [],
        rate_limit: data.rate_limit ?? null,
      })
      success('用户分组创建成功')
      selectedGroupId.value = created.id
    }

    await refreshData()
    closeUserGroupFormDialog()
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), data.id ? '更新分组失败' : '创建分组失败')
  } finally {
    userGroupFormDialogRef.value?.setSaving(false)
  }
}

async function removeUserGroup(group: UserGroup) {
  if (group.is_default) {
    error('默认分组不能删除', '操作不支持')
    return
  }

  const confirmed = await confirmDanger(
    `确定要删除分组 ${group.name} 吗？`,
    '删除分组',
    '删除'
  )

  if (!confirmed) return

  try {
    await usersApi.deleteUserGroup(group.id)
    success('用户分组已删除')
    if (selectedGroupId.value === group.id) {
      selectedGroupId.value = null
    }
    await refreshData()
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), '删除分组失败')
  }
}

async function submitBatchBinding(payload: BatchUserGroupBindingRequest, successMessage: string) {
  batchSubmitting.value = true
  try {
    const result = await usersApi.batchUpdateUserGroupBinding(payload)
    await refreshData()
    selectedUserIds.value = []
    success(`${successMessage}，成功 ${result.updated_count} 人，跳过 ${result.skipped_count} 人`)
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), '批量更新分组成员失败')
  } finally {
    batchSubmitting.value = false
  }
}

async function handleBatchBind() {
  if (!selectedGroup.value || selectedUserIds.value.length === 0) return

  const confirmed = await confirmWarning(
    `确定将选中的 ${selectedUserIds.value.length} 个用户绑定到分组 ${selectedGroup.value.name} 吗？`,
    '批量绑定用户'
  )
  if (!confirmed) return

  await submitBatchBinding(
    {
      action: 'bind',
      user_ids: selectedUserIds.value,
      group_id: selectedGroup.value.id,
    },
    '已批量绑定到当前分组'
  )
}

async function handleBatchUnbind() {
  if (!selectedGroup.value || selectedGroup.value.is_default || selectedCurrentGroupUserCount.value === 0) return

  const confirmed = await confirmWarning(
    `确定将选中的 ${selectedCurrentGroupUserCount.value} 个用户从分组 ${selectedGroup.value.name} 移回默认分组吗？`,
    '批量移回默认分组'
  )
  if (!confirmed) return

  await submitBatchBinding(
    {
      action: 'unbind',
      user_ids: selectedUserIds.value,
      source_group_id: selectedGroup.value.id,
    },
    '已批量移回默认分组'
  )
}

async function handleSingleBind(userId: string) {
  if (!selectedGroup.value) return
  await submitBatchBinding(
    {
      action: 'bind',
      user_ids: [userId],
      group_id: selectedGroup.value.id,
    },
    '用户已绑定到当前分组'
  )
}

async function handleSingleUnbind(userId: string) {
  if (!selectedGroup.value || selectedGroup.value.is_default) return
  await submitBatchBinding(
    {
      action: 'unbind',
      user_ids: [userId],
      source_group_id: selectedGroup.value.id,
    },
    '用户已移回默认分组'
  )
}

watch([groupSearchQuery], () => {
  if (selectedGroup.value) return
  if (filteredGroups.value.length > 0) {
    selectedGroupId.value = filteredGroups.value[0].id
  }
})

watch([selectedGroupId, membershipFilter, userSearchQuery], () => {
  currentPage.value = 1
  selectedUserIds.value = []
})

onMounted(async () => {
  await refreshData()
})
</script>
