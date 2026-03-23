<template>
  <div class="space-y-6 pb-8">
    <!-- 跳转数据分析链接 -->
    <div class="text-right">
      <RouterLink
        :to="isAdminPage ? '/admin/analytics' : '/dashboard/reports'"
        class="text-xs text-muted-foreground hover:text-foreground transition-colors"
      >
        {{ isAdminPage ? '查看数据分析 →' : '查看综合报表 →' }}
      </RouterLink>
    </div>

    <!-- 使用记录 -->
    <UsageRecordsTable
      :records="displayRecords"
      :is-admin="isAdminPage"
      :show-actual-cost="authStore.isAdmin"
      :loading="isLoadingRecords"
      :time-range="timeRange"
      :filter-search="filterSearch"
      :filter-user="filterUser"
      :filter-api-key="filterApiKey"
      :filter-model="filterModel"
      :filter-provider="filterProvider"
      :filter-api-format="filterApiFormat"
      :filter-status="filterStatus"
      :available-users="availableUsers"
      :available-api-keys="availableApiKeys"
      :available-models="availableModels"
      :available-api-formats="availableApiFormats"
      :available-providers="availableProviders"
      :available-statuses="availableStatuses"
      :current-page="currentPage"
      :page-size="pageSize"
      :total-records="effectiveTotalRecords"
      :page-size-options="pageSizeOptions"
      :auto-refresh="globalAutoRefresh"
      @update:time-range="handleTimeRangeChange"
      @update:filter-search="handleFilterSearchChange"
      @update:filter-user="handleFilterUserChange"
      @update:filter-api-key="handleFilterApiKeyChange"
      @update:filter-model="handleFilterModelChange"
      @update:filter-provider="handleFilterProviderChange"
      @update:filter-api-format="handleFilterApiFormatChange"
      @update:filter-status="handleFilterStatusChange"
      @update:current-page="handlePageChange"
      @update:page-size="handlePageSizeChange"
      @update:auto-refresh="handleAutoRefreshChange"
      @refresh="refreshData"
      @show-detail="showRequestDetail"
    />

    <!-- 请求详情抽屉 - 仅管理员可见 -->
    <RequestDetailDrawer
      v-if="isAdminPage"
      :is-open="detailModalOpen"
      :request-id="selectedRequestId"
      @close="detailModalOpen = false"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute } from 'vue-router'
// useLocalStorage no longer needed (analytics panel removed)
import { useAuthStore } from '@/stores/auth'
import { analyticsApi, type AnalyticsFilterOption } from '@/api/analytics'
import {
  UsageRecordsTable,
  RequestDetailDrawer,
} from '@/features/usage/components'
import {
  useUsageData,
  getDateRangeFromPeriod
} from '@/features/usage/composables'
import type { DateRangeParams, FilterStatusValue } from '@/features/usage/types'
import { log } from '@/utils/logger'
// ActivityHeatmap moved to analytics page
import { useToast } from '@/composables/useToast'

const route = useRoute()
const { warning } = useToast()
const authStore = useAuthStore()

// 判断是否是管理员页面
const isAdminPage = computed(() => route.path.startsWith('/admin'))

// 时间范围选择
const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('today'))

// 分页状态
const currentPage = ref(1)
const pageSize = ref(20)
const pageSizeOptions = [10, 20, 50, 100]

// 筛选状态
const filterSearch = ref('')
const filterUser = ref('__all__')
const filterApiKey = ref('__all__')
const filterModel = ref('__all__')
const filterProvider = ref('__all__')
const filterApiFormat = ref('__all__')
const filterStatus = ref<FilterStatusValue>('__all__')

// 使用 composables
const {
  isLoadingRecords,
  currentRecords,
  totalRecords,
  availableUsers,
  availableApiKeys,
  availableModels,
  availableApiFormats,
  availableProviders,
  availableStatuses,
  loadStats,
  loadRecords
} = useUsageData({ isAdminPage })

// 热力图已移至数据分析页

// 获取活跃请求的 ID 列表
const activeRequestIds = computed(() => {
  return currentRecords.value
    .filter(record => record.status === 'pending' || record.status === 'streaming')
    .map(record => record.id)
})

// 检查是否有活跃请求
const hasActiveRequests = computed(() => activeRequestIds.value.length > 0)

// 自动刷新定时器
let autoRefreshTimer: ReturnType<typeof setTimeout> | null = null
let globalAutoRefreshTimer: ReturnType<typeof setInterval> | null = null
let refreshInFlight: Promise<void> | null = null
const AUTO_REFRESH_INTERVAL = 1000 // 1秒刷新一次（用于活跃请求）
const GLOBAL_AUTO_REFRESH_INTERVAL = 3000 // 3秒刷新一次（全局自动刷新）
const globalAutoRefresh = ref(false) // 全局自动刷新开关（默认关闭）
const isPageVisible = ref(typeof document === 'undefined' ? true : !document.hidden)

// 轮询活跃请求状态（轻量级，只更新状态变化的记录）

let pollInFlight = false
async function pollActiveRequests() {
  if (!isPageVisible.value) return
  if (!hasActiveRequests.value) return
  if (pollInFlight) return
  pollInFlight = true

  try {
    const { requests } = await analyticsApi.getActiveRequests({
      scope: isAdminPage.value ? { kind: 'global' } : { kind: 'me' },
      ids: activeRequestIds.value,
    })

    let shouldRefresh = false

    const recordMap = new Map(currentRecords.value.map(record => [record.id, record]))

    for (const update of requests) {
      const record = recordMap.get(update.id)
      if (!record) {
        // 后端返回了未知的活跃请求，触发刷新以获取完整数据
        shouldRefresh = true
        continue
      }

      // 状态只允许单向推进，避免异步响应回退（pending -> streaming -> completed/failed/cancelled）
      const statusPriority: Record<string, number> = {
        pending: 0,
        streaming: 1,
        completed: 2,
        failed: 2,
        cancelled: 2
      }
      const currentRank = record.status ? (statusPriority[record.status] ?? 0) : 0
      const newRank = update.status ? (statusPriority[update.status] ?? 0) : 0
      const shouldApply = newRank >= currentRank

      if (shouldApply && record.status !== update.status) {
        record.status = update.status
      }
      if (shouldApply && ['completed', 'failed', 'cancelled'].includes(update.status)) {
        shouldRefresh = true
      }

      if (shouldApply) {
        // 进行中状态也需要持续更新（provider/key/TTFB 可能在 streaming 后才落库）
        record.input_tokens = update.input_tokens
        record.output_tokens = update.output_tokens
        record.cache_creation_input_tokens = update.cache_creation_input_tokens ?? undefined
        record.cache_read_input_tokens = update.cache_read_input_tokens ?? undefined
        record.cost = update.cost
        record.actual_cost = update.actual_cost ?? undefined
        record.rate_multiplier = update.rate_multiplier ?? undefined
        record.response_time_ms = update.response_time_ms ?? undefined
        record.first_byte_time_ms = update.first_byte_time_ms ?? undefined
        // API 格式/格式转换：streaming 时已可确定，轮询时同步更新
        if (update.api_format != null) record.api_format = update.api_format
        if (update.endpoint_api_format != null) record.endpoint_api_format = update.endpoint_api_format
        if (update.has_format_conversion != null) record.has_format_conversion = update.has_format_conversion
        // 模型映射：streaming 时已可确定
        if ('target_model' in update && (typeof update.target_model === 'string' || update.target_model === null)) {
          record.target_model = update.target_model
        }
        // 管理员接口返回额外字段
        // 只有当返回的 provider 不是 pending/unknown 时才更新，避免覆盖已有的正确值
        if ('provider' in update && typeof update.provider === 'string' &&
            update.provider !== 'pending' && update.provider !== 'unknown') {
          record.provider = update.provider
        }
        if ('api_key_name' in update) {
          record.api_key_name = typeof update.api_key_name === 'string' ? update.api_key_name : undefined
        }
      }
    }

    if (shouldRefresh) {
      await refreshData()
    }
  } catch (error) {
    log.error('轮询活跃请求状态失败:', error)
  } finally {
    pollInFlight = false
  }
}

function scheduleNextAutoRefresh() {
  if (autoRefreshTimer) return
  if (!isPageVisible.value || !hasActiveRequests.value) return
  autoRefreshTimer = setTimeout(async () => {
    autoRefreshTimer = null
    await pollActiveRequests()
    scheduleNextAutoRefresh()
  }, AUTO_REFRESH_INTERVAL)
}

// 启动自动刷新
function startAutoRefresh() {
  if (!isPageVisible.value) return
  scheduleNextAutoRefresh()
}

// 停止自动刷新
function stopAutoRefresh() {
  if (autoRefreshTimer) {
    clearTimeout(autoRefreshTimer)
    autoRefreshTimer = null
  }
}

// 监听活跃请求状态，自动启动/停止刷新
// 1秒轮询始终用于活跃请求的实时更新，不受全局刷新影响
watch(hasActiveRequests, (hasActive) => {
  if (hasActive && isPageVisible.value) {
    startAutoRefresh()
  } else {
    stopAutoRefresh()
  }
}, { immediate: true })

// 启动全局自动刷新
function startGlobalAutoRefresh() {
  if (!isPageVisible.value) return
  if (globalAutoRefreshTimer) return
  globalAutoRefreshTimer = setInterval(refreshData, GLOBAL_AUTO_REFRESH_INTERVAL)
}

// 停止全局自动刷新
function stopGlobalAutoRefresh() {
  if (globalAutoRefreshTimer) {
    clearInterval(globalAutoRefreshTimer)
    globalAutoRefreshTimer = null
  }
}

// 处理自动刷新开关变化
function handleAutoRefreshChange(value: boolean) {
  globalAutoRefresh.value = value
  if (value) {
    if (isPageVisible.value) {
      refreshData() // 立即刷新一次
    }
    startGlobalAutoRefresh()
  } else {
    stopGlobalAutoRefresh()
  }
}

function handleVisibilityChange() {
  isPageVisible.value = !document.hidden
  if (!isPageVisible.value) {
    stopAutoRefresh()
    stopGlobalAutoRefresh()
    return
  }
  if (hasActiveRequests.value) {
    startAutoRefresh()
  }
  if (globalAutoRefresh.value) {
    refreshData()
    startGlobalAutoRefresh()
  }
}

// 组件卸载时清理定时器
onUnmounted(() => {
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  stopAutoRefresh()
  stopGlobalAutoRefresh()
})

const paginatedRecords = computed(() => currentRecords.value)
const effectiveTotalRecords = computed(() => totalRecords.value)

// 显示的记录
const displayRecords = computed(() => paginatedRecords.value)


// 详情弹窗状态
const detailModalOpen = ref(false)
const selectedRequestId = ref<string | null>(null)

function hasOption(options: AnalyticsFilterOption[], value: string) {
  return options.some(option => option.value === value)
}

function normalizeFilterSelections() {
  let changed = false

  if (filterUser.value !== '__all__' && !hasOption(availableUsers.value, filterUser.value)) {
    filterUser.value = '__all__'
    changed = true
  }
  if (filterApiKey.value !== '__all__' && !hasOption(availableApiKeys.value, filterApiKey.value)) {
    filterApiKey.value = '__all__'
    changed = true
  }
  if (filterModel.value !== '__all__' && !hasOption(availableModels.value, filterModel.value)) {
    filterModel.value = '__all__'
    changed = true
  }
  if (filterProvider.value !== '__all__' && !hasOption(availableProviders.value, filterProvider.value)) {
    filterProvider.value = '__all__'
    changed = true
  }
  if (filterApiFormat.value !== '__all__' && !hasOption(availableApiFormats.value, filterApiFormat.value)) {
    filterApiFormat.value = '__all__'
    changed = true
  }
  if (filterStatus.value !== '__all__' && !hasOption(availableStatuses.value, filterStatus.value)) {
    filterStatus.value = '__all__'
    changed = true
  }

  return changed
}

async function syncFilterOptions() {
  await loadStats(timeRange.value, getCurrentFilters())

  if (!normalizeFilterSelections()) {
    return
  }

  await loadStats(timeRange.value, getCurrentFilters())
}

// 初始化加载
onMounted(async () => {
  document.addEventListener('visibilitychange', handleVisibilityChange)

  const statsTask = syncFilterOptions().catch(err => {
    log.error('加载统计数据失败:', err)
    warning('统计数据加载失败，请刷新重试')
  })
  await statsTask
  await loadRecords(
    { page: currentPage.value, pageSize: pageSize.value },
    getCurrentFilters()
  )

  if (globalAutoRefresh.value && isPageVisible.value) {
    startGlobalAutoRefresh()
  }
})

// 处理时间范围变化
async function handleTimeRangeChange(value: DateRangeParams) {
  timeRange.value = value
  currentPage.value = 1 // 重置到第一页
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

// 处理分页变化
async function handlePageChange(page: number) {
  currentPage.value = page
  await loadRecords({ page, pageSize: pageSize.value }, getCurrentFilters())
}

// 处理每页大小变化
async function handlePageSizeChange(size: number) {
  pageSize.value = size
  currentPage.value = 1  // 重置到第一页
  await loadRecords({ page: 1, pageSize: size }, getCurrentFilters())
}

// 获取当前筛选参数
function getCurrentFilters() {
  return {
    search: filterSearch.value.trim() || undefined,
    user_id: filterUser.value !== '__all__' ? filterUser.value : undefined,
    api_key_id: filterApiKey.value !== '__all__' ? filterApiKey.value : undefined,
    model: filterModel.value !== '__all__' ? filterModel.value : undefined,
    provider: filterProvider.value !== '__all__' ? filterProvider.value : undefined,
    api_format: filterApiFormat.value !== '__all__' ? filterApiFormat.value : undefined,
    status: filterStatus.value !== '__all__' ? filterStatus.value : undefined
  }
}

// 处理筛选变化
async function handleFilterSearchChange(value: string) {
  filterSearch.value = value
  currentPage.value = 1
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterUserChange(value: string) {
  filterUser.value = value
  currentPage.value = 1  // 重置到第一页
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterApiKeyChange(value: string) {
  filterApiKey.value = value
  currentPage.value = 1
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterModelChange(value: string) {
  filterModel.value = value
  currentPage.value = 1  // 重置到第一页
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterProviderChange(value: string) {
  filterProvider.value = value
  currentPage.value = 1
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterApiFormatChange(value: string) {
  filterApiFormat.value = value
  currentPage.value = 1
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

async function handleFilterStatusChange(value: string) {
  filterStatus.value = value as FilterStatusValue
  currentPage.value = 1
  await syncFilterOptions()
  await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
}

// 刷新数据
async function refreshData() {
  if (!isPageVisible.value) return
  if (refreshInFlight) return refreshInFlight

  refreshInFlight = (async () => {
    await syncFilterOptions()
    await loadRecords({ page: currentPage.value, pageSize: pageSize.value }, getCurrentFilters())
  })()

  try {
    await refreshInFlight
  } finally {
    refreshInFlight = null
  }
}

// 显示请求详情
function showRequestDetail(id: string) {
  if (!isAdminPage.value) return
  selectedRequestId.value = id
  detailModalOpen.value = true
}

</script>

<style scoped>
</style>
