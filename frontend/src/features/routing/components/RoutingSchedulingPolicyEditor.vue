<template>
  <section class="space-y-4">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h3 class="text-sm font-medium">
          调度配置
        </h3>
      </div>
      <Button
        v-if="scopeMode === 'selected'"
        type="button"
        variant="outline"
        size="sm"
        class="shrink-0 gap-1.5"
        :disabled="!canAddEntry"
        :title="addEntryHint"
        aria-label="添加调度配置"
        @click="addEntry"
      >
        <Plus class="h-3.5 w-3.5" />
        添加配置
      </Button>
    </div>

    <div
      role="group"
      aria-label="调度范围"
      class="scheduling-switch w-full grid-cols-2 sm:w-80"
    >
      <button
        v-for="mode in scopeModes"
        :key="mode.value"
        type="button"
        class="scheduling-switch__option"
        :aria-label="mode.label"
        :aria-pressed="scopeMode === mode.value"
        :disabled="disabled"
        @click="setScopeMode(mode.value)"
      >
        <component
          :is="mode.icon"
          class="h-4 w-4 shrink-0"
        />
        {{ mode.label }}
      </button>
    </div>

    <fieldset
      :disabled="disabled"
      :inert="disabled"
      class="min-w-0 space-y-3"
    >
      <section
        v-for="(entry, index) in entries"
        :key="entry.id"
        :class="scopeMode === 'selected' ? 'rounded-lg border border-border/60' : ''"
        :aria-label="`调度配置 ${index + 1}`"
      >
        <div
          v-if="scopeMode === 'selected'"
          class="flex items-center gap-3 px-4 py-3"
        >
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center gap-3 text-left"
            :aria-label="`${expandedId === entry.id ? '收起' : '展开'}调度配置 ${index + 1}`"
            :aria-expanded="expandedId === entry.id"
            @click="expandedId = expandedId === entry.id ? null : entry.id"
          >
            <ChevronDown
              class="h-4 w-4 shrink-0 text-muted-foreground transition-transform"
              :class="expandedId === entry.id ? 'rotate-180' : ''"
            />
            <span class="min-w-0">
              <span
                class="block truncate text-sm font-medium"
                :title="entry.scope === 'selected' ? entry.models.join('、') : '默认配置'"
              >
                {{ scopeSummary(entry) }}
              </span>
              <span class="mt-0.5 block text-xs text-muted-foreground">
                配置 {{ index + 1 }} · {{ entry.priorityMode === 'provider' ? 'Provider' : 'Key' }} · {{ schedulingModeLabel(entry.schedulingMode) }}
              </span>
            </span>
          </button>
          <Button
            v-if="entries.length > 1"
            type="button"
            variant="ghost"
            size="icon"
            class="h-8 w-8 shrink-0 text-muted-foreground hover:text-destructive"
            :aria-label="`删除调度配置 ${index + 1}`"
            @click="removeEntry(entry.id)"
          >
            <Trash2 class="h-4 w-4" />
          </Button>
        </div>

        <div
          v-if="scopeMode === 'all' || expandedId === entry.id"
          class="space-y-5"
          :class="scopeMode === 'selected' ? 'border-t border-border/60 p-4' : ''"
        >
          <div
            v-if="entry.scope === 'selected'"
            class="min-w-0"
          >
            <div class="min-w-0 space-y-2">
              <h4 class="text-sm font-medium">
                适用模型
              </h4>
              <RoutingModelSelector
                :model-value="entry.models"
                :models="globalModels"
                :assigned-models="otherModelOwners(entry.id)"
                :loading="loadingModels"
                :error="modelsError"
                :disabled="disabled"
                @update:model-value="models => updateEntry(entry.id, { models })"
                @reload="emit('reload-models')"
              />
            </div>
          </div>

          <div
            v-if="entry.scope === 'all' || entry.models.length > 0"
            class="space-y-4"
            :class="entry.scope === 'selected' ? 'border-t border-border/60 pt-4' : ''"
          >
            <h4 class="text-sm font-medium">
              调度设置
            </h4>
            <div class="grid grid-cols-1 gap-3 lg:grid-cols-2">
              <div class="space-y-1.5 text-sm">
                <span class="text-muted-foreground">调度优先级</span>
                <div
                  role="group"
                  aria-label="调度优先级"
                  class="scheduling-switch grid-cols-2"
                >
                  <button
                    v-for="mode in priorityModes"
                    :key="mode.value"
                    type="button"
                    class="scheduling-switch__option"
                    :aria-pressed="entry.priorityMode === mode.value"
                    :disabled="disabled"
                    @click="updateEntry(entry.id, { priorityMode: mode.value })"
                  >
                    <component
                      :is="mode.icon"
                      class="h-4 w-4"
                    />
                    {{ mode.label }}
                  </button>
                </div>
              </div>
              <div class="space-y-1.5 text-sm">
                <span class="text-muted-foreground">调度策略</span>
                <div
                  role="group"
                  aria-label="调度策略"
                  class="scheduling-switch grid-cols-3"
                >
                  <button
                    v-for="mode in schedulingModes"
                    :key="mode.value"
                    type="button"
                    class="scheduling-switch__option"
                    :aria-pressed="entry.schedulingMode === mode.value"
                    :disabled="disabled"
                    @click="updateEntry(entry.id, { schedulingMode: mode.value })"
                  >
                    {{ mode.label }}
                  </button>
                </div>
              </div>
            </div>
            <RoutingPriorityPolicyEditor
              :config="schedulingPolicyEditorConfig(config, entry)"
              :show-priority-mode="false"
              :show-scheduling-mode="false"
              subtitle="所选模型共用此排序，仅对各模型可用的候选生效"
              @update:config="value => updateEntry(entry.id, { policy: getDefaultModelPolicy(value) })"
            />
          </div>
          <p
            v-else
            class="border-t border-border/60 pt-4 text-xs text-muted-foreground"
          >
            选好模型后，即可设置调度方式和排序。
          </p>
        </div>
      </section>
    </fieldset>
    <p
      v-if="validationError && entries.every(entry => entry.scope === 'all' || entry.models.length > 0)"
      role="alert"
      class="text-xs text-destructive"
    >
      {{ validationError }}
    </p>
    <p
      v-if="scopeMode === 'selected' && !hasAllModels"
      class="text-xs text-muted-foreground"
    >
      {{ availableModels.length ? `还有 ${availableModels.length} 个模型可配置；` : '' }}未指定的模型继续使用默认调度。
    </p>
  </section>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ChevronDown, Globe, Key, Layers, ListFilter, Plus, Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui'
import type { GlobalModelResponse } from '@/api/global-models'
import RoutingPriorityPolicyEditor from './RoutingPriorityPolicyEditor.vue'
import RoutingModelSelector from './RoutingModelSelector.vue'
import { getDefaultModelPolicy, type RoutingGroupConfig, type RoutingPriorityMode, type RoutingSchedulingMode } from '../utils/routingPolicy'
import {
  createSchedulingPolicy,
  readSchedulingPolicies,
  schedulingPolicyEditorConfig,
  validateSchedulingPolicies,
  writeSchedulingPolicies,
  type SchedulingPolicy,
} from '../utils/schedulingPolicies'

const props = defineProps<{
  config: RoutingGroupConfig
  globalModels: GlobalModelResponse[]
  loadingModels?: boolean
  modelsError?: string | null
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:config': [value: RoutingGroupConfig]
  'validity-change': [valid: boolean]
  'reload-models': []
}>()

const scopeModes = [
  { value: 'all' as const, label: '全部模型', icon: Globe },
  { value: 'selected' as const, label: '区分模型', icon: ListFilter },
]
const priorityModes = [
  { value: 'provider' as RoutingPriorityMode, label: 'Provider', icon: Layers },
  { value: 'global_key' as RoutingPriorityMode, label: 'Key', icon: Key },
]
const schedulingModes: Array<{ value: RoutingSchedulingMode; label: string }> = [
  { value: 'cache_affinity', label: '缓存亲和' },
  { value: 'load_balance', label: '负载均衡' },
  { value: 'fixed_order', label: '固定顺序' },
]
const entries = ref(readSchedulingPolicies(props.config))
const scopeMode = ref<SchedulingPolicy['scope']>(entries.value.some(entry => entry.scope === 'selected') ? 'selected' : 'all')
let allModelsDraft: SchedulingPolicy[] | null = null
let selectedModelsDraft: SchedulingPolicy[] | null = null
const fallbackScheduling = {
  priority_mode: props.config.default_policy.priority_mode,
  scheduling_mode: props.config.default_policy.scheduling_mode,
}
const expandedId = ref<string | null>(entries.value[0]?.id ?? null)
const validationError = computed(() => validateSchedulingPolicies(entries.value))
const hasAllModels = computed(() => entries.value.some(entry => entry.scope === 'all'))
const assignedModels = computed(() => new Set(entries.value.filter(entry => entry.scope === 'selected').flatMap(entry => entry.models)))
const availableModels = computed(() => props.globalModels.filter(model => !assignedModels.value.has(model.name)))
const canAddEntry = computed(() => scopeMode.value === 'selected' && !props.disabled && !props.loadingModels && !props.modelsError
  && !validationError.value && availableModels.value.length > 0)
const addEntryHint = computed(() => {
  if (validationError.value) return validationError.value
  if (props.loadingModels) return '正在加载全局模型'
  if (props.modelsError) return '请先重新加载全局模型'
  return availableModels.value.length ? '为其他模型添加一套调度配置' : '所有全局模型都已有配置'
})

watch(validationError, error => emit('validity-change', !error), { immediate: true })

function schedulingModeLabel(mode: RoutingSchedulingMode): string {
  return schedulingModes.find(item => item.value === mode)?.label ?? mode
}

function scopeSummary(entry: SchedulingPolicy): string {
  if (entry.scope === 'all') return '默认配置'
  if (entry.models.length === 0) return '请选择适用模型'
  const labels = entry.models.slice(0, 2).map(name => props.globalModels.find(model => model.name === name)?.display_name || name)
  return labels.join('、') + (entry.models.length > 2 ? ` 等 ${entry.models.length} 个模型` : '')
}

function otherModelOwners(entryId: string): Record<string, number> {
  return Object.fromEntries(entries.value.flatMap((entry, index) => entry.id !== entryId && entry.scope === 'selected'
    ? entry.models.map(model => [model, index + 1])
    : []))
}

function publish(): void {
  emit('update:config', writeSchedulingPolicies({
    ...props.config,
    default_policy: { ...props.config.default_policy, ...fallbackScheduling },
  }, entries.value))
}

function setScopeMode(scope: SchedulingPolicy['scope']): void {
  if (props.disabled || scopeMode.value === scope) return
  if (scopeMode.value === 'all') allModelsDraft = entries.value
  else selectedModelsDraft = entries.value

  const saved = scope === 'all' ? allModelsDraft : selectedModelsDraft
  if (saved) {
    entries.value = saved
  } else {
    const source = entries.value.find(entry => entry.scope === 'all') ?? entries.value[0]
      ?? createSchedulingPolicy(props.config, scope)
    entries.value = [{
      ...createSchedulingPolicy(props.config, scope),
      priorityMode: source.priorityMode,
      schedulingMode: source.schedulingMode,
      policy: source.policy,
    }]
  }
  scopeMode.value = scope
  expandedId.value = entries.value[0]?.id ?? null
  publish()
}

function updateEntry(id: string, patch: Partial<SchedulingPolicy>): void {
  if (props.disabled) return
  const current = entries.value.find(entry => entry.id === id)
  if (!current || Object.entries(patch).every(([field, value]) => current[field as keyof SchedulingPolicy] === value)) return
  entries.value = entries.value.map(entry => {
    if (entry.id !== id) return entry
    const updated = { ...entry, ...patch }
    if (updated.scope === 'selected') {
      updated.models = updated.models.filter(model => !otherModelOwners(id)[model])
    }
    return updated
  })
  publish()
}

function addEntry(): void {
  if (!canAddEntry.value) return
  const entry = createSchedulingPolicy(props.config)
  entries.value.push(entry)
  expandedId.value = entry.id
  publish()
}

function removeEntry(id: string): void {
  if (props.disabled || entries.value.length === 1) return
  entries.value = entries.value.filter(entry => entry.id !== id)
  if (entries.value.every(entry => entry.scope === 'all')) {
    scopeMode.value = 'all'
    selectedModelsDraft = null
  }
  if (expandedId.value === id) expandedId.value = entries.value[0]?.id ?? null
  publish()
}
</script>

<style scoped>
.scheduling-switch {
  display: grid;
  gap: 4px;
  padding: 4px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: color-mix(in oklab, var(--muted) 40%, var(--background));
}

.scheduling-switch__option {
  position: relative;
  display: flex;
  min-width: 0;
  min-height: 38px;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 7px 4px;
  border: 1px solid transparent;
  border-radius: 5px;
  color: color-mix(in oklab, var(--foreground) 75%, var(--muted-foreground));
  font-size: 14px;
  font-weight: 600;
  line-height: 20px;
  letter-spacing: 0;
  cursor: pointer;
  transition: background-color 150ms, border-color 150ms, color 150ms, box-shadow 150ms;
}

.scheduling-switch__option:hover:not(:disabled) {
  background: color-mix(in oklab, var(--primary) 8%, var(--background));
  color: var(--foreground);
}

.scheduling-switch__option[aria-pressed='true'] {
  background: var(--primary);
  color: var(--primary-foreground);
  box-shadow: 0 1px 2px color-mix(in oklab, var(--primary) 20%, transparent);
}

.scheduling-switch__option[aria-pressed='true']:hover:not(:disabled) {
  background: color-mix(in oklab, var(--primary) 92%, black);
  color: var(--primary-foreground);
}

.scheduling-switch__option:focus-visible {
  z-index: 1;
  outline: 2px solid var(--ring);
  outline-offset: 2px;
}

.scheduling-switch__option:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

@media (prefers-reduced-motion: reduce) {
  .scheduling-switch__option {
    transition: none;
  }
}
</style>
