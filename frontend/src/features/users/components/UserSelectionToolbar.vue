<template>
  <div class="flex flex-col gap-2 border-b border-border/60 bg-muted/20 px-4 py-2.5 text-xs sm:flex-row sm:items-center sm:justify-between sm:px-6 xl:px-4">
    <div class="flex flex-wrap items-center gap-2 text-muted-foreground">
      <label class="flex items-center gap-2">
        <Checkbox
          :checked="isAllFilteredSelected"
          :indeterminate="isPartiallyFilteredSelected"
          :disabled="filteredUserCount === 0 || loading"
          @update:checked="handleToggleSelectFiltered"
        />
        <span>{{ legacyT('全选筛选结果') }}</span>
      </label>
      <span>{{ selectionSummary }}</span>
    </div>
    <div class="flex flex-wrap items-center gap-1.5">
      <Button
        variant="ghost"
        size="sm"
        class="h-7 px-2 text-[11px]"
        :disabled="currentPageCount === 0 || selectAllFiltered || loading"
        @click="$emit('toggleSelectCurrentPage')"
      >
        {{ legacyT(isCurrentPageFullySelected ? '取消本页全选' : '本页全选') }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        class="h-7 px-2 text-[11px]"
        :disabled="!canClearSelection || loading"
        @click="$emit('clearSelection')"
      >
        {{ legacyT('清空选择') }}
      </Button>
      <Button
        v-if="canOperateAdmin"
        size="sm"
        class="h-7 px-3 text-[11px]"
        :disabled="(selectedCount === 0 && groupCount === 0) || loading"
        @click="$emit('openBatchDialog')"
      >
        {{ legacyT('批量操作') }}
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Button from '@/components/ui/button.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import { useI18n } from '@/i18n'

const props = defineProps<{
  isAllFilteredSelected: boolean
  isPartiallyFilteredSelected: boolean
  filteredUserCount: number
  currentPageCount: number
  selectedCount: number
  isCurrentPageFullySelected: boolean
  canClearSelection: boolean
  selectAllFiltered: boolean
  loading: boolean
  canOperateAdmin: boolean
  groupCount: number
}>()

const emit = defineEmits<{
  toggleSelectFiltered: [checked: boolean]
  toggleSelectCurrentPage: []
  clearSelection: []
  openBatchDialog: []
}>()

const { legacyT } = useI18n()

const selectionSummary = computed(() => {
  return legacyT(`匹配 ${props.filteredUserCount} 个，当前页 ${props.currentPageCount} 个，已选 ${props.selectedCount} 个`)
})

function handleToggleSelectFiltered(value: boolean | 'indeterminate') {
  emit('toggleSelectFiltered', value === true)
}
</script>
