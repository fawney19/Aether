<template>
  <div class="space-y-6 pb-8">
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

    <!-- 请求详情 -->
    <UsageRecordsTable
      :records="displayRecords"
      :is-admin="isAdminPage"
      :show-actual-cost="authStore.isAdmin"
      :loading="isLoadingRecords"
      :selected-period="selectedPeriod"
      :filter-search="filterSearch"
      :filter-user="filterUser"
      :filter-model="filterModel"
      :filter-provider="filterProvider"
      :filter-status="filterStatus"
      :available-users="availableUsers"
      :available-models="availableModels"
      :available-providers="availableProviders"
      :current-page="currentPage"
      :page-size="pageSize"
      :total-records="totalRecords"
      :page-size-options="pageSizeOptions"
      :auto-refresh="globalAutoRefresh"
      @update:selected-period="handlePeriodChange"
      @update:filter-search="handleFilterSearchChange"
      @update:filter-user="handleFilterUserChange"
      @update:filter-model="handleFilterModelChange"
      @update:filter-provider="handleFilterProviderChange"
      @update:filter-status="handleFilterStatusChange"
      @update:current-page="handlePageChange"
      @update:page-size="handlePageSizeChange"
      @update:auto-refresh="handleAutoRefreshChange"
      @refresh="refreshData"
      @export="exportData"
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
import { useAuthStore } from '@/stores/auth'
import { usageApi } from '@/api/usage'
import { usersApi } from '@/api/users'
import { meApi } from '@/api/me'
import { getPageSizeCache } from '@/components/ui'
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
import type { PeriodValue, FilterStatusValue } from '@/features/usage/types'
import type { UserOption } from '@/features/usage/components/UsageRecordsTable.vue'
import { log } from '@/utils/logger'
import type { ActivityHeatmap } from '@/types/activity'
import { useToast } from '@/composables/useToast'

const route = useRoute()
const { warning } = useToast()
const authStore = useAuthStore()

// 判断是否是管理员页面
const isAdminPage = computed(() => route.path.startsWith('/admin'))

// 时间段选择
const selectedPeriod = ref<PeriodValue>('today')

// 分页状态
const currentPage = ref(1)
const pageSize = ref(getPageSizeCache('usage-records-page-size', 20))
const pageSizeOptions = [10, 20, 50, 100]

// 筛选状态
const filterSearch = ref('')
const filterUser = ref('__all__')
const filterModel = ref('__all__')
const filterProvider = ref('__all__')
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

    if (filterStatus.value !== '__all__') {
      if (filterStatus.value === 'stream') {
        records = records.filter(record =>
          record.is_stream && !record.error_message && (!record.status_code || record.status_code === 200)
        )
      } else if (filterStatus.value === 'standard') {
        records = records.filter(record =>
          !record.is_stream && !record.error_message && (!record.status_code || record.status_code === 200)
        )
      } else if (filterStatus.value === 'error') {
        records = records.filter(record =>
          record.error_message || (record.status_code && record.status_code >= 400)
        )
      } else if (filterStatus.value === 'active') {
        records = records.filter(record =>
          record.status === 'pending' || record.status === 'streaming'
        )
      } else if (filterStatus.value === 'pending') {
        records = records.filter(record => record.status === 'pending')
      } else if (filterStatus.value === 'streaming') {
        records = records.filter(record => record.status === 'streaming')
      } else if (filterStatus.value === 'completed') {
        records = records.filter(record => record.status === 'completed')
      } else if (filterStatus.value === 'failed') {
        // 失败请求需要同时考虑新旧两种判断方式：
        // 1. 新方式：status = "failed"
        // 2. 旧方式：status_code >= 400 或 error_message 不为空
        records = records.filter(record =>
          record.status === 'failed' ||
          (record.status_code && record.status_code >= 400) ||
          record.error_message
        )
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
let autoRefreshTimer: ReturnType<typeof setInterval> | null = null
let globalAutoRefreshTimer: ReturnType<typeof setInterval> | null = null
const AUTO_REFRESH_INTERVAL = 1000 // 1秒刷新一次（用于活跃请求）
const GLOBAL_AUTO_REFRESH_INTERVAL = 5000 // 5秒刷新一次（全局自动刷新）
const globalAutoRefresh = ref(false) // 全局自动刷新开关

// 轮询活跃请求状态（轻量级，只更新状态变化的记录）
async function pollActiveRequests() {
  if (!hasActiveRequests.value) return

  try {
    // 根据页面类型选择不同的 API
    const idsParam = activeRequestIds.value.join(',')
    const { requests } = isAdminPage.value
      ? await usageApi.getActiveRequests(activeRequestIds.value)
      : await meApi.getActiveRequests(idsParam)

    let shouldRefresh = false

    for (const update of requests) {
      const record = currentRecords.value.find(r => r.id === update.id)
      if (!record) {
        // 后端返回了未知的活跃请求，触发刷新以获取完整数据
        shouldRefresh = true
        continue
      }

      // 状态变化：completed/failed 需要刷新获取完整数据
      if (record.status !== update.status) {
        record.status = update.status
      }
      if (update.status === 'completed' || update.status === 'failed') {
        shouldRefresh = true
      }

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
      // 管理员接口返回额外字段
      if ('provider' in update && typeof update.provider === 'string') {
        record.provider = update.provider
      }
      if ('api_key_name' in update) {
        record.api_key_name = typeof update.api_key_name === 'string' ? update.api_key_name : undefined
      }
    }

    if (shouldRefresh) {
      await refreshData()
    }
  } catch (error) {
    log.error('轮询活跃请求状态失败:', error)
  }
}

// 启动自动刷新
function startAutoRefresh() {
  if (autoRefreshTimer) return
  autoRefreshTimer = setInterval(pollActiveRequests, AUTO_REFRESH_INTERVAL)
}

// 停止自动刷新
function stopAutoRefresh() {
  if (autoRefreshTimer) {
    clearInterval(autoRefreshTimer)
    autoRefreshTimer = null
  }
}

// 监听活跃请求状态，自动启动/停止刷新
// 1秒轮询始终用于活跃请求的实时更新，不受全局刷新影响
watch(hasActiveRequests, (hasActive) => {
  if (hasActive) {
    startAutoRefresh()
  } else {
    stopAutoRefresh()
  }
}, { immediate: true })

// 启动全局自动刷新
function startGlobalAutoRefresh() {
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
    refreshData() // 立即刷新一次
    startGlobalAutoRefresh()
  } else {
    stopGlobalAutoRefresh()
  }
}

// 组件卸载时清理定时器
onUnmounted(() => {
  stopAutoRefresh()
  stopGlobalAutoRefresh()
})

// 用户页面的前端分页
const paginatedRecords = computed(() => {
  if (!isAdminPage.value) {
    const start = (currentPage.value - 1) * pageSize.value
    const end = start + pageSize.value
    return filteredRecords.value.slice(start, end)
  }
  return currentRecords.value
})

// 显示的记录
const displayRecords = computed(() => paginatedRecords.value)


// 详情弹窗状态
const detailModalOpen = ref(false)
const selectedRequestId = ref<string | null>(null)

// 初始化加载
onMounted(async () => {
  const dateRange = getDateRangeFromPeriod(selectedPeriod.value)

  // 并行加载统计数据和热力图（使用 allSettled 避免其中一个失败影响另一个）
  const [statsResult, heatmapResult] = await Promise.allSettled([
    loadStats(dateRange),
    loadHeatmapData()
  ])

  // 检查加载结果并通知用户
  if (statsResult.status === 'rejected') {
    log.error('加载统计数据失败:', statsResult.reason)
    warning('统计数据加载失败，请刷新重试')
  }
  if (heatmapResult.status === 'rejected') {
    log.error('加载热力图数据失败:', heatmapResult.reason)
    // 热力图加载失败不提示，因为 UI 已显示占位符
  }

  // 加载记录和用户列表
  if (isAdminPage.value) {
    // 管理员页面：并行加载用户列表和记录
    const [users] = await Promise.all([
      usersApi.getAllUsers(),
      loadRecords({ page: currentPage.value, pageSize: pageSize.value }, getCurrentFilters())
    ])
    availableUsers.value = users.map(u => ({ id: u.id, username: u.username, email: u.email }))
  } else {
    // 用户页面：加载记录
    await loadRecords({ page: currentPage.value, pageSize: pageSize.value }, getCurrentFilters())
  }
})

// 处理时间段变化
async function handlePeriodChange(value: string) {
  selectedPeriod.value = value as PeriodValue
  currentPage.value = 1  // 重置到第一页

  const dateRange = getDateRangeFromPeriod(selectedPeriod.value)
  await loadStats(dateRange)
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
    model: filterModel.value !== '__all__' ? filterModel.value : undefined,
    provider: filterProvider.value !== '__all__' ? filterProvider.value : undefined,
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

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
  }
}

async function handleFilterModelChange(value: string) {
  filterModel.value = value
  currentPage.value = 1  // 重置到第一页

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
  }
}

async function handleFilterProviderChange(value: string) {
  filterProvider.value = value
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
  }
}

async function handleFilterStatusChange(value: string) {
  filterStatus.value = value as FilterStatusValue
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters())
  }
}

// 刷新数据
async function refreshData() {
  const dateRange = getDateRangeFromPeriod(selectedPeriod.value)
  await loadStats(dateRange)
  await loadRecords({ page: currentPage.value, pageSize: pageSize.value }, getCurrentFilters())
}

// 显示请求详情
function showRequestDetail(id: string) {
  if (!isAdminPage.value) return
  selectedRequestId.value = id
  detailModalOpen.value = true
}

// 导出数据
async function exportData(format: 'csv' | 'json') {
  try {
    const blob = await usageApi.exportUsage(format)
    const url = window.URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `usage-stats.${format}`
    a.click()
    window.URL.revokeObjectURL(url)
  } catch (error) {
    log.error('导出失败:', error)
  }
}
</script>

<style scoped>
</style>
