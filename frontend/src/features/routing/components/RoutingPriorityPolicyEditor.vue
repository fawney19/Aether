<template>
  <section class="space-y-4">
    <div
      v-if="showPriorityMode || showSchedulingMode"
      class="grid gap-3"
      :class="showPriorityMode ? 'lg:grid-cols-[1fr_1.4fr]' : ''"
    >
      <div
        v-if="showPriorityMode"
        class="space-y-1 text-sm"
      >
        <span class="text-muted-foreground">优先级模式</span>
        <div class="grid grid-cols-2 gap-1 rounded-lg bg-muted/40 p-1">
          <button
            type="button"
            class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
            :class="effectivePriorityMode === 'provider'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
            @click="updatePriorityMode('provider')"
          >
            <Layers class="h-4 w-4" />
            Provider
          </button>
          <button
            type="button"
            class="flex h-9 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-colors"
            :class="effectivePriorityMode === 'global_key'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
            @click="updatePriorityMode('global_key')"
          >
            <Key class="h-4 w-4" />
            Key
          </button>
        </div>
      </div>

      <div
        v-if="showSchedulingMode"
        class="space-y-1 text-sm"
      >
        <span class="text-muted-foreground">调度策略</span>
        <div class="grid grid-cols-3 gap-1 rounded-lg bg-muted/40 p-1">
          <button
            v-for="mode in schedulingModes"
            :key="mode.value"
            type="button"
            class="h-9 rounded-md px-3 text-sm font-medium transition-colors"
            :class="effectiveSchedulingMode === mode.value
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
            @click="updateSchedulingMode(mode.value)"
          >
            {{ mode.label }}
          </button>
        </div>
      </div>
    </div>

    <div class="rounded-lg border border-border/60">
      <div class="flex flex-col gap-3 border-b border-border/60 px-4 py-3 md:flex-row md:items-center md:justify-between">
        <div>
          <h3 class="text-sm font-medium">
            {{ effectivePriorityMode === 'provider' ? '提供商排序' : 'Key 排序' }}
          </h3>
          <p class="mt-1 text-xs text-muted-foreground">
            {{ subtitle }}
          </p>
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <button
            v-if="effectivePriorityMode === 'provider'"
            type="button"
            class="inline-flex h-8 items-center gap-2 rounded-md px-3 text-xs font-medium transition-colors"
            :class="providerMultiSelectEnabled
              ? 'bg-primary/10 text-primary hover:bg-primary/10 hover:text-primary'
              : 'text-muted-foreground hover:bg-muted hover:text-foreground'"
            @click="toggleProviderMultiSelect"
          >
            <ListChecks class="h-3.5 w-3.5" />
            {{ providerMultiSelectEnabled ? '退出多选' : '多选' }}
          </button>
          <Select
            v-if="effectivePriorityMode === 'global_key'"
            v-model="selectedApiFormat"
          >
            <SelectTrigger class="h-8 w-[180px] rounded-lg border-border/60 bg-background/80 px-3 text-xs">
              <SelectValue placeholder="选择端点" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="format in apiFormats"
                :key="format"
                :value="format"
              >
                {{ formatLabel(format) }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div class="min-h-[180px] max-h-[420px] overflow-y-auto p-3">
        <div
          v-if="loading"
          class="py-10 text-center text-sm text-muted-foreground"
        >
          正在加载
        </div>
        <div
          v-else-if="loadError"
          class="rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive"
        >
          {{ loadError }}
        </div>

        <div
          v-else-if="effectivePriorityMode === 'provider'"
          class="space-y-2"
        >
          <div
            v-if="providerRows.length === 0"
            class="rounded-lg border border-dashed border-border/70 px-4 py-8 text-center text-sm text-muted-foreground"
          >
            暂无 Provider
          </div>
          <div
            v-for="(row, index) in providerRows"
            v-else
            :key="row.id"
            class="group grid min-h-[56px] items-center gap-3 rounded-lg border px-3 py-2 transition-colors"
            :class="[providerGridClass, providerRowClass(row.id)]"
            draggable="true"
            @dragstart="handleProviderDragStart(row.id, $event)"
            @dragend="handleProviderDragEnd"
            @dragover.prevent="handleProviderDragOver(row.id)"
            @dragleave="handleProviderDragLeave"
            @drop="handleProviderDrop(row.id)"
          >
            <Checkbox
              v-if="providerMultiSelectEnabled"
              class="shrink-0"
              :checked="isProviderSelected(row.id)"
              :aria-label="`选择 ${row.name}`"
              @click.stop
              @change.stop
              @update:checked="checked => setProviderSelected(row.id, checked)"
            />
            <div class="cursor-grab rounded p-1 text-muted-foreground/40 transition-colors group-hover:text-muted-foreground active:cursor-grabbing">
              <GripVertical class="h-4 w-4" />
            </div>
            <div class="flex items-center gap-1">
              <button
                type="button"
                class="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-30"
                :disabled="providerMoveDisabled(row.id, index, -1)"
                @click="moveProvider(row.id, -1)"
              >
                <ArrowUp class="h-4 w-4" />
              </button>
              <button
                type="button"
                class="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-30"
                :disabled="providerMoveDisabled(row.id, index, 1)"
                @click="moveProvider(row.id, 1)"
              >
                <ArrowDown class="h-4 w-4" />
              </button>
            </div>
            <input
              :value="row.priority"
              type="number"
              min="0"
              class="priority-input h-8 w-14 rounded-md border border-border bg-background px-2 text-center text-sm"
              @change="event => setProviderPriority(row.id, event)"
            >
            <div class="min-w-0">
              <div class="flex items-center gap-2">
                <span class="truncate text-sm font-medium">{{ row.name }}</span>
                <span
                  v-if="row.kind === 'pool'"
                  class="rounded bg-primary/10 px-1.5 py-0.5 text-[10px] text-primary"
                >
                  Pool
                </span>
                <span
                  v-if="!row.is_active"
                  class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                >
                  停用
                </span>
              </div>
            </div>
            <div class="hidden max-w-[240px] flex-wrap justify-end gap-1 sm:flex">
              <span
                v-for="format in row.api_formats.slice(0, 3)"
                :key="format"
                :title="formatLabel(format)"
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ formatShortLabel(format) }}
              </span>
            </div>
          </div>
        </div>

        <div
          v-else
          class="space-y-2"
        >
          <div
            v-if="keyRows.length === 0"
            class="rounded-lg border border-dashed border-border/70 px-4 py-8 text-center text-sm text-muted-foreground"
          >
            暂无 Key
          </div>
          <div
            v-for="(row, index) in keyRows"
            v-else
            :key="row.id"
            class="group grid min-h-[56px] items-center gap-3 rounded-lg border px-3 py-2 transition-colors sm:grid-cols-[auto_auto_56px_minmax(0,1fr)_auto]"
            :class="draggedKeyId === row.id
              ? 'border-primary/50 bg-primary/5 shadow-sm'
              : dragOverKeyId === row.id
                ? 'border-primary/30 bg-primary/5'
                : 'border-border/50 bg-background hover:bg-muted/30'"
            draggable="true"
            @dragstart="handleKeyDragStart(row.id, $event)"
            @dragend="handleKeyDragEnd"
            @dragover.prevent="handleKeyDragOver(row.id)"
            @dragleave="handleKeyDragLeave"
            @drop="handleKeyDrop(row.id)"
          >
            <div class="cursor-grab rounded p-1 text-muted-foreground/40 transition-colors group-hover:text-muted-foreground active:cursor-grabbing">
              <GripVertical class="h-4 w-4" />
            </div>
            <div class="flex items-center gap-1">
              <button
                type="button"
                class="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-30"
                :disabled="index === 0"
                @click="moveKey(row.id, -1)"
              >
                <ArrowUp class="h-4 w-4" />
              </button>
              <button
                type="button"
                class="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-30"
                :disabled="index === keyRows.length - 1"
                @click="moveKey(row.id, 1)"
              >
                <ArrowDown class="h-4 w-4" />
              </button>
            </div>
            <input
              :value="row.priority"
              type="number"
              min="0"
              class="priority-input h-8 w-14 rounded-md border border-border bg-background px-2 text-center text-sm"
              @change="event => setKeyPriority(row.id, event)"
            >
            <div class="min-w-0">
              <div class="flex items-center gap-2">
                <span class="truncate text-sm font-medium">{{ row.name }}</span>
                <span
                  v-if="!row.is_active"
                  class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                >
                  停用
                </span>
              </div>
              <div class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                {{ row.masked }} · {{ row.provider_name }}
              </div>
            </div>
            <div class="hidden max-w-[240px] flex-wrap justify-end gap-1 sm:flex">
              <span
                v-for="format in row.api_formats.slice(0, 3)"
                :key="format"
                :title="formatLabel(format)"
                class="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
              >
                {{ formatShortLabel(format) }}
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, GripVertical, Key, Layers, ListChecks } from 'lucide-vue-next'

import client from '@/api/client'
import {
  Checkbox,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import {
  getProvidersSummary,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { formatApiFormat, formatApiFormatShort, normalizeApiFormatAlias, sortApiFormats } from '@/api/endpoints/types/api-format'
import { parseApiError } from '@/utils/errorParser'
import {
  DEFAULT_ROUTING_POLICY_MODEL,
  getDefaultModelPolicy,
  getModelPolicy,
  normalizeRoutingGroupConfig,
  setModelKeyPriorityOverrides,
  setModelPoolPriorityOverrides,
  setModelProviderPriorityOverrides,
  type RoutingDefaultPolicy,
  type RoutingGroupConfig,
  type RoutingPriorityMode,
  type RoutingSchedulingMode,
} from '../utils/routingPolicy'

interface ProviderPriorityRow {
  id: string
  name: string
  is_active: boolean
  api_formats: string[]
  priority: number
}

interface KeyPriorityRow {
  id: string
  kind: 'key' | 'pool'
  target_id: string
  name: string
  masked: string
  is_active: boolean
  api_formats: string[]
  priority: number
  provider_id: string
  provider_name: string
  pool_key_count?: number
  pool_active_key_count?: number
}

interface GlobalKeySource {
  id: string
  provider_id: string
  provider_name: string
  name: string
  api_key_masked: string
  internal_priority: number
  global_priority_by_format: Record<string, number> | null
  is_active: boolean
  provider_active: boolean
  api_formats: string[]
  api_format: string
  health_score: number | null
  request_count: number
}

const props = defineProps<{
  config: RoutingGroupConfig
  model?: string
  priorityMode?: RoutingPriorityMode
  schedulingMode?: RoutingSchedulingMode
  showPriorityMode?: boolean
  showSchedulingMode?: boolean
  subtitle?: string
}>()

const emit = defineEmits<{
  'update:config': [value: RoutingGroupConfig]
  'update:priority-mode': [value: RoutingPriorityMode]
  'update:scheduling-mode': [value: RoutingSchedulingMode]
}>()

const schedulingModes: Array<{ value: RoutingDefaultPolicy['scheduling_mode']; label: string }> = [
  { value: 'cache_affinity', label: '缓存亲和' },
  { value: 'load_balance', label: '负载均衡' },
  { value: 'fixed_order', label: '固定顺序' },
]

const providers = ref<ProviderWithEndpointsSummary[]>([])
const keysByFormat = ref<Record<string, GlobalKeySource[]>>({})
const selectedApiFormat = ref('')
const loadingProviders = ref(false)
const loadingKeys = ref(false)
const loadError = ref<string | null>(null)
const draggedProviderId = ref<string | null>(null)
const dragOverProviderId = ref<string | null>(null)
const draggedKeyId = ref<string | null>(null)
const dragOverKeyId = ref<string | null>(null)
const providerMultiSelectEnabled = ref(false)
const selectedProviderIds = ref<Set<string>>(new Set())

const config = computed(() => normalizeRoutingGroupConfig(props.config))
const targetModel = computed(() => props.model?.trim() || DEFAULT_ROUTING_POLICY_MODEL)
const targetModelPolicy = computed(() => targetModel.value === DEFAULT_ROUTING_POLICY_MODEL
  ? getDefaultModelPolicy(config.value)
  : getModelPolicy(config.value, targetModel.value))
const showPriorityMode = computed(() => props.showPriorityMode !== false)
const showSchedulingMode = computed(() => props.showSchedulingMode !== false)
const effectivePriorityMode = computed(() => props.priorityMode ?? config.value.default_policy.priority_mode)
const effectiveSchedulingMode = computed(() => props.schedulingMode ?? config.value.default_policy.scheduling_mode)
const subtitle = computed(() => props.subtitle ?? '默认作用于全部模型')
const loading = computed(() => loadingProviders.value || loadingKeys.value)
const apiFormats = computed(() => sortApiFormats(Object.keys(keysByFormat.value)))
const providerGridClass = computed(() => providerMultiSelectEnabled.value
  ? 'sm:grid-cols-[auto_auto_auto_56px_minmax(0,1fr)_auto]'
  : 'sm:grid-cols-[auto_auto_56px_minmax(0,1fr)_auto]')
const providerById = computed(() => {
  const map = new Map<string, ProviderWithEndpointsSummary>()
  for (const provider of providers.value) {
    map.set(provider.id, provider)
  }
  return map
})
const providerIdByName = computed(() => {
  const map = new Map<string, string>()
  for (const provider of providers.value) {
    if (!map.has(provider.name)) {
      map.set(provider.name, provider.id)
    }
  }
  return map
})
const poolProviderIds = computed(() => {
  const set = new Set<string>()
  for (const provider of providers.value) {
    if (provider.pool_advanced) {
      set.add(provider.id)
    }
  }
  return set
})

const providerRows = computed<ProviderPriorityRow[]>(() => {
  const overrides = targetModelPolicy.value.provider_priority_overrides
  return providers.value
    .map(provider => ({
      id: provider.id,
      name: provider.name,
      is_active: provider.is_active,
      api_formats: provider.api_formats ?? [],
      priority: priorityValue(overrides[provider.id], provider.provider_priority),
    }))
    .sort(comparePriorityRows)
})

const keyRows = computed<KeyPriorityRow[]>(() => {
  const format = selectedApiFormat.value
  const keyOverrides = targetModelPolicy.value.key_priority_overrides
  const poolOverrides = targetModelPolicy.value.pool_priority_overrides
  const normalRows: KeyPriorityRow[] = []
  const poolGroups = new Map<string, GlobalKeySource[]>()

  for (const key of keysByFormat.value[format] ?? []) {
    const providerId = resolveProviderId(key)
    if (isPoolManagedProvider(providerId)) {
      if (!poolGroups.has(providerId)) {
        poolGroups.set(providerId, [])
      }
      poolGroups.get(providerId)?.push(key)
      continue
    }
    normalRows.push({
      id: key.id,
      kind: 'key',
      target_id: key.id,
      name: key.name,
      masked: key.api_key_masked,
      is_active: key.is_active && key.provider_active,
      api_formats: key.api_formats,
      priority: priorityValue(keyOverrides[key.id], fallbackKeyPriority(key, format)),
      provider_id: providerId,
      provider_name: key.provider_name,
    })
  }

  const poolRows = Array.from(poolGroups.entries()).map(([providerId, keys]) =>
    buildPoolRow(format, providerId, keys, poolOverrides)
  )

  return [...normalRows, ...poolRows].sort(comparePriorityRows)
})

watch(effectivePriorityMode, mode => {
  if (mode === 'global_key') {
    void loadGlobalKeys()
    providerMultiSelectEnabled.value = false
    selectedProviderIds.value = new Set()
  }
})

watch(providerRows, rows => {
  const visibleIds = new Set(rows.map(row => row.id))
  const next = new Set([...selectedProviderIds.value].filter(id => visibleIds.has(id)))
  if (next.size !== selectedProviderIds.value.size) {
    selectedProviderIds.value = next
  }
})

watch(apiFormats, formats => {
  if (!formats.includes(selectedApiFormat.value)) {
    selectedApiFormat.value = formats[0] ?? ''
  }
})

onMounted(() => {
  void (async () => {
    await loadProviders()
    if (effectivePriorityMode.value === 'global_key') {
      await loadGlobalKeys()
    }
  })()
})

function updateConfig(value: RoutingGroupConfig): void {
  emit('update:config', normalizeRoutingGroupConfig(value))
}

function updateDefaultPolicy(patch: Partial<RoutingDefaultPolicy>): void {
  updateConfig({
    ...config.value,
    default_policy: {
      ...config.value.default_policy,
      ...patch,
    },
  })
}

function updatePriorityMode(mode: RoutingPriorityMode): void {
  if (props.priorityMode != null) {
    emit('update:priority-mode', mode)
    return
  }
  updateDefaultPolicy({ priority_mode: mode })
}

function updateSchedulingMode(mode: RoutingSchedulingMode): void {
  if (props.schedulingMode != null) {
    emit('update:scheduling-mode', mode)
    return
  }
  updateDefaultPolicy({ scheduling_mode: mode })
}

async function loadProviders(): Promise<void> {
  loadingProviders.value = true
  loadError.value = null
  try {
    const response = await getProvidersSummary({ page: 1, page_size: 9999 })
    providers.value = response.items
  } catch (err) {
    loadError.value = parseApiError(err, '加载 Provider 失败')
    providers.value = []
  } finally {
    loadingProviders.value = false
  }
}

async function loadGlobalKeys(force = false): Promise<void> {
  if (!force && Object.keys(keysByFormat.value).length > 0) return
  loadingKeys.value = true
  loadError.value = null
  try {
    const response = await client.get<Record<string, Record<string, unknown>[]>>(
      '/api/admin/endpoints/keys/grouped-by-format',
    )
    const next: Record<string, GlobalKeySource[]> = {}
    for (const [rawFormat, rawKeys] of Object.entries(response.data ?? {})) {
      const format = normalizeFormat(rawFormat)
      if (!format) continue
      next[format] = normalizeGlobalKeys(format, rawKeys)
    }
    keysByFormat.value = next
    if (!selectedApiFormat.value || !Object.keys(next).includes(selectedApiFormat.value)) {
      selectedApiFormat.value = sortApiFormats(Object.keys(next))[0] ?? ''
    }
  } catch (err) {
    loadError.value = parseApiError(err, '加载全局 Key 失败')
  } finally {
    loadingKeys.value = false
  }
}

function setProviderPriority(providerId: string, event: Event): void {
  const priority = readPriorityInput(event)
  if (priority == null) return
  updateProviderOverrides({
    ...targetModelPolicy.value.provider_priority_overrides,
    [providerId]: priority,
  })
}

function moveProvider(providerId: string, direction: -1 | 1): void {
  const movingIds = providerMoveIds(providerId)
  const rows = movingIds.length > 1
    ? moveRowsByGroup(providerRows.value, movingIds, direction)
    : moveRow(providerRows.value, providerId, direction)
  updateProviderOverrides(Object.fromEntries(rows.map((row, index) => [row.id, index])))
}

function updateProviderOverrides(overrides: Record<string, number>): void {
  updateConfig(setModelProviderPriorityOverrides(config.value, targetModel.value, overrides))
}

function isProviderSelected(providerId: string): boolean {
  return providerMultiSelectEnabled.value && selectedProviderIds.value.has(providerId)
}

function setProviderSelected(providerId: string, selected: boolean): void {
  if (!providerMultiSelectEnabled.value) return
  const next = new Set(selectedProviderIds.value)
  if (selected) {
    next.add(providerId)
  } else {
    next.delete(providerId)
  }
  selectedProviderIds.value = next
}

function toggleProviderMultiSelect(): void {
  providerMultiSelectEnabled.value = !providerMultiSelectEnabled.value
  if (!providerMultiSelectEnabled.value) {
    selectedProviderIds.value = new Set()
  }
}

function providerMoveIds(providerId: string): string[] {
  if (!providerMultiSelectEnabled.value || !selectedProviderIds.value.has(providerId)) {
    return [providerId]
  }
  return providerRows.value
    .map(row => row.id)
    .filter(id => selectedProviderIds.value.has(id))
}

function providerMoveDisabled(providerId: string, index: number, direction: -1 | 1): boolean {
  const movingIds = providerMoveIds(providerId)
  if (movingIds.length <= 1) {
    return direction === -1 ? index === 0 : index === providerRows.value.length - 1
  }
  const movingSet = new Set(movingIds)
  const movingIndexes = providerRows.value
    .map((row, rowIndex) => movingSet.has(row.id) ? rowIndex : -1)
    .filter(rowIndex => rowIndex >= 0)
  if (movingIndexes.length === 0) return true
  return direction === -1
    ? Math.min(...movingIndexes) === 0
    : Math.max(...movingIndexes) === providerRows.value.length - 1
}

function providerRowClass(providerId: string): string {
  if (isProviderDragged(providerId)) {
    return 'border-primary/50 bg-primary/5 shadow-sm'
  }
  if (dragOverProviderId.value === providerId) {
    return 'border-primary/30 bg-primary/5'
  }
  if (isProviderSelected(providerId)) {
    return 'border-primary/40 bg-primary/5'
  }
  return 'border-border/50 bg-background hover:bg-muted/30'
}

function isProviderDragged(providerId: string): boolean {
  const draggedId = draggedProviderId.value
  return Boolean(draggedId && providerMoveIds(draggedId).includes(providerId))
}

function setKeyPriority(keyId: string, event: Event): void {
  const priority = readPriorityInput(event)
  if (priority == null) return
  const row = keyRows.value.find(item => item.id === keyId)
  if (!row) return
  if (row.kind === 'pool') {
    updatePoolOverrides({
      ...targetModelPolicy.value.pool_priority_overrides,
      [row.target_id]: priority,
    })
  } else {
    updateKeyOverrides({
      ...targetModelPolicy.value.key_priority_overrides,
      [row.target_id]: priority,
    })
  }
}

function moveKey(keyId: string, direction: -1 | 1): void {
  const rows = moveRow(keyRows.value, keyId, direction)
  updateVisibleKeyAndPoolOverrides(rows)
}

function updateKeyOverrides(overrides: Record<string, number>): void {
  updateConfig(setModelKeyPriorityOverrides(config.value, targetModel.value, overrides))
}

function updatePoolOverrides(overrides: Record<string, number>): void {
  updateConfig(setModelPoolPriorityOverrides(config.value, targetModel.value, overrides))
}

function updateKeyAndPoolOverrides(
  keyOverrides: Record<string, number>,
  poolOverrides: Record<string, number>,
): void {
  const next = setModelPoolPriorityOverrides(
    setModelKeyPriorityOverrides(config.value, targetModel.value, keyOverrides),
    targetModel.value,
    poolOverrides,
  )
  updateConfig(next)
}

function updateVisibleKeyAndPoolOverrides(rows: KeyPriorityRow[]): void {
  const keyOverrides = { ...targetModelPolicy.value.key_priority_overrides }
  const poolOverrides = { ...targetModelPolicy.value.pool_priority_overrides }

  for (const row of keyRows.value) {
    if (row.kind === 'pool') {
      delete poolOverrides[row.target_id]
    } else {
      delete keyOverrides[row.target_id]
    }
  }

  rows.forEach((row, index) => {
    if (row.kind === 'pool') {
      poolOverrides[row.target_id] = index
    } else {
      keyOverrides[row.target_id] = index
    }
  })

  updateKeyAndPoolOverrides(keyOverrides, poolOverrides)
}

function handleProviderDragStart(providerId: string, event: DragEvent): void {
  draggedProviderId.value = providerId
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', providerId)
  }
}

function handleProviderDragEnd(): void {
  draggedProviderId.value = null
  dragOverProviderId.value = null
}

function handleProviderDragOver(providerId: string): void {
  dragOverProviderId.value = providerId
}

function handleProviderDragLeave(): void {
  dragOverProviderId.value = null
}

function handleProviderDrop(providerId: string): void {
  const draggedId = draggedProviderId.value
  if (!draggedId || draggedId === providerId) {
    handleProviderDragEnd()
    return
  }
  const movingIds = providerMoveIds(draggedId)
  if (movingIds.includes(providerId)) {
    handleProviderDragEnd()
    return
  }
  const rows = movingIds.length > 1
    ? reorderRowsByGroup(providerRows.value, movingIds, providerId)
    : reorderRows(providerRows.value, draggedId, providerId)
  updateProviderOverrides(Object.fromEntries(rows.map((row, index) => [row.id, index])))
  handleProviderDragEnd()
}

function handleKeyDragStart(keyId: string, event: DragEvent): void {
  draggedKeyId.value = keyId
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', keyId)
  }
}

function handleKeyDragEnd(): void {
  draggedKeyId.value = null
  dragOverKeyId.value = null
}

function handleKeyDragOver(keyId: string): void {
  dragOverKeyId.value = keyId
}

function handleKeyDragLeave(): void {
  dragOverKeyId.value = null
}

function handleKeyDrop(keyId: string): void {
  const draggedId = draggedKeyId.value
  if (!draggedId || draggedId === keyId) {
    handleKeyDragEnd()
    return
  }
  const rows = reorderRows(keyRows.value, draggedId, keyId)
  updateVisibleKeyAndPoolOverrides(rows)
  handleKeyDragEnd()
}

function moveRow<T extends { id: string }>(rows: T[], id: string, direction: -1 | 1): T[] {
  const next = [...rows]
  const index = next.findIndex(row => row.id === id)
  const targetIndex = index + direction
  if (index < 0 || targetIndex < 0 || targetIndex >= next.length) {
    return next
  }
  const [item] = next.splice(index, 1)
  next.splice(targetIndex, 0, item)
  return next
}

function moveRowsByGroup<T extends { id: string }>(rows: T[], movingIds: string[], direction: -1 | 1): T[] {
  const movingSet = new Set(movingIds)
  const movingRows = rows.filter(row => movingSet.has(row.id))
  if (movingRows.length === 0) return [...rows]

  const firstMovingIndex = rows.findIndex(row => movingSet.has(row.id))
  const remainingRows = rows.filter(row => !movingSet.has(row.id))
  const baseInsertIndex = rows
    .slice(0, firstMovingIndex)
    .filter(row => !movingSet.has(row.id))
    .length
  const insertIndex = direction === -1
    ? Math.max(0, baseInsertIndex - 1)
    : Math.min(remainingRows.length, baseInsertIndex + 1)

  const next = [...remainingRows]
  next.splice(insertIndex, 0, ...movingRows)
  return next
}

function reorderRows<T extends { id: string }>(rows: T[], draggedId: string, targetId: string): T[] {
  const next = [...rows]
  const fromIndex = next.findIndex(row => row.id === draggedId)
  const toIndex = next.findIndex(row => row.id === targetId)
  if (fromIndex < 0 || toIndex < 0) return next
  const [item] = next.splice(fromIndex, 1)
  next.splice(toIndex, 0, item)
  return next
}

function reorderRowsByGroup<T extends { id: string }>(rows: T[], movingIds: string[], targetId: string): T[] {
  const movingSet = new Set(movingIds)
  if (movingSet.has(targetId)) return [...rows]

  const movingRows = rows.filter(row => movingSet.has(row.id))
  if (movingRows.length === 0) return [...rows]

  const targetIndex = rows.findIndex(row => row.id === targetId)
  const firstMovingIndex = rows.findIndex(row => movingSet.has(row.id))
  if (targetIndex < 0 || firstMovingIndex < 0) return [...rows]

  const remainingRows = rows.filter(row => !movingSet.has(row.id))
  const remainingTargetIndex = remainingRows.findIndex(row => row.id === targetId)
  if (remainingTargetIndex < 0) return [...rows]

  const insertIndex = firstMovingIndex < targetIndex
    ? remainingTargetIndex + 1
    : remainingTargetIndex
  const next = [...remainingRows]
  next.splice(insertIndex, 0, ...movingRows)
  return next
}

function readPriorityInput(event: Event): number | null {
  const value = Number((event.target as HTMLInputElement).value)
  if (!Number.isFinite(value) || value < 0) {
    return null
  }
  return Math.trunc(value)
}

function priorityValue(override: number | undefined, fallback: number | null | undefined): number {
  if (typeof override === 'number' && Number.isFinite(override)) return override
  if (typeof fallback === 'number' && Number.isFinite(fallback)) return fallback
  return 0
}

function fallbackKeyPriority(key: GlobalKeySource, format: string): number {
  const normalizedFormat = normalizeFormat(format)
  if (normalizedFormat && typeof key.global_priority_by_format?.[normalizedFormat] === 'number') {
    return key.global_priority_by_format[normalizedFormat]
  }
  return key.internal_priority
}

function normalizeGlobalKeys(format: string, rawKeys: Record<string, unknown>[]): GlobalKeySource[] {
  const deduped = new Map<string, GlobalKeySource>()
  for (const raw of rawKeys) {
    const id = String(raw.id || '').trim()
    if (!id) continue
    const providerName = String(raw.provider_name || '')
    const providerId = String(raw.provider_id || '') || providerIdByName.value.get(providerName) || ''
    const priorityMap = normalizePriorityMap(raw.global_priority_by_format as Record<string, unknown> | null | undefined)
    const source: GlobalKeySource = {
      id,
      provider_id: providerId,
      provider_name: providerName || providerById.value.get(providerId)?.name || 'Unknown Provider',
      name: String(raw.name || 'Unnamed Key'),
      api_key_masked: String(raw.api_key_masked || '***'),
      internal_priority: toNumberOrNull(raw.internal_priority) ?? 0,
      global_priority_by_format: Object.keys(priorityMap).length > 0 ? priorityMap : null,
      is_active: raw.is_active !== false,
      provider_active: raw.provider_active !== false,
      api_formats: Array.isArray(raw.api_formats) ? raw.api_formats.map(item => normalizeFormat(String(item))).filter(Boolean) : [format],
      api_format: format,
      health_score: toNumberOrNull(raw.health_score),
      request_count: toNumberOrNull(raw.request_count) ?? 0,
    }
    const existing = deduped.get(id)
    if (!existing) {
      deduped.set(id, source)
      continue
    }
    deduped.set(id, {
      ...existing,
      ...source,
      global_priority_by_format: {
        ...(existing.global_priority_by_format ?? {}),
        ...(source.global_priority_by_format ?? {}),
      },
      api_formats: Array.from(new Set([...existing.api_formats, ...source.api_formats])),
    })
  }
  return Array.from(deduped.values())
}

function buildPoolRow(
  format: string,
  providerId: string,
  keys: GlobalKeySource[],
  overrides: Record<string, number>,
): KeyPriorityRow {
  const provider = providerById.value.get(providerId)
  const activeKeyCount = keys.filter(key => key.is_active).length
  return {
    id: `pool:${providerId}:${format}`,
    kind: 'pool',
    target_id: providerId,
    name: provider?.name || keys[0]?.provider_name || '未知 Provider',
    masked: '[Pool]',
    is_active: (provider?.is_active ?? keys.some(key => key.provider_active)) && activeKeyCount > 0,
    api_formats: [format],
    priority: priorityValue(
      overrides[providerId],
      provider?.pool_advanced?.global_priority ?? provider?.provider_priority ?? 999999,
    ),
    provider_id: providerId,
    provider_name: provider?.name || keys[0]?.provider_name || 'Unknown Provider',
    pool_key_count: keys.length,
    pool_active_key_count: activeKeyCount,
  }
}

function resolveProviderId(key: Pick<GlobalKeySource, 'provider_id' | 'provider_name'>): string {
  if (key.provider_id) return key.provider_id
  return providerIdByName.value.get(key.provider_name) || ''
}

function isPoolManagedProvider(providerId: string): boolean {
  return providerId !== '' && poolProviderIds.value.has(providerId)
}

function normalizeFormat(value: string | null | undefined): string {
  return normalizeApiFormatAlias(value).trim()
}

function formatLabel(format: string): string {
  return formatApiFormat(format)
}

function formatShortLabel(format: string): string {
  return formatApiFormatShort(format)
}

function normalizePriorityMap(value: Record<string, unknown> | null | undefined): Record<string, number> {
  if (!value) return {}
  const normalized: Record<string, number> = {}
  for (const [rawFormat, rawPriority] of Object.entries(value)) {
    const format = normalizeFormat(rawFormat)
    const priority = toNumberOrNull(rawPriority)
    if (!format || priority == null) continue
    normalized[format] = priority
  }
  return normalized
}

function toNumberOrNull(value: unknown): number | null {
  const numberValue = Number(value)
  return Number.isFinite(numberValue) ? Math.trunc(numberValue) : null
}

function comparePriorityRows(left: ProviderPriorityRow | KeyPriorityRow, right: ProviderPriorityRow | KeyPriorityRow): number {
  return left.priority - right.priority
    || Number(right.is_active) - Number(left.is_active)
    || left.name.localeCompare(right.name)
    || left.id.localeCompare(right.id)
}
</script>

<style scoped>
.priority-input::-webkit-outer-spin-button,
.priority-input::-webkit-inner-spin-button {
  margin: 0;
  appearance: none;
}

.priority-input[type='number'] {
  appearance: textfield;
}
</style>
