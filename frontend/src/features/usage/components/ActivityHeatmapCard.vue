<template>
  <Card class="p-4 overflow-hidden">
    <div class="flex items-center justify-between mb-3">
      <p class="text-sm font-semibold">
        {{ title }}
      </p>
      <div
        v-if="hasData"
        class="flex items-center gap-1 text-[11px] text-muted-foreground flex-shrink-0"
      >
        <span class="flex-shrink-0">少</span>
        <div
          v-for="(level, index) in legendLevels"
          :key="index"
          class="w-3 h-3 rounded-[3px] flex-shrink-0"
          :style="{ backgroundColor: `rgba(var(--color-primary-rgb), ${level})` }"
        />
        <span class="flex-shrink-0">多</span>
      </div>
    </div>
    <div
      v-if="isLoading"
      class="flex items-center justify-center text-sm text-muted-foreground"
      :style="{ minHeight: `${minHeight}px` }"
    >
      <Loader2 class="h-5 w-5 animate-spin mr-2" />
      加载中...
    </div>
    <div
      v-else-if="hasError"
      class="flex items-center justify-center text-sm text-destructive"
      :style="{ minHeight: `${minHeight}px` }"
    >
      <AlertCircle class="h-4 w-4 mr-1.5" />
      数据暂不可用
    </div>
    <ActivityHeatmap
      v-else-if="hasData"
      :data="data"
      :show-header="false"
      :value-label="valueLabel"
      :actual-value-label="actualValueLabel"
    />
    <div
      v-else
      class="flex items-center justify-center text-sm text-muted-foreground"
      :style="{ minHeight: `${minHeight}px` }"
    >
      暂无活跃数据
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Loader2, AlertCircle } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import ActivityHeatmap from '@/components/stats/ActivityHeatmap.vue'
import type { ActivityHeatmap as ActivityHeatmapData } from '@/types/activity'

const props = defineProps<{
  data: ActivityHeatmapData | null
  title: string
  isLoading?: boolean
  hasError?: boolean
  minHeight?: number
  valueLabel?: string
  actualValueLabel?: string
}>()

const legendLevels = [0.08, 0.25, 0.45, 0.65, 0.85]

const hasData = computed(() =>
  props.data && props.data.days && props.data.days.length > 0
)

const minHeight = computed(() => props.minHeight ?? 160)
</script>
