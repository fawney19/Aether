<template>
  <Dialog
    :model-value="internalOpen"
    title="优先级管理"
    description="调整提供商和 API Key 的优先级顺序，保存后自动切换对应的调度策略"
    :icon="ListOrdered"
    size="3xl"
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

      <!-- 内容区域 -->
      <div class="min-h-[420px]">
        <!-- 提供商优先级 -->
        <div
          v-show="activeMainTab === 'provider'"
          class="space-y-4"
        >
          <!-- 提示信息 -->
          <div class="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground bg-muted/30 rounded-md">
            <Info class="w-3.5 h-3.5 shrink-0" />
            <span>拖拽调整顺序，点击序号可编辑（相同数字为同级，负载均衡）</span>
          </div>

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
            class="space-y-2 max-h-[380px] overflow-y-auto pr-1"
          >
            <div
              v-for="(provider, index) in sortedProviders"
              :key="provider.id"
              class="group flex items-center gap-3 px-3 py-2.5 rounded-lg border transition-all duration-200"
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
                  title="点击编辑优先级，相同数字为同级（负载均衡）"
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
              <!-- API 格式标签 -->
              <div
                v-if="provider.api_formats?.length"
                class="flex items-center gap-1 shrink-0"
              >
                <span
                  v-for="fmt in provider.api_formats"
                  :key="fmt"
                  class="text-[10px] px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
                >
                  {{ API_FORMAT_SHORT[fmt] || fmt }}
                </span>
              </div>
              <!-- 余额显示-->
              <span
                v-if="formatBalanceDisplay(provider.id)"
                class="text-xs shrink-0"
              >
                <span class="font-semibold text-foreground/90">{{ formatBalanceDisplay(provider.id) }}</span>
              </span>
            </div>
          </div>
        </div>

        <!-- Key 优先级 -->
        <div
          v-show="activeMainTab === 'key'"
          class="space-y-3"
        >
          <!-- 提示信息 -->
          <div class="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground bg-muted/30 rounded-md">
            <Info class="w-3.5 h-3.5 shrink-0" />
            <span>拖拽调整顺序，点击序号可编辑（相同数字为同级，负载均衡）</span>
          </div>

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
            class="flex gap-4"
          >
            <!-- 左侧：API 格式列表 -->
            <div class="w-32 shrink-0 space-y-1">
              <button
                v-for="format in availableFormats"
                :key="format"
                type="button"
                class="w-full px-3 py-2 text-xs font-medium rounded-md text-left transition-all duration-200"
                :class="[
                  activeFormatTab === format
                    ? 'bg-primary text-primary-foreground'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                ]"
                @click="activeFormatTab = format"
              >
                {{ format }}
              </button>
            </div>

            <!-- 右侧：Key 列表 -->
            <div class="flex-1 min-w-0">
              <div
                v-for="format in availableFormats"
                v-show="activeFormatTab === format"
                :key="format"
              >
                <div
                  v-if="keysByFormat[format]?.length > 0"
                  class="space-y-2 max-h-[380px] overflow-y-auto pr-1"
                >
                  <div
                    v-for="(key, index) in keysByFormat[format]"
                    :key="key.id"
                    class="group flex items-center gap-3 px-3 py-2.5 rounded-lg border transition-all duration-200"
                    :class="[
                      draggedKey[format] === index
                        ? 'border-primary/50 bg-primary/5 shadow-md scale-[1.01]'
                        : dragOverKey[format] === index
                          ? 'border-primary/30 bg-primary/5'
                          : 'border-border/50 bg-background hover:border-border hover:bg-muted/30'
                    ]"
                    draggable="true"
                    @dragstart="handleKeyDragStart(format, index, $event)"
                    @dragend="handleKeyDragEnd(format)"
                    @dragover.prevent="handleKeyDragOver(format, index)"
                    @dragleave="handleKeyDragLeave(format)"
                    @drop="handleKeyDrop(format, index)"
                  >
                    <!-- 拖拽手柄 -->
                    <div class="cursor-grab active:cursor-grabbing p-1 rounded hover:bg-muted text-muted-foreground/40 group-hover:text-muted-foreground transition-colors shrink-0">
                      <GripVertical class="w-4 h-4" />
                    </div>

                    <!-- 可编辑序号 -->
                    <div class="shrink-0">
                      <input
                        v-if="editingKeyPriority[format] === key.id"
                        type="number"
                        min="1"
                        :value="key.priority"
                        class="w-8 h-6 rounded-md bg-background border border-primary text-xs font-medium text-center focus:outline-none [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
                        autofocus
                        @blur="finishEditKeyPriority(format, key, $event)"
                        @keydown.enter="($event.target as HTMLInputElement).blur()"
                        @keydown.escape="cancelEditKeyPriority(format)"
                      >
                      <div
                        v-else
                        class="w-6 h-6 rounded-md bg-muted/50 flex items-center justify-center text-xs font-medium text-muted-foreground cursor-pointer hover:bg-primary/10 hover:text-primary transition-colors"
                        title="点击编辑优先级，相同数字为同级（负载均衡）"
                        @click.stop="startEditKeyPriority(format, key)"
                      >
                        {{ key.priority }}
                      </div>
                    </div>

                    <!-- Key 信息 -->
                    <div class="flex-1 min-w-0 flex items-center gap-3">
                      <!-- 左侧：名称和来源 -->
                      <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2">
                          <span class="font-medium text-sm">{{ key.name }}</span>
                          <Badge
                            v-if="key.circuit_breaker_open"
                            variant="destructive"
                            class="text-[10px] h-5 px-1.5 shrink-0"
                          >
                            熔断
                          </Badge>
                          <Badge
                            v-else-if="!key.is_active"
                            variant="secondary"
                            class="text-[10px] h-5 px-1.5 shrink-0"
                          >
                            停用
                          </Badge>
                          <!-- 能力标签紧跟名称 -->
                          <template v-if="key.capabilities?.length">
                            <span
                              v-for="cap in key.capabilities.slice(0, 2)"
                              :key="cap"
                              class="px-1 py-0.5 bg-muted text-muted-foreground rounded text-[10px]"
                            >{{ cap }}</span>
                            <span
                              v-if="key.capabilities.length > 2"
                              class="text-[10px] text-muted-foreground"
                            >+{{ key.capabilities.length - 2 }}</span>
                          </template>
                        </div>
                        <div class="flex items-center gap-1.5 text-xs text-muted-foreground mt-1">
                          <span class="text-[10px] font-medium shrink-0">{{ key.provider_name }}</span>
                          <span class="font-mono text-[10px] opacity-60 truncate">{{ key.api_key_masked }}</span>
                        </div>
                      </div>

                      <!-- 右侧：健康度 + 速率 -->
                      <div class="shrink-0 flex items-center gap-3">
                        <!-- 健康度 -->
                        <div
                          v-if="key.health_score != null"
                          class="text-xs text-right"
                        >
                          <div
                            class="font-medium tabular-nums"
                            :class="[
                              key.health_score >= 0.95 ? 'text-green-600' :
                              key.health_score >= 0.5 ? 'text-yellow-600' : 'text-red-500'
                            ]"
                          >
                            {{ ((key.health_score || 0) * 100).toFixed(0) }}%
                          </div>
                          <div class="text-[10px] text-muted-foreground opacity-70">
                            {{ key.request_count }} reqs
                          </div>
                        </div>
                        <div
                          v-else
                          class="text-xs text-muted-foreground/50 text-right"
                        >
                          <div>--</div>
                          <div class="text-[10px]">
                            无数据
                          </div>
                        </div>
                        <!-- 速率倍数 -->
                        <div class="text-sm font-medium tabular-nums text-primary min-w-[40px] text-right">
                          {{ key.rate_multipliers?.[format] ?? 1 }}x
                        </div>
                      </div>
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
        <div class="flex items-center gap-4">
          <div class="text-xs text-muted-foreground">
            当前模式: <span class="font-medium">{{ activeMainTab === 'provider' ? '提供商优先' : 'Key 优先' }}</span>
          </div>
          <div class="flex items-center gap-2 pl-4 border-l border-border">
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
import { GripVertical, Layers, Key, Info, Loader2, ListOrdered } from 'lucide-vue-next'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import { useToast } from '@/composables/useToast'
import { updateProvider, updateProviderKey } from '@/api/endpoints'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { batchQueryBalance, type ActionResultResponse, type BalanceInfo } from '@/api/providerOps'
import { API_FORMAT_SHORT } from '@/api/endpoints/types'

interface KeyWithMeta {
  id: string
  name: string
  api_key_masked: string
  internal_priority: number
  global_priority_by_format: Record<string, number> | null
  format_priority: number | null  // 当前格式的优先级（后端计算）
  priority: number  // 用于编辑的优先级
  rate_multipliers: Record<string, number> | null
  is_active: boolean
  circuit_breaker_open: boolean
  provider_name: string
  endpoint_base_url: string
  api_format: string
  capabilities: string[]
  health_score: number | null
  success_rate: number | null
  avg_response_time_ms: number | null
  request_count: number
}

const props = defineProps<{
  modelValue: boolean
  providers: ProviderWithEndpointsSummary[]
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
const activeFormatTab = ref<string>('CLAUDE')

// 提供商排序状态
const sortedProviders = ref<ProviderWithEndpointsSummary[]>([])
const draggedProvider = ref<number | null>(null)
const dragOverProvider = ref<number | null>(null)

// Key 排序状态
const keysByFormat = ref<Record<string, KeyWithMeta[]>>({})
const draggedKey = ref<Record<string, number | null>>({})
const dragOverKey = ref<Record<string, number | null>>({})
const loadingKeys = ref(false)
const saving = ref(false)

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
    console.warn('[loadBalances] 加载余额数据失败:', e)
  }
}

// 可用的 API 格式
const availableFormats = computed(() => {
  return Object.keys(keysByFormat.value).sort()
})

// 监听 props.providers 变化
watch(() => props.providers, (newProviders) => {
  if (newProviders) {
    sortedProviders.value = [...newProviders].sort((a, b) => a.provider_priority - b.provider_priority)
  }
}, { immediate: true })

// 监听对话框打开
watch(internalOpen, async (open) => {
  if (open) {
    await loadCurrentPriorityMode()
    await loadKeysByFormat()
    // 异步加载余额数据
    loadBalances()
  }
})

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

    // 每个格式独立管理优先级，使用后端返回的 format_priority
    const data: Record<string, KeyWithMeta[]> = {}
    for (const [format, keys] of Object.entries(response.data as Record<string, any[]>)) {
      // 计算该格式下的默认优先级
      let maxPriority = 0
      for (const key of keys) {
        if (key.format_priority != null) {
          maxPriority = Math.max(maxPriority, key.format_priority)
        }
      }

      let nextPriority = maxPriority + 1
      data[format] = keys.map((key) => ({
        ...key,
        // 使用格式特定优先级，如果没有则分配默认值
        priority: key.format_priority ?? nextPriority++
      }))
      // 按优先级排序
      data[format].sort((a, b) => a.priority - b.priority)
    }
    keysByFormat.value = data

    const formats = Object.keys(data)
    if (formats.length > 0 && !formats.includes(activeFormatTab.value)) {
      activeFormatTab.value = formats[0]
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载 Key 列表失败', '错误')
  } finally {
    loadingKeys.value = false
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
    // 每个格式独立管理优先级，只更新当前格式
    key.priority = newPriority
    // 重新排序当前格式
    keysByFormat.value[format] = [...keysByFormat.value[format]].sort((a, b) => a.priority - b.priority)
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
    // 按 provider_priority 重新排序
    sortedProviders.value = [...sortedProviders.value].sort((a, b) => a.provider_priority - b.provider_priority)
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
  const draggedItem = providers[draggedProvider.value]
  const targetItem = providers[dropIndex]
  const draggedPriority = draggedItem.provider_priority
  const targetPriority = targetItem.provider_priority

  // 如果是同组内拖拽（同优先级），忽略操作
  if (draggedPriority === targetPriority) {
    draggedProvider.value = null
    dragOverProvider.value = null
    return
  }

  // 直接交换优先级
  draggedItem.provider_priority = targetPriority
  targetItem.provider_priority = draggedPriority

  // 重新排序
  sortedProviders.value = [...providers].sort((a, b) => a.provider_priority - b.provider_priority)
  draggedProvider.value = null
  dragOverProvider.value = null
}

// Key 拖拽处理
function handleKeyDragStart(format: string, index: number, event: DragEvent) {
  draggedKey.value[format] = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/html', '')
  }
}

function handleKeyDragEnd(format: string) {
  draggedKey.value[format] = null
  dragOverKey.value[format] = null
}

function handleKeyDragOver(format: string, index: number) {
  dragOverKey.value[format] = index
}

function handleKeyDragLeave(format: string) {
  dragOverKey.value[format] = null
}

function handleKeyDrop(format: string, dropIndex: number) {
  const dragIndex = draggedKey.value[format]
  if (dragIndex === null || dragIndex === dropIndex) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }

  const keys = keysByFormat.value[format]
  const draggedItem = keys[dragIndex]
  const targetItem = keys[dropIndex]
  const draggedPriority = draggedItem.priority
  const targetPriority = targetItem.priority

  // 如果是同组内拖拽（同优先级），忽略操作
  if (draggedPriority === targetPriority) {
    draggedKey.value[format] = null
    dragOverKey.value[format] = null
    return
  }

  // 每个格式独立管理优先级，只交换当前格式内的优先级
  draggedItem.priority = targetPriority
  targetItem.priority = draggedPriority

  // 重新排序当前格式
  keysByFormat.value[format] = [...keys].sort((a, b) => a.priority - b.priority)

  draggedKey.value[format] = null
  dragOverKey.value[format] = null
}

// 保存
async function save() {
  try {
    saving.value = true

    const newMode = activeMainTab.value === 'key' ? 'global_key' : 'provider'

    // 保存优先级模式和调度模式
    await Promise.all([
      adminApi.updateSystemConfig(
        'provider_priority_mode',
        newMode,
        'Provider/Key 优先级策略：provider(提供商优先模式) 或 global_key(全局Key优先模式)'
      ),
      adminApi.updateSystemConfig(
        'scheduling_mode',
        schedulingMode.value,
        '调度模式：fixed_order(固定顺序模式) 或 cache_affinity(缓存亲和模式)'
      )
    ])

    const providerUpdates = sortedProviders.value.map((provider) =>
      updateProvider(provider.id, { provider_priority: provider.provider_priority })
    )

    // 收集每个 Key 的按格式优先级（保留原有其他格式的配置）
    const keyPriorityByFormatMap = new Map<string, Record<string, number>>()
    for (const format of Object.keys(keysByFormat.value)) {
      const keys = keysByFormat.value[format]
      keys.forEach((key) => {
        // 合并原有配置，避免丢失未显示格式的优先级
        const existing = keyPriorityByFormatMap.get(key.id) || { ...key.global_priority_by_format }
        existing[format] = key.priority
        keyPriorityByFormatMap.set(key.id, existing)
      })
    }

    const keyUpdates = Array.from(keyPriorityByFormatMap.entries()).map(([keyId, priorityByFormat]) =>
      updateProviderKey(keyId, { global_priority_by_format: priorityByFormat })
    )

    await Promise.all([...providerUpdates, ...keyUpdates])

    await loadKeysByFormat()

    success('优先级已保存')
    emit('saved')

    // 提供商优先模式保存后关闭，Key 优先模式保存后保持打开方便继续调整
    if (activeMainTab.value === 'provider') {
      close()
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || '保存失败', '错误')
  } finally {
    saving.value = false
  }
}

function close() {
  emit('update:modelValue', false)
}
</script>
