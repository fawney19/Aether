<template>
  <Card class="overflow-hidden flex flex-col">
    <div class="px-3 py-2 border-b flex-shrink-0">
      <h3 class="text-sm font-medium">
        按API格式分析
      </h3>
    </div>
    <div class="overflow-auto max-h-[320px]">
      <Table class="text-sm">
        <TableHeader>
          <TableRow>
            <TableHead class="h-8 px-2">
              API格式
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              请求数
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              Tokens
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              费用
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              缓存Token
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              缓存命中率
            </TableHead>
            <TableHead class="h-8 px-2 text-right">
              平均响应
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="data.length === 0">
            <TableCell
              :colspan="7"
              class="text-center py-6 text-muted-foreground px-2"
            >
              暂无API格式统计数据
            </TableCell>
          </TableRow>
          <TableRow
            v-for="item in data"
            :key="item.api_format"
          >
            <TableCell class="font-medium py-2 px-2">
              {{ formatApiFormat(item.api_format) }}
            </TableCell>
            <TableCell class="text-right py-2 px-2">
              {{ item.request_count }}
            </TableCell>
            <TableCell class="text-right py-2 px-2">
              <span>{{ formatTokens(item.total_tokens) }}</span>
            </TableCell>
            <TableCell class="text-right py-2 px-2">
              <div class="flex flex-col items-end text-xs gap-0.5">
                <span class="text-primary font-medium">{{ formatCurrency(item.total_cost) }}</span>
                <span
                  v-if="isAdmin && item.actual_cost !== undefined"
                  class="text-muted-foreground text-[10px]"
                >
                  {{ formatCurrency(item.actual_cost) }}
                </span>
              </div>
            </TableCell>
            <TableCell class="text-right py-2 px-2">
              {{ formatTokens(item.cache_read_tokens || 0) }}
            </TableCell>
            <TableCell class="text-right py-2 px-2 text-muted-foreground">
              {{ formatHitRate(item.cache_hit_rate) }}
            </TableCell>
            <TableCell class="text-right text-muted-foreground py-2 px-2">
              {{ item.avgResponseTime }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </Card>
</template>

<script setup lang="ts">
import Card from '@/components/ui/card.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableCell from '@/components/ui/table-cell.vue'
import { formatTokens, formatCurrency } from '@/utils/format'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { ApiFormatStatsItem } from '../types'

defineProps<{
  data: ApiFormatStatsItem[]
  isAdmin: boolean
}>()

function formatHitRate(rate: number | undefined): string {
  if (typeof rate !== 'number' || Number.isNaN(rate)) return '-'
  return `${rate.toFixed(2)}%`
}

</script>
