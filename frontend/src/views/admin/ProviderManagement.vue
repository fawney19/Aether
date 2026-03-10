<template>
  <div class="space-y-4">
    <!-- 提供商表格 -->
    <Card
      variant="default"
    >
      <!-- 标题和操作栏 -->
      <ProviderTableHeader
        :search-query="searchQuery"
        :filter-status="filterStatus"
        :filter-api-format="filterApiFormat"
        :filter-model="filterModel"
        :status-filters="statusFilters"
        :api-format-filters="apiFormatFilters"
        :model-filters="modelFilters"
        :has-active-filters="hasActiveFilters"
        :priority-mode-label="priorityModeConfig.label"
        :loading="loading"
        @update:search-query="searchQuery = $event"
        @update:filter-status="filterStatus = $event"
        @update:filter-api-format="filterApiFormat = $event"
        @update:filter-model="filterModel = $event"
        @reset-filters="resetFilters"
        @open-priority-dialog="openPriorityDialog"
        @add-provider="openAddProviderDialog"
        @refresh="loadProviders"
      />

      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="flex items-center justify-center py-12"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <!-- 空状态 -->
      <div
        v-else-if="providers.length === 0"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="text-muted-foreground mb-2">
          <template v-if="hasActiveFilters">
            未找到匹配当前筛选条件的提供商
          </template>
          <template v-else>
            暂无提供商，点击右上角添加
          </template>
        </div>
        <Button
          v-if="hasActiveFilters"
          variant="outline"
          size="sm"
          @click="resetFilters"
        >
          清除筛选
        </Button>
      </div>

      <!-- 桌面端表格 -->
      <div
        v-else
        class="hidden xl:block overflow-x-auto"
      >
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead class="w-[18%] min-w-[140px]">
                提供商信息
              </TableHead>
              <TableHead class="w-[20%] min-w-[180px]">
                余额监控
              </TableHead>
              <TableHead class="w-[12%] min-w-[100px] text-center">
                资源统计
              </TableHead>
              <TableHead class="w-[24%] min-w-[260px]">
                端点健康
              </TableHead>
              <TableHead class="w-[8%] min-w-[60px] text-center">
                状态
              </TableHead>
              <TableHead class="w-[18%] min-w-[120px] text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <ProviderTableRow
              v-for="provider in providers"
              :key="provider.id"
              :provider="provider"
              :editing-description-id="editingDescriptionId"
              :is-balance-loading="isBalanceLoading"
              :get-provider-balance="getProviderBalance"
              :get-provider-balance-breakdown="getProviderBalanceBreakdown"
              :get-provider-balance-error="getProviderBalanceError"
              :get-provider-checkin="getProviderCheckin"
              :get-provider-cookie-expired="getProviderCookieExpired"
              :get-provider-balance-extra="getProviderBalanceExtra"
              :format-balance-display="formatBalanceDisplay"
              :format-reset-countdown="formatResetCountdown"
              :get-quota-used-color-class="getQuotaUsedColorClass"
              @mousedown="handleMouseDown"
              @row-click="handleRowClick"
              @view-detail="openProviderDrawer"
              @edit-provider="openEditProviderDialog"
              @open-ops-config="openOpsConfigDialog"
              @toggle-status="toggleProviderStatus"
              @delete-provider="handleDeleteProvider"
              @start-edit-description="startEditDescription"
              @save-description="saveDescription"
              @cancel-edit-description="cancelEditDescription"
            />
          </TableBody>
        </Table>
      </div>

      <!-- 移动端卡片列表 -->
      <div
        v-if="!loading && providers.length > 0"
        class="xl:hidden divide-y divide-border/40"
      >
        <ProviderMobileCard
          v-for="provider in providers"
          :key="provider.id"
          :provider="provider"
          :editing-description-id="editingDescriptionId"
          :is-balance-loading="isBalanceLoading"
          :get-provider-balance="getProviderBalance"
          :get-provider-balance-error="getProviderBalanceError"
          :get-provider-checkin="getProviderCheckin"
          :get-provider-cookie-expired="getProviderCookieExpired"
          :format-balance-display="formatBalanceDisplay"
          :get-quota-used-color-class="getQuotaUsedColorClass"
          @view-detail="openProviderDrawer"
          @edit-provider="openEditProviderDialog"
          @open-ops-config="openOpsConfigDialog"
          @toggle-status="toggleProviderStatus"
          @delete-provider="handleDeleteProvider"
          @start-edit-description="startEditDescription"
          @save-description="saveDescription"
          @cancel-edit-description="cancelEditDescription"
        />
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && total > 0"
        :current="currentPage"
        :total="total"
        :page-size="pageSize"
        cache-key="provider-management-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>
  </div>

  <!-- 对话框 -->
  <ProviderFormDialog
    v-model="providerDialogOpen"
    :provider="providerToEdit"
    :max-priority="maxProviderPriority"
    @provider-created="handleProviderAdded"
    @provider-updated="handleProviderUpdated"
  />

  <PriorityManagementDialog
    v-model="priorityDialogOpen"
    @saved="handlePrioritySaved"
  />

  <ProviderDetailDrawer
    :open="providerDrawerOpen"
    :provider-id="selectedProviderId"
    @update:open="providerDrawerOpen = $event"
    @edit="openEditProviderDialog"
    @toggle-status="toggleProviderStatus"
    @refresh="handleDrawerRefresh"
  />

  <ProviderAuthDialog
    v-model:open="opsConfigDialogOpen"
    :provider-id="opsConfigProviderId"
    :provider-website="opsConfigProviderWebsite"
    @saved="handleOpsConfigSaved"
  />
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import Button from '@/components/ui/button.vue'
import Card from '@/components/ui/card.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import Pagination from '@/components/ui/pagination.vue'
import { ProviderFormDialog, PriorityManagementDialog, ProviderAuthDialog } from '@/features/providers/components'
import ProviderDetailDrawer from '@/features/providers/components/ProviderDetailDrawer.vue'
import ProviderTableHeader from '@/features/providers/components/ProviderTableHeader.vue'
import ProviderTableRow from '@/features/providers/components/ProviderTableRow.vue'
import ProviderMobileCard from '@/features/providers/components/ProviderMobileCard.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useRowClick } from '@/composables/useRowClick'
import { useProviderFilters } from '@/features/providers/composables/useProviderFilters'
import { useProviderBalance } from '@/features/providers/composables/useProviderBalance'
import {
  getProvidersSummary,
  getProvider,
  deleteProvider,
  updateProvider,
  getGlobalModels,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { parseApiError } from '@/utils/errorParser'

const { error: showError, success: showSuccess } = useToast()
const { confirmDanger } = useConfirm()

// 状态
const loading = ref(false)
const providers = ref<ProviderWithEndpointsSummary[]>([])
let providersRequestId = 0
const providerDialogOpen = ref(false)
const providerToEdit = ref<ProviderWithEndpointsSummary | null>(null)
const priorityDialogOpen = ref(false)
const priorityMode = ref<'provider' | 'global_key'>('provider')
const providerDrawerOpen = ref(false)
const selectedProviderId = ref<string | null>(null)

// 全局模型数据（用于模型筛选下拉）
const globalModels = ref<{ id: string; name: string }[]>([])

// Composables
const {
  searchQuery,
  filterStatus,
  filterApiFormat,
  filterModel,
  statusFilters,
  apiFormatFilters,
  modelFilters,
  hasActiveFilters,
  currentPage,
  pageSize,
  total,
  queryParams,
  resetFilters,
} = useProviderFilters(
  () => globalModels.value,
)

const {
  loadArchitectureSchemas,
  loadBalances,
  getProviderBalance,
  getProviderBalanceBreakdown,
  getProviderBalanceError,
  isBalanceLoading,
  getProviderCheckin,
  getProviderCookieExpired,
  formatBalanceDisplay,
  formatResetCountdown,
  getProviderBalanceExtra,
  getQuotaUsedColorClass,
  startTick,
  stopTick,
} = useProviderBalance()

// 扩展操作配置对话框
const opsConfigDialogOpen = ref(false)
const opsConfigProviderId = ref('')
const opsConfigProviderWebsite = ref('')

// 内联编辑备注
const editingDescriptionId = ref<string | null>(null)

function startEditDescription(_event: Event, provider: ProviderWithEndpointsSummary) {
  editingDescriptionId.value = provider.id
}

function cancelEditDescription(_event?: Event) {
  editingDescriptionId.value = null
}

async function saveDescription(_event: Event, provider: ProviderWithEndpointsSummary, newValue: string) {
  const trimmed = newValue.trim()
  const oldValue = provider.description || ''
  if (trimmed === oldValue) {
    cancelEditDescription()
    return
  }
  try {
    await updateProvider(provider.id, { description: trimmed || null })
    provider.description = trimmed || undefined
    // 同步更新 providers 数组
    const target = providers.value.find(p => p.id === provider.id)
    if (target) {
      target.description = trimmed || undefined
    }
    cancelEditDescription()
  } catch (err: unknown) {
    showError(parseApiError(err, '更新备注失败'), '错误')
  }
}

// 优先级模式配置
const priorityModeConfig = computed(() => {
  return {
    label: priorityMode.value === 'global_key' ? '全局 Key 优先' : '提供商优先',
  }
})

// 当前已有提供商的最大优先级
const maxProviderPriority = computed(() => {
  if (providers.value.length === 0) return undefined
  const priorities = providers.value
    .map(p => p.provider_priority)
    .filter(v => typeof v === 'number' && Number.isFinite(v))
  return priorities.length > 0 ? Math.max(...priorities) : undefined
})

// 加载优先级模式
async function loadPriorityMode() {
  try {
    const response = await adminApi.getSystemConfig('provider_priority_mode')
    if (response.value) {
      priorityMode.value = response.value as 'provider' | 'global_key'
    }
  } catch {
    priorityMode.value = 'provider'
  }
}

// 加载全局模型列表（用于模型筛选下拉）
async function loadGlobalModelList() {
  try {
    const response = await getGlobalModels({ is_active: true, limit: 1000 })
    globalModels.value = response.models.map(m => ({ id: m.id, name: m.name }))
  } catch {
    globalModels.value = []
  }
}

// 加载提供商列表（服务端分页）
async function loadProviders() {
  const requestId = ++providersRequestId
  loading.value = true
  try {
    const response = await getProvidersSummary(queryParams.value)
    if (requestId !== providersRequestId) return
    providers.value = response.items
    total.value = response.total
    // 异步加载配置了 ops 的 provider 的余额数据
    loadBalances(providers.value)
  } catch (err: unknown) {
    if (requestId !== providersRequestId) return
    showError(parseApiError(err, '加载提供商列表失败'), '错误')
  } finally {
    if (requestId === providersRequestId) {
      loading.value = false
    }
  }
}

// 分页/筛选/搜索变化时重新加载
let debounceTimer: ReturnType<typeof setTimeout> | null = null
watch(queryParams, (newParams, oldParams) => {
  if (debounceTimer) clearTimeout(debounceTimer)
  // 搜索输入 debounce 300ms，其他变化立即执行
  const isSearchOnly = newParams.search !== oldParams?.search &&
    newParams.page === oldParams?.page &&
    newParams.page_size === oldParams?.page_size &&
    newParams.status === oldParams?.status &&
    newParams.api_format === oldParams?.api_format &&
    newParams.model_id === oldParams?.model_id
  if (isSearchOnly) {
    debounceTimer = setTimeout(loadProviders, 300)
  } else {
    loadProviders()
  }
}, { deep: true })

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

// 处理行点击 - 只在非选中文本时打开抽屉
function handleRowClick(event: MouseEvent, providerId: string) {
  if (!shouldTriggerRowClick(event)) return
  openProviderDrawer(providerId)
}

// 打开添加提供商对话框
function openAddProviderDialog() {
  providerToEdit.value = null
  providerDialogOpen.value = true
}

// 打开优先级管理对话框
function openPriorityDialog() {
  priorityDialogOpen.value = true
}

// 打开提供商详情抽屉
function openProviderDrawer(providerId: string) {
  selectedProviderId.value = providerId
  providerDrawerOpen.value = true
}

// 打开编辑提供商对话框
function openEditProviderDialog(provider: ProviderWithEndpointsSummary) {
  providerToEdit.value = provider
  providerDialogOpen.value = true
}

// 打开扩展操作配置对话框
function openOpsConfigDialog(provider: ProviderWithEndpointsSummary) {
  opsConfigProviderId.value = provider.id
  opsConfigProviderWebsite.value = provider.website || ''
  opsConfigDialogOpen.value = true
}

// 扩展操作配置保存回调
function handleOpsConfigSaved() {
  opsConfigDialogOpen.value = false
  loadProviders()
}

// 处理提供商编辑完成
function handleProviderUpdated(updated: ProviderWithEndpointsSummary) {
  const index = providers.value.findIndex(p => p.id === updated.id)
  if (index !== -1) {
    providers.value[index] = updated
    // 刷新该提供商的余额数据
    loadBalances([updated], false)
  }
}

// 处理详情抽屉内的刷新：只刷新当前查看的那一条提供商
async function handleDrawerRefresh() {
  if (!selectedProviderId.value) return
  try {
    const updated = await getProvider(selectedProviderId.value)
    const index = providers.value.findIndex(p => p.id === updated.id)
    if (index !== -1) {
      providers.value[index] = updated
      loadBalances([updated], false)
    }
  } catch (err) {
    showError(parseApiError(err, '刷新提供商数据失败'), '错误')
  }
}

// 优先级保存成功回调
async function handlePrioritySaved() {
  await loadProviders()
  await loadPriorityMode()
}

// 处理提供商添加
function handleProviderAdded() {
  loadProviders()
}

// 删除提供商
async function handleDeleteProvider(provider: ProviderWithEndpointsSummary) {
  const confirmed = await confirmDanger(
    '删除提供商',
    `确定要删除提供商 "${provider.name}" 吗？\n\n这将同时删除其所有端点、密钥和配置。此操作不可恢复！`,
  )

  if (!confirmed) return

  try {
    await deleteProvider(provider.id)
    showSuccess('提供商已删除')
    loadProviders()
  } catch (err: unknown) {
    showError(parseApiError(err, '删除提供商失败'), '错误')
  }
}

// 切换提供商状态
async function toggleProviderStatus(provider: ProviderWithEndpointsSummary) {
  try {
    const newStatus = !provider.is_active
    await updateProvider(provider.id, { is_active: newStatus })

    // 更新抽屉内部的 provider 对象
    provider.is_active = newStatus

    // 同时更新主页面 providers 数组中的对象，实现无感更新
    const targetProvider = providers.value.find(p => p.id === provider.id)
    if (targetProvider) {
      targetProvider.is_active = newStatus
    }

    showSuccess(newStatus ? '提供商已启用' : '提供商已停用')
  } catch (err: unknown) {
    showError(parseApiError(err, '操作失败'), '错误')
  }
}

// 点击外部自动取消编辑备注
function handleGlobalClick(event: MouseEvent) {
  if (!editingDescriptionId.value) return
  const target = event.target as HTMLElement
  if (target.closest('[data-desc-editor]')) return
  cancelEditDescription()
}

onMounted(() => {
  loadProviders()
  loadPriorityMode()
  loadGlobalModelList()
  loadArchitectureSchemas()
  document.addEventListener('click', handleGlobalClick, true)
  // 每秒更新一次倒计时
  startTick()
})

onUnmounted(() => {
  if (debounceTimer) clearTimeout(debounceTimer)
  document.removeEventListener('click', handleGlobalClick, true)
  stopTick()
})
</script>
