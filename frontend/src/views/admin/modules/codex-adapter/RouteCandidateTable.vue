<template>
  <section class="rounded-lg border border-border/60">
    <div class="flex items-center justify-between gap-3 border-b border-border/60 px-4 py-3">
      <div class="flex min-w-0 items-center gap-2">
        <h3 class="text-sm font-medium">
          候选模型
        </h3>
        <Badge variant="outline">
          {{ candidates.length }}
        </Badge>
      </div>

      <Button
        size="sm"
        variant="outline"
        @click="$emit('add', routeIndex)"
      >
        <Plus class="mr-2 h-4 w-4" />
        新增候选
      </Button>
    </div>

    <div class="space-y-2 p-3">
      <div
        v-if="!hasCatalogGlobalModels"
        class="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800"
      >
        当前还没有可选的全局模型，请先到「模型管理」创建 Global Model。
      </div>

      <div
        v-if="candidates.length === 0"
        class="rounded-lg border border-dashed border-border/70 px-4 py-6 text-center"
      >
        <p class="text-sm font-medium">
          还没有候选模型
        </p>
        <p class="mt-1 text-xs text-muted-foreground">
          新增后即可按优先级和顺序兜底。
        </p>
      </div>

      <template v-else>
        <div class="hidden items-center gap-3 px-3 text-[11px] text-muted-foreground lg:grid lg:grid-cols-[minmax(0,1.6fr)_72px_84px_76px_92px]">
          <span>全局模型</span>
          <span>优先级</span>
          <span>权重</span>
          <span>启用</span>
          <span class="text-right">操作</span>
        </div>

        <div
          v-for="(candidate, candidateIndex) in candidates"
          :key="`candidate-${candidateIndex}`"
          class="rounded-md border border-border/60 bg-background px-3 py-2.5"
        >
          <div class="grid gap-2.5 lg:grid-cols-[minmax(0,1.6fr)_72px_84px_76px_92px] lg:items-start">
            <div class="space-y-1">
              <label class="text-[11px] text-muted-foreground lg:hidden">全局模型</label>
              <Select
                :model-value="candidate.global_model"
                @update:model-value="(value) => updateCandidate(candidateIndex, { global_model: String(value) })"
              >
                <SelectTrigger class="h-8 rounded-lg px-3 text-xs">
                  <SelectValue placeholder="选择全局模型" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="option in globalModelOptions"
                    :key="option.value"
                    :disabled="option.value !== candidate.global_model && !option.compatible"
                    :value="option.value"
                  >
                    <div class="flex w-full items-center justify-between gap-3">
                      <span>{{ option.label }}</span>
                      <span
                        v-if="!option.compatible"
                        class="text-[11px] text-muted-foreground"
                      >
                        不兼容
                      </span>
                    </div>
                  </SelectItem>
                </SelectContent>
              </Select>
              <p
                v-if="compatibilitySummary(candidate.global_model)"
                class="text-[11px] text-destructive"
              >
                {{ compatibilitySummary(candidate.global_model) }}
              </p>
            </div>

            <label class="space-y-1 text-sm">
              <span class="text-[11px] text-muted-foreground lg:hidden">优先级</span>
              <Input
                :model-value="String(candidate.priority)"
                class="text-xs"
                size="sm"
                type="number"
                @update:model-value="(value) => updateCandidate(candidateIndex, { priority: toInteger(value, candidate.priority) })"
              />
            </label>

            <label class="space-y-1 text-sm">
              <span class="text-[11px] text-muted-foreground lg:hidden">权重</span>
              <Input
                :model-value="String(candidate.weight)"
                class="text-xs"
                min="1"
                size="sm"
                type="number"
                @update:model-value="(value) => updateCandidate(candidateIndex, { weight: toPositiveInteger(value, candidate.weight) })"
              />
            </label>

            <div class="flex h-8 items-center justify-between rounded-lg border border-border/60 bg-muted/20 px-2.5 text-xs lg:px-3">
              <span class="text-[11px] text-muted-foreground lg:hidden">启用</span>
              <Switch
                :model-value="candidate.enabled"
                @update:model-value="(value: boolean) => updateCandidate(candidateIndex, { enabled: value })"
              />
            </div>

            <div class="flex items-center justify-end gap-1">
              <Button
                :disabled="candidateIndex === 0"
                class="h-8 w-8"
                size="icon"
                title="上移"
                variant="ghost"
                @click="$emit('move', routeIndex, candidateIndex, -1)"
              >
                <ArrowUp class="h-4 w-4" />
              </Button>
              <Button
                :disabled="candidateIndex === candidates.length - 1"
                class="h-8 w-8"
                size="icon"
                title="下移"
                variant="ghost"
                @click="$emit('move', routeIndex, candidateIndex, 1)"
              >
                <ArrowDown class="h-4 w-4" />
              </Button>
              <Button
                class="h-8 w-8 text-destructive"
                size="icon"
                title="删除候选"
                variant="ghost"
                @click="$emit('remove', routeIndex, candidateIndex)"
              >
                <Trash2 class="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      </template>
    </div>
  </section>
</template>

<script setup lang="ts">
import { ArrowDown, ArrowUp, Plus, Trash2 } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@/components/ui'
import type { CodexAdapterCandidateConfig } from '@/api/modules'
import {
  toInteger,
  toPositiveInteger,
  type CodexAdapterGlobalModelOption,
} from '../codex-adapter-support'

const props = defineProps<{
  readonly candidates: readonly CodexAdapterCandidateConfig[]
  readonly globalModelOptions: readonly CodexAdapterGlobalModelOption[]
  readonly hasCatalogGlobalModels: boolean
  readonly routeIndex: number
}>()

const emit = defineEmits<{
  add: [routeIndex: number]
  move: [routeIndex: number, candidateIndex: number, direction: -1 | 1]
  remove: [routeIndex: number, candidateIndex: number]
  update: [routeIndex: number, candidateIndex: number, patch: Partial<CodexAdapterCandidateConfig>]
}>()

function updateCandidate(
  candidateIndex: number,
  patch: Partial<CodexAdapterCandidateConfig>,
): void {
  emit('update', props.routeIndex, candidateIndex, patch)
}

function compatibilitySummary(globalModel: string): string | null {
  const normalized = globalModel.trim()
  if (!normalized) return null
  const option = props.globalModelOptions.find((item) => item.value === normalized) ?? null
  if (!option || option.compatible) return null
  return option.summary ?? '无法承接 Responses 请求'
}
</script>
