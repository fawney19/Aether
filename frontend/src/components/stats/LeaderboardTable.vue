<template>
  <TableCard :title="title">
    <template #actions>
      <div
        v-if="isRefreshing"
        class="inline-flex h-7 items-center rounded-lg border border-border/70 bg-background px-2.5 text-[10px] text-muted-foreground"
      >
        更新中
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
        :title="unavailableTitle"
        :description="unavailableDescription"
      />
    </div>
    <div
      v-else-if="items.length === 0"
      class="p-6"
    >
      <EmptyState
        title="暂无数据"
        description="当前时间范围内没有统计结果"
      />
    </div>
    <Table
      v-else
      class="table-fixed"
      :class="{ 'opacity-60 transition-opacity': isRefreshing }"
    >
      <TableHeader>
        <TableRow>
          <TableHead class="w-14 px-2.5">
            排名
          </TableHead>
          <TableHead class="w-[34%] px-2.5">
            名称
          </TableHead>
          <TableHead class="w-[15%] px-2.5 text-right">
            请求数
          </TableHead>
          <TableHead class="w-[18%] px-2.5 text-right">
            Tokens
          </TableHead>
          <TableHead class="w-[16%] px-2.5 text-right">
            费用
          </TableHead>
          <TableHead class="w-[17%] px-2.5 text-right">
            成本
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="item in items"
          :key="item.id"
          class="cursor-pointer"
          :class="item.id === selectedId
            ? 'bg-[linear-gradient(90deg,rgba(232,145,89,0.16),rgba(232,145,89,0.05),transparent)] shadow-[inset_4px_0_0_0_rgba(232,145,89,0.9)] hover:bg-[linear-gradient(90deg,rgba(232,145,89,0.2),rgba(232,145,89,0.06),transparent)]'
            : 'hover:bg-muted/25'"
          @click="emit('select', item.id)"
        >
          <TableCell class="px-2.5 py-2.5">
            <div
              class="inline-flex h-7 min-w-7 items-center justify-center rounded-full border px-2 text-[11px] font-semibold tabular-nums transition-colors"
              :class="item.id === selectedId
                ? 'border-amber-500/70 bg-amber-500/15 text-amber-700 dark:text-amber-300'
                : 'border-border/70 bg-muted/35 text-muted-foreground'"
            >
              {{ item.rank }}
            </div>
          </TableCell>
          <TableCell class="px-2.5 py-2.5">
            <div class="min-w-0">
              <div
                class="truncate text-sm font-medium"
                :title="item.name"
              >
                {{ item.name }}
              </div>
              <div class="mt-0.5 truncate text-[10px] text-muted-foreground">
                费用优先
              </div>
            </div>
          </TableCell>
          <TableCell class="px-2.5 py-2.5 text-right text-[11px] tabular-nums">
            {{ item.requests.toLocaleString() }}
          </TableCell>
          <TableCell class="px-2.5 py-2.5 text-right text-[11px] tabular-nums">
            {{ formatTokens(item.tokens) }}
          </TableCell>
          <TableCell class="px-2.5 py-2.5 text-right text-[11px] font-medium tabular-nums text-amber-700 dark:text-amber-300">
            {{ formatCurrency(item.cost) }}
          </TableCell>
          <TableCell class="px-2.5 py-2.5 text-right text-[11px] tabular-nums text-muted-foreground">
            {{ formatCurrency(item.actualCost) }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <template v-if="showPagination" #pagination>
      <Pagination
        :current="currentPage"
        :total="totalItems"
        :page-size="pageSize"
        :show-page-size-selector="false"
        @update:current="emit('update:currentPage', $event)"
      />
    </template>
  </TableCard>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { EmptyState, LoadingState } from '@/components/common'
import { Pagination, Table, TableBody, TableCell, TableHead, TableHeader, TableRow, TableCard } from '@/components/ui'
import { formatCurrency, formatTokens } from '@/utils/format'

interface LeaderboardItem {
  rank: number
  id: string
  name: string
  requests: number
  tokens: number
  cost: number
  actualCost: number
}

interface Props {
  title: string
  items: LeaderboardItem[]
  selectedId?: string
  loading?: boolean
  hasLoaded?: boolean
  currentPage?: number
  totalItems?: number
  pageSize?: number
  showPagination?: boolean
  unavailable?: boolean
  unavailableTitle?: string
  unavailableDescription?: string
}

const props = withDefaults(defineProps<Props>(), {
  selectedId: undefined,
  loading: false,
  hasLoaded: false,
  currentPage: 1,
  totalItems: 0,
  pageSize: 5,
  showPagination: false,
  unavailable: false,
  unavailableTitle: '排行榜暂不可用',
  unavailableDescription: '接口未返回结果，请稍后重试',
})

const emit = defineEmits<{
  (e: 'select', value: string): void
  (e: 'update:currentPage', value: number): void
}>()

const selectedId = computed(() => props.selectedId)
const showLoadingState = computed(() => props.loading && (!props.hasLoaded || props.items.length === 0))
const showUnavailableState = computed(() => props.unavailable && !props.loading && props.items.length === 0)
const isRefreshing = computed(() => props.loading && props.hasLoaded && props.items.length > 0)
const currentPage = computed(() => props.currentPage)
const totalItems = computed(() => props.totalItems)
const pageSize = computed(() => props.pageSize)
const showPagination = computed(() => props.showPagination && props.totalItems > props.pageSize)
</script>
