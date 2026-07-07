<template>
  <div class="space-y-4">
    <ProviderDeleteProgressCard
      :progress="providerDeleteProgress"
      :stage-label="providerDeleteStageLabel"
      :total-units="providerDeleteTotalUnits"
      :completed-units="providerDeleteCompletedUnits"
      :overall-percent="providerDeleteOverallPercent"
      :keys-percent="providerDeleteKeysPercent"
      :endpoints-percent="providerDeleteEndpointsPercent"
    />

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
        @batch-process="openProviderBatchDialog"
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
        class="contents"
      >
        <ProviderEmptyState
          :has-active-filters="hasActiveFilters"
          @reset-filters="resetFilters"
        />
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
                {{ legacyT('提供商信息') }}
              </TableHead>
              <TableHead class="w-[20%] min-w-[180px]">
                {{ legacyT('余额监控') }}
              </TableHead>
              <SortableTableHead
                class="w-[12%] min-w-[100px] text-center"
                column-key="model"
                :sortable="false"
                align="center"
                :filter-active="filterModel !== 'all'"
                :filter-title="legacyT('筛选模型')"
                filter-content-class="w-64 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
              >
                {{ legacyT('资源统计') }}
                <template #filter="{ close }">
                  <TableFilterMenu
                    v-model="filterModel"
                    :options="modelFilters"
                    @select="close"
                  />
                </template>
              </SortableTableHead>
              <SortableTableHead
                class="w-[24%] min-w-[260px]"
                column-key="api_format"
                :sortable="false"
                :filter-active="filterApiFormat !== 'all'"
                :filter-title="legacyT('筛选 API 格式')"
                filter-content-class="w-72 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
              >
                {{ legacyT('端点健康') }}
                <template #filter="{ close }">
                  <TableFilterMenu
                    v-model="filterApiFormat"
                    :options="apiFormatFilters"
                    @select="close"
                  />
                </template>
              </SortableTableHead>
              <SortableTableHead
                class="w-[8%] min-w-[60px] text-center"
                column-key="status"
                :sortable="false"
                align="center"
                :filter-active="filterStatus !== 'all'"
                :filter-title="legacyT('筛选状态')"
                filter-content-class="w-40 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
              >
                {{ legacyT('状态') }}
                <template #filter="{ close }">
                  <TableFilterMenu
                    v-model="filterStatus"
                    :options="statusFilters"
                    @select="close"
                  />
                </template>
              </SortableTableHead>
              <TableHead class="w-[18%] min-w-[160px] text-center">
                {{ legacyT('操作') }}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <ProviderTableRow
              v-for="provider in displayedProviders"
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
          v-for="provider in displayedProviders"
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

  <ProviderBatchActionDialog
    v-model="providerBatchDialogOpen"
    :providers="displayedProviders"
    @changed="handleProviderBatchChanged"
  />

  <PriorityManagementDialog
    v-model="priorityDialogOpen"
    @saved="handlePrioritySaved"
  />

  <ProviderDetailDrawer
    :open="providerDrawerOpen"
    :provider-id="selectedProviderId"
    :initial-provider="selectedProvider"
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
import Card from '@/components/ui/card.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import SortableTableHead from '@/components/ui/sortable-table-head.vue'
import TableFilterMenu from '@/components/ui/table-filter-menu.vue'
import Pagination from '@/components/ui/pagination.vue'
import { ProviderFormDialog, PriorityManagementDialog, ProviderAuthDialog } from '@/features/providers/components'
import ProviderBatchActionDialog from '@/features/providers/components/ProviderBatchActionDialog.vue'
import ProviderDetailDrawer from '@/features/providers/components/ProviderDetailDrawer.vue'
import ProviderTableHeader from '@/features/providers/components/ProviderTableHeader.vue'
import ProviderTableRow from '@/features/providers/components/ProviderTableRow.vue'
import ProviderMobileCard from '@/features/providers/components/ProviderMobileCard.vue'
import ProviderDeleteProgressCard from '@/features/providers/components/ProviderDeleteProgressCard.vue'
import ProviderEmptyState from '@/features/providers/components/ProviderEmptyState.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useRowClick } from '@/composables/useRowClick'
import { useProviderFilters } from '@/features/providers/composables/useProviderFilters'
import { useProviderBalance } from '@/features/providers/composables/useProviderBalance'
import {
  getProvidersSummary,
  getProvider,
  deleteProvider,
  getProviderDeleteTask,
  updateProvider,
  getGlobalModels,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { parseApiError } from '@/utils/errorParser'
import { useI18n } from '@/i18n'

interface ProviderDeleteProgressState {
  providerId: string
  providerName: string
  taskId: string
  status: string
  stage: string
  totalKeys: number
  deletedKeys: number
  totalEndpoints: number
  deletedEndpoints: number
  message: string
}

const { error: showError, success: showSuccess, info: showInfo } = useToast()
const { confirmDanger } = useConfirm()
const { legacyT } = useI18n()

function showLegacyError(err: unknown, fallback: string, title = '错误') {
  showError(legacyT(parseApiError(err, fallback)), legacyT(title))
}

// 状态
const loading = ref(false)
const providers = ref<ProviderWithEndpointsSummary[]>([])
let providersRequestId = 0
const providerDialogOpen = ref(false)
const providerBatchDialogOpen = ref(false)
const providerToEdit = ref<ProviderWithEndpointsSummary | null>(null)
const priorityDialogOpen = ref(false)
const priorityMode = ref<'provider' | 'global_key'>('provider')
const providerDrawerOpen = ref(false)
const selectedProviderId = ref<string | null>(null)
const selectedProvider = computed<ProviderWithEndpointsSummary | null>(() => {
  if (!selectedProviderId.value) return null
  return providers.value.find(provider => provider.id === selectedProviderId.value) ?? null
})
const providerDeleteProgress = ref<ProviderDeleteProgressState | null>(null)
let deletePollAbort: AbortController | null = null

const DELETE_POLL_INTERVAL_MS = 2000
const DELETE_POLL_MAX_MS = 30 * 60 * 1000
const DELETE_POLL_MAX_FAILURES = 3
const PROVIDER_SUMMARY_CACHE_TTL_MS = 10 * 1000
const PROVIDER_PRIORITY_MODE_CACHE_TTL_MS = 30 * 1000
const PROVIDER_MODEL_FILTER_CACHE_TTL_MS = 10 * 1000

async function pollProviderDeleteTask(providerId: string, taskId: string) {
  deletePollAbort?.abort()
  const abort = new AbortController()
  deletePollAbort = abort

  const deadline = Date.now() + DELETE_POLL_MAX_MS
  let consecutiveFailures = 0

  while (Date.now() < deadline) {
    if (abort.signal.aborted) return null
    try {
      const task = await getProviderDeleteTask(providerId, taskId)
      consecutiveFailures = 0
      if (providerDeleteProgress.value?.taskId === taskId) {
        providerDeleteProgress.value = {
          ...providerDeleteProgress.value,
          status: task.status,
          stage: task.stage,
          totalKeys: task.total_keys,
          deletedKeys: task.deleted_keys,
          totalEndpoints: task.total_endpoints,
          deletedEndpoints: task.deleted_endpoints,
          message: task.message,
        }
      }
      if (task.status === 'completed' || task.status === 'failed') {
        return task
      }
    } catch {
      consecutiveFailures += 1
      if (consecutiveFailures >= DELETE_POLL_MAX_FAILURES) {
        throw new Error('provider delete task polling failed')
      }
    }
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, DELETE_POLL_INTERVAL_MS)
      abort.signal.addEventListener('abort', () => { clearTimeout(timer); resolve(undefined) }, { once: true })
    })
  }

  throw new Error('provider delete task timeout')
}

const providerDeleteStageLabel = computed(() => {
  switch (providerDeleteProgress.value?.stage) {
    case 'preparing':
      return legacyT('准备删除')
    case 'disabling':
      return legacyT('停用提供商')
    case 'cleaning_restrictions':
      return legacyT('清理访问限制')
    case 'cleaning_provider_refs':
      return legacyT('清理历史引用')
    case 'deleting_keys':
      return legacyT('删除号池账号')
    case 'deleting_endpoints':
      return legacyT('删除端点')
    case 'completed':
      return legacyT('删除完成')
    case 'failed':
      return legacyT('删除失败')
    default:
      return legacyT('等待执行')
  }
})

const providerDeleteTotalUnits = computed(() => {
  const progress = providerDeleteProgress.value
  if (!progress) return 0
  return progress.totalKeys + progress.totalEndpoints
})

const providerDeleteCompletedUnits = computed(() => {
  const progress = providerDeleteProgress.value
  if (!progress) return 0
  return Math.min(progress.deletedKeys + progress.deletedEndpoints, providerDeleteTotalUnits.value)
})

const providerDeleteOverallPercent = computed(() => {
  const progress = providerDeleteProgress.value
  if (!progress) return 0
  if (progress.status === 'completed') return 100
  if (providerDeleteTotalUnits.value <= 0) return 0
  return Math.min(
    100,
    Math.round((providerDeleteCompletedUnits.value / providerDeleteTotalUnits.value) * 100),
  )
})

const providerDeleteKeysPercent = computed(() => {
  const progress = providerDeleteProgress.value
  if (!progress?.totalKeys) return 0
  return Math.min(100, Math.round((progress.deletedKeys / progress.totalKeys) * 100))
})

const providerDeleteEndpointsPercent = computed(() => {
  const progress = providerDeleteProgress.value
  if (!progress?.totalEndpoints) return 0
  return Math.min(100, Math.round((progress.deletedEndpoints / progress.totalEndpoints) * 100))
})

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

function sortProvidersByActiveAndPriority(items: ProviderWithEndpointsSummary[]) {
  return [...items].sort((a, b) => {
    if (a.is_active !== b.is_active) {
      return a.is_active ? -1 : 1
    }
    if (a.provider_priority !== b.provider_priority) {
      return a.provider_priority - b.provider_priority
    }
    return new Date(a.created_at).getTime() - new Date(b.created_at).getTime()
  })
}

const displayedProviders = computed(() => sortProvidersByActiveAndPriority(providers.value))

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
    showLegacyError(err, '更新备注失败')
  }
}

// 优先级模式配置
const priorityModeConfig = computed(() => {
  return {
    label: legacyT(priorityMode.value === 'global_key' ? '全局 Key 优先' : '提供商优先'),
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
async function loadPriorityMode(options: { cacheTtlMs?: number } = {}) {
  try {
    const response = await adminApi.getSystemConfig('provider_priority_mode', {
      cacheTtlMs: options.cacheTtlMs ?? 0,
    })
    if (response.value) {
      priorityMode.value = response.value as 'provider' | 'global_key'
    }
  } catch {
    priorityMode.value = 'provider'
  }
}

// 加载全局模型列表（用于模型筛选下拉）
async function loadGlobalModelList(options: { cacheTtlMs?: number } = {}) {
  try {
    const response = await getGlobalModels(
      { is_active: true, limit: 1000 },
      { cacheTtlMs: options.cacheTtlMs ?? 0 },
    )
    globalModels.value = response.models.map(m => ({ id: m.id, name: m.name }))
  } catch {
    globalModels.value = []
  }
}

// 加载提供商列表（服务端分页）
async function loadProviders(options: { cacheTtlMs?: number } = {}) {
  const requestId = ++providersRequestId
  loading.value = true
  try {
    const response = await getProvidersSummary(queryParams.value, {
      cacheTtlMs: options.cacheTtlMs ?? 0,
    })
    if (requestId !== providersRequestId) return
    providers.value = response.items
    total.value = response.total
    // 异步加载配置了 ops 的 provider 的余额数据
    loadBalances(providers.value)
  } catch (err: unknown) {
    if (requestId !== providersRequestId) return
    showLegacyError(err, '加载提供商列表失败')
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
    debounceTimer = setTimeout(() => {
      void loadProviders({ cacheTtlMs: PROVIDER_SUMMARY_CACHE_TTL_MS })
    }, 300)
  } else {
    void loadProviders({ cacheTtlMs: PROVIDER_SUMMARY_CACHE_TTL_MS })
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

function openProviderBatchDialog() {
  providerBatchDialogOpen.value = true
}

async function handleProviderBatchChanged() {
  await loadProviders()
}

// 打开提供商详情抽屉
function openProviderDrawer(providerId: string) {
  selectedProviderId.value = providerId
  providerDrawerOpen.value = true
}

function mergeUpdatedProvider(updated: ProviderWithEndpointsSummary) {
  const index = providers.value.findIndex(p => p.id === updated.id)
  if (index !== -1) {
    providers.value[index] = updated
    loadBalances([updated], false)
  }
}

async function refreshProviderSnapshot(
  providerId: string,
  fallbackErrorMessage = '刷新提供商数据失败',
): Promise<ProviderWithEndpointsSummary | null> {
  try {
    const updated = await getProvider(providerId)
    mergeUpdatedProvider(updated)
    return updated
  } catch (err) {
    showLegacyError(err, fallbackErrorMessage)
    return null
  }
}

// 打开编辑提供商对话框
async function openEditProviderDialog(provider: ProviderWithEndpointsSummary) {
  const latest = await refreshProviderSnapshot(provider.id, '刷新提供商状态失败')
  providerToEdit.value = latest ?? provider
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
  void loadProviders()
}

// 处理提供商编辑完成
function handleProviderUpdated(updated: ProviderWithEndpointsSummary) {
  mergeUpdatedProvider(updated)
}

// 处理详情抽屉内的刷新：只刷新当前查看的那一条提供商
async function handleDrawerRefresh() {
  if (!selectedProviderId.value) return
  await refreshProviderSnapshot(selectedProviderId.value)
}

// 优先级保存成功回调
async function handlePrioritySaved() {
  await loadProviders()
  await loadPriorityMode()
}

// 处理提供商添加
function handleProviderAdded() {
  void loadProviders()
}

// 删除提供商
async function handleDeleteProvider(provider: ProviderWithEndpointsSummary) {
  const confirmed = await confirmDanger(
    legacyT('删除提供商'),
    legacyT(`确定要删除提供商 "${provider.name}" 吗？\n\n这将同时删除其所有端点、密钥和配置。此操作不可恢复！`),
  )

  if (!confirmed) return

  try {
    const result = await deleteProvider(provider.id)
    providerDeleteProgress.value = {
      providerId: provider.id,
      providerName: provider.name,
      taskId: result.task_id,
      status: result.status,
      stage: 'queued',
      totalKeys: provider.total_keys || 0,
      deletedKeys: 0,
      totalEndpoints: provider.total_endpoints || 0,
      deletedEndpoints: 0,
      message: result.message || '删除任务已提交，后台处理中',
    }
    showInfo(legacyT(result.message || '删除任务已提交，后台处理中'))

    const task = await pollProviderDeleteTask(provider.id, result.task_id)
    if (!task) return // aborted
    if (task.status === 'failed') {
      throw new Error(task.message || 'provider delete task failed')
    }

    showSuccess(legacyT('提供商已删除'))
    providerDeleteProgress.value = null
    void loadProviders()
  } catch (err: unknown) {
    providerDeleteProgress.value = null
    showLegacyError(err, '删除提供商失败')
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

    showSuccess(legacyT(newStatus ? '提供商已启用' : '提供商已停用'))
  } catch (err: unknown) {
    showLegacyError(err, '操作失败')
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
  void loadProviders({ cacheTtlMs: PROVIDER_SUMMARY_CACHE_TTL_MS })
  void loadPriorityMode({ cacheTtlMs: PROVIDER_PRIORITY_MODE_CACHE_TTL_MS })
  void loadGlobalModelList({ cacheTtlMs: PROVIDER_MODEL_FILTER_CACHE_TTL_MS })
  void loadArchitectureSchemas()
  document.addEventListener('click', handleGlobalClick, true)
  // 每秒更新一次倒计时
  startTick()
})

onUnmounted(() => {
  deletePollAbort?.abort()
  if (debounceTimer) clearTimeout(debounceTimer)
  document.removeEventListener('click', handleGlobalClick, true)
  stopTick()
})
</script>
