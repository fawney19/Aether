<template>
  <div class="space-y-4">
    <Card
      v-if="isDetailMode"
      class="overflow-hidden"
    >
      <div class="flex flex-col gap-3 border-b border-border/60 px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h3 class="text-sm font-semibold">
            用量报表
          </h3>
          <p class="mt-1 text-[11px] text-muted-foreground">
            按{{ reportGranularityLabel }}查看请求、总 Tokens、费用、效率、平均响应与 TTFB
          </p>
        </div>
        <div class="flex flex-wrap items-center gap-2 sm:justify-end">
          <Tabs
            v-model="currentGranularitySelection"
            class="self-start"
          >
            <TabsList class="tabs-button-list flex-wrap justify-start gap-1">
              <TabsTrigger
                v-for="option in detailGranularityTabStates"
                :key="option.value"
                :value="option.value"
                :disabled="option.disabled"
                class="min-w-[38px] px-2 py-0.5 text-[10px]"
                :class="option.disabled ? 'cursor-not-allowed opacity-40' : ''"
              >
                {{ option.label }}
              </TabsTrigger>
            </TabsList>
          </Tabs>
          <div
            v-if="isRefreshing"
            class="inline-flex h-7 items-center gap-1 rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
          >
            <Loader2 class="h-3 w-3 animate-spin" />
            更新中
          </div>
          <button
            class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-3 text-[10px] font-medium text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
            @click="exportDetailStats"
          >
            导出 CSV
          </button>
        </div>
      </div>

      <div class="sm:hidden">
        <div
          v-if="isInitialLoading"
          class="flex items-center justify-center px-4 py-8"
        >
          <Skeleton class="h-5 w-5 rounded-full" />
        </div>
        <div
          v-else-if="reportsUnavailable"
          class="px-4 py-8 text-center text-xs text-muted-foreground"
        >
          用量报表暂不可用
        </div>
        <div
          v-else-if="bucketStats.length === 0"
          class="px-4 py-8 text-center text-xs text-muted-foreground"
        >
          暂无数据
        </div>
        <div
          v-else
          class="divide-y divide-border/60"
          :class="{ 'opacity-60 transition-opacity': isRefreshing }"
        >
          <div
            v-for="stat in paginatedBucketStats"
            :key="stat.bucket_start"
            class="space-y-2 p-4"
          >
            <div class="flex items-center justify-between">
              <span class="text-sm font-medium">{{ formatBucketLabel(stat.bucket_start, stat.bucket_end) }}</span>
              <Badge
                variant="success"
                class="text-[10px]"
              >
                ${{ stat.cost.toFixed(4) }}
              </Badge>
            </div>
            <div class="grid grid-cols-2 gap-2 text-xs">
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">请求</span>
                <span>{{ stat.requests.toLocaleString() }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">Tokens（总计）</span>
                <span>{{ formatTokens(getBucketTotalTokens(stat.bucket_start, stat.tokens)) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">平均响应</span>
                <span>{{ formatResponseTime(stat.avg_response_time) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">TTFB</span>
                <span>{{ formatResponseTime(stat.avg_first_byte_time) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">Tokens（明细）</span>
                <span class="text-right text-muted-foreground">
                  <span
                    v-for="line in getBucketCompositionLines(stat.bucket_start)"
                    :key="`${stat.bucket_start}-${line}`"
                    class="block leading-5"
                  >
                    {{ line }}
                  </span>
                </span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">缓存命中率</span>
                <span>{{ getBucketCacheHitRateLabel(stat.bucket_start, stat.requests) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">效率</span>
                <span>{{ getBucketEfficiencyLabel(stat) }}</span>
              </div>
              <div class="col-span-2 flex justify-between gap-3">
                <span class="text-muted-foreground">使用模型</span>
                <Badge
                  variant="outline"
                  class="min-w-[48px] justify-center text-[10px]"
                >
                  {{ getBucketModelCountLabel(stat.requests, stat.models_used_count) }}
                </Badge>
              </div>
            </div>
          </div>
        </div>
      </div>

      <Table
        class="hidden sm:table"
        :class="{ 'opacity-60 transition-opacity': isRefreshing }"
      >
        <TableHeader>
          <TableRow>
            <TableHead class="text-left">
              {{ detailPeriodColumnLabel }}
            </TableHead>
            <TableHead class="text-center">
              请求次数
            </TableHead>
            <TableHead class="text-center">
              Tokens（总计）
            </TableHead>
            <TableHead class="text-center">
              Tokens（明细）
            </TableHead>
            <TableHead class="text-center">
              缓存命中率
            </TableHead>
            <TableHead class="text-center">
              费用
            </TableHead>
            <TableHead class="text-center">
              效率
            </TableHead>
            <TableHead class="text-center">
              平均响应
            </TableHead>
            <TableHead class="text-center">
              TTFB
            </TableHead>
            <TableHead class="text-center">
              使用模型
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="isInitialLoading">
            <TableCell
              colspan="10"
              class="py-8 text-center"
            >
              <div class="flex items-center justify-center gap-2">
                <Skeleton class="h-5 w-5 rounded-full" />
                <span class="text-xs text-muted-foreground">加载中...</span>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-else-if="reportsUnavailable">
            <TableCell
              colspan="10"
              class="py-8 text-center text-xs text-muted-foreground"
            >
              用量报表暂不可用
            </TableCell>
          </TableRow>
          <TableRow v-else-if="bucketStats.length === 0">
            <TableCell
              colspan="10"
              class="py-8 text-center text-xs text-muted-foreground"
            >
              暂无数据
            </TableCell>
          </TableRow>
          <template v-else>
            <TableRow
              v-for="stat in paginatedBucketStats"
              :key="stat.bucket_start"
            >
              <TableCell class="text-xs font-medium">
                {{ formatBucketLabel(stat.bucket_start, stat.bucket_end) }}
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ stat.requests.toLocaleString() }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="secondary"
                  class="text-[10px]"
                >
                  {{ formatTokens(getBucketTotalTokens(stat.bucket_start, stat.tokens)) }}
                </Badge>
              </TableCell>
              <TableCell class="max-w-[220px] text-center text-xs text-muted-foreground">
                <div class="space-y-0.5 leading-5">
                  <div
                    v-for="line in getBucketCompositionLines(stat.bucket_start)"
                    :key="`${stat.bucket_start}-${line}`"
                  >
                    {{ line }}
                  </div>
                </div>
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ getBucketCacheHitRateLabel(stat.bucket_start, stat.requests) }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="success"
                  class="text-[10px]"
                >
                  ${{ stat.cost.toFixed(4) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ getBucketEfficiencyLabel(stat) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ formatResponseTime(stat.avg_response_time) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ formatResponseTime(stat.avg_first_byte_time) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="min-w-[48px] justify-center text-[10px]"
                >
                  {{ getBucketModelCountLabel(stat.requests, stat.models_used_count) }}
                </Badge>
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>

      <div
        v-if="bucketStats.length > 0"
        class="border-t border-border bg-muted/30 px-4 py-3 text-xs backdrop-blur-sm"
      >
        <div class="grid grid-cols-2 gap-4 sm:grid-cols-3 xl:grid-cols-6">
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总请求
            </div>
            <div class="font-semibold text-foreground">
              {{ totalStats.requests.toLocaleString() }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总Tokens
            </div>
            <div class="font-semibold text-foreground">
              {{ formatTokens(totalStats.tokens) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总费用
            </div>
            <div class="font-semibold text-foreground">
              {{ formatCurrency(totalStats.cost) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              效率
            </div>
            <div class="font-semibold text-foreground">
              {{ overallEfficiencyLabel }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              平均响应
            </div>
            <div class="font-semibold text-foreground">
              {{ averageBucketResponseTime }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              TTFB
            </div>
            <div class="font-semibold text-foreground">
              {{ averageBucketTtfb }}
            </div>
          </div>
        </div>
      </div>
      <Pagination
        v-if="bucketStats.length > 0"
        :current="detailCurrentPage"
        :total="bucketStatsReversed.length"
        :page-size="detailPageSize"
        :page-size-options="PAGE_SIZE_OPTIONS"
        cache-key="admin-analytics-detail-page-size"
        @update:current="detailCurrentPage = $event"
        @update:page-size="detailPageSize = $event"
      />
    </Card>

    <Card
      v-else
      class="overflow-hidden"
    >
      <div class="flex flex-col gap-3 border-b border-border/60 px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <h3 class="text-sm font-semibold">
            模型报表
          </h3>
          <p class="mt-1 text-[11px] text-muted-foreground">
            按模型查看请求、总 Tokens、费用、效率、平均响应与 TTFB
          </p>
        </div>
        <div class="flex flex-wrap items-center gap-2 sm:justify-end">
          <div
            v-if="isRefreshing"
            class="inline-flex h-7 items-center gap-1 rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
          >
            <Loader2 class="h-3 w-3 animate-spin" />
            更新中
          </div>
          <button
            class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-3 text-[10px] font-medium text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
            @click="exportModelStats"
          >
            导出 CSV
          </button>
        </div>
      </div>

      <div class="sm:hidden">
        <div
          v-if="isInitialLoading"
          class="flex items-center justify-center px-4 py-8"
        >
          <Skeleton class="h-5 w-5 rounded-full" />
        </div>
        <div
          v-else-if="reportsUnavailable"
          class="px-4 py-8 text-center text-xs text-muted-foreground"
        >
          模型报表暂不可用
        </div>
        <div
          v-else-if="modelSummary.length === 0"
          class="px-4 py-8 text-center text-xs text-muted-foreground"
        >
          暂无数据
        </div>
        <div
          v-else
          class="divide-y divide-border/60"
          :class="{ 'opacity-60 transition-opacity': isRefreshing }"
        >
          <div
            v-for="model in paginatedModelSummary"
            :key="model.model"
            class="space-y-2 p-4"
          >
            <div class="flex items-center justify-between">
              <span class="text-sm font-medium">{{ model.model }}</span>
              <Badge
                variant="success"
                class="text-[10px]"
              >
                ${{ model.cost.toFixed(4) }}
              </Badge>
            </div>
            <div class="grid grid-cols-2 gap-2 text-xs">
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">请求</span>
                <span>{{ model.requests.toLocaleString() }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">Tokens（总计）</span>
                <span>{{ formatTokens(model.tokens) }}</span>
              </div>
              <div class="col-span-2 flex justify-between gap-3">
                <span class="text-muted-foreground">Tokens（明细）</span>
                <span class="text-right text-muted-foreground">
                  <span
                    v-for="line in getModelCompositionLines(model)"
                    :key="`${model.model}-${line}`"
                    class="block leading-5"
                  >
                    {{ line }}
                  </span>
                </span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">缓存命中率</span>
                <span>{{ getModelCacheHitRateLabel(model) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">效率</span>
                <span>{{ getModelEfficiencyLabel(model) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">平均响应</span>
                <span>{{ formatResponseTime(model.avg_response_time ?? 0) }}</span>
              </div>
              <div class="flex justify-between gap-3">
                <span class="text-muted-foreground">TTFB</span>
                <span>{{ formatResponseTime(model.avg_first_byte_time ?? 0) }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <Table
        class="hidden sm:table"
        :class="{ 'opacity-60 transition-opacity': isRefreshing }"
      >
        <TableHeader>
          <TableRow>
            <TableHead class="text-left">
              模型
            </TableHead>
            <TableHead class="text-center">
              请求次数
            </TableHead>
            <TableHead class="text-center">
              Tokens（总计）
            </TableHead>
            <TableHead class="text-center">
              Tokens（明细）
            </TableHead>
            <TableHead class="text-center">
              缓存命中率
            </TableHead>
            <TableHead class="text-center">
              费用
            </TableHead>
            <TableHead class="text-center">
              效率
            </TableHead>
            <TableHead class="text-center">
              平均响应
            </TableHead>
            <TableHead class="text-center">
              TTFB
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="isInitialLoading">
            <TableCell
              colspan="9"
              class="py-8 text-center"
            >
              <div class="flex items-center justify-center gap-2">
                <Skeleton class="h-5 w-5 rounded-full" />
                <span class="text-xs text-muted-foreground">加载中...</span>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-else-if="reportsUnavailable">
            <TableCell
              colspan="9"
              class="py-8 text-center text-xs text-muted-foreground"
            >
              模型报表暂不可用
            </TableCell>
          </TableRow>
          <TableRow v-else-if="modelSummary.length === 0">
            <TableCell
              colspan="9"
              class="py-8 text-center text-xs text-muted-foreground"
            >
              暂无数据
            </TableCell>
          </TableRow>
          <template v-else>
            <TableRow
              v-for="model in paginatedModelSummary"
              :key="model.model"
            >
              <TableCell class="font-medium text-xs">
                {{ model.model }}
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ model.requests.toLocaleString() }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="secondary"
                  class="text-[10px]"
                >
                  {{ formatTokens(model.tokens) }}
                </Badge>
              </TableCell>
              <TableCell class="max-w-[220px] text-center text-xs text-muted-foreground">
                <div class="space-y-0.5 leading-5">
                  <div
                    v-for="line in getModelCompositionLines(model)"
                    :key="`${model.model}-${line}`"
                  >
                    {{ line }}
                  </div>
                </div>
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ getModelCacheHitRateLabel(model) }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="success"
                  class="text-[10px]"
                >
                  ${{ model.cost.toFixed(4) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ getModelEfficiencyLabel(model) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ formatResponseTime(model.avg_response_time ?? 0) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ formatResponseTime(model.avg_first_byte_time ?? 0) }}
                </Badge>
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>

      <div
        v-if="modelSummary.length > 0"
        class="border-t border-border bg-muted/30 px-4 py-3 text-xs backdrop-blur-sm"
      >
        <div class="grid grid-cols-2 gap-4 sm:grid-cols-3 xl:grid-cols-7">
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              模型数
            </div>
            <div class="font-semibold text-foreground">
              {{ summary?.models_used_count ?? 0 }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总请求
            </div>
            <div class="font-semibold text-foreground">
              {{ totalStats.requests.toLocaleString() }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总Tokens
            </div>
            <div class="font-semibold text-foreground">
              {{ formatTokens(totalStats.tokens) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              总费用
            </div>
            <div class="font-semibold text-foreground">
              {{ formatCurrency(totalStats.cost) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              效率
            </div>
            <div class="font-semibold text-foreground">
              {{ overallEfficiencyLabel }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              平均响应
            </div>
            <div class="font-semibold text-foreground">
              {{ averageBucketResponseTime }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-muted-foreground">
              TTFB
            </div>
            <div class="font-semibold text-foreground">
              {{ averageBucketTtfb }}
            </div>
          </div>
        </div>
      </div>
      <Pagination
        v-if="modelSummary.length > 0"
        :current="modelCurrentPage"
        :total="modelSummary.length"
        :page-size="modelPageSize"
        :page-size-options="PAGE_SIZE_OPTIONS"
        cache-key="admin-analytics-model-page-size"
        @update:current="modelCurrentPage = $event"
        @update:page-size="modelPageSize = $event"
      />
    </Card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Card,
  Badge,
  Skeleton,
  Tabs,
  TabsList,
  TabsTrigger,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Pagination,
} from '@/components/ui'
import { useInjectedAnalyticsFilters } from '@/composables/useAnalyticsFilters'
import { useReportsData, type ModelSummary } from '@/composables/useReportsData'
import type { DateRangeParams } from '@/features/usage/types'
import { formatTokens, formatCurrency } from '@/utils/format'
import { downloadCsv } from '@/utils/csvExport'
import type { DailyUsageBreakdown } from '@/utils/usageBreakdown'
import { Loader2 } from 'lucide-vue-next'

const props = defineProps<{
  mode: 'detail' | 'models'
  selectedUserLabel?: string
  selectedApiKeyLabel?: string
}>()

const PAGE_SIZE_OPTIONS = [10, 20, 50]
const filters = useInjectedAnalyticsFilters()
const {
  timeRange,
  summary,
  bucketStats,
  bucketBreakdowns,
  modelSummary,
  loading,
  loadError,
  hasLoaded,
  resolvedGranularity,
} = useReportsData({
  scope: { kind: 'global' },
  timeRange: filters.timeRange,
  userFilter: filters.userFilter,
  apiKeyFilter: filters.apiKeyFilter,
  loadApiKeyOptions: false,
})

const isDetailMode = computed(() => props.mode === 'detail')
const detailCurrentPage = ref(1)
const detailPageSize = ref(10)
const modelCurrentPage = ref(1)
const modelPageSize = ref(10)

const isInitialLoading = computed(() => loading.value && !hasLoaded.value)
const isRefreshing = computed(() => loading.value && hasLoaded.value)
const reportsUnavailable = computed(() => loadError.value && !hasLoaded.value)
const bucketStatsReversed = computed(() => bucketStats.value.slice().reverse())
const paginatedBucketStats = computed(() => {
  const start = (detailCurrentPage.value - 1) * detailPageSize.value
  return bucketStatsReversed.value.slice(start, start + detailPageSize.value)
})
const paginatedModelSummary = computed(() => {
  const start = (modelCurrentPage.value - 1) * modelPageSize.value
  return modelSummary.value.slice(start, start + modelPageSize.value)
})

const totalStats = computed(() => {
  if (summary.value) {
    return {
      requests: summary.value.requests_total,
      tokens: summary.value.total_tokens,
      cost: summary.value.total_cost_usd,
    }
  }

  if (!bucketStats.value.length) {
    return { requests: 0, tokens: 0, cost: 0 }
  }

  return {
    requests: bucketStats.value.reduce((sum, item) => sum + item.requests, 0),
    tokens: bucketStats.value.reduce((sum, item) => sum + item.tokens, 0),
    cost: bucketStats.value.reduce((sum, item) => sum + item.cost, 0),
  }
})

const averageBucketResponseTime = computed(() => {
  const averageMs = summary.value?.avg_response_time_ms ?? 0
  if (averageMs <= 0) return '-'
  return formatResponseTime(averageMs / 1000)
})

const averageBucketTtfb = computed(() => {
  const averageMs = summary.value?.avg_first_byte_time_ms ?? 0
  if (averageMs <= 0) return '-'
  return formatResponseTime(averageMs / 1000)
})
const isSingleDayRange = computed(() => {
  const { startDate, endDate } = resolveExportRangeBounds(timeRange.value)
  if (!startDate || !endDate) return false
  return startDate.getTime() === endDate.getTime()
})

type ReportGranularity = Exclude<NonNullable<DateRangeParams['granularity']>, 'auto'>
type GranularityOption = NonNullable<DateRangeParams['granularity']>

const reportGranularityLabelMap: Record<ReportGranularity, string> = {
  hour: '小时',
  day: '天',
  week: '周',
  month: '月',
}

const currentGranularitySelection = computed<GranularityOption>({
  get: () => timeRange.value.granularity || 'auto',
  set: value => setDetailGranularity(value),
})
const detailGranularityTabStates = computed<Array<{ value: GranularityOption; label: string; disabled?: boolean }>>(() => [
  { value: 'auto', label: '自动' },
  { value: 'hour', label: '小时', disabled: !isSingleDayRange.value },
  { value: 'day', label: '天' },
  { value: 'week', label: '周' },
  { value: 'month', label: '月' },
])
const reportGranularityLabel = computed(() => reportGranularityLabelMap[resolvedGranularity.value as ReportGranularity])
const detailPeriodColumnLabel = computed(() => (
  resolvedGranularity.value === 'week' || resolvedGranularity.value === 'month'
    ? '周期'
    : '时间'
))
const overallEfficiencyLabel = computed(() => formatEfficiency(getEfficiencyValue(totalStats.value.tokens, totalStats.value.cost)))

function setDetailGranularity(granularity: GranularityOption): void {
  if (granularity === 'hour' && !isSingleDayRange.value) {
    granularity = 'auto'
  }

  timeRange.value = {
    ...timeRange.value,
    granularity,
  }
}

watch(
  [timeRange, bucketStatsReversed],
  () => {
    detailCurrentPage.value = 1
    modelCurrentPage.value = 1
  },
  { deep: true },
)

watch([detailPageSize, bucketStatsReversed], () => {
  const maxPage = Math.max(1, Math.ceil(bucketStatsReversed.value.length / detailPageSize.value))
  if (detailCurrentPage.value > maxPage) {
    detailCurrentPage.value = maxPage
  }
})

watch([modelPageSize, modelSummary], () => {
  const maxPage = Math.max(1, Math.ceil(modelSummary.value.length / modelPageSize.value))
  if (modelCurrentPage.value > maxPage) {
    modelCurrentPage.value = maxPage
  }
}, { deep: true })

function padDatePart(value: number): string {
  return String(value).padStart(2, '0')
}

function formatDateForExportName(date: Date): string {
  return `${date.getFullYear()}-${padDatePart(date.getMonth() + 1)}-${padDatePart(date.getDate())}`
}

function parseDateOnly(value?: string): Date | null {
  if (!value) return null
  const date = new Date(`${value}T00:00:00`)
  return Number.isNaN(date.getTime()) ? null : date
}

function resolveExportRangeBounds(range: DateRangeParams): { startDate: Date | null; endDate: Date | null } {
  const today = new Date()

  switch (range.preset) {
    case 'today':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last7days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 6),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last30days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 29),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last180days':
      return {
        startDate: new Date(today.getFullYear(), today.getMonth(), today.getDate() - 179),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    case 'last1year':
      return {
        startDate: new Date(today.getFullYear() - 1, today.getMonth(), today.getDate()),
        endDate: new Date(today.getFullYear(), today.getMonth(), today.getDate()),
      }
    default:
      return {
        startDate: parseDateOnly(range.start_date),
        endDate: parseDateOnly(range.end_date),
      }
  }
}

function getExportRangeLabel(): string {
  const { startDate, endDate } = resolveExportRangeBounds(timeRange.value)
  if (!startDate || !endDate) return 'unknown-range'

  const startLabel = formatDateForExportName(startDate)
  const endLabel = formatDateForExportName(endDate)
  return startLabel === endLabel ? startLabel : `${startLabel}_to_${endLabel}`
}

function getExportGranularityLabel(): string {
  return reportGranularityLabelMap[resolvedGranularity.value as ReportGranularity]
}

function sanitizeFilenameSegment(value: string): string {
  return value
    .trim()
    .replace(/[\\/:*?"<>|]/g, '-')
    .replace(/\s+/g, ' ')
}

function buildExportFilename(baseName: string, includeGranularity = true): string {
  const prefixes = [props.selectedUserLabel, props.selectedApiKeyLabel]
    .filter((value): value is string => Boolean(value && value.trim()))
    .map(value => sanitizeFilenameSegment(value))
  const prefix = prefixes.length > 0 ? `${prefixes.join('-')}-` : ''
  const rangeLabel = getExportRangeLabel()
  if (!includeGranularity) {
    return `${prefix}${baseName}-${rangeLabel}`
  }
  return `${prefix}${baseName}-${rangeLabel}-${getExportGranularityLabel()}`
}

function formatMonthDay(dateStr: string): string {
  const date = new Date(`${dateStr}T00:00:00`)
  return `${date.getMonth() + 1}/${date.getDate()}`
}

function formatMonth(dateStr: string): string {
  return dateStr.slice(0, 7)
}

function getPreviousDateKey(dateStr: string): string {
  const date = new Date(`${dateStr}T00:00:00`)
  date.setDate(date.getDate() - 1)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function formatBucketLabel(bucketStart: string, bucketEnd: string): string {
  const bucketDate = bucketStart.slice(0, 10)

  if (resolvedGranularity.value === 'week') {
    return `${formatMonthDay(bucketDate)} - ${formatMonthDay(getPreviousDateKey(bucketEnd.slice(0, 10)))}`
  }

  if (resolvedGranularity.value === 'month') {
    return formatMonth(bucketDate)
  }

  if (resolvedGranularity.value === 'hour') {
    return `${bucketStart.slice(11, 13)}:00`
  }

  return formatMonthDay(bucketDate)
}

function formatBucketExportLabel(bucketStart: string, bucketEnd: string): string {
  const bucketDate = bucketStart.slice(0, 10)

  if (resolvedGranularity.value === 'week') {
    return `${bucketDate} - ${getPreviousDateKey(bucketEnd.slice(0, 10))}`
  }

  if (resolvedGranularity.value === 'month') {
    return formatMonth(bucketDate)
  }

  if (resolvedGranularity.value === 'hour') {
    return `${bucketDate} ${bucketStart.slice(11, 13)}:00`
  }

  return bucketDate
}

function formatResponseTime(seconds: number): string {
  if (seconds <= 0) return '--'
  if (seconds < 1) return `${Math.round(seconds * 1000)}ms`
  return `${seconds.toFixed(1)}s`
}

function getBucketTotalTokens(bucketStart: string, fallbackTokens: number): number {
  return bucketBreakdowns.value[bucketStart]?.totalTrackedTokens || fallbackTokens
}

function buildCompositionLines(summaryItem: Pick<DailyUsageBreakdown, 'inputTokens' | 'outputTokens' | 'cacheCreationTokens' | 'cacheReadTokens'>): [string, string] {
  return [
    `输入 ${formatTokens(summaryItem.inputTokens)} · 输出 ${formatTokens(summaryItem.outputTokens)}`,
    `缓存创建 ${formatTokens(summaryItem.cacheCreationTokens)} · 缓存读取 ${formatTokens(summaryItem.cacheReadTokens)}`,
  ]
}

function getBucketCompositionLines(bucketStart: string): [string, string] {
  const summaryItem: Pick<DailyUsageBreakdown, 'inputTokens' | 'outputTokens' | 'cacheCreationTokens' | 'cacheReadTokens'> = bucketBreakdowns.value[bucketStart] ?? {
    inputTokens: 0,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
  }

  return buildCompositionLines(summaryItem)
}

function getBucketCacheHitRateLabel(bucketStart: string, requests: number): string {
  if (requests <= 0) return '--'
  const value = bucketBreakdowns.value[bucketStart]?.cacheHitRate
  if (typeof value !== 'number' || Number.isNaN(value)) return '--'
  return `${value.toFixed(1)}%`
}

function getBucketModelCountLabel(requests: number, count: number): string {
  if (requests <= 0) return '--'
  return `${count || 0} 个`
}

function getBucketModelCountExportValue(requests: number, count: number): number | string {
  if (requests <= 0) return '--'
  return count || 0
}

function getEfficiencyValue(tokens: number, cost: number): number | null {
  if (tokens <= 0) return null
  return (cost * 1000000) / tokens
}

function formatEfficiency(value: number | null): string {
  if (value == null || Number.isNaN(value)) return '--'
  if (value <= 0 || value < 0.00005) return '$0/M'
  return `${formatCurrency(value)}/M`
}

function getBucketEfficiencyValue(stat: { bucket_start: string; tokens: number; cost: number }): number | null {
  return getEfficiencyValue(getBucketTotalTokens(stat.bucket_start, stat.tokens), stat.cost)
}

function getBucketEfficiencyLabel(stat: { bucket_start: string; tokens: number; cost: number }): string {
  return formatEfficiency(getBucketEfficiencyValue(stat))
}

function getModelCompositionLines(model: ModelSummary): [string, string] {
  return buildCompositionLines({
    inputTokens: model.inputTokens,
    outputTokens: model.outputTokens,
    cacheCreationTokens: model.cacheCreationTokens,
    cacheReadTokens: model.cacheReadTokens,
  })
}

function getModelCacheHitRateLabel(model: ModelSummary): string {
  if (model.requests <= 0) return '--'
  if (typeof model.cacheHitRate !== 'number' || Number.isNaN(model.cacheHitRate)) return '--'
  return `${model.cacheHitRate.toFixed(1)}%`
}

function getModelEfficiencyValue(model: ModelSummary): number | null {
  return getEfficiencyValue(model.tokens, model.cost)
}

function getModelEfficiencyLabel(model: ModelSummary): string {
  return formatEfficiency(getModelEfficiencyValue(model))
}

function formatExportDecimal(value: number | null | undefined, digits: number): string {
  if (value == null || Number.isNaN(value) || value <= 0) return '--'
  return value.toFixed(digits)
}

function formatExportEfficiency(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '--'
  if (value <= 0 || value < 0.0000005) return '0'
  return value.toFixed(6)
}

function exportDetailStats(): void {
  if (!bucketStats.value.length) return
  downloadCsv(
    buildExportFilename('用量报表'),
    ['时间', '请求次数', 'Tokens（总计）', '输入Tokens', '输出Tokens', '缓存创建', '缓存读取', '缓存命中率', '费用', '效率（$/1M Tokens）', '平均响应(s)', 'TTFB(s)', '使用模型'],
    bucketStats.value.map(stat => [
      formatBucketExportLabel(stat.bucket_start, stat.bucket_end),
      stat.requests,
      getBucketTotalTokens(stat.bucket_start, stat.tokens),
      bucketBreakdowns.value[stat.bucket_start]?.inputTokens ?? 0,
      bucketBreakdowns.value[stat.bucket_start]?.outputTokens ?? 0,
      bucketBreakdowns.value[stat.bucket_start]?.cacheCreationTokens ?? 0,
      bucketBreakdowns.value[stat.bucket_start]?.cacheReadTokens ?? 0,
      getBucketCacheHitRateLabel(stat.bucket_start, stat.requests),
      stat.cost.toFixed(6),
      formatExportEfficiency(getBucketEfficiencyValue(stat)),
      formatExportDecimal(stat.avg_response_time, 3),
      formatExportDecimal(stat.avg_first_byte_time, 3),
      getBucketModelCountExportValue(stat.requests, stat.models_used_count),
    ]),
  )
}

function exportModelStats(): void {
  if (!modelSummary.value.length) return
  downloadCsv(
    buildExportFilename('模型报表', false),
    ['模型', '请求次数', 'Tokens（总计）', '输入Tokens', '输出Tokens', '缓存创建', '缓存读取', '缓存命中率', '费用', '效率（$/1M Tokens）', '平均响应(s)', 'TTFB(s)'],
    modelSummary.value.map(model => [
      model.model,
      model.requests,
      model.tokens,
      model.inputTokens,
      model.outputTokens,
      model.cacheCreationTokens,
      model.cacheReadTokens,
      getModelCacheHitRateLabel(model),
      model.cost.toFixed(6),
      formatExportEfficiency(getModelEfficiencyValue(model)),
      formatExportDecimal(model.avg_response_time, 3),
      formatExportDecimal(model.avg_first_byte_time, 3),
    ]),
  )
}
</script>
