<template>
  <div class="space-y-6 pb-8">
    <!-- 面包屑旁的折叠按钮 -->
    <Teleport
      to="#header-actions-right"
      defer
    >
      <button
        class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
        :title="statsExpanded ? '收起用量分析' : '展开用量分析'"
        @click="statsExpanded = !statsExpanded"
      >
        <PanelTopClose
          v-if="statsExpanded"
          class="h-4 w-4"
        />
        <PanelTopOpen
          v-else
          class="h-4 w-4"
        />
      </button>
    </Teleport>

    <!-- 用量分析面板（可折叠） -->
    <div
      v-if="statsExpanded"
      class="space-y-4"
    >
      <!-- 活跃度热图 + 请求间隔时间线 -->
      <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
        <ActivityHeatmapCard
          :data="activityHeatmapData"
          :title="isAdminPage ? '总体活跃天数' : '我的活跃天数'"
          :is-loading="isLoadingHeatmap"
          :has-error="heatmapError"
        />
        <IntervalTimelineCard
          :title="isAdminPage ? '请求间隔时间线' : '我的请求间隔'"
          :is-admin="isAdminPage"
          :hours="24"
        />
      </div>

      <!-- 分析统计 -->
      <!-- 管理员：模型 + 提供商 + API格式（3列） -->
      <div
        v-if="isAdminPage"
        class="grid grid-cols-1 lg:grid-cols-3 gap-4"
      >
        <UsageModelTable
          :data="enhancedModelStats"
          :is-admin="authStore.isAdmin"
        />
        <UsageProviderTable
          :data="providerStats"
          :is-admin="authStore.isAdmin"
        />
        <UsageApiFormatTable
          :data="apiFormatStats"
          :is-admin="authStore.isAdmin"
        />
      </div>
      <!-- 用户：模型 + API格式（2列） -->
      <div
        v-else
        class="grid grid-cols-1 lg:grid-cols-2 gap-4"
      >
        <UsageModelTable
          :data="enhancedModelStats"
          :is-admin="authStore.isAdmin"
        />
        <UsageApiFormatTable
          :data="apiFormatStats"
          :is-admin="false"
        />
      </div>
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
      :filter-model="filterModel"
      :filter-provider="filterProvider"
      :filter-api-format="filterApiFormat"
      :filter-status="filterStatus"
      :available-users="availableUsers"
      :available-models="availableModels"
      :available-providers="availableProviders"
      :current-page="currentPage"
      :page-size="pageSize"
      :total-records="effectiveTotalRecords"
      :page-size-options="pageSizeOptions"
      :auto-refresh="globalAutoRefresh"
      @update:time-range="handleTimeRangeChange"
      @update:filter-search="handleFilterSearchChange"
      @update:filter-user="handleFilterUserChange"
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
import { useLocalStorage } from '@vueuse/core'
import { useAuthStore } from '@/stores/auth'
import { usageApi } from '@/api/usage'
import { usersApi } from '@/api/users'
import { meApi } from '@/api/me'
import { PanelTopClose, PanelTopOpen } from 'lucide-vue-next'
import {
  UsageModelTable,
  UsageProviderTable,
  UsageApiFormatTable,
  UsageRecordsTable,
  ActivityHeatmapCard,
  RequestDetailDrawer,
  IntervalTimelineCard
} from '@/features/usage/components'
import {
  useUsageData,
  getDateRangeFromPeriod
} from '@/features/usage/composables'
import { reconcileActiveRequestDiscovery } from '@/features/usage/utils/activeRequestDiscovery'
import { isUsageRecordFailed } from '@/features/usage/utils/status'
import type { DateRangeParams, FilterStatusValue } from '@/features/usage/types'
import type { UserOption } from '@/features/usage/components/UsageRecordsTable.vue'
import { log } from '@/utils/logger'
import type { ActivityHeatmap } from '@/types/activity'
import { useToast } from '@/composables/useToast'

const route = useRoute()
const { warning } = useToast()
const authStore = useAuthStore()

// 判断是否是管理员页面
const isAdminPage = computed(() => route.path.startsWith('/admin'))

// 用量分析面板折叠状态（默认展开，持久化到 localStorage）
const statsExpanded = useLocalStorage('usage-stats-expanded', true)

// 时间范围选择
const timeRange = ref<DateRangeParams>(
  getDateRangeFromPeriod('today')
)

// 分页状态
const currentPage = ref(1)
const pageSize = ref(20)
const pageSizeOptions = [10, 20, 50, 100]

// 筛选状态
const filterSearch = ref('')
const filterUser = ref('__all__')
const filterModel = ref('__all__')
const filterProvider = ref('__all__')
const filterApiFormat = ref('__all__')
const filterStatus = ref<FilterStatusValue>('__all__')

// 用户列表（仅管理员页面使用）
const availableUsers = ref<UserOption[]>([])

// 使用 composables
const {
  isLoadingRecords,
  providerStats,
  apiFormatStats,
  currentRecords,
  totalRecords,
  enhancedModelStats,
  availableModels,
  availableProviders,
  loadStats,
  loadRecords
} = useUsageData({ isAdminPage })

// 热力图状态
const activityHeatmapData = ref<ActivityHeatmap | null>(null)
const isLoadingHeatmap = ref(false)
const heatmapError = ref(false)
const ADMIN_ANALYTICS_REFRESH_INTERVAL = 60000
let adminAnalyticsRefreshInFlight: Promise<void> | null = null
let lastAdminAnalyticsRefreshAt = 0

// 加载热力图数据
async function loadHeatmapData() {
  isLoadingHeatmap.value = true
  heatmapError.value = false
  try {
    if (isAdminPage.value) {
      activityHeatmapData.value = await usageApi.getActivityHeatmap()
    } else {
      activityHeatmapData.value = await meApi.getActivityHeatmap()
    }
  } catch (error) {
    log.error('加载热力图数据失败:', error)
    heatmapError.value = true
  } finally {
    isLoadingHeatmap.value = false
  }
}

async function loadAdminUsers() {
  try {
    const users = await usersApi.getAllUsers()
    availableUsers.value = users.map(u => ({ id: u.id, username: u.username, email: u.email }))
  } catch (error) {
    log.error('加载用户列表失败:', error)
  }
}

async function refreshAdminAnalytics(options: { force?: boolean } = {}) {
  if (!isAdminPage.value) return
  if (!options.force && !isPageVisible.value) return

  const now = Date.now()
  if (!options.force && now - lastAdminAnalyticsRefreshAt < ADMIN_ANALYTICS_REFRESH_INTERVAL) {
    return
  }
  if (adminAnalyticsRefreshInFlight) {
    return adminAnalyticsRefreshInFlight
  }

  adminAnalyticsRefreshInFlight = (async () => {
    let hasSuccessfulRefresh = false

    try {
      await loadStats(timeRange.value)
      hasSuccessfulRefresh = true
    } catch (error) {
      log.error('加载统计数据失败:', error)
      warning('统计数据加载失败，请刷新重试')
    }

    try {
      await loadHeatmapData()
      hasSuccessfulRefresh = true
    } catch (error) {
      log.error('加载热力图数据失败:', error)
    }

    if (hasSuccessfulRefresh) {
      lastAdminAnalyticsRefreshAt = Date.now()
    }
  })()

  try {
    await adminAnalyticsRefreshInFlight
  } finally {
    adminAnalyticsRefreshInFlight = null
  }
}

// 用户页面需要前端筛选
const filteredRecords = computed(() => {
  if (!isAdminPage.value) {
    let records = [...currentRecords.value]

    if (filterModel.value !== '__all__') {
      records = records.filter(record => record.model === filterModel.value)
    }

    if (filterProvider.value !== '__all__') {
      records = records.filter(record => record.provider === filterProvider.value)
    }

    if (filterApiFormat.value !== '__all__') {
      records = records.filter(record =>
        record.api_format?.toUpperCase() === filterApiFormat.value.toUpperCase()
      )
    }

    if (filterStatus.value !== '__all__') {
      if (filterStatus.value === 'stream') {
        records = records.filter(record =>
          record.is_stream && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'standard') {
        records = records.filter(record =>
          !record.is_stream && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'active') {
        records = records.filter(record =>
          record.status === 'pending' || record.status === 'streaming'
        )
      } else if (filterStatus.value === 'failed') {
        records = records.filter(record => isUsageRecordFailed(record))
      } else if (filterStatus.value === 'cancelled') {
        records = records.filter(record => record.status === 'cancelled')
      }
    }

    return records
  }
  return currentRecords.value
})

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
let activeDiscoveryTimer: ReturnType<typeof setTimeout> | null = null
let globalAutoRefreshTimer: ReturnType<typeof setInterval> | null = null
let refreshInFlight: Promise<void> | null = null
const AUTO_REFRESH_INTERVAL = 1000 // 1秒刷新一次（用于活跃请求）
const ACTIVE_DISCOVERY_HOT_INTERVAL = 1000 // 有活跃请求时 1 秒扫描一次
const ACTIVE_DISCOVERY_IDLE_INTERVAL = 5000 // 空闲时降频，避免后台持续刷日志
const GLOBAL_AUTO_REFRESH_INTERVAL = 3000 // 3秒刷新一次（全局自动刷新）
const globalAutoRefresh = ref(false) // 全局自动刷新开关（默认关闭）
const isPageVisible = ref(typeof document === 'undefined' ? true : !document.hidden)

// 轮询活跃请求状态（轻量级，只更新状态变化的记录）

let pollInFlight = false
let activeDiscoveryInFlight = false
const discoveredActiveRequestIds = new Set<string>()

async function loadActiveRequestUpdates(ids?: string[]) {
  if (isAdminPage.value) {
    return usageApi.getActiveRequests(ids, timeRange.value)
  }
  const idsParam = ids?.length ? ids.join(',') : undefined
  return meApi.getActiveRequests(idsParam)
}

async function pollActiveRequests() {
  if (!isPageVisible.value) return
  if (!hasActiveRequests.value) return
  if (pollInFlight) return
  pollInFlight = true

  try {
    const { requests } = await loadActiveRequestUpdates(activeRequestIds.value)

    const recordMap = new Map(currentRecords.value.map(record => [record.id, record]))

    for (const update of requests) {
      const record = recordMap.get(update.id)
      if (!record) continue

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
      if (shouldApply) {
        // 进行中状态也需要持续更新（provider/key/TTFB 可能在 streaming 后才落库）
        record.input_tokens = update.input_tokens
        record.effective_input_tokens = update.effective_input_tokens ?? record.effective_input_tokens
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
        if ('provider_key_name' in update) {
          record.provider_key_name = typeof update.provider_key_name === 'string'
            ? update.provider_key_name
            : undefined
        }
      }
    }

    // 不再因活跃请求完成而全表刷新，字段已在上方就地更新
    // 未知请求（shouldRefresh 由 !record 触发）理论上不应出现在已知 ID 轮询中，忽略即可
  } catch (error) {
    log.error('轮询活跃请求状态失败:', error)
  } finally {
    pollInFlight = false
  }
}

async function discoverActiveRequests() {
  if (!isPageVisible.value) return
  if (activeDiscoveryInFlight) return
  if (refreshInFlight || isLoadingRecords.value) return
  activeDiscoveryInFlight = true

  try {
    const { requests } = await loadActiveRequestUpdates()
    const {
      retainedDiscoveredActiveRequestIds,
      unseenActiveRequestIds
    } = reconcileActiveRequestDiscovery({
      activeRequestIds: requests.map(request => request.id),
      knownRecordIds: currentRecords.value.map(record => record.id),
      discoveredActiveRequestIds
    })

    discoveredActiveRequestIds.clear()
    retainedDiscoveredActiveRequestIds.forEach(id => discoveredActiveRequestIds.add(id))

    if (unseenActiveRequestIds.length > 0) {
      unseenActiveRequestIds.forEach(id => discoveredActiveRequestIds.add(id))
      await refreshData()
    }
  } catch (error) {
    log.error('发现新活跃请求失败:', error)
  } finally {
    activeDiscoveryInFlight = false
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

function scheduleNextActiveDiscovery() {
  if (activeDiscoveryTimer) return
  if (!isPageVisible.value) return
  if (!globalAutoRefresh.value) return
  const interval = hasActiveRequests.value || discoveredActiveRequestIds.size > 0
    ? ACTIVE_DISCOVERY_HOT_INTERVAL
    : ACTIVE_DISCOVERY_IDLE_INTERVAL
  activeDiscoveryTimer = setTimeout(async () => {
    activeDiscoveryTimer = null
    await discoverActiveRequests()
    scheduleNextActiveDiscovery()
  }, interval)
}

// 启动自动刷新
function startAutoRefresh() {
  if (!isPageVisible.value) return
  scheduleNextAutoRefresh()
}

function startActiveDiscovery() {
  if (!isPageVisible.value) return
  if (!globalAutoRefresh.value) return
  if (activeDiscoveryTimer || activeDiscoveryInFlight) return
  void (async () => {
    await discoverActiveRequests()
    scheduleNextActiveDiscovery()
  })()
}

// 停止自动刷新
function stopAutoRefresh() {
  if (autoRefreshTimer) {
    clearTimeout(autoRefreshTimer)
    autoRefreshTimer = null
  }
}

function stopActiveDiscovery() {
  if (activeDiscoveryTimer) {
    clearTimeout(activeDiscoveryTimer)
    activeDiscoveryTimer = null
  }
}

// 监听活跃请求状态，已显示的活跃行始终自刷新到终态。
// “自动刷新”开关只控制全局 3 秒刷新和新活跃请求发现。
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
      startActiveDiscovery()
      if (hasActiveRequests.value) {
        startAutoRefresh()
      }
    }
    startGlobalAutoRefresh()
  } else {
    stopActiveDiscovery()
    stopGlobalAutoRefresh()
  }
}

function handleVisibilityChange() {
  isPageVisible.value = !document.hidden
  if (!isPageVisible.value) {
    stopAutoRefresh()
    stopActiveDiscovery()
    stopGlobalAutoRefresh()
    return
  }
  if (hasActiveRequests.value) {
    startAutoRefresh()
  }
  if (globalAutoRefresh.value) {
    startActiveDiscovery()
    refreshData()
    startGlobalAutoRefresh()
  }
}

// 组件卸载时清理定时器
onUnmounted(() => {
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  stopAutoRefresh()
  stopActiveDiscovery()
  stopGlobalAutoRefresh()
})

// 用户页面的前端分页（后端一次性返回所有记录，前端分页+筛选）
const paginatedRecords = computed(() => {
  if (!isAdminPage.value) {
    const start = (currentPage.value - 1) * pageSize.value
    const end = start + pageSize.value
    return filteredRecords.value.slice(start, end)
  }
  return currentRecords.value
})

// 用户页面使用前端筛选后的总数，管理员页面使用后端返回的总数
const effectiveTotalRecords = computed(() => {
  if (!isAdminPage.value) {
    return filteredRecords.value.length
  }
  return totalRecords.value
})

// 显示的记录
const displayRecords = computed(() => paginatedRecords.value)


// 详情弹窗状态
const detailModalOpen = ref(false)
const selectedRequestId = ref<string | null>(null)

// 初始化加载
onMounted(async () => {
  document.addEventListener('visibilitychange', handleVisibilityChange)

  if (isAdminPage.value) {
    // 管理员页面优先加载记录，统计面板在后台顺序刷新，避免瞬时并发打满后端。
    await loadRecords(
      { page: currentPage.value, pageSize: pageSize.value },
      getCurrentFilters(),
      timeRange.value
    )
    void (async () => {
      await refreshAdminAnalytics({ force: true })
      await loadAdminUsers()
    })()
  } else {
    // 用户页面：loadStats 已包含记录加载，不需要单独调用 loadRecords
    await Promise.allSettled([
      loadStats(timeRange.value).catch(err => {
        log.error('加载统计数据失败:', err)
        warning('统计数据加载失败，请刷新重试')
      }),
      loadHeatmapData().catch(err => {
        log.error('加载热力图数据失败:', err)
      })
    ])
  }

  if (globalAutoRefresh.value && isPageVisible.value) {
    startActiveDiscovery()
  }

  if (globalAutoRefresh.value && isPageVisible.value) {
    startGlobalAutoRefresh()
  }
})

// 处理时间范围变化
async function handleTimeRangeChange(value: DateRangeParams) {
  timeRange.value = value
  currentPage.value = 1 // 重置到第一页
  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
    await refreshAdminAnalytics({ force: true })
    return
  }
  await loadStats(timeRange.value)
  // 用户页面：loadStats 已包含记录加载
}

// 处理分页变化
async function handlePageChange(page: number) {
  currentPage.value = page
  if (isAdminPage.value) {
    await loadRecords({ page, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
  // 用户页面使用前端分页，无需重新请求
}

// 处理每页大小变化
async function handlePageSizeChange(size: number) {
  pageSize.value = size
  currentPage.value = 1  // 重置到第一页
  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: size }, getCurrentFilters(), timeRange.value)
  }
  // 用户页面使用前端分页，无需重新请求
}

// 获取当前筛选参数
function getCurrentFilters() {
  return {
    search: filterSearch.value.trim() || undefined,
    user_id: filterUser.value !== '__all__' ? filterUser.value : undefined,
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

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
  // 用户页面：search 需要重新从后端拉取数据（后端支持 search 参数）
  // 但通过 filteredRecords 做前端过滤已覆盖，无需额外请求
}

async function handleFilterUserChange(value: string) {
  filterUser.value = value
  currentPage.value = 1  // 重置到第一页

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

async function handleFilterModelChange(value: string) {
  filterModel.value = value
  currentPage.value = 1  // 重置到第一页

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

async function handleFilterProviderChange(value: string) {
  filterProvider.value = value
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

async function handleFilterApiFormatChange(value: string) {
  filterApiFormat.value = value
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

async function handleFilterStatusChange(value: string) {
  filterStatus.value = value as FilterStatusValue
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

// 刷新数据
async function refreshData() {
  if (!isPageVisible.value) return
  if (refreshInFlight) return refreshInFlight

  refreshInFlight = (async () => {
    if (isAdminPage.value) {
      await loadRecords(
        { page: currentPage.value, pageSize: pageSize.value },
        getCurrentFilters(),
        timeRange.value
      )
      void refreshAdminAnalytics()
      return
    }

    await loadStats(timeRange.value)
    // 用户页面：loadStats 已包含记录加载
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
