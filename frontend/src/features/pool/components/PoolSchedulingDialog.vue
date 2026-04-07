<template>
  <Dialog
    :model-value="modelValue"
    title="号池调度"
    description="管理号池内 Key 的分配模式和排序偏好"
    size="lg"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="max-h-[calc(100dvh-13rem)] space-y-5 overflow-y-auto overscroll-contain pr-1 sm:max-h-[min(72vh,42rem)] sm:space-y-6 sm:pr-2">
      <!-- Section 1: 分配模式 (distribution_mode 互斥组, 四选一) -->
      <div class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4">
        <div class="space-y-1">
          <h3 class="text-sm font-medium">
            分配模式
          </h3>
          <p class="text-xs text-muted-foreground">
            控制 Key 的基础分配方式，选择一种模式。
          </p>
        </div>

        <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <button
            v-for="{ index, item } in distributionItems"
            :key="item.preset"
            type="button"
            class="min-h-11 rounded-xl border px-3 py-2.5 text-sm font-medium leading-tight transition-all duration-200"
            :disabled="!item.applicable"
            :class="[
              activeDistributionPreset === item.preset
                ? 'border-primary bg-primary text-primary-foreground shadow-sm shadow-primary/20'
                : item.applicable
                  ? 'border-border/60 bg-background text-foreground hover:border-border hover:bg-muted/40'
                  : 'border-border/30 bg-muted/20 text-muted-foreground/50 cursor-not-allowed'
            ]"
            @click="item.applicable && selectDistribution(index, item.preset)"
          >
            {{ item.label }}
          </button>
        </div>

        <div
          v-if="activeDistributionLabel || activeDistributionDesc"
          class="rounded-xl border border-primary/15 bg-primary/5 px-3 py-2.5"
        >
          <p
            v-if="activeDistributionDesc"
            class="mt-1 text-xs leading-5 text-muted-foreground"
          >
            {{ activeDistributionDesc }}
          </p>
        </div>
      </div>

      <!-- Section 2: 策略调度 (非互斥, 可叠加组合 + 拖拽排序) -->
      <div class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4">
        <div class="space-y-1">
          <div class="flex flex-wrap items-center gap-2">
            <h3 class="text-sm font-medium">
              策略调度
            </h3>
            <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
              已启用 {{ enabledStrategyCount }} 项
            </span>
          </div>
          <p class="text-xs text-muted-foreground">
            在分配模式基础上叠加排序因素，可组合启用。
          </p>
          <p class="text-xs text-muted-foreground">
            桌面端支持拖拽排序，移动端可点按上下调整优先级。
          </p>
        </div>

        <div class="space-y-1.5">
          <div
            v-for="{ index, item } in strategyItems"
            :key="item.preset"
            class="group rounded-xl border px-3 py-2.5 transition-all duration-200"
            :class="[
              !item.applicable
                ? 'border-border/40 bg-muted/20 opacity-80'
                : draggedIndex === index
                  ? 'border-primary/50 bg-primary/5 shadow-md'
                  : dragOverIndex === index
                    ? 'border-primary/30 bg-primary/5'
                    : item.enabled
                      ? 'border-primary/20 bg-primary/5 hover:border-primary/30'
                      : 'border-border/60 bg-background hover:border-border hover:bg-muted/30'
            ]"
            :draggable="item.applicable"
            @dragstart="item.applicable && handleDragStart(index, $event)"
            @dragend="handleDragEnd"
            @dragover.prevent="item.applicable && handleDragOver(index)"
            @dragleave="handleDragLeave"
            @drop="item.applicable && handleDrop(index)"
          >
            <div class="flex items-start gap-2.5">
              <!-- Drag handle -->
              <div
                class="hidden rounded-lg p-1 transition-colors sm:flex sm:shrink-0"
                :class="item.applicable
                  ? 'cursor-grab active:cursor-grabbing text-muted-foreground/40 group-hover:text-muted-foreground'
                  : 'text-muted-foreground/20 cursor-default'"
              >
                <GripVertical class="h-4 w-4" />
              </div>

              <div class="min-w-0 flex-1">
                <div class="flex items-start gap-2.5">
                  <div class="min-w-0 flex-1">
                    <div class="flex flex-wrap items-center gap-2">
                      <span
                        class="text-sm font-medium"
                        :class="!item.applicable ? 'text-muted-foreground' : 'text-foreground'"
                      >{{ item.label }}</span>
                      <span
                        v-if="getStrategyPriority(index)"
                        class="rounded-full bg-primary/10 px-2 py-0.5 text-[11px] font-medium text-primary"
                      >
                        #{{ getStrategyPriority(index) }}
                      </span>
                      <span
                        v-if="!item.applicable"
                        class="rounded-full border border-border/60 bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
                      >
                        当前不可用
                      </span>
                    </div>
                    <p class="mt-0.5 text-xs leading-5 text-muted-foreground">
                      {{ item.desc }}
                    </p>
                  </div>

                  <div
                    v-if="item.applicable"
                    class="flex shrink-0 items-center gap-1.5 sm:hidden"
                  >
                    <button
                      type="button"
                      class="inline-flex h-7 items-center justify-center rounded-lg border border-border/60 px-2.5 text-[11px] font-medium text-foreground transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
                      :disabled="!canMoveStrategy(index, -1)"
                      @click="moveStrategy(index, -1)"
                    >
                      上移
                    </button>
                    <button
                      type="button"
                      class="inline-flex h-7 items-center justify-center rounded-lg border border-border/60 px-2.5 text-[11px] font-medium text-foreground transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
                      :disabled="!canMoveStrategy(index, 1)"
                      @click="moveStrategy(index, 1)"
                    >
                      下移
                    </button>
                  </div>

                  <Switch
                    :model-value="item.enabled && item.applicable"
                    :disabled="!item.applicable"
                    class="mt-0.5 shrink-0"
                    @update:model-value="(v: boolean) => togglePreset(index, v)"
                  />
                </div>

                <div
                  v-if="(item.modeOptions.length > 0 && item.enabled && item.applicable) || item.applicable"
                  class="mt-2 flex flex-col gap-1.5 sm:flex-row sm:items-center sm:justify-between"
                >
                  <!-- Mode sub-config -->
                  <div
                    v-if="item.modeOptions.length > 0 && item.enabled && item.applicable"
                    class="flex flex-wrap gap-1 rounded-lg bg-muted/40 p-1"
                  >
                    <button
                      v-for="modeOpt in item.modeOptions"
                      :key="modeOpt.value"
                      type="button"
                      class="rounded-md px-2.5 py-1 text-xs font-medium transition-all"
                      :class="[
                        item.mode === modeOpt.value
                          ? 'bg-primary text-primary-foreground shadow-sm'
                          : 'text-muted-foreground hover:bg-background/70 hover:text-foreground'
                      ]"
                      @click="setPresetModeByPreset(item.preset, modeOpt.value)"
                    >
                      {{ modeOpt.label }}
                    </button>
                  </div>

                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        class="min-w-[96px] flex-1 sm:flex-none"
        :disabled="loading"
        @click="emit('update:modelValue', false)"
      >
        取消
      </Button>
      <Button
        class="min-w-[96px] flex-1 sm:flex-none"
        :disabled="loading"
        @click="handleSave"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { GripVertical } from 'lucide-vue-next'
import { Dialog, Button, Switch } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { updateProvider } from '@/api/endpoints'
import { getPoolSchedulingPresets } from '@/api/endpoints/pool'
import { moveStrategyItem } from '@/features/pool/utils/poolSchedulingDialog'
import type { PoolPresetMeta } from '@/api/endpoints/pool'
import type {
  PoolAdvancedConfig,
  SchedulingPresetItem,
  ProviderWithEndpointsSummary,
} from '@/api/endpoints/types/provider'

interface PresetModeOption {
  value: string
  label: string
}

interface PresetListItem {
  preset: string
  label: string
  desc: string
  enabled: boolean
  mode: string | null
  modeOptions: PresetModeOption[]
  applicable: boolean
  mutexGroup: string | null
  evidenceHint: string
}

const props = defineProps<{
  modelValue: boolean
  providerId: string
  providerType?: string
  currentConfig: PoolAdvancedConfig | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  saved: [provider: ProviderWithEndpointsSummary]
}>()

const DISTRIBUTION_GROUP = 'distribution_mode'

const FALLBACK_PRESET_DEFS: PoolPresetMeta[] = [
  {
    name: 'cache_affinity',
    label: '缓存亲和',
    description: '优先复用最近使用过的 Key，利用 Prompt Caching',
    mutex_group: DISTRIBUTION_GROUP,
    evidence_hint: '依据 LRU 时间戳（最近使用优先，与 LRU 轮转相反）',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'lru',
    label: 'LRU 轮转',
    description: '最久未使用的 Key 优先',
    mutex_group: DISTRIBUTION_GROUP,
    evidence_hint: '依据 LRU 时间戳（最近未使用优先）',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'single_account',
    label: '单号优先',
    description: '集中使用同一账号（反向 LRU）',
    mutex_group: DISTRIBUTION_GROUP,
    evidence_hint: '先按账号优先级（internal_priority），同级再按反向 LRU 集中',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'load_balance',
    label: '负载均衡',
    description: '随机分散 Key 使用，均匀分摊负载',
    mutex_group: DISTRIBUTION_GROUP,
    evidence_hint: '每次随机分值，实现完全均匀分散',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'free_first',
    label: 'Free 优先',
    description: '优先消耗 Free 账号（依赖 plan_type）',
    evidence_hint: '依据 plan_type（Free 账号优先调度）',
    providers: ['codex', 'kiro'],
    modes: null,
    default_mode: null,
  },
  {
    name: 'team_first',
    label: 'Team 优先',
    description: '优先消耗 Team 账号（依赖 plan_type）',
    evidence_hint: '依据 plan_type（Team 账号优先调度）',
    providers: ['codex', 'kiro'],
    modes: null,
    default_mode: null,
  },
  {
    name: 'plus_first',
    label: 'Plus 优先',
    description: '优先消耗 Plus/Pro 账号（依赖 plan_type）',
    evidence_hint: '依据 plan_type（Plus/Pro 账号优先调度）',
    providers: ['codex', 'kiro'],
    modes: null,
    default_mode: null,
  },
  {
    name: 'quota_balanced',
    label: '额度平均',
    description: '优先选额度消耗最少的账号',
    evidence_hint: '依据账号配额使用率；无配额时回退到窗口成本使用',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'recent_refresh',
    label: '额度刷新优先',
    description: '优先选即将刷新额度的账号',
    evidence_hint: '依据账号额度重置倒计时（next_reset / reset_seconds）',
    providers: ['codex', 'kiro'],
    modes: null,
    default_mode: null,
  },
  {
    name: 'priority_first',
    label: '优先级优先',
    description: '按账号优先级顺序调度（数字越小越优先）',
    evidence_hint: '依据 internal_priority（支持拖拽/手工编辑）',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'health_first',
    label: '健康优先',
    description: '优先选择健康分更高、失败更少的账号',
    evidence_hint: '依据 health_by_format 聚合分（含熔断/失败衰减）',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'latency_first',
    label: '延迟优先',
    description: '优先选择最近延迟更低的账号',
    evidence_hint: '依据号池延迟窗口均值（latency_window_seconds）',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'cost_first',
    label: '成本优先',
    description: '优先选择窗口消耗更低的账号',
    evidence_hint: '依据窗口成本/Token 用量，缺失时回退配额使用率',
    providers: [],
    modes: null,
    default_mode: null,
  },
]

const DEFAULT_ENABLED_PRESETS = new Set(['cache_affinity', 'recent_refresh'])

const { success, error: showError } = useToast()
const loading = ref(false)
const presetDefs = ref<PoolPresetMeta[]>([])
const presetDefsLoaded = ref(false)
const loadingPresetDefs = ref(false)

const draggedIndex = ref<number | null>(null)
const dragOverIndex = ref<number | null>(null)
const presetList = ref<PresetListItem[]>([])

function normalizeProviderType(value: string | undefined): string {
  return (value || '').trim().toLowerCase()
}

function normalizePresetName(value: unknown): string {
  return String(value ?? '').trim().toLowerCase()
}

function normalizeMode(value: unknown): string | null {
  const normalized = String(value ?? '').trim().toLowerCase()
  return normalized || null
}

function normalizeMutexGroup(value: unknown): string | null {
  const normalized = String(value ?? '').trim().toLowerCase()
  return normalized || null
}

const FALLBACK_ORDER = FALLBACK_PRESET_DEFS.map(d => d.name)

function normalizePresetDefs(defs: PoolPresetMeta[]): PoolPresetMeta[] {
  const ordered: PoolPresetMeta[] = []
  const seen = new Set<string>()
  for (const raw of defs) {
    const name = normalizePresetName(raw.name)
    if (!name || seen.has(name)) continue
    seen.add(name)
    const providers = Array.isArray(raw.providers)
      ? raw.providers.map(p => normalizeProviderType(p)).filter(Boolean)
      : []
    const modes = Array.isArray(raw.modes)
      ? raw.modes
        .map(mode => ({
          value: normalizePresetName(mode.value),
          label: String(mode.label ?? '').trim() || String(mode.value ?? '').trim(),
        }))
        .filter(mode => Boolean(mode.value))
      : null
    const defaultMode = normalizeMode(raw.default_mode)
    ordered.push({
      name,
      label: String(raw.label ?? '').trim() || name,
      description: String(raw.description ?? '').trim(),
      providers,
      modes: modes && modes.length > 0 ? modes : null,
      default_mode: defaultMode,
      mutex_group: normalizeMutexGroup(raw.mutex_group),
      evidence_hint: String(raw.evidence_hint ?? '').trim() || null,
    })
  }
  // Re-order by FALLBACK_PRESET_DEFS order; unknown presets go to the end.
  ordered.sort((a, b) => {
    const ia = FALLBACK_ORDER.indexOf(a.name)
    const ib = FALLBACK_ORDER.indexOf(b.name)
    return (ia === -1 ? 9999 : ia) - (ib === -1 ? 9999 : ib)
  })
  return ordered
}

function getPresetDefs(): PoolPresetMeta[] {
  if (presetDefs.value.length > 0) {
    return presetDefs.value
  }
  return FALLBACK_PRESET_DEFS
}

async function ensurePresetDefsLoaded(): Promise<void> {
  if (presetDefsLoaded.value || loadingPresetDefs.value) return
  loadingPresetDefs.value = true
  try {
    const remoteDefs = await getPoolSchedulingPresets()
    const normalized = normalizePresetDefs(Array.isArray(remoteDefs) ? remoteDefs : [])
    if (normalized.length > 0) {
      presetDefs.value = normalized
    }
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    presetDefsLoaded.value = true
    loadingPresetDefs.value = false
  }
}

function isApplicablePreset(def: PoolPresetMeta): boolean {
  const providerType = normalizeProviderType(props.providerType)
  const providers = Array.isArray(def.providers) ? def.providers : []
  if (providers.length === 0) return true
  if (!providerType) return true
  return providers.includes(providerType)
}

function getModeOptions(def: PoolPresetMeta): PresetModeOption[] {
  const modes = Array.isArray(def.modes) ? def.modes : []
  return modes
    .map(mode => ({
      value: normalizePresetName(mode.value),
      label: String(mode.label ?? '').trim() || String(mode.value ?? '').trim(),
    }))
    .filter(mode => Boolean(mode.value))
}

function defaultModeForPreset(def: PoolPresetMeta): string | null {
  const options = getModeOptions(def)
  if (options.length === 0) return null
  const normalizedDefault = normalizeMode(def.default_mode)
  if (normalizedDefault && options.some(option => option.value === normalizedDefault)) {
    return normalizedDefault
  }
  return options[0].value
}

function buildPresetListItem(def: PoolPresetMeta, enabled: boolean, mode?: unknown): PresetListItem {
  return {
    preset: def.name,
    label: def.label,
    desc: def.description,
    enabled,
    mode: mode !== undefined ? resolveMode(def, mode) : defaultModeForPreset(def),
    modeOptions: getModeOptions(def),
    applicable: isApplicablePreset(def),
    mutexGroup: normalizeMutexGroup(def.mutex_group),
    evidenceHint: String(def.evidence_hint ?? '').trim(),
  }
}

function buildDefaultPresetList(): PresetListItem[] {
  return getPresetDefs().map(def => buildPresetListItem(def, DEFAULT_ENABLED_PRESETS.has(def.name)))
}

function isNewFormatPresetItem(item: unknown): item is SchedulingPresetItem {
  return typeof item === 'object' && item !== null && 'preset' in item
}

function resolveMode(def: PoolPresetMeta, mode: unknown): string | null {
  const options = getModeOptions(def)
  if (options.length === 0) return null
  const normalized = normalizeMode(mode)
  if (normalized && options.some(option => option.value === normalized)) {
    return normalized
  }
  return defaultModeForPreset(def)
}

function insertMissingByPreferredOrder(
  ordered: PresetListItem[],
  seen: Set<string>,
  defs: PoolPresetMeta[],
  defsByName: Map<string, PoolPresetMeta>,
) {
  // Insert missing presets at their preferred position from FALLBACK_ORDER
  // rather than appending to the end.
  for (const name of FALLBACK_ORDER) {
    if (seen.has(name)) continue
    const def = defsByName.get(name)
    if (!def) continue
    seen.add(name)
    const item = buildPresetListItem(def, false)
    // Find the best insertion point: right after the last item whose
    // FALLBACK_ORDER index is smaller than ours.
    const myIdx = FALLBACK_ORDER.indexOf(name)
    let insertAt = ordered.length
    for (let i = ordered.length - 1; i >= 0; i--) {
      const peerIdx = FALLBACK_ORDER.indexOf(ordered[i].preset)
      if (peerIdx !== -1 && peerIdx < myIdx) {
        insertAt = i + 1
        break
      }
      if (i === 0) insertAt = 0
    }
    ordered.splice(insertAt, 0, item)
  }
  // Any remaining defs not in FALLBACK_ORDER go to the end.
  for (const def of defs) {
    if (seen.has(def.name)) continue
    seen.add(def.name)
    ordered.push(buildPresetListItem(def, false))
  }
}

function reorderDistributionGroup(items: PresetListItem[]): PresetListItem[] {
  // Distribution mode items are rendered as a fixed button group,
  // so their order should always match FALLBACK_ORDER regardless of saved config.
  const distIndexes: number[] = []
  const distItems: PresetListItem[] = []
  items.forEach((item, i) => {
    if (item.mutexGroup === DISTRIBUTION_GROUP) {
      distIndexes.push(i)
      distItems.push(item)
    }
  })
  if (distItems.length <= 1) return items

  distItems.sort((a, b) => {
    const ia = FALLBACK_ORDER.indexOf(a.preset)
    const ib = FALLBACK_ORDER.indexOf(b.preset)
    return (ia === -1 ? 9999 : ia) - (ib === -1 ? 9999 : ib)
  })

  const result = [...items]
  distIndexes.forEach((origIdx, i) => {
    result[origIdx] = distItems[i]
  })
  return result
}

function loadFromConfig(cfg: PoolAdvancedConfig | null): PresetListItem[] {
  const defs = getPresetDefs()
  const defsByName = new Map(defs.map(def => [def.name, def]))
  const defaults = buildDefaultPresetList()
  if (!cfg) return defaults

  const rawPresets = cfg.scheduling_presets
  if (!Array.isArray(rawPresets) || rawPresets.length === 0) {
    if (cfg.scheduling_mode === 'lru' || (!cfg.scheduling_mode && cfg.lru_enabled !== false)) {
      return defaults.map(item => ({
        ...item,
        enabled: item.preset === 'lru',
      }))
    }
    return defaults
  }

  const first = rawPresets[0]
  if (isNewFormatPresetItem(first)) {
    const configItems = rawPresets as SchedulingPresetItem[]
    const ordered: PresetListItem[] = []
    const seen = new Set<string>()

    for (const ci of configItems) {
      const presetName = normalizePresetName(ci.preset)
      const def = defsByName.get(presetName)
      if (!def || seen.has(presetName)) continue
      seen.add(presetName)
      ordered.push(buildPresetListItem(def, ci.enabled !== false, ci.mode))
    }

    insertMissingByPreferredOrder(ordered, seen, defs, defsByName)
    return reorderDistributionGroup(ordered)
  }

  const legacyPresets = rawPresets as string[]
  const lruEnabled = cfg.lru_enabled !== false
  const ordered: PresetListItem[] = []
  const seen = new Set<string>()

  const lruDef = defsByName.get('lru')
  if (lruDef) {
    ordered.push(buildPresetListItem(lruDef, lruEnabled))
    seen.add('lru')
  }

  for (const name of legacyPresets) {
    const presetName = normalizePresetName(name)
    const def = defsByName.get(presetName)
    if (!def || seen.has(presetName)) continue
    seen.add(presetName)
    ordered.push(buildPresetListItem(def, true, undefined))
  }

  insertMissingByPreferredOrder(ordered, seen, defs, defsByName)
  return reorderDistributionGroup(ordered)
}

function normalizeMutexSelection(items: PresetListItem[]): PresetListItem[] {
  const next = [...items]
  const groups = new Map<string, number[]>()

  next.forEach((item, index) => {
    if (!item.mutexGroup) return
    if (!groups.has(item.mutexGroup)) groups.set(item.mutexGroup, [])
    groups.get(item.mutexGroup)?.push(index)
  })

  for (const indexes of groups.values()) {
    if (indexes.length <= 1) continue
    const enabledApplicable = indexes.find(index => {
      const item = next[index]
      return item.enabled && item.applicable
    })
    const firstApplicable = indexes.find(index => next[index].applicable)
    const winner = enabledApplicable ?? firstApplicable ?? indexes[0]
    indexes.forEach((index) => {
      next[index].enabled = index === winner && next[index].applicable
    })
  }

  return next
}

function togglePreset(index: number, enabled: boolean) {
  const item = presetList.value[index]
  if (!item) return
  item.enabled = enabled
}

function selectDistribution(_anchorIndex: number, presetName: string) {
  presetList.value.forEach(item => {
    if (item.mutexGroup === DISTRIBUTION_GROUP) {
      item.enabled = item.preset === presetName && item.applicable
    }
  })
}

function setPresetModeByPreset(preset: string, mode: string) {
  const targetIndex = presetList.value.findIndex(item => item.preset === preset)
  if (targetIndex < 0) return
  presetList.value[targetIndex].mode = mode
}

const distributionItems = computed(() => {
  const items: { index: number; item: PresetListItem }[] = []
  presetList.value.forEach((item, index) => {
    if (item.mutexGroup === DISTRIBUTION_GROUP) {
      items.push({ index, item })
    }
  })
  return items
})

const activeDistributionPreset = computed(() => {
  const found = distributionItems.value.find(({ item }) => item.enabled && item.applicable)
  return found?.item.preset ?? null
})

const activeDistributionDesc = computed(() => {
  const found = distributionItems.value.find(({ item }) => item.enabled && item.applicable)
  return found?.item.desc ?? null
})

const activeDistributionLabel = computed(() => {
  const found = distributionItems.value.find(({ item }) => item.enabled && item.applicable)
  return found?.item.label ?? null
})

const strategyItems = computed(() => {
  const items: { index: number; item: PresetListItem }[] = []
  presetList.value.forEach((item, index) => {
    if (!item.mutexGroup) {
      items.push({ index, item })
    }
  })
  return items
})

const enabledStrategyPriorityMap = computed(() => {
  const priorities = new Map<number, number>()
  let priority = 0

  strategyItems.value.forEach(({ index, item }) => {
    if (!item.enabled || !item.applicable) return
    priority += 1
    priorities.set(index, priority)
  })

  return priorities
})

const enabledStrategyCount = computed(() => enabledStrategyPriorityMap.value.size)

function getStrategyPriority(index: number): number | null {
  return enabledStrategyPriorityMap.value.get(index) ?? null
}

function canMoveStrategy(index: number, direction: -1 | 1): boolean {
  const strategyIndexes = strategyItems.value.map(({ index: currentIndex }) => currentIndex)
  const currentPosition = strategyIndexes.indexOf(index)
  if (currentPosition === -1) return false
  const targetPosition = currentPosition + direction
  return targetPosition >= 0 && targetPosition < strategyIndexes.length
}

function moveStrategy(index: number, direction: -1 | 1) {
  presetList.value = moveStrategyItem(presetList.value, index, direction)
}

function handleDragStart(index: number, event: DragEvent) {
  draggedIndex.value = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/html', '')
  }
}

function handleDragEnd() {
  draggedIndex.value = null
  dragOverIndex.value = null
}

function handleDragOver(index: number) {
  dragOverIndex.value = index
}

function handleDragLeave() {
  dragOverIndex.value = null
}

function handleDrop(dropIndex: number) {
  if (draggedIndex.value === null || draggedIndex.value === dropIndex) {
    draggedIndex.value = null
    dragOverIndex.value = null
    return
  }
  const items = [...presetList.value]
  const [draggedItem] = items.splice(draggedIndex.value, 1)
  items.splice(dropIndex, 0, draggedItem)
  presetList.value = items
  draggedIndex.value = null
  dragOverIndex.value = null
}

watch(() => props.modelValue, async (open) => {
  if (!open) return
  await ensurePresetDefsLoaded()
  presetList.value = normalizeMutexSelection(loadFromConfig(props.currentConfig))
})

async function handleSave() {
  loading.value = true
  try {
    presetList.value = normalizeMutexSelection(presetList.value)
    const schedulingPresets: SchedulingPresetItem[] = presetList.value.map(item => {
      const result: SchedulingPresetItem = {
        preset: item.preset,
        enabled: item.enabled && item.applicable,
      }
      if (item.modeOptions.length > 0 && item.mode) {
        result.mode = item.mode
      }
      return result
    })

    const mergedAdvanced: Record<string, unknown> = {
      ...(props.currentConfig ?? {}),
      scheduling_presets: schedulingPresets,
    }
    const payload: Parameters<typeof updateProvider>[1] = {
      pool_advanced: mergedAdvanced as PoolAdvancedConfig,
    }
    const updatedProvider = await updateProvider(props.providerId, payload)

    success('号池调度已保存')
    emit('saved', updatedProvider)
    emit('update:modelValue', false)
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    loading.value = false
  }
}
</script>
