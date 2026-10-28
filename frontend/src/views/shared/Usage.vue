<template>
  <div class="space-y-6 pb-8">
    <!-- 面包屑旁的折叠按钮 -->
    <Teleport
      to="#header-actions-right"
      defer
    >
      <button
        v-if="!hasScopedRecordFilters"
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
      v-if="statsExpanded && !hasScopedRecordFilters"
      class="space-y-4"
    >
      <!-- 活跃度热图 + 请求间隔时间线 -->
      <div
        v-if="!isAdminPage"
        class="grid grid-cols-1 xl:grid-cols-2 gap-4"
      >
        <ActivityHeatmapCard
          :data="activityHeatmapData"
          title="我的活跃天数"
          :is-loading="isLoadingHeatmap"
          :has-error="heatmapError"
        />
        <IntervalTimelineCard
          :title="intervalTimelineTitle"
          :is-admin="isAdminPage"
          :hours="intervalTimelineHours"
          :refresh-interval-ms="0"
        />
      </div>

      <!-- 管理员：模型 + 提供商 + API格式（3列） -->
      <div
        v-if="isAdminPage"
        class="grid grid-cols-1 lg:grid-cols-3 gap-4"
      >
        <UsageModelTable
          :data="enhancedModelStats"
          :is-admin="authStore.canAccessAdmin"
        />
        <UsageProviderTable
          :data="providerStats"
          :is-admin="authStore.canAccessAdmin"
        />
        <UsageApiFormatTable
          :data="apiFormatStats"
          :is-admin="authStore.canAccessAdmin"
        />
      </div>
      <!-- 用户：模型 + API格式（2列） -->
      <div
        v-else
        class="grid grid-cols-1 lg:grid-cols-2 gap-4"
      >
        <UsageModelTable
          :data="enhancedModelStats"
          :is-admin="authStore.canAccessAdmin"
        />
        <UsageApiFormatTable
          :data="apiFormatStats"
          :is-admin="false"
        />
      </div>
    </div>

    <div
      v-if="hasScopedRecordFilters"
      class="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground"
    >
      <span>{{ overviewT('请求筛选结果', 'Filtered requests') }}</span>
      <Button
        variant="ghost"
        size="sm"
        class="h-8"
        @click="clearRecordFilters"
      >
        <X class="mr-1 h-3 w-3" />{{ overviewT('清除筛选', 'Clear filters') }}
      </Button>
    </div>

    <div
      v-if="isAdminPage && deepLinkFilters.length"
      class="flex flex-wrap gap-2 text-xs"
    >
      <span
        v-for="filter in deepLinkFilters"
        :key="filter.key"
        class="inline-flex max-w-full items-center gap-1 rounded border px-2 py-1"
      >
        <span class="min-w-0 break-all">{{ filter.label }}: {{ filter.value }}</span>
        <button
          type="button"
          class="flex h-5 w-5 shrink-0 items-center justify-center hover:text-primary"
          :aria-label="`${overviewT('清除', 'Clear')} ${filter.label}`"
          :title="`${overviewT('清除', 'Clear')} ${filter.label}`"
          @click="clearDeepLinkFilter(filter.key)"
        ><X class="h-3 w-3" /></button>
      </span>
    </div>

    <!-- 使用记录 -->
    <UsageRecordsTable
      :records="displayRecords"
      :is-admin="isAdminPage"
      :show-actual-cost="authStore.canAccessAdmin"
      :loading="isLoadingRecords"
      :time-range="timeRange"
      :filter-search="filterSearch"
      :filter-user="filterUser"
      :filter-model="filterModel"
      :filter-provider="filterProvider"
      :filter-api-format="filterApiFormat"
      :filter-status="filterStatus"
      :filter-client-family="filterClientFamily"
      :available-users="availableUsers"
      :available-models="availableModels"
      :available-providers="availableProviders"
      :available-client-families="availableClientFamilies"
      :current-page="currentPage"
      :page-size="pageSize"
      :total-records="effectiveTotalRecords"
      :page-size-options="pageSizeOptions"
      :auto-refresh="globalAutoRefresh"
      :hide-unknown-records="hideUnknownRecords"
      @update:time-range="handleTimeRangeChange"
      @update:filter-search="handleFilterSearchChange"
      @update:filter-user="handleFilterUserChange"
      @update:filter-model="handleFilterModelChange"
      @update:filter-provider="handleFilterProviderChange"
      @update:filter-api-format="handleFilterApiFormatChange"
      @update:filter-status="handleFilterStatusChange"
      @update:filter-client-family="handleFilterClientFamilyChange"
      @update:current-page="handlePageChange"
      @update:page-size="handlePageSizeChange"
      @update:auto-refresh="handleAutoRefreshChange"
      @update:hide-unknown-records="handleHideUnknownRecordsChange"
      @refresh="handleManualRefresh"
      @prefetch-detail="prefetchRequestDetail"
      @show-detail="showRequestDetail"
    />

    <!-- 请求详情抽屉 - 仅管理员可见 -->
    <RequestDetailDrawer
      v-if="isAdminPage"
      :is-open="detailModalOpen"
      :request-id="selectedRequestId"
      :summary-record="selectedRequestSummary"
      @close="closeRequestDetail"
      @request-state="handleDetailRequestState"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { mergeUsageBillingSnapshot } from '@/features/usage/utils/usageBilling'
import { useRoute, useRouter } from 'vue-router'
import { useLocalStorage } from '@vueuse/core'
import { useAuthStore } from '@/stores/auth'
import { usageApi } from '@/api/usage'
import type { ImageProgress } from '@/api/requestTrace'
import { usersApi } from '@/api/users'
import { meApi } from '@/api/me'
import { dashboardApi } from '@/api/dashboard'
import { PanelTopClose, PanelTopOpen, X } from 'lucide-vue-next'
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
import {
  mergeUsageRecordErrorMessage,
  mergeUsageRecordFirstByteTimeMs,
  mergeUsageRecordLifecycleSnapshot,
  mergeUsageRecordResponseTiming,
  parseUsageTimestampMs,
} from '@/features/usage/utils/recordSync'
import {
  hasUsageFallback,
  isUsageRecordFailed,
  isUsageUpstreamStream,
  isUsageWebSocket,
  normalizeRequestStatus,
  resolveDisplayRequestStatus,
} from '@/features/usage/utils/status'
import { matchesUsageRecordSearch } from '@/features/usage/utils/recordSearch'
import {
  isUserLocalOnlyRecordStatus,
  shouldUseServerUserRecordFilters,
} from '@/features/usage/utils/recordFilterPolicy'
import type { DateRangeParams, FilterStatusValue, RequestStatus, UsageRecord } from '@/features/usage/types'
import type { UserOption } from '@/features/usage/components/UsageRecordsTable.vue'
import { log } from '@/utils/logger'
import type { ActivityHeatmap } from '@/types/activity'
import { useToast } from '@/composables/useToast'
import { queryString, rangeFromQuery, presetRange } from '@/features/overview/query'
import { useOverviewI18n } from '@/features/overview/i18n'
import { attributionLabel } from '@/features/overview/format'
import { Button } from '@/components/ui'

const route = useRoute()
const router = useRouter()
const { t: overviewT } = useOverviewI18n()
const { warning } = useToast()
const authStore = useAuthStore()

// 判断是否是管理员页面
const isAdminPage = computed(() => route.path.startsWith('/admin'))

// 用量分析面板折叠状态（默认展开，持久化到 localStorage）
const statsExpanded = useLocalStorage('usage-stats-expanded', true)
const hideUnknownRecords = useLocalStorage('usage-hide-unknown-records', false)

// 时间范围选择
const timeRange = ref<DateRangeParams>(
  getDateRangeFromPeriod('today')
)

// 分页状态
const currentPage = ref(1)
const pageSize = ref(20)
const pageSizeOptions = [10, 20, 50, 100]

function clampIntervalTimelineHours(hours: number): number {
  return Math.min(720, Math.max(1, Math.ceil(hours)))
}

function getIntervalTimelineHours(dateRange: DateRangeParams): number {
  switch (dateRange.preset) {
    case 'yesterday':
      return 48
    case 'last7days':
      return 24 * 7
    case 'last30days':
      return 24 * 30
    case 'last90days':
      return 24 * 30
    case 'today':
      return 24
    default:
      break
  }

  if (dateRange.start_date && dateRange.end_date) {
    const start = new Date(`${dateRange.start_date}T00:00:00`)
    const end = new Date(`${dateRange.end_date}T23:59:59`)
    const diffMs = end.getTime() - start.getTime()
    if (!Number.isNaN(diffMs) && diffMs >= 0) {
      return clampIntervalTimelineHours(diffMs / (1000 * 60 * 60))
    }
  }

  return 24
}

function formatIntervalTimelineWindow(hours: number): string {
  if (hours === 24) return '最近24小时'
  if (hours % 24 === 0) return `最近${hours / 24}天`
  return `最近${hours}小时`
}

// 筛选状态
const filterSearch = ref('')
const filterUser = ref('__all__')
const filterModel = ref('__all__')
const filterProvider = ref('__all__')
const filterApiFormat = ref('__all__')
const filterStatus = ref<FilterStatusValue>('__all__')
const filterClientFamily = ref('__all__')
const filterProviderId = ref('')
const filterApiKeyId = ref('')
const filterRequestId = ref('')
const filterAttribution = ref('')
const filterEndpointKind = ref('')
const filterRequestType = ref('')
const filterIsStream = ref<boolean | undefined>()
const filterFormatConversion = ref<boolean | undefined>()
const filterSlowThreshold = ref<number | undefined>()
const hasScopedRecordFilters = computed(() => isAdminPage.value && (
  filterSearch.value.trim() !== '' || [filterUser.value, filterModel.value, filterProvider.value, filterApiFormat.value, filterStatus.value, filterClientFamily.value].some(value => value !== '__all__') ||
  !!filterProviderId.value || !!filterApiKeyId.value || !!filterRequestId.value || !!filterAttribution.value || !!filterEndpointKind.value || !!filterRequestType.value || filterIsStream.value !== undefined || filterFormatConversion.value !== undefined || filterSlowThreshold.value !== undefined || hideUnknownRecords.value
))
function clearRecordFilters() {
  const fields = ['search', 'user_id', 'model', 'provider', 'api_format', 'status', 'client_family', 'provider_id', 'api_key_id', 'request_id', 'attribution_kind', 'endpoint_kind', 'request_type', 'is_stream', 'has_format_conversion', 'slow_threshold_ms', 'detail_id', 'page']
  hideUnknownRecords.value = false
  void router.replace({ query: { ...route.query, ...Object.fromEntries(fields.map(key => [key, undefined])) } })
}
const deepLinkFilters = computed(() => [
  { key: 'provider_id', label: overviewT('提供商 ID', 'Provider ID'), value: filterProviderId.value },
  { key: 'api_key_id', label: overviewT('凭证 ID', 'Credential ID'), value: filterApiKeyId.value },
  { key: 'request_id', label: overviewT('请求 ID', 'Request ID'), value: filterRequestId.value },
  { key: 'attribution_kind', label: overviewT('归属', 'Attribution'), value: filterAttribution.value ? attributionLabel(filterAttribution.value) : '' },
  { key: 'endpoint_kind', label: overviewT('端点类型', 'Endpoint kind'), value: filterEndpointKind.value },
  { key: 'request_type', label: overviewT('请求类型', 'Request type'), value: filterRequestType.value },
  { key: 'is_stream', label: overviewT('流式', 'Streaming'), value: filterIsStream.value === undefined ? '' : filterIsStream.value ? overviewT('是', 'Yes') : overviewT('否', 'No') },
  { key: 'has_format_conversion', label: overviewT('格式转换', 'Format conversion'), value: filterFormatConversion.value === undefined ? '' : filterFormatConversion.value ? overviewT('是', 'Yes') : overviewT('否', 'No') },
  { key: 'slow_threshold_ms', label: overviewT('响应时间', 'Response time'), value: filterSlowThreshold.value ? `>= ${filterSlowThreshold.value} ms` : '' },
].filter(filter => filter.value !== ''))
function clearDeepLinkFilter(key: string) { void router.replace({ query: { ...route.query, [key]: undefined, page: undefined } }) }

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
const intervalTimelineHours = computed(() => getIntervalTimelineHours(timeRange.value))
const intervalTimelineTitle = computed(() => {
  const baseTitle = isAdminPage.value ? overviewT('全站最近请求间隔', 'Site-wide recent request intervals') : '我的请求间隔'
  return `${baseTitle}（${formatIntervalTimelineWindow(intervalTimelineHours.value)}）`
})
const ADMIN_ANALYTICS_REFRESH_INTERVAL = 60000
let adminAnalyticsRefreshInFlight: Promise<void> | null = null
let lastAdminAnalyticsRefreshAt = 0
let adminAnalyticsRefreshGeneration = 0

// 加载热力图数据
async function loadHeatmapData() {
  isLoadingHeatmap.value = true
  heatmapError.value = false
  try {
    activityHeatmapData.value = await meApi.getActivityHeatmap()
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

async function refreshAdminAnalytics(options: { force?: boolean; preserveOnFailure?: boolean } = {}) {
  if (!isAdminPage.value) return
  if (!options.force && !isPageVisible.value) return

  const now = Date.now()
  if (!options.force && now - lastAdminAnalyticsRefreshAt < ADMIN_ANALYTICS_REFRESH_INTERVAL) {
    return
  }
  if (!options.force && adminAnalyticsRefreshInFlight) {
    return adminAnalyticsRefreshInFlight
  }
  if (!options.force) {
    lastAdminAnalyticsRefreshAt = now
  }

  const refreshGeneration = ++adminAnalyticsRefreshGeneration
  const refreshPromise = (async () => {
    let hasSuccessfulRefresh = false

    try {
      const hadFailure = await loadStats(getCurrentStatsFilters(), {
        force: options.force,
        preserveOnFailure: options.preserveOnFailure,
      })
      if (refreshGeneration !== adminAnalyticsRefreshGeneration) {
        return
      }
      hasSuccessfulRefresh = !hadFailure
      if (hadFailure) {
        warning('统计数据加载失败，请刷新重试')
      }
    } catch (error) {
      if (refreshGeneration !== adminAnalyticsRefreshGeneration) {
        return
      }
      log.error('加载统计数据失败:', error)
      warning('统计数据加载失败，请刷新重试')
    }

    if (hasSuccessfulRefresh && refreshGeneration === adminAnalyticsRefreshGeneration) {
      lastAdminAnalyticsRefreshAt = Date.now()
    }
  })()
  adminAnalyticsRefreshInFlight = refreshPromise

  try {
    await refreshPromise
  } finally {
    if (adminAnalyticsRefreshInFlight === refreshPromise) {
      adminAnalyticsRefreshInFlight = null
    }
  }
}

function getCurrentStatsFilters() {
  const filters = getCurrentFilters()
  return {
    ...timeRange.value,
    user_id: filters.user_id,
    model: filters.model,
    provider: filters.provider,
  }
}

async function refreshAdminAnalyticsForSelectionChange() {
  if (!isAdminPage.value) return
  await refreshAdminAnalytics({ force: true, preserveOnFailure: false })
}

function isUnknownUsageLabel(value: unknown): boolean {
  if (typeof value !== 'string') return false
  const normalized = value.trim().toLowerCase()
  return normalized === 'unknown' || normalized === 'unknow'
}

function hasUnknownModelOrProvider(record: UsageRecord): boolean {
  if (isUnknownUsageLabel(record.model)) return true
  if (isUnknownUsageLabel(record.provider)) return true
  return false
}

// 用户页面需要前端筛选；隐藏 unknown 的开关对管理员当前页也生效。
const filteredRecords = computed(() => {
  let records = hideUnknownRecords.value
    ? currentRecords.value.filter(record => !hasUnknownModelOrProvider(record))
    : [...currentRecords.value]

  if (!isAdminPage.value) {
    if (isUserLocalOnlyRecordStatus(filterStatus.value) && filterSearch.value.trim()) {
      records = records.filter(record => matchesUsageRecordSearch(record, filterSearch.value))
    }

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
      if (filterStatus.value === 'websocket') {
        records = records.filter(record => isUsageWebSocket(record))
      } else if (filterStatus.value === 'stream') {
        records = records.filter(record =>
          isUsageUpstreamStream(record)
          && !isUsageWebSocket(record)
          && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'standard') {
        records = records.filter(record =>
          !isUsageUpstreamStream(record)
          && !isUsageWebSocket(record)
          && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'active') {
        records = records.filter(record =>
          resolveDisplayRequestStatus(record) === 'pending' ||
          resolveDisplayRequestStatus(record) === 'streaming'
        )
      } else if (filterStatus.value === 'failed') {
        records = records.filter(record => isUsageRecordFailed(record))
      } else if (filterStatus.value === 'cancelled') {
        records = records.filter(record => record.status === 'cancelled')
      } else if (filterStatus.value === 'has_fallback') {
        records = records.filter(record => hasUsageFallback(record))
      } else if (filterStatus.value === 'has_retry') {
        records = records.filter(record => record.has_retry === true)
      } else if (filterStatus.value === 'has_skipped_candidate') {
        records = records.filter(record => record.has_skipped_candidate === true)
      }
    }

    if (filterClientFamily.value !== '__all__') {
      records = records.filter(record => record.client_family === filterClientFamily.value)
    }

    return records
  }
  return records
})

// 获取活跃请求的 ID 列表
const activeRequestIds = computed(() => {
  return currentRecords.value
    .filter((record) => {
      const displayStatus = resolveDisplayRequestStatus(record)
      return displayStatus === 'pending' || displayStatus === 'streaming'
    })
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
      const currentUpdatedAtMs = parseUsageTimestampMs(record.updated_at)
      const updateUpdatedAtMs = parseUsageTimestampMs(update.updated_at)
      const updateSnapshotIsOlder = currentUpdatedAtMs != null &&
        updateUpdatedAtMs != null &&
        updateUpdatedAtMs < currentUpdatedAtMs
      const shouldApply = !updateSnapshotIsOlder && newRank >= currentRank
      const updateHasFailureSignal =
        (typeof update.status_code === 'number' && update.status_code >= 400) ||
        (typeof update.error_message === 'string' && update.error_message.trim().length > 0) ||
        update.image_progress?.phase === 'failed'
      const shouldApplyData = shouldApply || (
        !updateSnapshotIsOlder && currentRank < 2 && updateHasFailureSignal
      )

      if (shouldApply && record.status !== update.status) {
        record.status = update.status
      }
      if (shouldApplyData) {
        record.is_queued = update.is_queued === true
        if ('image_progress' in update) {
          record.image_progress = update.image_progress ?? null
        }
        // 进行中状态也需要持续更新（provider/key/TTFB 可能在 streaming 后才落库）
        record.input_tokens = update.input_tokens
        record.effective_input_tokens = update.effective_input_tokens ?? record.effective_input_tokens
        record.output_tokens = update.output_tokens
        record.cache_creation_input_tokens = update.cache_creation_input_tokens ?? undefined
        record.cache_creation_ephemeral_5m_input_tokens =
          update.cache_creation_ephemeral_5m_input_tokens ?? undefined
        record.cache_creation_ephemeral_1h_input_tokens =
          update.cache_creation_ephemeral_1h_input_tokens ?? undefined
        record.cache_read_input_tokens = update.cache_read_input_tokens ?? undefined
        Object.assign(record, mergeUsageBillingSnapshot(record, update))
        record.cost = update.cost
        record.actual_cost = update.actual_cost ?? undefined
        record.rate_multiplier = update.rate_multiplier ?? record.rate_multiplier
        const responseTiming = mergeUsageRecordResponseTiming(
          {
            response_time_ms: record.response_time_ms,
            response_time_updated_at: record.response_time_updated_at,
          },
          {
            response_time_ms: update.response_time_ms,
            response_time_updated_at: update.response_time_updated_at,
          },
          {
            preferNext: update.status === 'completed' ||
              update.status === 'failed' ||
              update.status === 'cancelled',
          },
        )
        record.response_time_ms = responseTiming.response_time_ms
        record.response_time_updated_at = responseTiming.response_time_updated_at
        record.first_byte_time_ms = mergeUsageRecordFirstByteTimeMs(
          record.first_byte_time_ms,
          update.first_byte_time_ms
        )
        if ('updated_at' in update) {
          if (typeof update.updated_at === 'string') {
            record.updated_at = update.updated_at
          }
        }
        record.status_code = update.status_code ?? undefined
        record.error_message = mergeUsageRecordErrorMessage(
          record.error_message,
          update.error_message,
          { authoritative: shouldApply },
        )
        if (typeof update.upstream_is_stream === 'boolean') {
          record.upstream_is_stream = update.upstream_is_stream
          record.is_stream = update.upstream_is_stream
        } else if (typeof update.is_stream === 'boolean') {
          record.is_stream = update.is_stream
          record.upstream_is_stream = update.is_stream
        }
        if (typeof update.is_websocket === 'boolean') {
          record.is_websocket = record.is_websocket === true || update.is_websocket
        }
        if (typeof update.websocket_transport === 'string' && update.websocket_transport.trim()) {
          record.websocket_transport = update.websocket_transport
        }
        if (typeof update.usage_available === 'boolean') {
          record.usage_available = record.usage_available === false || update.usage_available === false
            ? false
            : true
        }
        if (typeof update.usage_pricing_available === 'boolean') {
          record.usage_pricing_available = record.usage_pricing_available === false
            || update.usage_pricing_available === false
            ? false
            : true
        }
        if (typeof update.input_audio_tokens === 'number') {
          record.input_audio_tokens = update.input_audio_tokens
        }
        if (typeof update.output_audio_tokens === 'number') {
          record.output_audio_tokens = update.output_audio_tokens
        }
        if (typeof update.client_is_stream === 'boolean') {
          record.client_is_stream = update.client_is_stream
          record.client_requested_stream = update.client_is_stream
        } else if (typeof update.client_requested_stream === 'boolean') {
          record.client_requested_stream = update.client_requested_stream
          record.client_is_stream = update.client_requested_stream
        }
        // API 格式/格式转换：streaming 时已可确定，轮询时同步更新
        if (update.api_format != null) record.api_format = update.api_format
        if (update.endpoint_api_format != null) record.endpoint_api_format = update.endpoint_api_format
        if (update.has_format_conversion != null) record.has_format_conversion = update.has_format_conversion
        if (typeof update.has_fallback === 'boolean') {
          record.has_fallback = record.has_fallback === true || update.has_fallback
        }
        if (typeof update.chat_pii_redaction === 'boolean') {
          record.chat_pii_redaction = record.chat_pii_redaction === true || update.chat_pii_redaction
        }
        // Active responses are complete final-provider snapshots. Absence clears facts left by
        // a previous candidate, while requested reasoning remains tied to the client request.
        record.target_model = typeof update.target_model === 'string' && update.target_model.trim()
          ? update.target_model
          : null
        record.reasoning_effort = typeof update.reasoning_effort === 'string' && update.reasoning_effort.trim()
          ? update.reasoning_effort
          : null
        if (typeof update.request_type === 'string' && update.request_type.trim()) {
          record.request_type = update.request_type
        }
        if (typeof update.requested_reasoning_effort === 'string' && update.requested_reasoning_effort.trim()) {
          record.requested_reasoning_effort = update.requested_reasoning_effort
        }
        // Active responses describe the current final provider request. Clear an old Fast fact
        // when the refreshed snapshot has no request-side tier instead of retaining it forever.
        record.service_tier = typeof update.service_tier === 'string' && update.service_tier.trim()
          ? update.service_tier
          : null
        record.actual_service_tier = typeof update.actual_service_tier === 'string' && update.actual_service_tier.trim()
          ? update.actual_service_tier
          : null
        // 活跃接口返回的是当前最终候选快照，服务端空值要清除旧响应模型。
        record.response_model = typeof update.response_model === 'string' && update.response_model.trim()
          ? update.response_model
          : null
        // 管理员接口返回额外字段
        // 只有当返回的 provider 不是 pending/unknown/unknow 时才更新，避免覆盖已有的正确值
        if ('provider' in update && typeof update.provider === 'string') {
          const updateProviderLabel = update.provider.trim().toLowerCase()
          if (updateProviderLabel && !['pending', 'unknown', 'unknow'].includes(updateProviderLabel)) {
            record.provider = update.provider
          }
        }
        if ('api_key_name' in update) {
          record.api_key_name = typeof update.api_key_name === 'string' ? update.api_key_name : undefined
        }
        if ('provider_key_name' in update) {
          record.provider_key_name = typeof update.provider_key_name === 'string'
            ? update.provider_key_name
            : undefined
        }
        if ('client_family' in update) {
          record.client_family = typeof update.client_family === 'string' ? update.client_family : null
        }
        if ('client_ip' in update) {
          record.client_ip = typeof update.client_ip === 'string' ? update.client_ip : null
        }
        if ('user_agent' in update) {
          record.user_agent = typeof update.user_agent === 'string' ? update.user_agent : null
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

async function handleHideUnknownRecordsChange(value: boolean) {
  hideUnknownRecords.value = value
  currentPage.value = 1
  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
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

// Retry/fallback are derived from the locally loaded records and are not accepted by the
// normal-user records API. Keep those statuses entirely local, including when combined with
// search/API-format filters, so an unsupported status never produces a misleading server total.
// 普通用户的 API 格式/传输类型/搜索筛选由后端执行，避免只筛选当前已加载页。
// 模型及后端不支持的 retry/fallback 筛选仍保持现有的本地分页语义。
const userUsesServerRecordFilters = computed(() => !isAdminPage.value && (
  shouldUseServerUserRecordFilters({
    search: filterSearch.value,
    apiFormat: filterApiFormat.value,
    status: filterStatus.value,
  })
))

const paginatedRecords = computed(() => {
  if (!isAdminPage.value && !userUsesServerRecordFilters.value) {
    const start = (currentPage.value - 1) * pageSize.value
    const end = start + pageSize.value
    return filteredRecords.value.slice(start, end)
  }
  return filteredRecords.value
})

// 用户页面使用前端筛选后的总数，管理员页面使用后端返回的总数
const effectiveTotalRecords = computed(() => {
  if (!isAdminPage.value && !userUsesServerRecordFilters.value) {
    return filteredRecords.value.length
  }
  return totalRecords.value
})

// 显示的记录
const displayRecords = computed(() => paginatedRecords.value)

const availableClientFamilies = computed(() => {
  const families = new Set<string>()
  currentRecords.value.forEach((record) => {
    const family = record.client_family?.trim()
    if (family) families.add(family)
  })
  return Array.from(families).sort()
})


// 详情弹窗状态
const detailModalOpen = ref(false)
const selectedRequestId = ref<string | null>(null)
const selectedRequestSummary = computed(() => (
  currentRecords.value.find(record => record.id === selectedRequestId.value) ?? null
))

function currentRouteState() {
  return JSON.stringify([timeRange.value, currentPage.value, pageSize.value, filterSearch.value, filterUser.value, filterModel.value, filterProvider.value, filterApiFormat.value, filterStatus.value, filterClientFamily.value, filterProviderId.value, filterApiKeyId.value, filterRequestId.value, filterAttribution.value, filterEndpointKind.value, filterRequestType.value, filterIsStream.value, filterFormatConversion.value, filterSlowThreshold.value])
}

function applyUsageRoute() {
  const query = route.query ?? {}
  const before = currentRouteState()
  const from = queryString(query, 'from')
  const to = queryString(query, 'to')
  if (from && to && Number.isFinite(Date.parse(from)) && Date.parse(from) < Date.parse(to)) {
    timeRange.value = { ...rangeFromQuery(query, presetRange('today')) }
  } else if (queryString(query, 'start_date') || queryString(query, 'preset')) {
    timeRange.value = queryString(query, 'preset')
      ? getDateRangeFromPeriod(queryString(query, 'preset') as Parameters<typeof getDateRangeFromPeriod>[0])
      : { start_date: queryString(query, 'start_date'), end_date: queryString(query, 'end_date'), timezone: queryString(query, 'timezone') || undefined }
  }
  filterSearch.value = queryString(query, 'search')
  filterUser.value = queryString(query, 'user_id') || '__all__'
  filterModel.value = queryString(query, 'model') || '__all__'
  filterProvider.value = queryString(query, 'provider') || '__all__'
  filterApiFormat.value = queryString(query, 'api_format') || '__all__'
  const status = queryString(query, 'status')
  filterStatus.value = (status === 'success' ? 'completed' : status || '__all__') as FilterStatusValue
  filterClientFamily.value = queryString(query, 'client_family') || '__all__'
  filterProviderId.value = queryString(query, 'provider_id')
  filterApiKeyId.value = queryString(query, 'api_key_id')
  filterRequestId.value = queryString(query, 'request_id')
  filterAttribution.value = queryString(query, 'attribution_kind')
  filterEndpointKind.value = queryString(query, 'endpoint_kind')
  filterRequestType.value = queryString(query, 'request_type')
  filterIsStream.value = ['true', 'false'].includes(queryString(query, 'is_stream')) ? queryString(query, 'is_stream') === 'true' : undefined
  filterFormatConversion.value = ['true', 'false'].includes(queryString(query, 'has_format_conversion')) ? queryString(query, 'has_format_conversion') === 'true' : undefined
  const slowThreshold = Number(queryString(query, 'slow_threshold_ms'))
  filterSlowThreshold.value = Number.isFinite(slowThreshold) && slowThreshold > 0 ? slowThreshold : undefined
  currentPage.value = Math.max(1, Number(queryString(query, 'page')) || 1)
  pageSize.value = Math.min(100, Math.max(10, Number(queryString(query, 'page_size')) || 20))
  const detail = queryString(query, 'detail_id')
  selectedRequestId.value = detail || null
  detailModalOpen.value = isAdminPage.value && !!detail
  return before !== currentRouteState()
}

function syncUsageRoute() {
  if (!isAdminPage.value) return
  const filters = getCurrentFilters()
  const next = { ...route.query, ...timeRange.value, ...filters, hideUnknownRecords: undefined, page: currentPage.value > 1 ? String(currentPage.value) : undefined, page_size: String(pageSize.value) }
  if (timeRange.value.from) { next.start_date = undefined; next.end_date = undefined; next.preset = undefined }
  else { next.from = undefined; next.to = undefined }
  void router.replace({ query: Object.fromEntries(Object.entries(next).map(([key, value]) => [key, typeof value === 'boolean' ? String(value) : value])) })
}

let routeReady = false
applyUsageRoute()
watch(() => route.query, async () => {
  if (applyUsageRoute() && routeReady && isAdminPage.value) {
    await loadRecords({ page: currentPage.value, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
    await refreshAdminAnalyticsForSelectionChange()
  }
})
watch([timeRange, currentPage, pageSize, filterSearch, filterUser, filterModel, filterProvider, filterApiFormat, filterStatus, filterClientFamily], () => { if (routeReady) syncUsageRoute() })

// 初始化加载
onMounted(async () => {
  routeReady = true
  document.addEventListener('visibilitychange', handleVisibilityChange)

  if (isAdminPage.value) {
    const adminUsersPromise = loadAdminUsers()

    await loadRecords(
      { page: currentPage.value, pageSize: pageSize.value },
      getCurrentFilters(),
      timeRange.value
    )
    void (async () => {
      await refreshAdminAnalytics({ force: true, preserveOnFailure: false })
      await adminUsersPromise
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
    await refreshAdminAnalyticsForSelectionChange()
    return
  }
  await loadStats(timeRange.value)
  if (userUsesServerRecordFilters.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

// 处理分页变化
async function handlePageChange(page: number) {
  currentPage.value = page
  if (isAdminPage.value || userUsesServerRecordFilters.value) {
    await loadRecords({ page, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  }
}

// 处理每页大小变化
async function handlePageSizeChange(size: number) {
  pageSize.value = size
  currentPage.value = 1  // 重置到第一页
  if (isAdminPage.value || userUsesServerRecordFilters.value) {
    await loadRecords({ page: 1, pageSize: size }, getCurrentFilters(), timeRange.value)
  }
}

// 获取当前筛选参数
function getCurrentFilters() {
  return {
    provider_id: filterProviderId.value || undefined,
    api_key_id: filterApiKeyId.value || undefined,
    request_id: filterRequestId.value || undefined,
    attribution_kind: filterAttribution.value || undefined,
    endpoint_kind: filterEndpointKind.value || undefined,
    request_type: filterRequestType.value || undefined,
    is_stream: filterIsStream.value,
    has_format_conversion: filterFormatConversion.value,
    slow_threshold_ms: filterSlowThreshold.value,
    search: filterSearch.value.trim() || undefined,
    user_id: filterUser.value !== '__all__' ? filterUser.value : undefined,
    model: filterModel.value !== '__all__' ? filterModel.value : undefined,
    provider: filterProvider.value !== '__all__' ? filterProvider.value : undefined,
    api_format: filterApiFormat.value !== '__all__' ? filterApiFormat.value : undefined,
    status: filterStatus.value !== '__all__' ? filterStatus.value : undefined,
    client_family: filterClientFamily.value !== '__all__' ? filterClientFamily.value : undefined,
    hideUnknownRecords: hideUnknownRecords.value || undefined
  }
}

// 处理筛选变化
async function handleFilterSearchChange(value: string) {
  filterSearch.value = value
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  } else if (userUsesServerRecordFilters.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  } else {
    await loadStats(timeRange.value)
  }
}

async function handleFilterUserChange(value: string) {
  filterUser.value = value
  currentPage.value = 1  // 重置到第一页

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
    await refreshAdminAnalyticsForSelectionChange()
  }
}

async function handleFilterModelChange(value: string) {
  filterModel.value = value
  currentPage.value = 1  // 重置到第一页

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
    await refreshAdminAnalyticsForSelectionChange()
  }
}

async function handleFilterProviderChange(value: string) {
  filterProvider.value = value
  currentPage.value = 1

  if (isAdminPage.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
    await refreshAdminAnalyticsForSelectionChange()
  }
}

async function handleFilterApiFormatChange(value: string) {
  filterApiFormat.value = value
  currentPage.value = 1

  if (isAdminPage.value || userUsesServerRecordFilters.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  } else {
    await loadStats(timeRange.value)
  }
}

async function handleFilterStatusChange(value: string) {
  filterStatus.value = value as FilterStatusValue
  currentPage.value = 1

  if (isAdminPage.value || userUsesServerRecordFilters.value) {
    await loadRecords({ page: 1, pageSize: pageSize.value }, getCurrentFilters(), timeRange.value)
  } else {
    await loadStats(timeRange.value)
  }
}

async function handleFilterClientFamilyChange(value: string) {
  filterClientFamily.value = value
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
    if (isAdminPage.value || userUsesServerRecordFilters.value) {
      await loadRecords(
        { page: currentPage.value, pageSize: pageSize.value },
        getCurrentFilters(),
        timeRange.value
      )
      return
    }

    await loadStats(timeRange.value)
  })()

  try {
    await refreshInFlight
  } finally {
    refreshInFlight = null
  }
}

async function handleManualRefresh() {
  if (!isPageVisible.value) return
  await refreshData()
}

// 显示请求详情
function showRequestDetail(id: string) {
  if (!isAdminPage.value) return
  selectedRequestId.value = id
  detailModalOpen.value = true
  void router.push({ query: { ...route.query, detail_id: id } })
}

function closeRequestDetail() {
  detailModalOpen.value = false
  void router.replace({ query: { ...route.query, detail_id: undefined } })
}

function sameImageProgress(left?: ImageProgress | null, right?: ImageProgress | null): boolean {
  if (!left && !right) return true
  if (!left || !right) return false
  return left.phase === right.phase &&
    left.upstream_ttfb_ms === right.upstream_ttfb_ms &&
    left.upstream_sse_frame_count === right.upstream_sse_frame_count &&
    left.last_upstream_event === right.last_upstream_event &&
    left.last_upstream_frame_at_unix_ms === right.last_upstream_frame_at_unix_ms &&
    left.partial_image_count === right.partial_image_count &&
    left.last_client_visible_event === right.last_client_visible_event &&
    left.downstream_heartbeat_count === right.downstream_heartbeat_count &&
    left.last_downstream_heartbeat_at_unix_ms === right.last_downstream_heartbeat_at_unix_ms &&
    left.downstream_heartbeat_interval_ms === right.downstream_heartbeat_interval_ms
}

function handleDetailRequestState(update: {
  id: string
  status?: RequestStatus
  statusCode?: number | null
  inputTokens?: number | null
  effectiveInputTokens?: number | null
  outputTokens?: number | null
  totalTokens?: number | null
  cacheCreationInputTokens?: number | null
  cacheCreationEphemeral5mInputTokens?: number | null
  cacheCreationEphemeral1hInputTokens?: number | null
  cacheReadInputTokens?: number | null
  cost?: number | null
  actualCost?: number | null
  billingMultiplier?: number | null
  billingCost?: number | null
  routingGroupId?: string | null
  routingGroupName?: string | null
  responseTimeMs?: number | null
  firstByteTimeMs?: number | null
  isStream?: boolean | null
  isWebSocket?: boolean | null
  websocketTransport?: string | null
  usageAvailable?: boolean | null
  usagePricingAvailable?: boolean | null
  inputAudioTokens?: number | null
  outputAudioTokens?: number | null
  upstreamIsStream?: boolean | null
  clientRequestedStream?: boolean | null
  clientIsStream?: boolean | null
  apiFormat?: string | null
  endpointApiFormat?: string | null
  hasFormatConversion?: boolean | null
  chatPiiRedaction?: boolean | null
  targetModel?: string | null
  requestedReasoningEffort?: string | null
  reasoningEffort?: string | null
  serviceTier?: string | null
  actualServiceTier?: string | null
  responseModel?: string | null
  imageProgress?: ImageProgress | null
  errorMessage?: string | null
  updatedAt?: string | null
}) {
  const record = currentRecords.value.find(record => record.id === update.id)
  if (!record) return

  const nextStatus = resolveDetailUpdateStatus(update)
  const lifecycle = mergeUsageRecordLifecycleSnapshot(record, {
    ...(nextStatus ? { status: nextStatus } : {}),
    ...('statusCode' in update ? { statusCode: update.statusCode } : {}),
    ...('errorMessage' in update ? { errorMessage: update.errorMessage } : {}),
    ...('updatedAt' in update ? { updatedAt: update.updatedAt } : {}),
  })
  record.status = lifecycle.status
  record.status_code = lifecycle.status_code
  record.error_message = lifecycle.error_message
  record.updated_at = lifecycle.updated_at
  if (!lifecycle.accepted) return

  if ('inputTokens' in update && update.inputTokens != null) {
    record.input_tokens = update.inputTokens
  }
  if ('effectiveInputTokens' in update && update.effectiveInputTokens != null) {
    record.effective_input_tokens = update.effectiveInputTokens
  }
  if ('outputTokens' in update && update.outputTokens != null) {
    record.output_tokens = update.outputTokens
  }
  if ('totalTokens' in update && update.totalTokens != null) {
    record.total_tokens = update.totalTokens
  }
  if ('cacheCreationInputTokens' in update && update.cacheCreationInputTokens != null) {
    record.cache_creation_input_tokens = update.cacheCreationInputTokens
  }
  if ('cacheCreationEphemeral5mInputTokens' in update && update.cacheCreationEphemeral5mInputTokens != null) {
    record.cache_creation_ephemeral_5m_input_tokens = update.cacheCreationEphemeral5mInputTokens
  }
  if ('cacheCreationEphemeral1hInputTokens' in update && update.cacheCreationEphemeral1hInputTokens != null) {
    record.cache_creation_ephemeral_1h_input_tokens = update.cacheCreationEphemeral1hInputTokens
  }
  if ('cacheReadInputTokens' in update && update.cacheReadInputTokens != null) {
    record.cache_read_input_tokens = update.cacheReadInputTokens
  }
  Object.assign(record, mergeUsageBillingSnapshot(record, {
    cost: update.cost,
    billing_multiplier: update.billingMultiplier,
    billing_cost: update.billingCost,
    routing_group_id: update.routingGroupId,
    routing_group_name: update.routingGroupName,
  }))
  if ('cost' in update && update.cost != null) {
    record.cost = update.cost
  }
  if ('actualCost' in update && update.actualCost != null) {
    record.actual_cost = update.actualCost
  }
  if ('responseTimeMs' in update) {
    const responseTiming = mergeUsageRecordResponseTiming(
      {
        response_time_ms: record.response_time_ms,
        response_time_updated_at: record.response_time_updated_at,
      },
      {
        response_time_ms: update.responseTimeMs,
        response_time_updated_at: null,
      },
      {
        preferNext: lifecycle.accepted && (
          nextStatus === 'completed' ||
          nextStatus === 'failed' ||
          nextStatus === 'cancelled'
        ),
      },
    )
    record.response_time_ms = responseTiming.response_time_ms
    record.response_time_updated_at = responseTiming.response_time_updated_at
  }
  if ('firstByteTimeMs' in update) {
    record.first_byte_time_ms = mergeUsageRecordFirstByteTimeMs(
      record.first_byte_time_ms,
      update.firstByteTimeMs
    )
  }
  if ('isStream' in update && typeof update.isStream === 'boolean') {
    record.is_stream = update.isStream
  }
  if ('isWebSocket' in update && typeof update.isWebSocket === 'boolean') {
    record.is_websocket = record.is_websocket === true || update.isWebSocket
  }
  if (
    'websocketTransport' in update
    && typeof update.websocketTransport === 'string'
    && update.websocketTransport.trim()
  ) {
    record.websocket_transport = update.websocketTransport
  }
  if ('usageAvailable' in update && typeof update.usageAvailable === 'boolean') {
    record.usage_available = update.usageAvailable
  }
  if (
    'usagePricingAvailable' in update
    && typeof update.usagePricingAvailable === 'boolean'
  ) {
    record.usage_pricing_available = update.usagePricingAvailable
  }
  if ('inputAudioTokens' in update && typeof update.inputAudioTokens === 'number') {
    record.input_audio_tokens = update.inputAudioTokens
  }
  if ('outputAudioTokens' in update && typeof update.outputAudioTokens === 'number') {
    record.output_audio_tokens = update.outputAudioTokens
  }
  if ('upstreamIsStream' in update && typeof update.upstreamIsStream === 'boolean') {
    record.upstream_is_stream = update.upstreamIsStream
  }
  if ('clientRequestedStream' in update && typeof update.clientRequestedStream === 'boolean') {
    record.client_requested_stream = update.clientRequestedStream
  }
  if ('clientIsStream' in update && typeof update.clientIsStream === 'boolean') {
    record.client_is_stream = update.clientIsStream
  }
  if ('apiFormat' in update && typeof update.apiFormat === 'string') {
    record.api_format = update.apiFormat
  }
  if ('endpointApiFormat' in update && typeof update.endpointApiFormat === 'string') {
    record.endpoint_api_format = update.endpointApiFormat
  }
  if ('hasFormatConversion' in update && typeof update.hasFormatConversion === 'boolean') {
    record.has_format_conversion = update.hasFormatConversion
  }
  if ('chatPiiRedaction' in update && typeof update.chatPiiRedaction === 'boolean') {
    record.chat_pii_redaction = record.chat_pii_redaction === true || update.chatPiiRedaction
  }
  if ('targetModel' in update) {
    record.target_model = typeof update.targetModel === 'string' ? update.targetModel : update.targetModel ?? undefined
  }
  if ('reasoningEffort' in update) {
    record.reasoning_effort = typeof update.reasoningEffort === 'string' ? update.reasoningEffort : null
  }
  if ('requestedReasoningEffort' in update) {
    record.requested_reasoning_effort = typeof update.requestedReasoningEffort === 'string'
      ? update.requestedReasoningEffort
      : null
  }
  if ('serviceTier' in update) {
    record.service_tier = typeof update.serviceTier === 'string' ? update.serviceTier : null
  }
  if ('actualServiceTier' in update) {
    record.actual_service_tier = typeof update.actualServiceTier === 'string'
      ? update.actualServiceTier
      : null
  }
  if ('responseModel' in update) {
    record.response_model = typeof update.responseModel === 'string'
      ? update.responseModel
      : null
  }
  if ('imageProgress' in update) {
    const nextProgress = update.imageProgress ?? null
    if (!sameImageProgress(record.image_progress, nextProgress)) {
      record.image_progress = nextProgress
    }
  }
}

function resolveDetailUpdateStatus(update: {
  status?: RequestStatus
  statusCode?: number | null
  imageProgress?: ImageProgress | null
  errorMessage?: string | null
}): RequestStatus | undefined {
  const status = normalizeRequestStatus(update.status)
  const hasFailureSignal =
    (typeof update.statusCode === 'number' && update.statusCode >= 400) ||
    (typeof update.errorMessage === 'string' && update.errorMessage.trim().length > 0) ||
    update.imageProgress?.phase === 'failed'

  if ((status == null || status === 'pending' || status === 'streaming') && hasFailureSignal) {
    return 'failed'
  }
  return status
}

function prefetchRequestDetail(id: string) {
  if (!isAdminPage.value) return
  void dashboardApi.prefetchRequestDetail(id).catch(error => {
    log.debug('预取请求详情失败', error)
  })
}

</script>

<style scoped>
</style>
