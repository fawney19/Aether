<template>
  <Card
    v-if="progress"
    class="border-primary/30 bg-primary/5"
  >
    <div class="px-5 py-4 space-y-4">
      <div class="flex items-start justify-between gap-4">
        <div class="min-w-0">
          <div class="text-sm font-semibold text-foreground">
            {{ legacyT('正在删除提供商') }}: {{ progress.providerName }}
          </div>
          <div class="mt-1 text-xs text-muted-foreground">
            {{ stageLabel }} · {{ legacyT(progress.message || '后台处理中') }}
          </div>
        </div>
        <div class="shrink-0 text-right">
          <div class="text-xs font-medium text-primary">
            {{ overallPercent }}%
          </div>
          <div class="text-[11px] text-muted-foreground">
            {{ completedUnits }}/{{ totalUnits }}
          </div>
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex items-center justify-between text-xs text-muted-foreground">
          <span>{{ legacyT('总体进度') }}</span>
          <span>{{ completedUnits }}/{{ totalUnits }}</span>
        </div>
        <div class="h-2 rounded-full bg-primary/10 overflow-hidden">
          <div
            class="h-full bg-primary transition-all duration-300"
            :style="{ width: `${overallPercent}%` }"
          />
        </div>
      </div>

      <div class="grid gap-3 md:grid-cols-2">
        <div class="space-y-2">
          <div class="flex items-center justify-between text-xs text-muted-foreground">
            <span>{{ legacyT('账号删除') }}</span>
            <span>{{ progress.deletedKeys }}/{{ progress.totalKeys || '...' }}</span>
          </div>
          <div class="h-2 rounded-full bg-primary/10 overflow-hidden">
            <div
              class="h-full bg-primary/80 transition-all duration-300"
              :style="{ width: `${keysPercent}%` }"
            />
          </div>
        </div>

        <div class="space-y-2">
          <div class="flex items-center justify-between text-xs text-muted-foreground">
            <span>{{ legacyT('端点删除') }}</span>
            <span>{{ progress.deletedEndpoints }}/{{ progress.totalEndpoints || '...' }}</span>
          </div>
          <div class="h-2 rounded-full bg-primary/10 overflow-hidden">
            <div
              class="h-full bg-primary/60 transition-all duration-300"
              :style="{ width: `${endpointsPercent}%` }"
            />
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import Card from '@/components/ui/card.vue'
import { useI18n } from '@/i18n'

export interface ProviderDeleteProgressView {
  providerName: string
  totalKeys: number
  deletedKeys: number
  totalEndpoints: number
  deletedEndpoints: number
  message: string
}

defineProps<{
  progress: ProviderDeleteProgressView | null
  stageLabel: string
  totalUnits: number
  completedUnits: number
  overallPercent: number
  keysPercent: number
  endpointsPercent: number
}>()

const { legacyT } = useI18n()
</script>
