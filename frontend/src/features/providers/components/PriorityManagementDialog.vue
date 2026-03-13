<template>
  <Dialog
    :model-value="internalOpen"
    title="优先级管理"
    description="拖拽调整顺序，点击序号可编辑（相同数字为同级），保存后自动切换对应的调度策略"
    :icon="ListOrdered"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-4">
      <!-- 主 Tab 切换 -->
      <div class="flex gap-1 p-1 bg-muted/40 rounded-lg">
        <button
          type="button"
          class="flex-1 flex items-center justify-center gap-2 px-4 py-2 text-sm font-medium rounded-md transition-all duration-200"
          :class="[
            activeMainTab === 'provider'
              ? 'bg-primary text-primary-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
          ]"
          @click="activeMainTab = 'provider'"
        >
          <Layers class="w-4 h-4" />
          <span>提供商优先</span>
        </button>
        <button
          type="button"
          class="flex-1 flex items-center justify-center gap-2 px-4 py-2 text-sm font-medium rounded-md transition-all duration-200"
          :class="[
            activeMainTab === 'key'
              ? 'bg-primary text-primary-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
          ]"
          @click="activeMainTab = 'key'"
        >
          <Key class="w-4 h-4" />
          <span>Key 优先</span>
        </button>
      </div>

      <!-- 内容区域：固定高度，切换 tab 时不跳动 -->
      <div class="h-[min(65vh,520px)]">
        <!-- 提供商优先级 -->
        <div
          v-show="activeMainTab === 'provider'"
          class="h-full"
        >
          <!-- 空状态 -->
          <div
            v-if="sortedProviders.length === 0"
            class="flex flex-col items-center justify-center py-20 text-muted-foreground"
          >
            <Layers class="w-10 h-10 mb-3 opacity-20" />
            <span class="text-sm">暂无提供商</span>
          </div>

          <!-- 提供商列表 -->
          <div
            v-else
            class="space-y-0.5 h-full overflow-y-auto pr-1"
          >
            <div
              v-for="(provider, index) in sortedProviders"
              :key="provider.id"
              class="group flex items-center gap-3 px-3 py-1.5 rounded-lg border transition-all duration-200"
              :class="[
                draggedProvider === index
                  ? 'border-primary/50 bg-primary/5 shadow-md scale-[1.01]'
                  : dragOverProvider === index
                    ? 'border-primary/30 bg-primary/5'
                    : 'border-border/50 bg-background hover:border-border hover:bg-muted/30'
              ]"
              draggable="true"
              @dragstart="handleProviderDragStart(index, $event)"
              @dragend="handleProviderDragEnd"
              @dragover.prevent="handleProviderDragOver(index)"
              @dragleave="handleProviderDragLeave"
              @drop="handleProviderDrop(index)"
            >
              <!-- 拖拽手柄 -->
              <div class="cursor-grab active:cursor-grabbing p-1 rounded hover:bg-muted text-muted-foreground/40 group-hover:text-muted-foreground transition-colors">
                <GripVertical class="w-4 h-4" />
              </div>

              <!-- 可编辑序号 -->
              <div class="shrink-0">
                <input
                  v-if="editingProviderPriority === provider.id"
                  type="number"
                  min="1"
                  :value="provider.provider_priority"
                  class="w-8 h-6 rounded-md bg-background border border-primary text-xs font-medium text-center focus:outline-none [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                  autofocus
                  @blur="finishEditProviderPriority(provider, $event)"
                  @keydown.enter="($event.target as HTMLInputElement).blur()"
                  @keydown.escape="cancelEditProviderPriority()"
                >
                <div
                  v-else
                  class="w-6 h-6 rounded-md bg-muted/50 flex items-center justify-center text-xs font-medium text-muted-foreground cursor-pointer hover:bg-primary/10 hover:text-primary transition-colors"
                  title="点击编辑优先级，相同数字为同级"
                  @click.stop="startEditProviderPriority(provider)"
                >
                  {{ provider.provider_priority }}
                </div>
              </div>

              <!-- 提供商信息 -->
              <div class="flex-1 min-w-0 flex items-center gap-2">
                <span class="font-medium text-sm truncate">{{ provider.name }}</span>
                <Badge
                  v-if="!provider.is_active"
                  variant="secondary"
                  class="text-[10px] px-1.5 h-5 shrink-0"
                >
                  停用
                </Badge>
              </div>
              <div class="flex items-center gap-3 shrink-0 ml-2">
                <!-- API 格式标签 (自适应宽度) -->
                <div class="flex items-center justify-end gap-1">
                  <template v-if="provider.api_formats?.length">
                    <span
                      v-for="fmt in provider.api_formats"
                      :key="fmt"
                      class="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground whitespace-nowrap"
                    >
                      {{ API_FORMAT_SHORT[fmt] || fmt }}
                    </span>
                  </template>
                </div>
                
                <!-- 余额显示 (表格对齐) -->
                <div class="min-w-[4rem] text-right">
                  <span
                    v-if="formatBalanceDisplay(provider.id)"
                    class="text-xs font-semibold text-foreground/90 tabular-nums"
                  >
                    {{ formatBalanceDisplay(provider.id) }}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Key 优先级 -->
        <div
          v-show="activeMainTab === 'key'"
          class="h-full"
        >
          <!-- 加载状态 -->
          <div
            v-if="loadingKeys"
            class="flex items-center justify-center py-20"
          >
            <div class="flex flex-col items-center gap-2">
              <div class="animate-spin rounded-full h-5 w-5 border-2 border-muted border-t-primary" />
              <span class="text-xs text-muted-foreground">加载中...</span>
            </div>
          </div>

          <!-- 空状态 -->
          <div
            v-else-if="availableFormats.length === 0"
            class="flex flex-col items-center justify-center py-20 text-muted-foreground"
          >
            <Key class="w-10 h-10 mb-3 opacity-20" />
            <span class="text-sm">暂无 API Key</span>
          </div>

          <!-- 左右布局：格式列表 + Key 列表 -->
          <div
            v-else
            class="flex gap-0 h-full"
          >
            <!-- 左侧：API 格式列表（按 family 分组） -->
            <div class="w-36 shrink-0 overflow-y-auto border-r border-border/50 pr-3 mr-3 py-0.5">
              <div
                v-for="(group, gi) in groupedFormats"
                :key="group.family"
                :class="gi > 0 ? 'mt-3' : ''"
              >
                <div class="px-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60">
                  {{ group.label }}
                </div>
                <div class="space-y-0.5">
                  <button
                    v-for="format in group.formats"
                    :key="format"
                    type="button"
                    class="w-full px-3 py-1.5 text-xs font-medium rounded-lg text-left transition-all duration-200"
                    :class="[
                      activeFormatTab === format
                        ? 'bg-primary text-primary-foreground shadow-sm'
                        : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                    ]"
                    @click="activeFormatTab = format"
                  >
                    {{ formatKind(format) }}
                  </button>
                </div>
              </div>
            </div>

            <!-- 右侧：Key 列表 -->
            <div class="flex-1 min-w-0 h-full overflow-hidden">
              <div
                v-for="format in availableFormats"
                v-show="activeFormatTab === format"
                :key="format"
                class="h-full"
              >
                <div
                  v-if="displayKeysByFormat[format]?.length > 0"
                  class="space-y-0.5 h-full overflow-y-auto pr-1"
                >
                  <div
                    v-for="key in displayKeysByFormat[format]"
                    :key="key.id"
                    class="group flex items-center gap-2 px-2.5 py-1.5 rounded-lg border transition-all duration-200"
                    :class="[
                      !(key.is_active && key.provider_active)
                        ? 'border-border/30 bg-muted/20 opacity-50'
                        : draggedKey[format] === key.id
                          ? 'border-primary/50 bg-primary/5 shadow-md scale-[1.01]'
                          : dragOverKey[format] === key.id
                            ? 'border-primary/30 bg-primary/5'
                            : 'border-border/50 bg-background hover:border-border hover:bg-muted/30'
                    ]"
                    :draggable="key.is_active && key.provider_active"
                    @dragstart="(key.is_active && key.provider_active) && handleKeyDragStart(format, key.id, $event)"
                    @dragend="handleKeyDragEnd(format)"
                    @dragover.prevent="handleKeyDragOver(format, key.id)"
                    @dragleave="handleKeyDragLeave(format)"
                    @drop="handleKeyDrop(format, key.id)"
                  >
                    <!-- 拖拽手柄 -->
                    <div
                      class="p-0.5 rounded transition-colors shrink-0"
                      :class="(key.is_active && key.provider_active)
                        ? 'cursor-grab active:cursor-grabbing text-muted-foreground/30 group-hover:text-muted-foreground'
                        : 'text-muted-foreground/15 cursor-default'"
                    >
                      <GripVertical class="w-3.5 h-3.5" />
                    </div>

                    <!-- 可编辑序号 -->
                    <div class="shrink-0">
                      <input
                        v-if="editingKeyPriority[format] === key.id"
                        type="number"
                        min="1"
                        :value="key.priority"
                        class="w-7 h-5 rounded bg-background border border-primary text-[11px] font-medium text-center focus:outline-none [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                        autofocus
                        @blur="finishEditKeyPriority(format, key, $event)"
                        @keydown.enter="($event.target as HTMLInputElement).blur()"
                        @keydown.escape="cancelEditKeyPriority(format)"
                      >
                      <div
                        v-else
                        class="w-5 h-5 rounded bg-muted/50 flex items-center justify-center text-[11px] font-medium transition-colors text-muted-foreground cursor-pointer hover:bg-primary/10 hover:text-primary"
                        title="点击编辑优先级"
                        @click.stop="startEditKeyPriority(format, key)"
                      >
                        {{ key.priority }}
                      </div>
                    </div>

                    <!-- Key 信息（两行） -->
                    <div class="flex-1 min-w-0">
                      <!-- 第一行：Key 名称 + Key 级别状态 -->
                      <div class="flex items-center gap-1.5">
                        <span
                          class="text-sm font-medium truncate"
                          :class="!(key.is_active && key.provider_active) ? 'text-muted-foreground' : ''"
                        >{{ key.name }}</span>
                        <Badge
                          v-if="key.is_pool_aggregate"
                          variant="outline"
                          class="text-[9px] h-4 px-1 shrink-0"
                        >
                          号池
                        </Badge>
                        <Badge
                          v-else-if="key.circuit_breaker_open"
                          variant="destructive"
                          class="text-[9px] h-4 px-1 shrink-0"
                        >
                          熔断
                        </Badge>
                        <Badge
                          v-else-if="!key.is_active && key.provider_active"
                          variant="secondary"
                          class="text-[9px] h-4 px-1 shrink-0"
                        >
                          停用
                        </Badge>
                      </div>
                      <!-- 第二行：密钥脱敏 · Provider 名称 + Provider 级别状态 -->
                      <div class="flex items-center gap-0 mt-0.5">
                        <template v-if="key.is_pool_aggregate">
                          <span class="text-[10px] text-muted-foreground/70 truncate">
                            号池: {{ key.pool_active_key_count ?? 0 }}/{{ key.pool_key_count ?? 0 }}
                          </span>
                          <template v-if="key.provider_type">
                            <span class="text-[10px] text-muted-foreground/40 mx-1">·</span>
                            <span class="text-[10px] text-muted-foreground shrink-0">{{ formatProviderType(key.provider_type) }}</span>
                          </template>
                        </template>
                        <template v-else>
                          <span class="font-mono text-[10px] text-muted-foreground/50 truncate">{{ key.api_key_masked }}</span>
                          <span class="text-[10px] text-muted-foreground/40 mx-1">·</span>
                          <span class="text-[10px] text-muted-foreground shrink-0">{{ key.provider_name }}</span>
                        </template>
                        <Badge
                          v-if="!key.provider_active"
                          variant="secondary"
                          class="text-[9px] h-4 px-1 shrink-0 ml-1"
                        >
                          停用
                        </Badge>
                      </div>
                    </div>

                    <!-- 右侧：健康度/倍率 + 开关 -->
                    <div class="shrink-0 flex items-center gap-2">
                      <!-- 健康度 + 倍率（两行） -->
                      <div class="text-right min-w-[36px]">
                        <div
                          v-if="key.health_score != null"
                          class="text-[11px] font-medium tabular-nums text-foreground"
                        >
                          {{ ((key.health_score || 0) * 100).toFixed(0) }}%
                        </div>
                        <div
                          v-else
                          class="text-[11px] text-muted-foreground/40"
                        >
                          --
                        </div>
                        <div class="text-[10px] text-muted-foreground tabular-nums">
                          {{ key.is_pool_aggregate ? 'Pool' : (key.rate_multipliers?.[format] ?? 1) + 'x' }}
                        </div>
                      </div>
                      <!-- 快捷启用/禁用开关 -->
                      <button
                        class="p-0.5 rounded transition-colors shrink-0"
                        :class="(key.is_pool_aggregate || !key.provider_active)
                          ? 'text-muted-foreground/20 cursor-not-allowed'
                          : key.is_active
                            ? 'text-foreground/70 hover:bg-muted hover:text-foreground'
                            : 'text-muted-foreground hover:bg-muted hover:text-foreground'"
                        :title="key.is_pool_aggregate
                          ? '号池聚合项不支持在此单独开关'
                          : !key.provider_active
                            ? 'Provider 停用'
                            : key.is_active
                              ? '点击停用'
                              : '点击启用'"
                        :disabled="key.is_pool_aggregate || !key.provider_active"
                        @click.stop="!key.is_pool_aggregate && toggleKeyActive(format, key)"
                      >
                        <Power class="w-3.5 h-3.5" />
                      </button>
                    </div>
                  </div>
                </div>

                <div
                  v-else
                  class="flex flex-col items-center justify-center py-20 text-muted-foreground"
                >
                  <Key class="w-10 h-10 mb-3 opacity-20" />
                  <span class="text-sm">暂无 {{ format }} 格式的 Key</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <div class="flex items-center justify-between w-full">
        <div class="flex items-center gap-3">
          <div class="text-xs text-muted-foreground whitespace-nowrap">
            当前模式: <span class="font-medium text-foreground/80">{{ activeMainTab === 'provider' ? '提供商优先' : 'Key 优先' }}</span>
          </div>
          <div class="flex items-center gap-1.5 pl-3 border-l border-border/60">
            <span class="text-xs text-muted-foreground">调度:</span>
            <div class="flex gap-0.5 p-0.5 bg-muted/40 rounded-md">
              <button
                type="button"
                class="px-2 py-1 text-xs font-medium rounded transition-all"
                :class="[
                  schedulingMode === 'cache_affinity'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                ]"
                title="优先使用已缓存的Provider，利用Prompt Cache"
                @click="schedulingMode = 'cache_affinity'"
              >
                缓存亲和
              </button>
              <button
                type="button"
                class="px-2 py-1 text-xs font-medium rounded transition-all"
                :class="[
                  schedulingMode === 'load_balance'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                ]"
                title="同优先级内随机轮换，不考虑缓存"
                @click="schedulingMode = 'load_balance'"
              >
                负载均衡
              </button>
              <button
                type="button"
                class="px-2 py-1 text-xs font-medium rounded transition-all"
                :class="[
                  schedulingMode === 'fixed_order'
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                ]"
                title="严格按优先级顺序，不考虑缓存"
                @click="schedulingMode = 'fixed_order'"
              >
                固定顺序
              </button>
            </div>
          </div>
        </div>
        <div class="flex gap-2">
          <Button
            size="sm"
            :disabled="saving"
            class="min-w-[72px]"
            @click="save"
          >
            <Loader2
              v-if="saving"
              class="w-3.5 h-3.5 mr-1.5 animate-spin"
            />
            {{ saving ? '保存中' : '保存' }}
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="min-w-[72px]"
            @click="close"
          >
            取消
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { GripVertical, Layers, Key, Loader2, ListOrdered, Power } from 'lucide-vue-next'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { updateProvider, updateProviderKey } from '@/api/endpoints'
import { getProvidersSummary, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { batchQueryBalance, type ActionResultResponse, type BalanceInfo } from '@/api/providerOps'
import { API_FORMAT_SHORT } from '@/api/endpoints/types'
import { sortApiFormats, groupApiFormats, parseApiFormat, API_FORMAT_KIND_LABELS } from '@/api/endpoints/types/api-format'
import { log } from '@/utils/logger'

interface KeyWithMeta {
  id: string
  provider_id: string
  name: string
  api_key_masked: string
  internal_priority: number
  global_priority_by_format: Record<string, number> | null
  format_priority: number | null  // 当前格式的优先级（后端计算）
  priority: number  // 用于编辑的优先级
  rate_multipliers: Record<string, number> | null
  is_active: boolean
  provider_active: boolean
  circuit_breaker_open: boolean
  provider_name: string
  endpoint_base_url: string
  api_format: string
  capabilities: string[]
  health_score: number | null
  success_rate: number | null
  avg_response_time_ms: number | null
  request_count: number
  is_pool_aggregate?: boolean
  pool_key_count?: number
  pool_active_key_count?: number
  provider_type?: string
}

const props = defineProps<{
  modelValue: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  saved: []
}>()

const { success, error: showError } = useToast()

// 内部状态
const internalOpen = computed(() => props.modelValue)

function handleDialogUpdate(value: boolean) {
  emit('update:modelValue', value)
}

// 主 Tab 状态
const activeMainTab = ref<'provider' | 'key'>('provider')
const activeFormatTab = ref<string>('claude:chat')

// 提供商排序状态
const sortedProviders = ref<ProviderWithEndpointsSummary[]>([])
const draggedProvider = ref<number | null>(null)
const dragOverProvider = ref<number | null>(null)

// Key 排序状态
const keysByFormat = ref<Record<string, KeyWithMeta[]>>({})
const draggedKey = ref<Record<string, string | null>>({})
const dragOverKey = ref<Record<string, string | null>>({})
const loadingKeys = ref(false)
const saving = ref(false)

const SAVE_CONCURRENCY = 6

let originalProviderPriorityById = new Map<string, number>()
let originalPoolPriorityByProviderId = new Map<string, number | null>()
let originalKeyPriorityById = new Map<string, Record<string, number>>()

// Key 优先级编辑状态
const editingKeyPriority = ref<Record<string, string | null>>({})  // format -> keyId

// Provider 优先级编辑状态
const editingProviderPriority = ref<string | null>(null)  // providerId

// 调度模式状态
const schedulingMode = ref<'fixed_order' | 'load_balance' | 'cache_affinity'>('cache_affinity')

// 余额数据缓存 {providerId: ActionResultResponse}
const balanceCache = ref<Record<string, ActionResultResponse>>({})

// 类型守卫函数
function isBalanceInfo(data: unknown): data is BalanceInfo {
  return data !== null && typeof data === 'object' && 'total_available' in data
}

// 获取 provider 的余额显示
function getProviderBalance(providerId: string): { available: number | null; currency: string } | null {
  const result = balanceCache.value[providerId]
  if (!result || result.status !== 'success' || !result.data) {
    return null
  }
  if (!isBalanceInfo(result.data)) {
    return null
  }
  return {
    available: result.data.total_available,
    currency: result.data.currency || 'USD'
  }
}

// 格式化余额显示
function formatBalanceDisplay(providerId: string): string {
  const balance = getProviderBalance(providerId)
  if (!balance || balance.available == null) {
    return ''
  }
  const symbol = balance.currency === 'USD' ? '$' : balance.currency
  return `${symbol}${balance.available.toFixed(2)}`
}

// 异步加载余额数据（使用批量接口）
async function loadBalances() {
  try {
    const opsProviderIds = sortedProviders.value
      .filter(p => p.ops_configured)
      .map(p => p.id)
    if (opsProviderIds.length === 0) return

    const results = await batchQueryBalance(opsProviderIds)

    // 将成功的结果存入缓存
    for (const [providerId, result] of Object.entries(results)) {
      if (result.status === 'success') {
        balanceCache.value[providerId] = result
      }
    }
  } catch (e) {
    log.warn('[loadBalances] 加载余额数据失败', e)
  }
}

const LEGACY_API_FORMAT_MAP: Record<string, string> = {
  CLAUDE: 'claude:chat',
  CLAUDE_CLI: 'claude:cli',
  OPENAI: 'openai:chat',
  OPENAI_CLI: 'openai:cli',
  OPENAI_COMPACT: 'openai:compact',
  OPENAI_VIDEO: 'openai:video',
  GEMINI: 'gemini:chat',
  GEMINI_CLI: 'gemini:cli',
  GEMINI_VIDEO: 'gemini:video',
}

function normalizeApiFormatKey(value: string | null | undefined): string {
  const raw = String(value || '').trim()
  if (!raw) return ''

  if (raw.includes(':')) {
    const [family, kind] = raw.split(':', 2)
    const familyNorm = family?.trim().toLowerCase()
    const kindNorm = kind?.trim().toLowerCase()
    if (familyNorm && kindNorm) return `${familyNorm}:${kindNorm}`
  }

  const legacy = raw.toUpperCase().replace(/-/g, '_')
  return LEGACY_API_FORMAT_MAP[legacy] || raw.toLowerCase()
}

function normalizePriorityMap(
  value: Record<string, unknown> | null | undefined
): Record<string, number> {
  if (!value) return {}
  const normalized: Record<string, number> = {}
  for (const [rawFormat, rawPriority] of Object.entries(value)) {
    const format = normalizeApiFormatKey(rawFormat)
    if (!format || format in normalized) continue
    const num = Number(rawPriority)
    if (!Number.isFinite(num)) continue
    normalized[format] = Math.trunc(num)
  }
  return normalized
}

function normalizeOptionalPriority(value: unknown): number | null {
  const num = Number(value)
  if (!Number.isFinite(num)) return null
  return Math.trunc(num)
}

function normalizeRequiredPriority(value: unknown, fallback: number): number {
  return normalizeOptionalPriority(value) ?? fallback
}

function normalizeProvidersForEditing(
  providers: ProviderWithEndpointsSummary[]
): ProviderWithEndpointsSummary[] {
  const normalized = providers.map((provider, index) => ({
    ...provider,
    provider_priority: normalizeRequiredPriority(provider.provider_priority, 100 + index),
    pool_advanced: provider.pool_advanced
      ? {
          ...provider.pool_advanced,
          global_priority: normalizeOptionalPriority(provider.pool_advanced.global_priority),
        }
      : provider.pool_advanced,
  }))

  const minProviderPriority = normalized.reduce(
    (min, provider) => Math.min(min, provider.provider_priority),
    Number.POSITIVE_INFINITY,
  )
  const providerOffset = Number.isFinite(minProviderPriority) && minProviderPriority < 1
    ? 1 - minProviderPriority
    : 0

  const explicitPoolPriorities = normalized
    .map((provider) => normalizeOptionalPriority(provider.pool_advanced?.global_priority))
    .filter((priority): priority is number => priority != null)
  const minPoolPriority = explicitPoolPriorities.length > 0
    ? Math.min(...explicitPoolPriorities)
    : null
  const poolOffset = minPoolPriority != null && minPoolPriority < 1
    ? 1 - minPoolPriority
    : 0

  return normalized.map((provider) => ({
    ...provider,
    provider_priority: provider.provider_priority + providerOffset,
    pool_advanced: provider.pool_advanced
      ? {
          ...provider.pool_advanced,
          global_priority: provider.pool_advanced.global_priority == null
            ? null
            : provider.pool_advanced.global_priority + poolOffset,
        }
      : provider.pool_advanced,
  }))
}

function snapshotProviderBaseline(providers: ProviderWithEndpointsSummary[]) {
  originalProviderPriorityById = new Map(
    providers.map((provider, index) => [
      provider.id,
      normalizeRequiredPriority(provider.provider_priority, 100 + index),
    ])
  )
  originalPoolPriorityByProviderId = new Map(
    providers.map((provider) => [
      provider.id,
      normalizeOptionalPriority(provider.pool_advanced?.global_priority),
    ])
  )
}

function normalizeRateMultipliers(
  value: Record<string, unknown> | null | undefined
): Record<string, number> | null {
  if (!value) return null
  const normalized: Record<string, number> = {}
  for (const [rawFormat, rawMultiplier] of Object.entries(value)) {
    const format = normalizeApiFormatKey(rawFormat)
    if (!format || format in normalized) continue
    const num = Number(rawMultiplier)
    if (!Number.isFinite(num)) continue
    normalized[format] = num
  }
  return Object.keys(normalized).length > 0 ? normalized : null
}

function buildEditableKeyPriorityMap(): Map<string, Record<string, number>> {
  const priorityMapByKeyId = new Map<string, Record<string, number>>()

  for (const format of Object.keys(keysByFormat.value)) {
    const normalizedFormat = normalizeApiFormatKey(format)
    if (!normalizedFormat) continue

    const keys = keysByFormat.value[format].filter((key) => !isPoolManagedKey(key))
    keys.forEach((key) => {
      const existing = priorityMapByKeyId.get(key.id) || normalizePriorityMap(key.global_priority_by_format)
      existing[normalizedFormat] = Math.max(0, Math.trunc(key.priority))
      priorityMapByKeyId.set(key.id, existing)
    })
  }

  return priorityMapByKeyId
}

function snapshotKeyBaseline() {
  originalKeyPriorityById = new Map(
    Array.from(buildEditableKeyPriorityMap().entries()).map(([keyId, priorityMap]) => [
      keyId,
      { ...priorityMap },
    ])
  )
}

function arePriorityMapsEqual(
  left: Record<string, number> | undefined,
  right: Record<string, number> | undefined,
): boolean {
  const leftEntries = Object.entries(left || {}).sort(([a], [b]) => a.localeCompare(b))
  const rightEntries = Object.entries(right || {}).sort(([a], [b]) => a.localeCompare(b))

  if (leftEntries.length !== rightEntries.length) return false

  return leftEntries.every(([format, priority], index) => {
    const [rightFormat, rightPriority] = rightEntries[index] || []
    return format === rightFormat && priority === rightPriority
  })
}

async function runTasksWithConcurrency(
  tasks: Array<() => Promise<unknown>>,
  concurrency: number = SAVE_CONCURRENCY,
) {
  if (tasks.length === 0) return

  let cursor = 0
  const runNext = async (): Promise<void> => {
    while (cursor < tasks.length) {
      const index = cursor++
      await tasks[index]()
    }
  }

  const workers = Array.from(
    { length: Math.min(concurrency, tasks.length) },
    () => runNext(),
  )
  await Promise.all(workers)
}

const providerById = computed(() => {
  const map = new Map<string, ProviderWithEndpointsSummary>()
  sortedProviders.value.forEach((provider) => {
    map.set(provider.id, provider)
  })
  return map
})

const providerIdByName = computed(() => {
  const map = new Map<string, string>()
  sortedProviders.value.forEach((provider) => {
    if (!map.has(provider.name)) {
      map.set(provider.name, provider.id)
    }
  })
  return map
})

function resolveProviderId(key: Pick<KeyWithMeta, 'provider_id' | 'provider_name'>): string {
  if (key.provider_id) return key.provider_id
  return providerIdByName.value.get(key.provider_name) || ''
}

const poolProviderIds = computed(() => {
  const set = new Set<string>()
  sortedProviders.value.forEach((provider) => {
    if (provider.pool_advanced) {
      set.add(provider.id)
    }
  })
  return set
})

const PROVIDER_TYPE_LABELS: Record<string, string> = {
  custom: '自定义',
  vertex_ai: 'Vertex AI',
  claude_code: 'ClaudeCode',
  codex: 'Codex',
  gemini_cli: 'Gemini CLI',
  antigravity: 'Antigravity',
  kiro: 'Kiro',
}

function formatProviderType(type?: string): string {
  if (!type) return ''
  return PROVIDER_TYPE_LABELS[type] || type
}

function isPoolManagedProvider(providerId: string): boolean {
  return providerId !== '' && poolProviderIds.value.has(providerId)
}

function isPoolManagedKey(key: KeyWithMeta): boolean {
  return isPoolManagedProvider(resolveProviderId(key))
}

function isPoolAggregateItem(key: KeyWithMeta): boolean {
  return key.is_pool_aggregate === true
}

// 将号池聚合项的优先级写回 provider 的 pool_advanced.global_priority（本地状态）
function updatePoolGlobalPriority(providerId: string, priority: number) {
  const provider = sortedProviders.value.find((p) => p.id === providerId)
  if (!provider) return
  if (provider.pool_advanced) {
    provider.pool_advanced.global_priority = priority
  } else {
    provider.pool_advanced = { global_priority: priority }
  }
}

function toNumberOrNull(value: unknown): number | null {
  const num = Number(value)
  return Number.isFinite(num) ? num : null
}

// 可用的 API 格式
const availableFormats = computed(() => {
  return sortApiFormats(Object.keys(keysByFormat.value))
})

// 按 family 分组的 API 格式（用于侧边栏分组显示）
const groupedFormats = computed(() => {
  return groupApiFormats(availableFormats.value)
})

// 获取格式的 kind 显示名称
function formatKind(format: string): string {
  const { kind } = parseApiFormat(format)
  return API_FORMAT_KIND_LABELS[kind] || kind || format
}

// 排序 Key：活跃的在前，停用的(Key或Provider)在后，各自按优先级排序
function sortKeysByActiveAndPriority(keys: KeyWithMeta[]): KeyWithMeta[] {
  return [...keys].sort((a, b) => {
    const aActive = a.is_active && a.provider_active
    const bActive = b.is_active && b.provider_active
    if (aActive !== bActive) return aActive ? -1 : 1
    return a.priority - b.priority
  })
}

function buildPoolAggregateItem(format: string, providerId: string, sourceKeys: KeyWithMeta[]): KeyWithMeta {
  const provider = providerById.value.get(providerId)
  const poolPriorityRaw = provider?.pool_advanced?.global_priority
  const fallbackPriority = provider?.provider_priority ?? 999999
  const poolPriority = Number.isFinite(poolPriorityRaw ?? NaN)
    ? Number(poolPriorityRaw)
    : fallbackPriority

  const providerName = provider?.name || sourceKeys[0]?.provider_name || '未知 Provider'
  const activeKeyCount = sourceKeys.filter((k) => k.is_active).length
  const providerActive = provider?.is_active ?? sourceKeys.some((k) => k.provider_active)
  const healthCandidates = sourceKeys.map((k) => k.health_score).filter((v): v is number => v != null)
  const avgHealth = healthCandidates.length > 0
    ? healthCandidates.reduce((sum, score) => sum + score, 0) / healthCandidates.length
    : null

  return {
    id: `pool:${providerId}:${format}`,
    provider_id: providerId,
    name: providerName,
    api_key_masked: '[Pool]',
    internal_priority: 0,
    global_priority_by_format: null,
    format_priority: poolPriority,
    priority: poolPriority,
    rate_multipliers: { [format]: 1 },
    is_active: activeKeyCount > 0,
    provider_active: providerActive,
    circuit_breaker_open: false,
    provider_name: providerName,
    endpoint_base_url: sourceKeys.find((k) => k.endpoint_base_url)?.endpoint_base_url || '',
    api_format: format,
    capabilities: [],
    health_score: avgHealth,
    success_rate: null,
    avg_response_time_ms: null,
    request_count: sourceKeys.reduce((sum, key) => sum + (key.request_count || 0), 0),
    is_pool_aggregate: true,
    pool_key_count: sourceKeys.length,
    pool_active_key_count: activeKeyCount,
    provider_type: provider?.provider_type || undefined,
  }
}

const displayKeysByFormat = computed<Record<string, KeyWithMeta[]>>(() => {
  const display: Record<string, KeyWithMeta[]> = {}

  for (const [format, rawKeys] of Object.entries(keysByFormat.value)) {
    const normalKeys: KeyWithMeta[] = []
    const poolGroups = new Map<string, KeyWithMeta[]>()

    for (const key of rawKeys) {
      const providerId = resolveProviderId(key)
      if (isPoolManagedProvider(providerId)) {
        if (!poolGroups.has(providerId)) {
          poolGroups.set(providerId, [])
        }
        poolGroups.get(providerId)?.push(key)
      } else {
        normalKeys.push(key)
      }
    }

    const poolItems = Array.from(poolGroups.entries()).map(([providerId, keys]) =>
      buildPoolAggregateItem(format, providerId, keys)
    )

    display[format] = sortKeysByActiveAndPriority([...normalKeys, ...poolItems])
  }

  return display
})

// 排序 providers：启用的在前，停用的在后，各自按优先级排序
function sortProvidersByActiveAndPriority(providers: ProviderWithEndpointsSummary[]) {
  return [...providers].sort((a, b) => {
    if (a.is_active !== b.is_active) {
      return a.is_active ? -1 : 1
    }
    return a.provider_priority - b.provider_priority
  })
}

// 监听对话框打开
watch(internalOpen, async (open) => {
  if (open) {
    await loadAllProviders()
    await loadCurrentPriorityMode()
    await loadKeysByFormat()
    // 异步加载余额数据
    loadBalances()
  }
})

// 加载全量 providers（优先级管理需要完整列表）
async function loadAllProviders() {
  try {
    const response = await getProvidersSummary({ page: 1, page_size: 9999 })
    snapshotProviderBaseline(response.items)
    sortedProviders.value = sortProvidersByActiveAndPriority(
      normalizeProvidersForEditing(response.items)
    )
  } catch {
    originalProviderPriorityById = new Map()
    originalPoolPriorityByProviderId = new Map()
    sortedProviders.value = []
  }
}

// 加载当前的优先级模式配置
async function loadCurrentPriorityMode() {
  try {
    const [priorityResponse, schedulingResponse] = await Promise.all([
      adminApi.getSystemConfig('provider_priority_mode'),
      adminApi.getSystemConfig('scheduling_mode')
    ])
    const currentMode = priorityResponse.value || 'provider'
    activeMainTab.value = currentMode === 'global_key' ? 'key' : 'provider'

    const currentSchedulingMode = schedulingResponse.value || 'cache_affinity'
    if (currentSchedulingMode === 'fixed_order' || currentSchedulingMode === 'load_balance' || currentSchedulingMode === 'cache_affinity') {
      schedulingMode.value = currentSchedulingMode
    } else {
      schedulingMode.value = 'cache_affinity'
    }
  } catch {
    activeMainTab.value = 'provider'
    schedulingMode.value = 'cache_affinity'
  }
}

// 加载按格式分组的 Keys
async function loadKeysByFormat() {
  try {
    loadingKeys.value = true
    const { default: client } = await import('@/api/client')
    const response = await client.get('/api/admin/endpoints/keys/grouped-by-format')

    // 每个格式独立管理优先级，额外做一次前端归一化兜底，避免历史数据导致重复格式/脏键
    const data: Record<string, KeyWithMeta[]> = {}
    const grouped = response.data as Record<string, Record<string, unknown>[]>

    for (const [rawFormat, keys] of Object.entries(grouped)) {
      const format = normalizeApiFormatKey(rawFormat)
      if (!format) continue

      if (!data[format]) {
        data[format] = []
      }

      for (const key of keys) {
        const providerName = typeof key.provider_name === 'string' ? key.provider_name : ''
        const providerIdRaw = typeof key.provider_id === 'string' ? key.provider_id : ''
        const providerId = providerIdRaw || providerIdByName.value.get(providerName) || ''

        const priorityMap = normalizePriorityMap(
          (key.global_priority_by_format as Record<string, unknown> | null | undefined)
        )
        const rateMultipliers = normalizeRateMultipliers(
          (key.rate_multipliers as Record<string, unknown> | null | undefined)
        )
        const explicitFormatPriority = toNumberOrNull(key.format_priority)
        const inferredFormatPriority = priorityMap[format]
        const formatPriority = explicitFormatPriority ?? (typeof inferredFormatPriority === 'number'
          ? inferredFormatPriority
          : null)

        data[format].push({
          id: String(key.id || ''),
          provider_id: providerId,
          name: String(key.name || 'Unnamed Key'),
          api_key_masked: String(key.api_key_masked || '***'),
          internal_priority: toNumberOrNull(key.internal_priority) ?? 0,
          global_priority_by_format: Object.keys(priorityMap).length > 0 ? priorityMap : null,
          format_priority: formatPriority,
          priority: formatPriority ?? 0,
          rate_multipliers: rateMultipliers,
          is_active: key.is_active !== false,
          provider_active: key.provider_active !== false,
          circuit_breaker_open: key.circuit_breaker_open === true,
          provider_name: providerName || 'Unknown Provider',
          endpoint_base_url: String(key.endpoint_base_url || ''),
          api_format: format,
          capabilities: Array.isArray(key.capabilities)
            ? key.capabilities.map((cap) => String(cap))
            : [],
          health_score: toNumberOrNull(key.health_score),
          success_rate: toNumberOrNull(key.success_rate),
          avg_response_time_ms: toNumberOrNull(key.avg_response_time_ms),
          request_count: toNumberOrNull(key.request_count) ?? 0,
        })
      }
    }

    for (const [format, keys] of Object.entries(data)) {
      const dedupedById = new Map<string, KeyWithMeta>()
      for (const key of keys) {
        if (!key.id) continue
        const existing = dedupedById.get(key.id)
        if (!existing) {
          dedupedById.set(key.id, key)
          continue
        }

        const mergedPriorityMap = {
          ...(existing.global_priority_by_format || {}),
          ...(key.global_priority_by_format || {})
        }
        const mergedRateMap = {
          ...(existing.rate_multipliers || {}),
          ...(key.rate_multipliers || {})
        }

        const preferredFormatPriority = existing.format_priority ?? key.format_priority
        dedupedById.set(key.id, {
          ...existing,
          ...key,
          global_priority_by_format: Object.keys(mergedPriorityMap).length > 0 ? mergedPriorityMap : null,
          rate_multipliers: Object.keys(mergedRateMap).length > 0 ? mergedRateMap : null,
          format_priority: preferredFormatPriority,
          priority: preferredFormatPriority ?? existing.priority ?? key.priority,
        })
      }

      const deduped = Array.from(dedupedById.values())
      let maxPriority = 0
      for (const key of deduped) {
        if (key.format_priority != null) {
          maxPriority = Math.max(maxPriority, key.format_priority)
        }
      }

      let nextPriority = maxPriority + 1
      data[format] = deduped.map((key) => ({
        ...key,
        priority: key.format_priority ?? nextPriority++
      }))
      // 按优先级排序：活跃的在前，停用的(Key或Provider)在后，各自按优先级排序
      data[format] = sortKeysByActiveAndPriority(data[format])
    }
    keysByFormat.value = data
    snapshotKeyBaseline()

    const formats = sortApiFormats(Object.keys(data))
    if (formats.length > 0 && !formats.includes(activeFormatTab.value)) {
      activeFormatTab.value = formats[0]
    }
  } catch (err: unknown) {
    originalKeyPriorityById = new Map()
    showError(parseApiError(err, '加载 Key 列表失败'), '错误')
  } finally {
    loadingKeys.value = false
  }
}

// 快捷切换 Key 启用/禁用状态
async function toggleKeyActive(format: string, key: KeyWithMeta) {
  if (isPoolAggregateItem(key)) return
  const newStatus = !key.is_active
  try {
    await updateProviderKey(key.id, { is_active: newStatus })
    // 更新本地状态（该 key 可能出现在多个格式中）
    for (const fmt of Object.keys(keysByFormat.value)) {
      const keys = keysByFormat.value[fmt]
      const found = keys.find(k => k.id === key.id)
      if (found) {
        found.is_active = newStatus
      }
    }
    // 重新排序所有格式（该 key 可能出现在多个格式中，状态变更后都需要重排）
    for (const fmt of Object.keys(keysByFormat.value)) {
      keysByFormat.value[fmt] = sortKeysByActiveAndPriority(keysByFormat.value[fmt])
    }
    success(newStatus ? 'Key 已启用' : 'Key 已停用')
  } catch (err: unknown) {
    showError(parseApiError(err, '操作失败'), '错误')
  }
}

// Key 优先级编辑
function startEditKeyPriority(format: string, key: KeyWithMeta) {
  editingKeyPriority.value[format] = key.id
}

function cancelEditKeyPriority(format: string) {
  editingKeyPriority.value[format] = null
}

function finishEditKeyPriority(format: string, key: KeyWithMeta, event: FocusEvent) {
  const input = event.target as HTMLInputElement
  const newPriority = parseInt(input.value, 10)

  if (!isNaN(newPriority) && newPriority >= 1) {
    if (isPoolAggregateItem(key) && key.provider_id) {
      // 号池聚合项：写回 provider 的 pool_advanced.global_priority
      updatePoolGlobalPriority(key.provider_id, newPriority)
    } else {
      // 普通 key：直接更新优先级
      key.priority = newPriority
    }
    // 重新排序当前格式
    keysByFormat.value[format] = sortKeysByActiveAndPriority(keysByFormat.value[format])
  }

  editingKeyPriority.value[format] = null
}

// Provider 优先级编辑
function startEditProviderPriority(provider: ProviderWithEndpointsSummary) {
  editingProviderPriority.value = provider.id
}

function cancelEditProviderPriority() {
  editingProviderPriority.value = null
}

function finishEditProviderPriority(provider: ProviderWithEndpointsSummary, event: FocusEvent) {
  const input = event.target as HTMLInputElement
  const newPriority = parseInt(input.value, 10)

  if (!isNaN(newPriority) && newPriority >= 1) {
    // 更新该 provider 的优先级
    const idx = sortedProviders.value.findIndex(p => p.id === provider.id)
    if (idx !== -1) {
      sortedProviders.value[idx] = {
        ...sortedProviders.value[idx],
        provider_priority: newPriority
      }
    }
    // 重新排序
    sortedProviders.value = sortProvidersByActiveAndPriority(sortedProviders.value)
  }

  editingProviderPriority.value = null
}

// Provider 拖拽处理
function handleProviderDragStart(index: number, event: DragEvent) {
  draggedProvider.value = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/html', '')
  }
}

function handleProviderDragEnd() {
  draggedProvider.value = null
  dragOverProvider.value = null
}

function handleProviderDragOver(index: number) {
  dragOverProvider.value = index
}

function handleProviderDragLeave() {
  dragOverProvider.value = null
}

function handleProviderDrop(dropIndex: number) {
  if (draggedProvider.value === null || draggedProvider.value === dropIndex) {
    draggedProvider.value = null
    dragOverProvider.value = null
    return
  }

  const providers = sortedProviders.value
  const dragIndex = draggedProvider.value
  const draggedItem = providers[dragIndex]
  const targetItem = providers[dropIndex]
  const draggedPriority = draggedItem.provider_priority
  const targetPriority = targetItem.provider_priority

  // 同优先级组内拖拽，忽略
  if (draggedPriority === targetPriority) {
    draggedProvider.value = null
    dragOverProvider.value = null
    return
  }

  // 记录每个 provider 的原始优先级
  const originalPriorityMap = new Map<string, number>()
  providers.forEach(p => {
    originalPriorityMap.set(p.id, p.provider_priority)
  })

  // 重排数组：将被拖动项移到目标位置
  const items = [...providers]
  items.splice(dragIndex, 1)
  items.splice(dropIndex, 0, draggedItem)

  // 按新顺序分配优先级：被拖动项单独成组，其他同组项保持在一起
  const groupNewPriority = new Map<number, number>()
  let currentPriority = 1

  items.forEach(provider => {
    const originalPriority = originalPriorityMap.get(provider.id) ?? 0

    if (provider === draggedItem) {
      // 被拖动的项单独成组
      provider.provider_priority = currentPriority
      currentPriority++
    } else {
      if (groupNewPriority.has(originalPriority)) {
        // 同组的其他项使用相同的新优先级
        provider.provider_priority = groupNewPriority.get(originalPriority) ?? currentPriority
      } else {
        // 新组，分配新优先级
        groupNewPriority.set(originalPriority, currentPriority)
        provider.provider_priority = currentPriority
        currentPriority++
      }
    }
  })

  // 重新排序
  sortedProviders.value = sortProvidersByActiveAndPriority(items)
  draggedProvider.value = null
  dragOverProvider.value = null
}

// Key 拖拽处理
function getEditableKeysForFormat(format: string): KeyWithMeta[] {
  return displayKeysByFormat.value[format] || []
}

function handleKeyDragStart(format: string, keyId: string, event: DragEvent) {
  draggedKey.value[format] = keyId
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/html', '')
  }
}

function handleKeyDragEnd(format: string) {
  draggedKey.value[format] = null
  dragOverKey.value[format] = null
}

function handleKeyDragOver(format: string, keyId: string) {
  dragOverKey.value[format] = keyId
}

function handleKeyDragLeave(format: string) {
  dragOverKey.value[format] = null
}

function handleKeyDrop(format: string, dropKeyId: string) {
  const draggedKeyId = draggedKey.value[format]
  if (!draggedKeyId || draggedKeyId === dropKeyId) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }

  const editableKeys = getEditableKeysForFormat(format)
  const dragIndex = editableKeys.findIndex((k) => k.id === draggedKeyId)
  const dropIndex = editableKeys.findIndex((k) => k.id === dropKeyId)
  if (dragIndex === -1 || dropIndex === -1) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }

  const draggedItem = editableKeys[dragIndex]
  const targetItem = editableKeys[dropIndex]
  const draggedPriority = draggedItem.priority
  const targetPriority = targetItem.priority

  // 同优先级组内拖拽，忽略
  if (draggedPriority === targetPriority) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }

  // 记录每个 key 的原始优先级
  const originalPriorityMap = new Map<string, number>()
  editableKeys.forEach(k => {
    originalPriorityMap.set(k.id, k.priority)
  })

  // 重排数组：将被拖动项移到目标位置
  const items = editableKeys.map((key) => ({ ...key }))
  const dragItemIndex = items.findIndex((k) => k.id === draggedKeyId)
  const dropItemIndex = items.findIndex((k) => k.id === dropKeyId)
  if (dragItemIndex === -1 || dropItemIndex === -1) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }
  const draggedClone = items[dragItemIndex]
  items.splice(dragItemIndex, 1)
  items.splice(dropItemIndex, 0, draggedClone)

  // 按新顺序分配优先级：被拖动项单独成组，其他同组项保持在一起
  const groupNewPriority = new Map<number, number>()
  let currentPriority = 1

  items.forEach(key => {
    const originalPriority = originalPriorityMap.get(key.id) ?? 0

    if (key.id === draggedKeyId) {
      // 被拖动的项单独成组
      key.priority = currentPriority
      currentPriority++
    } else {
      if (groupNewPriority.has(originalPriority)) {
        // 同组的其他项使用相同的新优先级
        key.priority = groupNewPriority.get(originalPriority) ?? currentPriority
      } else {
        // 新组，分配新优先级
        groupNewPriority.set(originalPriority, currentPriority)
        key.priority = currentPriority
        currentPriority++
      }
    }
  })

  const updatedPriorityById = new Map<string, number>()
  items.forEach((key) => {
    updatedPriorityById.set(key.id, key.priority)
  })

  // 将新的优先级写回原始数据（普通 key）
  keysByFormat.value[format] = sortKeysByActiveAndPriority(
    (keysByFormat.value[format] || []).map((key) => {
      const nextPriority = updatedPriorityById.get(key.id)
      if (nextPriority == null) return key
      return {
        ...key,
        priority: nextPriority,
      }
    })
  )

  // 号池聚合项的优先级写回 provider 的 pool_advanced.global_priority
  for (const item of items) {
    if (item.is_pool_aggregate && item.provider_id) {
      updatePoolGlobalPriority(item.provider_id, item.priority)
    }
  }

  draggedKey.value[format] = null
  dragOverKey.value[format] = null
}

// 保存
async function save() {
  try {
    saving.value = true

    const newMode = activeMainTab.value === 'key' ? 'global_key' : 'provider'

    // 第一步：先保存所有 Provider 和 Key 的优先级数据
    // 确保优先级数据全部到位后，再切换调度模式，避免瞬态不一致
    const providerTasks: Array<() => Promise<unknown>> = []
    sortedProviders.value.forEach((provider) => {
      const payload: Parameters<typeof updateProvider>[1] = {}
      const currentProviderPriority = Math.max(0, Math.trunc(provider.provider_priority))
      const originalProviderPriority = originalProviderPriorityById.get(provider.id)
      if (originalProviderPriority == null || originalProviderPriority !== currentProviderPriority) {
        payload.provider_priority = currentProviderPriority
      }

      const currentPoolPriority = normalizeOptionalPriority(provider.pool_advanced?.global_priority)
      const originalPoolPriority = originalPoolPriorityByProviderId.get(provider.id) ?? null
      if (currentPoolPriority !== originalPoolPriority) {
        payload.pool_advanced = provider.pool_advanced
          ? {
              ...provider.pool_advanced,
              global_priority: currentPoolPriority,
            }
          : null
      }

      if (Object.keys(payload).length > 0) {
        providerTasks.push(() => updateProvider(provider.id, payload))
      }
    })

    // 收集每个 Key 的按格式优先级（保留原有其他格式的配置）
    const keyPriorityByFormatMap = buildEditableKeyPriorityMap()
    const keyTasks = Array.from(keyPriorityByFormatMap.entries())
      .filter(([keyId, priorityByFormat]) => !arePriorityMapsEqual(
        originalKeyPriorityById.get(keyId),
        priorityByFormat,
      ))
      .map(([keyId, priorityByFormat]) => () =>
        updateProviderKey(keyId, { global_priority_by_format: priorityByFormat })
      )

    await runTasksWithConcurrency([...providerTasks, ...keyTasks])

    // 第二步：优先级数据全部就绪后，顺序保存调度配置
    // 先保存优先级模式，再保存调度模式，确保 Scheduler 状态完整切换
    await adminApi.updateSystemConfig(
      'provider_priority_mode',
      newMode,
      'Provider/Key 优先级策略：provider(提供商优先模式) 或 global_key(全局Key优先模式)'
    )
    await adminApi.updateSystemConfig(
      'scheduling_mode',
      schedulingMode.value,
      '调度模式：cache_affinity(缓存亲和模式) 或 load_balance(负载均衡模式) 或 fixed_order(固定顺序模式)'
    )

    await loadAllProviders()
    await loadKeysByFormat()

    success('优先级已保存')
    emit('saved')

    // 提供商优先模式保存后关闭，Key 优先模式保存后保持打开方便继续调整
    if (activeMainTab.value === 'provider') {
      close()
    }
  } catch (err: unknown) {
    showError(parseApiError(err, '保存失败'), '错误')
  } finally {
    saving.value = false
  }
}

function close() {
  emit('update:modelValue', false)
}
</script>
