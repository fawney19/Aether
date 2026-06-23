<template>
  <Card class="overflow-hidden">
    <template v-if="route">
      <div class="border-b border-border/60 px-4 py-3">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="truncate text-sm font-semibold">
                {{ route.codex_model || `路由 ${routeIndex + 1}` }}
              </h2>
              <Badge :variant="route.enabled ? 'default' : 'secondary'">
                {{ route.enabled ? '启用' : '停用' }}
              </Badge>
            </div>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ schedulingModeHelp[route.scheduling_mode] }}
            </p>
          </div>

          <Button
            class="h-8 w-8 text-destructive"
            size="icon"
            title="删除路由"
            variant="ghost"
            @click="emit('removeRoute', routeIndex)"
          >
            <Trash2 class="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div class="space-y-4 p-4">
        <section class="grid gap-2.5 lg:grid-cols-[minmax(0,1.5fr)_180px_132px]">
          <label class="space-y-1 text-sm">
            <span class="text-xs text-muted-foreground">Codex 请求模型</span>
            <Input
              :model-value="route.codex_model"
              class="font-mono"
              placeholder="例如 gpt-5.5"
              size="sm"
              @update:model-value="(value) => updateRoute({ codex_model: String(value) })"
            />
          </label>

          <label class="space-y-1 text-sm">
            <span class="text-xs text-muted-foreground">调度策略</span>
            <Select
              :model-value="route.scheduling_mode"
              @update:model-value="(value) => updateRoute({ scheduling_mode: normalizeSchedulingMode(String(value)) })"
            >
              <SelectTrigger class="h-8 rounded-lg px-3 text-xs">
                <SelectValue placeholder="选择调度策略" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="option in schedulingModeOptions"
                  :key="option.value"
                  :value="option.value"
                >
                  {{ option.label }}
                </SelectItem>
              </SelectContent>
            </Select>
          </label>

          <div class="space-y-1">
            <span class="block text-xs text-muted-foreground">路由启用</span>
            <div class="flex h-8 items-center justify-between rounded-lg border border-border/60 bg-muted/20 px-3">
              <span class="text-xs text-muted-foreground">参与调度</span>
              <Switch
                :model-value="route.enabled"
                @update:model-value="(value: boolean) => updateRoute({ enabled: value })"
              />
            </div>
          </div>
        </section>

        <RouteCandidateTable
          :candidates="route.candidates"
          :global-model-options="globalModelOptions"
          :has-catalog-global-models="hasCatalogGlobalModels"
          :route-index="routeIndex"
          @add="(nextRouteIndex) => emit('addCandidate', nextRouteIndex)"
          @move="(nextRouteIndex, candidateIndex, direction) => emit('moveCandidate', nextRouteIndex, candidateIndex, direction)"
          @remove="(nextRouteIndex, candidateIndex) => emit('removeCandidate', nextRouteIndex, candidateIndex)"
          @update="(nextRouteIndex, candidateIndex, patch) => emit('updateCandidate', nextRouteIndex, candidateIndex, patch)"
        />
      </div>
    </template>

    <div
      v-else
      class="flex min-h-[280px] items-center justify-center px-6 py-10"
    >
      <div class="max-w-sm text-center">
        <h2 class="text-sm font-semibold">
          先选择一条路由
        </h2>
        <p class="mt-2 text-sm text-muted-foreground">
          左侧只保留概览，右侧集中编辑当前项，避免把页面拉成长表单。
        </p>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { Trash2 } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@/components/ui'
import type {
  CodexAdapterCandidateConfig,
  CodexAdapterRouteConfig,
  CodexAdapterSchedulingMode,
} from '@/api/modules'
import {
  normalizeCodexAdapterSchedulingMode as normalizeSchedulingMode,
  type CodexAdapterGlobalModelOption,
} from '../codex-adapter-support'
import RouteCandidateTable from './RouteCandidateTable.vue'

const props = defineProps<{
  readonly globalModelOptions: readonly CodexAdapterGlobalModelOption[]
  readonly hasCatalogGlobalModels: boolean
  readonly route: CodexAdapterRouteConfig | null
  readonly routeIndex: number
  readonly schedulingModeHelp: Readonly<Record<CodexAdapterSchedulingMode, string>>
  readonly schedulingModeOptions: ReadonlyArray<{
    readonly value: CodexAdapterSchedulingMode
    readonly label: string
  }>
}>()

const emit = defineEmits<{
  addCandidate: [routeIndex: number]
  moveCandidate: [routeIndex: number, candidateIndex: number, direction: -1 | 1]
  removeCandidate: [routeIndex: number, candidateIndex: number]
  removeRoute: [routeIndex: number]
  updateCandidate: [routeIndex: number, candidateIndex: number, patch: Partial<CodexAdapterCandidateConfig>]
  updateRoute: [routeIndex: number, patch: Partial<CodexAdapterRouteConfig>]
}>()

function updateRoute(patch: Partial<CodexAdapterRouteConfig>): void {
  if (!props.route) return
  emit('updateRoute', props.routeIndex, patch)
}
</script>
