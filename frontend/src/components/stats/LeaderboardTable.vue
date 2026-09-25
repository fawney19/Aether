<template>
  <TableCard :title="title">
    <template #actions>
      <slot name="actions">
        <Select
          v-if="showMetricSelect"
          :model-value="metric"
          @update:model-value="emitMetric"
        >
          <SelectTrigger class="h-8 text-xs w-28">
            <SelectValue :placeholder="t('stats.metric.placeholder')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="requests">
              {{ t('stats.metric.requests') }}
            </SelectItem>
            <SelectItem value="tokens">
              {{ t('stats.metric.tokens') }}
            </SelectItem>
            <SelectItem value="cost">
              {{ t('stats.metric.cost') }}
            </SelectItem>
          </SelectContent>
        </Select>
      </slot>
    </template>

    <div
      v-if="loading"
      class="p-6"
    >
      <LoadingState />
    </div>
    <div
      v-else-if="items.length === 0"
      class="p-6"
    >
      <EmptyState
        :title="t('stats.empty.title')"
        :description="t('stats.empty.description')"
      />
    </div>
    <Table v-else>
      <TableHeader>
        <TableRow>
          <TableHead class="w-16">
            {{ t('stats.column.rank') }}
          </TableHead>
          <TableHead>{{ t('stats.column.name') }}</TableHead>
          <TableHead
            v-if="showMemberCount"
            class="text-right"
          >
            {{ t('stats.column.members') }}
          </TableHead>
          <TableHead class="text-right">
            {{ t('stats.metric.requests') }}
          </TableHead>
          <TableHead class="text-right">
            {{ t('stats.metric.tokens') }}
          </TableHead>
          <TableHead class="text-right">
            {{ t('stats.metric.cost') }}
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="item in items"
          :key="item.id"
          :class="selectable ? 'cursor-pointer hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring' : undefined"
          :tabindex="selectable ? 0 : undefined"
          @click="selectable && emit('select', item)"
          @keydown.enter.prevent="selectable && emit('select', item)"
          @keydown.space.prevent="selectable && emit('select', item)"
        >
          <TableCell class="font-medium">
            {{ item.rank }}
          </TableCell>
          <TableCell>{{ showMemberCount && item.id === '__ungrouped__' ? t('userStats.ungrouped') : item.name }}</TableCell>
          <TableCell
            v-if="showMemberCount"
            class="text-right"
          >
            {{ item.active_member_count ?? 0 }} / {{ item.member_count ?? 0 }}
          </TableCell>
          <TableCell class="text-right">
            {{ item.requests }}
          </TableCell>
          <TableCell class="text-right">
            {{ formatTokens(item.tokens) }}
          </TableCell>
          <TableCell class="text-right">
            {{ formatCurrency(item.cost) }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <slot name="pagination" />
  </TableCard>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { EmptyState, LoadingState } from '@/components/common'
import { TableCard } from '@/components/ui'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from '@/components/ui'
import { formatCurrency, formatTokens } from '@/utils/format'
import { useI18n } from '@/i18n'
import type { LeaderboardItem } from '@/api/admin'

interface Props {
  title: string
  items: LeaderboardItem[]
  metric: 'requests' | 'tokens' | 'cost'
  loading?: boolean
  showMetricSelect?: boolean
  showMemberCount?: boolean
  selectable?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  loading: false,
  showMetricSelect: true,
  showMemberCount: false,
  selectable: false
})

const emit = defineEmits<{
  (e: 'update:metric', value: 'requests' | 'tokens' | 'cost'): void
  (e: 'select', value: LeaderboardItem): void
}>()

const { t } = useI18n()

const metric = computed(() => props.metric)

function emitMetric(value: string) {
  if (value === 'requests' || value === 'tokens' || value === 'cost') {
    emit('update:metric', value)
  }
}
</script>
