<template>
  <Dialog
    :model-value="modelValue"
    title="号池调度"
    description="拖拽排序调度维度，越靠前优先级越高"
    size="lg"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="space-y-5">
      <!-- Preset List -->
      <div class="space-y-3">
        <div class="space-y-1">
          <h3 class="text-sm font-medium border-b pb-2">
            调度维度
          </h3>
          <p class="text-xs text-muted-foreground">
            拖拽排序，越靠前优先级越高。不适用当前 Provider 类型的维度已禁用。
          </p>
        </div>

        <div class="space-y-0.5">
          <div
            v-for="(item, index) in presetList"
            :key="item.preset"
            class="group flex items-center gap-3 px-3 py-2.5 rounded-lg border transition-all duration-200"
            :class="[
              !item.applicable
                ? 'border-border/30 bg-muted/20 opacity-50'
                : draggedIndex === index
                  ? 'border-primary/50 bg-primary/5 shadow-md scale-[1.01]'
                  : dragOverIndex === index
                    ? 'border-primary/30 bg-primary/5'
                    : 'border-border/50 bg-background hover:border-border hover:bg-muted/30'
            ]"
            :draggable="item.applicable"
            @dragstart="item.applicable && handleDragStart(index, $event)"
            @dragend="handleDragEnd"
            @dragover.prevent="item.applicable && handleDragOver(index)"
            @dragleave="handleDragLeave"
            @drop="item.applicable && handleDrop(index)"
          >
            <!-- Drag handle -->
            <div
              class="p-1 rounded transition-colors shrink-0"
              :class="item.applicable
                ? 'cursor-grab active:cursor-grabbing text-muted-foreground/40 group-hover:text-muted-foreground'
                : 'text-muted-foreground/15 cursor-default'"
            >
              <GripVertical class="w-4 h-4" />
            </div>

            <!-- Enable/disable switch -->
            <Switch
              :model-value="item.enabled"
              :disabled="!item.applicable"
              @update:model-value="(v: boolean) => togglePreset(index, v)"
            />

            <!-- Info -->
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2">
                <span
                  class="text-sm font-medium"
                  :class="!item.applicable ? 'text-muted-foreground' : ''"
                >{{ item.label }}</span>
                <span
                  v-if="!item.applicable"
                  class="text-[10px] text-muted-foreground/60"
                >
                  (不适用)
                </span>
              </div>
              <p class="text-xs text-muted-foreground mt-0.5">
                {{ item.desc }}
              </p>

              <!-- Mode sub-config -->
              <div
                v-if="item.modeOptions.length > 0 && item.enabled && item.applicable"
                class="flex gap-0.5 mt-2 p-0.5 bg-muted/40 rounded-md w-fit"
              >
                <button
                  v-for="modeOpt in item.modeOptions"
                  :key="modeOpt.value"
                  type="button"
                  class="px-2.5 py-1 text-xs font-medium rounded transition-all"
                  :class="[
                    item.mode === modeOpt.value
                      ? 'bg-primary text-primary-foreground shadow-sm'
                      : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                  ]"
                  @click="setPresetMode(index, modeOpt.value)"
                >
                  {{ modeOpt.label }}
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Advanced toggle -->
      <div class="pt-1">
        <Button
          type="button"
          size="sm"
          variant="ghost"
          @click="showAdvanced = !showAdvanced"
        >
          {{ showAdvanced ? '收起高级参数' : '展开高级参数' }}
        </Button>
      </div>

      <!-- Advanced options -->
      <div
        v-if="showAdvanced"
        class="space-y-4"
      >
        <!-- Cooldown & Health -->
        <div class="space-y-3">
          <h3 class="text-sm font-medium border-b pb-2">
            冷却与健康
          </h3>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">健康策略</span>
              <p class="text-xs text-muted-foreground">
                按上游错误自动冷却并跳过账号
              </p>
            </div>
            <Switch
              :model-value="form.health_policy_enabled"
              @update:model-value="(v: boolean) => form.health_policy_enabled = v"
            />
          </div>
          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-1.5">
              <Label>
                429 冷却
                <span class="text-xs text-muted-foreground">(秒)</span>
              </Label>
              <Input
                :model-value="form.rate_limit_cooldown_seconds ?? ''"
                type="number"
                min="10"
                max="3600"
                placeholder="300"
                @update:model-value="(v) => form.rate_limit_cooldown_seconds = parseNum(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>
                529 冷却
                <span class="text-xs text-muted-foreground">(秒)</span>
              </Label>
              <Input
                :model-value="form.overload_cooldown_seconds ?? ''"
                type="number"
                min="5"
                max="600"
                placeholder="30"
                @update:model-value="(v) => form.overload_cooldown_seconds = parseNum(v)"
              />
            </div>
          </div>
          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-1.5">
              <Label>
                粘性会话 TTL
                <span class="text-xs text-muted-foreground">(秒)</span>
              </Label>
              <Input
                :model-value="form.sticky_session_ttl_seconds ?? ''"
                type="number"
                min="60"
                max="86400"
                placeholder="3600 (留空禁用)"
                @update:model-value="(v) => form.sticky_session_ttl_seconds = parseNum(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>
                全局优先级
                <span class="text-xs text-muted-foreground">(global_key)</span>
              </Label>
              <Input
                :model-value="form.global_priority ?? ''"
                type="number"
                min="0"
                max="999999"
                placeholder="留空回退 provider_priority"
                @update:model-value="(v) => form.global_priority = parseNum(v)"
              />
            </div>
          </div>
        </div>

        <!-- Claude Code -->
        <div
          v-if="isClaudeCode"
          class="space-y-3"
        >
          <h3 class="text-sm font-medium border-b pb-2">
            Claude Code
          </h3>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">TLS 指纹模拟</span>
              <p class="text-xs text-muted-foreground">
                模拟 Node.js / Claude Code 客户端指纹
              </p>
            </div>
            <Switch
              :model-value="claudeForm.enable_tls_fingerprint"
              @update:model-value="(v: boolean) => claudeForm.enable_tls_fingerprint = v"
            />
          </div>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">Session ID 伪装</span>
              <p class="text-xs text-muted-foreground">
                固定 metadata.user_id 中 session 片段
              </p>
            </div>
            <Switch
              :model-value="claudeForm.session_id_masking_enabled"
              @update:model-value="(v: boolean) => claudeForm.session_id_masking_enabled = v"
            />
          </div>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">仅限 CLI 客户端</span>
              <p class="text-xs text-muted-foreground">
                仅允许 Claude Code CLI 格式请求
              </p>
            </div>
            <Switch
              :model-value="claudeForm.cli_only_enabled"
              @update:model-value="(v: boolean) => claudeForm.cli_only_enabled = v"
            />
          </div>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">Cache TTL 统一</span>
              <p class="text-xs text-muted-foreground">
                强制所有 cache_control 使用相同 TTL 类型
              </p>
            </div>
            <Switch
              :model-value="claudeForm.cache_ttl_override_enabled"
              @update:model-value="(v: boolean) => claudeForm.cache_ttl_override_enabled = v"
            />
          </div>
          <div
            v-if="claudeForm.cache_ttl_override_enabled"
            class="pl-3"
          >
            <div class="space-y-1.5">
              <Label>TTL 类型</Label>
              <div class="flex gap-0.5 p-0.5 bg-muted/40 rounded-md w-fit">
                <button
                  v-for="opt in ['ephemeral']"
                  :key="opt"
                  type="button"
                  class="px-2.5 py-1 text-xs font-medium rounded transition-all"
                  :class="[
                    claudeForm.cache_ttl_override_target === opt
                      ? 'bg-primary text-primary-foreground shadow-sm'
                      : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                  ]"
                  @click="claudeForm.cache_ttl_override_target = opt"
                >
                  {{ opt }}
                </button>
              </div>
            </div>
          </div>
          <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
            <div class="space-y-0.5">
              <span class="text-sm font-medium">会话数量控制</span>
              <p class="text-xs text-muted-foreground">
                限制单 Key 同时活跃会话数
              </p>
            </div>
            <Switch
              :model-value="claudeForm.session_control_enabled"
              @update:model-value="(v: boolean) => claudeForm.session_control_enabled = v"
            />
          </div>
          <div
            v-if="claudeForm.session_control_enabled"
            class="grid grid-cols-2 gap-4"
          >
            <div class="space-y-1.5">
              <Label>
                最大会话数
              </Label>
              <Input
                :model-value="claudeForm.max_sessions ?? ''"
                type="number"
                min="1"
                max="100"
                placeholder="留空 = 不限"
                @update:model-value="(v) => claudeForm.max_sessions = parseNum(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>
                空闲超时
                <span class="text-xs text-muted-foreground">(分钟)</span>
              </Label>
              <Input
                :model-value="claudeForm.session_idle_timeout_minutes ?? ''"
                type="number"
                min="1"
                max="1440"
                placeholder="5"
                @update:model-value="(v) => claudeForm.session_idle_timeout_minutes = parseNum(v) ?? 5"
              />
            </div>
          </div>
        </div>

        <!-- Cost Control -->
        <div class="space-y-3">
          <h3 class="text-sm font-medium border-b pb-2">
            成本控制
          </h3>
          <div class="grid grid-cols-2 gap-4">
            <div class="space-y-1.5">
              <Label>
                成本窗口
                <span class="text-xs text-muted-foreground">(秒)</span>
              </Label>
              <Input
                :model-value="form.cost_window_seconds ?? ''"
                type="number"
                min="3600"
                max="86400"
                placeholder="18000 (5 小时)"
                @update:model-value="(v) => form.cost_window_seconds = parseNum(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>
                Key 窗口限额
                <span class="text-xs text-muted-foreground">(tokens)</span>
              </Label>
              <Input
                :model-value="form.cost_limit_per_key_tokens ?? ''"
                type="number"
                min="0"
                placeholder="留空 = 不限"
                @update:model-value="(v) => form.cost_limit_per_key_tokens = parseNum(v)"
              />
            </div>
            <div class="space-y-1.5">
              <Label>
                软阈值
                <span class="text-xs text-muted-foreground">(%)</span>
              </Label>
              <Input
                :model-value="form.cost_soft_threshold_percent ?? ''"
                type="number"
                min="0"
                max="100"
                placeholder="80"
                @update:model-value="(v) => form.cost_soft_threshold_percent = parseNum(v)"
              />
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="loading"
        @click="emit('update:modelValue', false)"
      >
        取消
      </Button>
      <Button
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
import { Dialog, Button, Input, Label, Switch } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { updateProvider } from '@/api/endpoints'
import { getPoolSchedulingPresets } from '@/api/endpoints/pool'
import type { PoolPresetMeta } from '@/api/endpoints/pool'
import type { PoolAdvancedConfig, ClaudeCodeAdvancedConfig, SchedulingPresetItem } from '@/api/endpoints/types/provider'

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
}

const props = defineProps<{
  modelValue: boolean
  providerId: string
  providerType?: string
  currentConfig: PoolAdvancedConfig | null
  currentClaudeConfig?: ClaudeCodeAdvancedConfig | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  saved: [payload: {
    pool_advanced: PoolAdvancedConfig | null
    claude_code_advanced: ClaudeCodeAdvancedConfig | null
  }]
}>()

const FALLBACK_PRESET_DEFS: PoolPresetMeta[] = [
  {
    name: 'lru',
    label: 'LRU 轮转',
    description: '最久未使用的 Key 优先',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'free_team_first',
    label: 'Free/Team 优先',
    description: '优先消耗低档账号（依赖 plan_type）',
    providers: ['codex', 'kiro'],
    modes: [
      { value: 'free_only', label: 'Free' },
      { value: 'team_only', label: 'Team' },
      { value: 'both', label: '全部' },
    ],
    default_mode: 'both',
  },
  {
    name: 'quota_balanced',
    label: '额度平均',
    description: '优先选额度消耗最少的账号',
    providers: [],
    modes: null,
    default_mode: null,
  },
  {
    name: 'recent_refresh',
    label: '额度刷新优先',
    description: '优先选即将刷新额度的账号',
    providers: ['codex', 'kiro'],
    modes: null,
    default_mode: null,
  },
  {
    name: 'single_account',
    label: '单号优先',
    description: '集中使用同一账号（反向 LRU）',
    providers: [],
    modes: null,
    default_mode: null,
  },
]

const DEFAULT_ENABLED_PRESETS = new Set(['lru', 'quota_balanced'])

const { success, error: showError } = useToast()
const loading = ref(false)
const showAdvanced = ref(false)
const presetDefs = ref<PoolPresetMeta[]>([])
const presetDefsLoaded = ref(false)
const loadingPresetDefs = ref(false)

const draggedIndex = ref<number | null>(null)
const dragOverIndex = ref<number | null>(null)
const presetList = ref<PresetListItem[]>([])

const form = ref({
  global_priority: null as number | null | undefined,
  sticky_session_ttl_seconds: null as number | null | undefined,
  health_policy_enabled: true,
  rate_limit_cooldown_seconds: null as number | null | undefined,
  overload_cooldown_seconds: null as number | null | undefined,
  cost_window_seconds: null as number | null | undefined,
  cost_limit_per_key_tokens: null as number | null | undefined,
  cost_soft_threshold_percent: null as number | null | undefined,
})

const isClaudeCode = computed(() => normalizeProviderType(props.providerType) === 'claude_code')

interface ClaudeFormState {
  session_control_enabled: boolean
  max_sessions: number | undefined
  session_idle_timeout_minutes: number
  enable_tls_fingerprint: boolean
  session_id_masking_enabled: boolean
  cache_ttl_override_enabled: boolean
  cache_ttl_override_target: string
  cli_only_enabled: boolean
}

const claudeForm = ref<ClaudeFormState>({
  session_control_enabled: true,
  max_sessions: undefined,
  session_idle_timeout_minutes: 5,
  enable_tls_fingerprint: true,
  session_id_masking_enabled: true,
  cache_ttl_override_enabled: false,
  cache_ttl_override_target: 'ephemeral',
  cli_only_enabled: false,
})

function parseNum(v: string | number): number | undefined {
  if (v === '' || v === null || v === undefined) return undefined
  const n = Number(v)
  return Number.isNaN(n) ? undefined : n
}

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
    })
  }
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

function buildDefaultPresetList(): PresetListItem[] {
  return getPresetDefs().map(def => ({
    preset: def.name,
    label: def.label,
    desc: def.description,
    enabled: DEFAULT_ENABLED_PRESETS.has(def.name),
    mode: defaultModeForPreset(def),
    modeOptions: getModeOptions(def),
    applicable: isApplicablePreset(def),
  }))
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
      ordered.push({
        preset: presetName,
        label: def.label,
        desc: def.description,
        enabled: ci.enabled !== false,
        mode: resolveMode(def, ci.mode),
        modeOptions: getModeOptions(def),
        applicable: isApplicablePreset(def),
      })
    }

    for (const def of defs) {
      if (seen.has(def.name)) continue
      ordered.push({
        preset: def.name,
        label: def.label,
        desc: def.description,
        enabled: false,
        mode: defaultModeForPreset(def),
        modeOptions: getModeOptions(def),
        applicable: isApplicablePreset(def),
      })
    }
    return ordered
  }

  const legacyPresets = rawPresets as string[]
  const lruEnabled = cfg.lru_enabled !== false
  const ordered: PresetListItem[] = []
  const seen = new Set<string>()

  const lruDef = defsByName.get('lru')
  if (lruDef) {
    ordered.push({
      preset: 'lru',
      label: lruDef.label,
      desc: lruDef.description,
      enabled: lruEnabled,
      mode: null,
      modeOptions: [],
      applicable: isApplicablePreset(lruDef),
    })
    seen.add('lru')
  }

  for (const name of legacyPresets) {
    const presetName = normalizePresetName(name)
    const def = defsByName.get(presetName)
    if (!def || seen.has(presetName)) continue
    seen.add(presetName)
    ordered.push({
      preset: presetName,
      label: def.label,
      desc: def.description,
      enabled: true,
      mode: resolveMode(def, undefined),
      modeOptions: getModeOptions(def),
      applicable: isApplicablePreset(def),
    })
  }

  for (const def of defs) {
    if (seen.has(def.name)) continue
    ordered.push({
      preset: def.name,
      label: def.label,
      desc: def.description,
      enabled: false,
      mode: defaultModeForPreset(def),
      modeOptions: getModeOptions(def),
      applicable: isApplicablePreset(def),
    })
  }
  return ordered
}

function togglePreset(index: number, enabled: boolean) {
  presetList.value[index].enabled = enabled
}

function setPresetMode(index: number, mode: string) {
  presetList.value[index].mode = mode
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
  showAdvanced.value = false
  await ensurePresetDefsLoaded()
  presetList.value = loadFromConfig(props.currentConfig)

  const cfg = props.currentConfig
  form.value = {
    global_priority: cfg?.global_priority ?? null,
    sticky_session_ttl_seconds: cfg?.sticky_session_ttl_seconds ?? null,
    health_policy_enabled: cfg?.health_policy_enabled !== false,
    rate_limit_cooldown_seconds: cfg?.rate_limit_cooldown_seconds ?? null,
    overload_cooldown_seconds: cfg?.overload_cooldown_seconds ?? null,
    cost_window_seconds: cfg?.cost_window_seconds ?? null,
    cost_limit_per_key_tokens: cfg?.cost_limit_per_key_tokens ?? null,
    cost_soft_threshold_percent: cfg?.cost_soft_threshold_percent ?? null,
  }

  const cc = props.currentClaudeConfig
  claudeForm.value = {
    session_control_enabled: cc?.max_sessions !== null,
    max_sessions: cc?.max_sessions ?? undefined,
    session_idle_timeout_minutes: cc?.session_idle_timeout_minutes ?? 5,
    enable_tls_fingerprint: cc?.enable_tls_fingerprint !== false,
    session_id_masking_enabled: cc?.session_id_masking_enabled !== false,
    cache_ttl_override_enabled: cc?.cache_ttl_override_enabled ?? false,
    cache_ttl_override_target: cc?.cache_ttl_override_target ?? 'ephemeral',
    cli_only_enabled: cc?.cli_only_enabled ?? false,
  }
})

async function handleSave() {
  loading.value = true
  try {
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

    const payload: Parameters<typeof updateProvider>[1] = {
      pool_advanced: {
        global_priority: form.value.global_priority ?? undefined,
        sticky_session_ttl_seconds: form.value.sticky_session_ttl_seconds ?? undefined,
        scheduling_presets: schedulingPresets,
        scoring_weights: undefined,
        latency_window_seconds: undefined,
        latency_sample_limit: undefined,
        cost_window_seconds: form.value.cost_window_seconds ?? undefined,
        cost_limit_per_key_tokens: form.value.cost_limit_per_key_tokens ?? undefined,
        cost_soft_threshold_percent: form.value.cost_soft_threshold_percent ?? undefined,
        rate_limit_cooldown_seconds: form.value.rate_limit_cooldown_seconds ?? undefined,
        overload_cooldown_seconds: form.value.overload_cooldown_seconds ?? undefined,
        health_policy_enabled: form.value.health_policy_enabled,
      },
    }
    if (isClaudeCode.value) {
      const cf = claudeForm.value
      payload.claude_code_advanced = {
        max_sessions: cf.session_control_enabled ? (cf.max_sessions ?? null) : null,
        session_idle_timeout_minutes: cf.session_control_enabled ? cf.session_idle_timeout_minutes : null,
        enable_tls_fingerprint: cf.enable_tls_fingerprint,
        session_id_masking_enabled: cf.session_id_masking_enabled,
        cache_ttl_override_enabled: cf.cache_ttl_override_enabled,
        cache_ttl_override_target: cf.cache_ttl_override_enabled ? cf.cache_ttl_override_target : undefined,
        cli_only_enabled: cf.cli_only_enabled,
      }
    }
    await updateProvider(props.providerId, payload)
    success('号池调度已保存')
    emit('saved', {
      pool_advanced: payload.pool_advanced ?? null,
      claude_code_advanced: payload.claude_code_advanced ?? null,
    })
    emit('update:modelValue', false)
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    loading.value = false
  }
}
</script>
