<template>
  <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/50">
    <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
      <!-- 左侧：标题 -->
      <h3 class="text-sm sm:text-base font-semibold text-foreground shrink-0">
        提供商管理
      </h3>

      <!-- 右侧：操作区 -->
      <div class="flex flex-wrap items-center gap-2">
        <!-- 搜索框 -->
        <div class="relative">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/70 z-10 pointer-events-none" />
          <Input
            id="provider-search"
            :model-value="searchQuery"
            type="text"
            placeholder="搜索提供商..."
            class="w-32 sm:w-44 pl-8 pr-3 h-8 text-sm bg-muted/30 border-border/50 focus:border-primary/50 transition-colors"
            @update:model-value="$emit('update:searchQuery', $event)"
          />
        </div>

        <!-- 状态筛选 -->
        <div class="xl:hidden">
          <Select
            :model-value="filterStatus"
            @update:model-value="$emit('update:filterStatus', $event)"
          >
            <SelectTrigger class="w-20 sm:w-28 h-8 text-xs border-border/60">
              <SelectValue placeholder="全部状态" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="status in statusFilters"
                :key="status.value"
                :value="status.value"
              >
                {{ status.label }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <!-- API 格式筛选 -->
        <div class="xl:hidden">
          <Select
            :model-value="filterApiFormat"
            @update:model-value="$emit('update:filterApiFormat', $event)"
          >
            <SelectTrigger class="w-20 sm:w-28 h-8 text-xs border-border/60">
              <SelectValue placeholder="全部格式" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="fmt in apiFormatFilters"
                :key="fmt.value"
                :value="fmt.value"
              >
                {{ fmt.label }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <!-- 模型筛选 -->
        <div class="xl:hidden">
          <Select
            :model-value="filterModel"
            @update:model-value="$emit('update:filterModel', $event)"
          >
            <SelectTrigger class="w-20 sm:w-36 h-8 text-xs border-border/60">
              <SelectValue placeholder="全部模型" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="model in modelFilters"
                :key="model.value"
                :value="model.value"
              >
                {{ model.label }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <!-- 重置筛选 -->
        <Button
          v-if="hasActiveFilters"
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          title="重置筛选"
          @click="$emit('resetFilters')"
        >
          <FilterX class="w-3.5 h-3.5" />
        </Button>

        <div class="hidden sm:block h-4 w-px bg-border" />

        <!-- 调度策略 -->
        <button
          class="group inline-flex items-center gap-1.5 px-2.5 h-8 rounded-md border border-border/50 bg-muted/20 hover:bg-muted/40 hover:border-primary/40 transition-all duration-200 text-xs"
          title="点击调整调度策略"
          @click="$emit('openPriorityDialog')"
        >
          <span class="text-muted-foreground/80 hidden sm:inline">调度:</span>
          <span class="font-medium text-foreground/90">{{ priorityModeLabel }}</span>
          <ChevronDown class="w-3 h-3 text-muted-foreground/70 group-hover:text-foreground transition-colors" />
        </button>

        <div class="hidden sm:block h-4 w-px bg-border" />

        <!-- 操作按钮 -->
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          title="新增提供商"
          @click="$emit('addProvider')"
        >
          <Plus class="w-3.5 h-3.5" />
        </Button>
        <RefreshButton
          :loading="loading"
          @click="$emit('refresh')"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { Search, Plus, ChevronDown, FilterX } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import type { FilterOption } from '@/features/providers/composables/useProviderFilters'

defineProps<{
  searchQuery: string
  filterStatus: string
  filterApiFormat: string
  filterModel: string
  statusFilters: FilterOption[]
  apiFormatFilters: FilterOption[]
  modelFilters: FilterOption[]
  hasActiveFilters: boolean
  priorityModeLabel: string
  loading: boolean
}>()

defineEmits<{
  'update:searchQuery': [value: string]
  'update:filterStatus': [value: string]
  'update:filterApiFormat': [value: string]
  'update:filterModel': [value: string]
  'resetFilters': []
  'openPriorityDialog': []
  'addProvider': []
  'refresh': []
}>()
</script>
