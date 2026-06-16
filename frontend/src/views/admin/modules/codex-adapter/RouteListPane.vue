<template>
  <Card class="overflow-hidden">
    <div class="border-b border-border/60 px-4 py-3">
      <div class="flex items-start justify-between gap-3">
        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <h2 class="text-sm font-semibold">
              路由规则
            </h2>
            <Badge variant="outline">
              {{ routes.length }}
            </Badge>
          </div>
          <p class="mt-1 line-clamp-2 text-xs text-muted-foreground">
            {{ statusText }}
          </p>
        </div>
        <Badge
          v-if="hasChanges"
          variant="secondary"
          class="shrink-0"
        >
          未保存
        </Badge>
      </div>

      <div class="mt-2 flex flex-wrap gap-2 text-[11px] text-muted-foreground">
        <span class="inline-flex items-center rounded-md border border-border/60 bg-muted/20 px-2 py-1">
          {{ globalModelCount }} 个全局模型
        </span>
      </div>
    </div>

    <div class="p-3">
      <Button
        class="mb-2 w-full"
        size="sm"
        variant="outline"
        @click="$emit('add')"
      >
        <Plus class="mr-2 h-4 w-4" />
        新增路由
      </Button>

      <div
        v-if="loading"
        class="py-10 text-center text-sm text-muted-foreground"
      >
        正在加载 Codex 路由
      </div>

      <div
        v-else-if="routes.length === 0"
        class="rounded-lg border border-dashed border-border/70 px-4 py-8 text-center"
      >
        <p class="text-sm font-medium">
          还没有路由
        </p>
        <p class="mt-1 text-xs text-muted-foreground">
          先新增一条 Codex 请求模型映射
        </p>
      </div>

      <div
        v-else
        class="max-h-[calc(100vh-16rem)] space-y-2 overflow-y-auto pr-1"
      >
        <button
          v-for="(route, routeIndex) in routes"
          :key="`route-${routeIndex}`"
          type="button"
          class="w-full rounded-lg border px-3 py-2.5 text-left transition-colors"
          :class="routeIndex === selectedIndex
            ? 'border-primary/60 bg-primary/10'
            : 'border-border/60 bg-background hover:border-primary/40 hover:bg-muted/50'"
          @click="$emit('select', routeIndex)"
        >
          <div class="flex items-center justify-between gap-3">
            <div class="min-w-0">
              <p class="truncate text-sm font-medium">
                {{ route.codex_model || `未命名路由 ${routeIndex + 1}` }}
              </p>
              <p class="mt-1 text-[11px] text-muted-foreground">
                {{ schedulingModeLabels[route.scheduling_mode] }} · {{ route.candidates.length }} 个候选
              </p>
            </div>
            <span
              class="h-2.5 w-2.5 shrink-0 rounded-full"
              :class="route.enabled ? 'bg-emerald-500' : 'bg-muted-foreground/40'"
            />
          </div>
        </button>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { Badge, Button, Card } from '@/components/ui'
import { Plus } from 'lucide-vue-next'
import type { CodexAdapterRouteConfig, CodexAdapterSchedulingMode } from '@/api/modules'

defineProps<{
  readonly globalModelCount: number
  readonly hasChanges: boolean
  readonly loading: boolean
  readonly routes: readonly CodexAdapterRouteConfig[]
  readonly selectedIndex: number
  readonly statusText: string
}>()

defineEmits<{
  add: []
  select: [routeIndex: number]
}>()

const schedulingModeLabels: Readonly<Record<CodexAdapterSchedulingMode, string>> = {
  priority: '优先级',
  sticky: '粘性',
  load_balance: '负载均衡',
}
</script>
