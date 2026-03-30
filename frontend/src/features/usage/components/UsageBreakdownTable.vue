<template>
  <TableCard>
    <template #header>
      <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
        <div class="min-w-0">
          <h3 class="text-sm font-semibold tracking-[0.01em]">
            {{ title }}
          </h3>
          <p class="mt-1 text-[11px] text-muted-foreground">
            {{ description }}
          </p>
        </div>

        <div class="flex flex-col items-start gap-2 lg:items-end">
          <Tabs
            v-if="tabs.length > 0"
            :model-value="activeTabValue"
            class="w-full lg:w-auto"
            @update:model-value="handleTabChange"
          >
            <TabsList class="tabs-button-list h-auto flex-wrap justify-start gap-1 rounded-xl border border-border/70 bg-background/80 p-1">
              <TabsTrigger
                v-for="tab in tabs"
                :key="tab.value"
                :value="tab.value"
                class="min-w-[84px] px-3 py-1.5 text-[11px]"
              >
                {{ tab.label }}
              </TabsTrigger>
            </TabsList>
          </Tabs>

          <div
            v-if="isRefreshing"
            class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
          >
            更新中
          </div>
        </div>
      </div>
    </template>

    <div
      v-if="showLoadingState"
      class="p-6"
    >
      <LoadingState />
    </div>
    <div
      v-else-if="showUnavailableState"
      class="p-6"
    >
      <EmptyState
        type="error"
        :title="`${entityLabel}统计暂不可用`"
        description="接口未返回结果，请稍后重试"
      />
    </div>
    <div
      v-else-if="rows.length === 0"
      class="p-6"
    >
      <EmptyState
        :title="`暂无${entityLabel}统计数据`"
        description="当前时间范围内没有可展示的聚合结果"
      />
    </div>
    <div
      v-else
      class="max-h-[460px] overflow-auto"
      :class="{ 'opacity-60 transition-opacity': isRefreshing }"
    >
      <div class="divide-y divide-border/60 sm:hidden">
        <div
          v-for="row in rows"
          :key="row.key"
          class="space-y-3 px-4 py-3.5"
        >
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0 flex-1">
              <div
                class="truncate text-sm font-medium tracking-[0.01em]"
                :title="row.label"
              >
                {{ row.label }}
              </div>
              <div class="mt-1 flex flex-wrap items-center gap-1.5 text-[10px] text-muted-foreground">
                <span class="rounded-full bg-muted/45 px-2 py-0.5 tabular-nums">
                  {{ formatNumber(row.requests_total) }} 请求
                </span>
                <span class="rounded-full bg-muted/45 px-2 py-0.5 tabular-nums">
                  {{ formatTokens(row.total_tokens) }} Tokens
                </span>
              </div>
            </div>
            <div class="flex flex-col items-end gap-1.5">
              <Badge
                variant="outline"
                class="min-w-[64px] justify-center text-[10px] tabular-nums"
                :class="successRateClass(row.success_rate)"
              >
                {{ formatHitRate(row.success_rate) }}
              </Badge>
              <Badge
                variant="outline"
                class="min-w-[64px] justify-center border-border/70 bg-muted/30 text-[10px] tabular-nums"
              >
                {{ formatHitRate(row.cache_hit_rate) }}
              </Badge>
            </div>
          </div>

          <div class="grid grid-cols-2 gap-2 text-[10px]">
            <div class="rounded-xl border border-border/50 bg-muted/[0.14] px-2.5 py-2">
              <div class="text-muted-foreground">
                输入 / 输出
              </div>
              <div class="mt-1 font-medium tabular-nums">
                {{ formatTokens(row.input_tokens) }} / {{ formatTokens(row.output_tokens) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/[0.14] px-2.5 py-2">
              <div class="text-muted-foreground">
                创建 / 读取
              </div>
              <div class="mt-1 font-medium tabular-nums">
                {{ formatTokens(row.cache_creation_input_tokens) }} / {{ formatTokens(row.cache_read_input_tokens) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/[0.14] px-2.5 py-2">
              <div class="text-muted-foreground">
                费用 / 成本
              </div>
              <div class="mt-1 font-medium tabular-nums">
                {{ formatCurrency(row.total_cost_usd) }}
              </div>
              <div
                v-if="secondaryCostText(row)"
                class="mt-0.5 text-muted-foreground tabular-nums"
              >
                {{ secondaryCostText(row) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/[0.14] px-2.5 py-2">
              <div class="text-muted-foreground">
                效率
              </div>
              <div class="mt-1 font-medium tabular-nums">
                {{ formatEfficiency(row.total_tokens, row.total_cost_usd) }}
              </div>
            </div>
            <div class="rounded-xl border border-border/50 bg-muted/[0.14] px-2.5 py-2">
              <div class="text-muted-foreground">
                响应 / TTFB
              </div>
              <div class="mt-1 font-medium tabular-nums">
                {{ formatLatency(row.avg_response_time_ms) }} / {{ formatLatency(row.avg_first_byte_time_ms) }}
              </div>
            </div>
          </div>
        </div>
      </div>

      <Table class="hidden table-fixed sm:table">
        <TableHeader>
          <TableRow>
            <TableHead class="w-[21%] px-3 py-2.5">
              {{ entityLabel }}
            </TableHead>
            <TableHead class="w-[13%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>请求</span>
                <span class="text-muted-foreground font-normal">总 Tokens</span>
              </div>
            </TableHead>
            <TableHead class="w-[14%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>输入 / 输出</span>
                <span class="text-muted-foreground font-normal">请求输入 / 模型生成</span>
              </div>
            </TableHead>
            <TableHead class="w-[15%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>缓存</span>
                <span class="text-muted-foreground font-normal">创建 / 读取 / 命中</span>
              </div>
            </TableHead>
            <TableHead class="w-[13%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>费用 / 成本</span>
                <span class="text-muted-foreground font-normal">账面 / 实际</span>
              </div>
            </TableHead>
            <TableHead class="w-[10%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>效率</span>
                <span class="text-muted-foreground font-normal">$/1M Tokens</span>
              </div>
            </TableHead>
            <TableHead class="w-[14%] px-3 py-2.5 text-right">
              <div class="flex flex-col items-end text-[10px] leading-4">
                <span>成功 / 延迟</span>
                <span class="text-muted-foreground font-normal">成功率 / 响应 / TTFB</span>
              </div>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="row in rows"
            :key="row.key"
            class="align-top"
          >
            <TableCell class="px-3 py-2.5">
              <div
                class="truncate text-[13px] font-medium leading-5 tracking-[0.01em]"
                :title="row.label"
              >
                {{ row.label }}
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="space-y-0.5 text-[11px] tabular-nums">
                <div class="font-medium text-foreground">
                  {{ formatNumber(row.requests_total) }}
                </div>
                <div class="text-muted-foreground">
                  {{ formatTokens(row.total_tokens) }}
                </div>
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="text-[11px] tabular-nums">
                <div>{{ formatTokens(row.input_tokens) }} / {{ formatTokens(row.output_tokens) }}</div>
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="space-y-0.5 text-[11px] tabular-nums">
                <div>{{ formatTokens(row.cache_creation_input_tokens) }} / {{ formatTokens(row.cache_read_input_tokens) }}</div>
                <div class="text-muted-foreground">
                  {{ formatHitRate(row.cache_hit_rate) }}
                </div>
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="space-y-0.5 text-[11px] tabular-nums">
                <div class="font-medium text-amber-700 dark:text-amber-300">
                  {{ formatCurrency(row.total_cost_usd) }}
                </div>
                <div class="text-muted-foreground">
                  {{ secondaryCostText(row) }}
                </div>
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="text-[11px] font-medium tabular-nums text-foreground">
                {{ formatEfficiency(row.total_tokens, row.total_cost_usd) }}
              </div>
            </TableCell>
            <TableCell class="px-3 py-2.5 text-right">
              <div class="flex flex-col items-end gap-1">
                <Badge
                  variant="outline"
                  class="min-w-[72px] justify-center text-[10px] tabular-nums"
                  :class="successRateClass(row.success_rate)"
                >
                  {{ formatHitRate(row.success_rate) }}
                </Badge>
                <div class="text-[11px] tabular-nums text-muted-foreground">
                  {{ formatLatency(row.avg_response_time_ms) }} / {{ formatLatency(row.avg_first_byte_time_ms) }}
                </div>
              </div>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </TableCard>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { AnalyticsBreakdownRow } from '@/api/analytics'
import { EmptyState, LoadingState } from '@/components/common'
import {
  Badge,
  Table,
  TableBody,
  TableCard,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsList,
  TabsTrigger,
} from '@/components/ui'
import { formatCurrency, formatHitRate, formatNumber, formatTokens } from '@/utils/format'

interface BreakdownTabOption {
  value: string
  label: string
}

interface Props {
  title: string
  description: string
  entityLabel: string
  rows: AnalyticsBreakdownRow[]
  tabs?: BreakdownTabOption[]
  activeTab?: string
  loading?: boolean
  hasLoaded?: boolean
  error?: boolean
  showActualCost?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  tabs: () => [],
  activeTab: '',
  loading: false,
  hasLoaded: false,
  error: false,
  showActualCost: false,
})

const emit = defineEmits<{
  (e: 'update:activeTab', value: string): void
}>()

const showLoadingState = computed(() => props.loading && (!props.hasLoaded || props.rows.length === 0))
const showUnavailableState = computed(() => props.error && !props.loading && props.rows.length === 0)
const isRefreshing = computed(() => props.loading && props.hasLoaded && props.rows.length > 0)
const tabs = computed(() => props.tabs ?? [])
const activeTabValue = computed(() => props.activeTab || tabs.value[0]?.value || '')

function formatLatency(value: number | null | undefined): string {
  if (typeof value !== 'number' || Number.isNaN(value) || value <= 0) {
    return '-'
  }
  if (value < 1000) {
    return `${Math.round(value)}ms`
  }
  return `${(value / 1000).toFixed(value >= 10000 ? 1 : 2)}s`
}

function handleTabChange(value: string | number) {
  if (typeof value === 'string') {
    emit('update:activeTab', value)
  }
}

function secondaryCostText(row: AnalyticsBreakdownRow): string {
  if (props.showActualCost) {
    return formatCurrency(row.actual_total_cost_usd)
  }
  return `${row.share_of_total_cost.toFixed(2)}%`
}

function getEfficiencyValue(tokens: number, cost: number): number | null {
  if (tokens <= 0) return null
  return (cost * 1000000) / tokens
}

function formatEfficiency(tokens: number, cost: number): string {
  const value = getEfficiencyValue(tokens, cost)
  if (value == null || Number.isNaN(value)) return '--'
  if (value <= 0 || value < 0.00005) return '$0/M'
  return `${formatCurrency(value)}/M`
}

function successRateClass(rate: number): string {
  if (rate >= 97) {
    return 'border-emerald-500/35 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
  }
  if (rate >= 90) {
    return 'border-amber-500/35 bg-amber-500/10 text-amber-700 dark:text-amber-300'
  }
  return 'border-rose-500/35 bg-rose-500/10 text-rose-700 dark:text-rose-300'
}
</script>
