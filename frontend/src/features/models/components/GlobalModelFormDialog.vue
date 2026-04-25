<template>
  <Dialog
    :model-value="open"
    :title="isEditMode ? '编辑模型' : '创建统一模型'"
    :description="isEditMode ? '修改模型配置和价格信息' : ''"
    :icon="isEditMode ? SquarePen : Layers"
    size="4xl"
    @update:model-value="handleDialogUpdate"
  >
    <div
      class="flex gap-4"
      :class="isEditMode ? '' : 'h-[600px]'"
    >
      <!-- 左侧：模型选择（仅创建模式） -->
      <div
        v-if="!isEditMode"
        class="w-[260px] shrink-0 flex flex-col h-full"
      >
        <!-- 搜索框 -->
        <div class="relative mb-3">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            v-model="searchQuery"
            type="text"
            placeholder="搜索模型、提供商..."
            class="pl-8 h-8 text-sm"
          />
        </div>

        <!-- 模型列表（两级结构） -->
        <div class="flex-1 overflow-y-auto border rounded-lg min-h-0 scrollbar-thin">
          <div
            v-if="loading"
            class="flex items-center justify-center h-32"
          >
            <Loader2 class="w-5 h-5 animate-spin text-muted-foreground" />
          </div>
          <template v-else>
            <!-- 提供商分组 -->
            <div
              v-for="group in groupedModels"
              :key="group.providerId"
              class="border-b last:border-b-0"
            >
              <!-- 提供商标题行 -->
              <div
                class="flex items-center gap-2 px-2.5 py-2 cursor-pointer hover:bg-muted text-sm"
                @click="toggleProvider(group.providerId)"
              >
                <ChevronRight
                  class="w-3.5 h-3.5 text-muted-foreground transition-transform shrink-0"
                  :class="expandedProvider === group.providerId ? 'rotate-90' : ''"
                />
                <img
                  :src="getProviderLogoUrl(group.providerId)"
                  :alt="group.providerName"
                  class="w-4 h-4 rounded shrink-0 dark:invert dark:brightness-90"
                  @error="handleLogoError"
                >
                <span class="truncate font-medium text-xs flex-1">{{ group.providerName }}</span>
                <span class="text-[10px] text-muted-foreground shrink-0">{{ group.models.length }}</span>
              </div>
              <!-- 模型列表 -->
              <div
                v-if="expandedProvider === group.providerId"
                class="bg-muted/30"
              >
                <div
                  v-for="item in group.models"
                  :key="item.modelId"
                  class="flex flex-col gap-0.5 pl-7 pr-2.5 py-1.5 cursor-pointer text-xs border-t"
                  :class="selectedModel?.modelId === item.modelId && selectedModel?.providerId === item.providerId
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-muted'"
                  @click="selectModel(item)"
                >
                  <span class="truncate font-medium">{{ item.modelName }}</span>
                  <span
                    class="truncate text-[10px]"
                    :class="selectedModel?.modelId === item.modelId && selectedModel?.providerId === item.providerId
                      ? 'text-primary-foreground/70'
                      : 'text-muted-foreground'"
                  >{{ item.modelId }}</span>
                </div>
              </div>
            </div>
            <div
              v-if="groupedModels.length === 0"
              class="text-center py-8 text-sm text-muted-foreground"
            >
              {{ searchQuery ? '未找到模型' : '加载中...' }}
            </div>
          </template>
        </div>
      </div>

      <!-- 右侧：表单 -->
      <div
        class="flex-1 overflow-y-auto h-full scrollbar-thin"
        :class="isEditMode ? 'max-h-[70vh]' : ''"
      >
        <form
          class="space-y-5"
          @submit.prevent="handleSubmit"
        >
          <!-- 基本信息 -->
          <section class="space-y-3">
            <h4 class="font-medium text-sm">
              基本信息
            </h4>
            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-1.5">
                <Label
                  for="model-display-name"
                  class="text-xs"
                >名称 *</Label>
                <Input
                  id="model-display-name"
                  v-model="form.display_name"
                  placeholder="Claude 3.5 Sonnet"
                  required
                />
              </div>
              <div class="space-y-1.5">
                <Label
                  for="model-name"
                  class="text-xs"
                >模型ID *</Label>
                <Input
                  id="model-name"
                  v-model="form.name"
                  placeholder="claude-3-5-sonnet-20241022"
                  :disabled="isEditMode"
                  required
                />
              </div>
            </div>
            <div class="space-y-1.5">
              <Label
                for="model-description"
                class="text-xs"
              >描述</Label>
              <Input
                id="model-description"
                :model-value="form.config?.description || ''"
                placeholder="简短描述此模型的特点"
                @update:model-value="(v) => setConfigField('description', v || undefined)"
              />
            </div>
            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-1.5">
                <Label
                  for="model-output-limit"
                  class="text-xs"
                >最大输出 Token</Label>
                <Input
                  id="model-output-limit"
                  :model-value="form.config?.output_limit ?? ''"
                  type="number"
                  min="1"
                  placeholder="如 8192"
                  @update:model-value="(v) => setConfigField('output_limit', parseNumberInput(v, { allowFloat: false }))"
                />
              </div>
              <div class="space-y-1.5">
                <Label
                  for="model-context-limit"
                  class="text-xs"
                >上下文窗口</Label>
                <Input
                  id="model-context-limit"
                  :model-value="form.config?.context_limit ?? ''"
                  type="number"
                  min="1"
                  placeholder="如 200000"
                  @update:model-value="(v) => setConfigField('context_limit', parseNumberInput(v, { allowFloat: false }))"
                />
              </div>
            </div>
          </section>

          <!-- 价格配置 -->
          <section class="space-y-3">
            <h4 class="font-medium text-sm">
              价格配置
            </h4>
            <TieredPricingEditor
              ref="tieredPricingEditorRef"
              v-model="tieredPricing"
              :show-cache1h="true"
            />
            <div class="flex items-center gap-3 pt-2 border-t">
              <Label class="text-xs whitespace-nowrap">按次计费</Label>
              <Input
                :model-value="form.default_price_per_request ?? ''"
                type="number"
                step="0.001"
                min="0"
                class="w-24"
                placeholder="$/次"
                @update:model-value="(v) => form.default_price_per_request = parseNumberInput(v, { allowFloat: true })"
              />
              <span class="text-xs text-muted-foreground">可与 Token 计费叠加</span>
            </div>

            <!-- 视频计费（分辨率 × 时长） -->
            <div class="pt-3 border-t space-y-2">
              <div class="text-sm font-medium">
                视频计费（分辨率 × 时长）
              </div>

              <div class="flex items-center gap-1.5 flex-wrap">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="fillVideoResolutionPricePreset('common')"
                >
                  通用
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="fillVideoResolutionPricePreset('sora')"
                >
                  Sora
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="fillVideoResolutionPricePreset('veo')"
                >
                  Veo
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="addVideoResolutionPriceRow"
                >
                  <Plus class="w-3.5 h-3.5 mr-0.5" />
                  自定义
                </Button>
              </div>

              <div
                v-if="videoResolutionPrices.length > 0"
                class="rounded-lg border border-border overflow-hidden"
              >
                <div class="grid grid-cols-[1fr_1fr_32px] gap-0 text-xs text-muted-foreground bg-muted/50 px-3 py-1.5 border-b border-border">
                  <span>分辨率</span>
                  <span>单价（$/秒）</span>
                  <span />
                </div>
                <div class="divide-y divide-border">
                  <div
                    v-for="(row, idx) in videoResolutionPrices"
                    :key="idx"
                    class="grid grid-cols-[1fr_1fr_32px] gap-2 items-center px-3 py-1.5"
                  >
                    <Input
                      v-model="row.resolution"
                      class="h-7 text-sm"
                      placeholder="如 720p"
                    />
                    <Input
                      :model-value="row.price_per_second ?? ''"
                      type="number"
                      step="0.0001"
                      min="0"
                      class="h-7 text-sm"
                      placeholder="0"
                      @update:model-value="(v) => row.price_per_second = parseNumberInput(v, { allowFloat: true })"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      title="删除"
                      @click="removeVideoResolutionPriceRow(idx)"
                    >
                      <Trash2 class="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </section>
        </form>
      </div>
    </div>

    <template #footer>
      <Button
        type="button"
        variant="outline"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        :disabled="submitting || !form.name || !form.display_name"
        @click="handleSubmit"
      >
        <Loader2
          v-if="submitting"
          class="w-4 h-4 mr-2 animate-spin"
        />
        {{ isEditMode ? '保存' : '添加' }}
      </Button>
      <Button
        v-if="selectedModel && !isEditMode"
        type="button"
        variant="ghost"
        @click="clearSelection"
      >
        清空
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import {
  Loader2, Layers, SquarePen,
  Search, ChevronRight, Plus, Trash2
} from 'lucide-vue-next'
import { Dialog, Button, Input, Label } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseNumberInput, sortResolutionEntries } from '@/utils/form'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import TieredPricingEditor from './TieredPricingEditor.vue'
import {
  getModelsDevList,
  getProviderLogoUrl,
  type ModelsDevModelItem,
} from '@/api/models-dev'
import {
  createGlobalModel,
  updateGlobalModel,
  type GlobalModelResponse,
  type GlobalModelCreate,
  type GlobalModelUpdate,
} from '@/api/global-models'
import type { TieredPricingConfig } from '@/api/endpoints/types'

const props = defineProps<{
  open: boolean
  model?: GlobalModelResponse | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'success': []
}>()

const { success, error: showError } = useToast()
const submitting = ref(false)
const tieredPricingEditorRef = ref<InstanceType<typeof TieredPricingEditor> | null>(null)

// 模型列表相关
const loading = ref(false)
const searchQuery = ref('')
const allModelsCache = ref<ModelsDevModelItem[]>([]) // 全部模型（缓存）
const selectedModel = ref<ModelsDevModelItem | null>(null)
const expandedProvider = ref<string | null>(null)

// 当前显示的模型列表：有搜索词时用全部，否则只用官方
const allModels = computed(() => {
  if (searchQuery.value) {
    return allModelsCache.value
  }
  return allModelsCache.value.filter(m => m.official)
})

// 按提供商分组的模型
interface ProviderGroup {
  providerId: string
  providerName: string
  models: ModelsDevModelItem[]
}

const groupedModels = computed(() => {
  let models = allModels.value.filter(m => !m.deprecated)
  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    models = models.filter(model => {
      const searchableText = `${model.providerId} ${model.providerName} ${model.modelId} ${model.modelName} ${model.family || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 按提供商分组
  const groups = new Map<string, ProviderGroup>()
  for (const model of models) {
    if (!groups.has(model.providerId)) {
      groups.set(model.providerId, {
        providerId: model.providerId,
        providerName: model.providerName,
        models: []
      })
    }
    groups.get(model.providerId)?.models.push(model)
  }

  // 转换为数组并排序
  const result = Array.from(groups.values())

  // 如果有搜索词，把提供商名称/ID匹配的排在前面
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    result.sort((a, b) => {
      const aText = `${a.providerId} ${a.providerName}`.toLowerCase()
      const bText = `${b.providerId} ${b.providerName}`.toLowerCase()
      const aProviderMatch = keywords.some(k => aText.includes(k))
      const bProviderMatch = keywords.some(k => bText.includes(k))
      if (aProviderMatch && !bProviderMatch) return -1
      if (!aProviderMatch && bProviderMatch) return 1
      return a.providerName.localeCompare(b.providerName)
    })
  } else {
    result.sort((a, b) => a.providerName.localeCompare(b.providerName))
  }

  return result
})

// 搜索时如果只有一个提供商，自动展开
watch(groupedModels, (groups) => {
  if (searchQuery.value && groups.length === 1) {
    expandedProvider.value = groups[0].providerId
  }
})

// 切换提供商展开状态
function toggleProvider(providerId: string) {
  expandedProvider.value = expandedProvider.value === providerId ? null : providerId
}

// 阶梯计费配置
const tieredPricing = ref<TieredPricingConfig | null>(null)

type VideoResolutionPriceRow = { resolution: string; price_per_second: number | undefined }

const videoResolutionPrices = ref<VideoResolutionPriceRow[]>([])

const VIDEO_RESOLUTION_PRICE_PRESETS: Record<
  'common' | 'sora' | 'veo',
  VideoResolutionPriceRow[]
> = {
  common: [
    { resolution: '480p', price_per_second: 0 },
    { resolution: '720p', price_per_second: 0 },
    { resolution: '1080p', price_per_second: 0 },
    { resolution: '4k', price_per_second: 0 },
  ],
  sora: [
    { resolution: '720x1080', price_per_second: 0 },
    { resolution: '1024x1792', price_per_second: 0 },
  ],
  veo: [
    { resolution: '720p', price_per_second: 0 },
    { resolution: '1080p', price_per_second: 0 },
    { resolution: '4k', price_per_second: 0 },
  ],
}

interface FormData {
  name: string
  display_name: string
  default_price_per_request?: number
  supported_capabilities?: string[]
  config?: Record<string, unknown>
  is_active?: boolean
}

const defaultForm = (): FormData => ({
  name: '',
  display_name: '',
  default_price_per_request: undefined,
  supported_capabilities: [],
  config: { streaming: true },
  is_active: true,
})

const form = ref<FormData>(defaultForm())

const KEEP_FALSE_CONFIG_KEYS = new Set(['streaming'])

// 设置 config 字段
function setConfigField(key: string, value: unknown) {
  if (!form.value.config) {
    form.value.config = {}
  }
  if (value === undefined || value === '' || (value === false && !KEEP_FALSE_CONFIG_KEYS.has(key))) {
    delete form.value.config[key]
  } else {
    form.value.config[key] = value
  }
}

function getNested(obj: unknown, path: string): unknown {
  if (!obj || typeof obj !== 'object') return undefined
  const parts = path.split('.').filter(Boolean)
  let cur = obj as Record<string, unknown>
  for (const p of parts) {
    if (!cur || typeof cur !== 'object') return undefined
    cur = cur[p] as Record<string, unknown>
  }
  return cur
}

function setNested(obj: unknown, path: string, value: unknown) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur = obj as Record<string, unknown>
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') {
      cur[p] = {}
    }
    cur = cur[p] as Record<string, unknown>
  }
  cur[parts[parts.length - 1]] = value
}

function deleteNested(obj: unknown, path: string) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur = obj as Record<string, unknown>
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') return
    cur = cur[p] as Record<string, unknown>
  }
  delete cur[parts[parts.length - 1]]
}

function pruneEmptyBillingConfig() {
  const cfg = form.value.config
  if (!cfg || typeof cfg !== 'object') return
  const billing = cfg.billing as Record<string, unknown> | undefined
  if (!billing || typeof billing !== 'object') return
  const video = billing.video as Record<string, unknown> | undefined
  if (video && typeof video === 'object' && Object.keys(video).length === 0) {
    delete billing.video
  }
  if (Object.keys(billing).length === 0) {
    delete cfg.billing
  }
}

/**
 * Normalize resolution key:
 * - lowercase, remove spaces, × → x
 * - For WxH format, sort dimensions so smaller comes first (720x1080 = 1080x720)
 */
function normalizeResolutionKey(raw: string): string {
  let k = (raw || '').trim().toLowerCase().replace(/\s+/g, '').replace(/×/g, 'x')
  // Check if it's WxH format (e.g., 1080x720)
  const match = k.match(/^(\d+)x(\d+)$/)
  if (match) {
    const a = parseInt(match[1], 10)
    const b = parseInt(match[2], 10)
    // Sort: smaller dimension first
    k = a <= b ? `${a}x${b}` : `${b}x${a}`
  }
  return k
}

function loadVideoPricingFromConfig() {
  const cfg = form.value.config || {}
  const raw = getNested(cfg, 'billing.video.price_per_second_by_resolution')
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    // 按分辨率从低到高排序
    const sortedEntries = sortResolutionEntries(Object.entries(raw as Record<string, unknown>))
    videoResolutionPrices.value = sortedEntries.map(([k, v]) => ({
      resolution: String(k),
      price_per_second: typeof v === 'number' ? v : undefined,
    }))
  } else {
    videoResolutionPrices.value = []
  }
}

function applyVideoPricingToConfig() {
  if (!form.value.config) {
    form.value.config = {}
  }
  const cfg = form.value.config

  // Clean legacy keys
  deleteNested(cfg, 'billing.video.price_per_second')
  deleteNested(cfg, 'billing.video.resolution_multipliers')

  // resolution/size prices (normalized: 1080x720 → 720x1080)
  const map: Record<string, number> = {}
  for (const row of videoResolutionPrices.value) {
    const k = normalizeResolutionKey(row.resolution || '')
    const v = row.price_per_second
    if (!k) continue
    if (typeof v !== 'number' || Number.isNaN(v)) continue
    map[k] = v
  }
  if (Object.keys(map).length > 0) {
    setNested(cfg, 'billing.video.price_per_second_by_resolution', map)
  } else {
    deleteNested(cfg, 'billing.video.price_per_second_by_resolution')
  }

  pruneEmptyBillingConfig()
}

function addVideoResolutionPriceRow() {
  videoResolutionPrices.value.push({ resolution: '', price_per_second: undefined })
}

function removeVideoResolutionPriceRow(idx: number) {
  videoResolutionPrices.value.splice(idx, 1)
}

function fillVideoResolutionPricePreset(preset: 'common' | 'sora' | 'veo') {
  videoResolutionPrices.value = VIDEO_RESOLUTION_PRICE_PRESETS[preset].map(r => ({
    resolution: r.resolution,
    price_per_second: r.price_per_second,
  }))
}


// 加载模型列表
async function loadModels() {
  if (allModelsCache.value.length > 0) return
  loading.value = true
  try {
    // 只加载一次全部模型，过滤在 computed 中完成
    allModelsCache.value = await getModelsDevList(false)
  } catch (err) {
    log.error('Failed to load models:', err)
  } finally {
    loading.value = false
  }
}

// 打开对话框时加载数据
watch(() => props.open, (isOpen) => {
  if (isOpen && !props.model) {
    loadModels()
  }
})

// 选择模型并填充表单
function selectModel(model: ModelsDevModelItem) {
  selectedModel.value = model
  expandedProvider.value = model.providerId
  form.value.name = model.modelId
  form.value.display_name = model.modelName

  // 构建 config
  const config: Record<string, unknown> = {
    streaming: true,
  }
  if (model.supportsVision) config.vision = true
  if (model.supportsToolCall) config.function_calling = true
  if (model.supportsReasoning) config.extended_thinking = true
  if (model.supportsStructuredOutput) config.structured_output = true
  if (model.supportsTemperature !== false) config.temperature = model.supportsTemperature
  if (model.supportsAttachment) config.attachment = true
  if (model.openWeights) config.open_weights = true
  if (model.contextLimit) config.context_limit = model.contextLimit
  if (model.outputLimit) config.output_limit = model.outputLimit
  if (model.knowledgeCutoff) config.knowledge_cutoff = model.knowledgeCutoff
  if (model.family) config.family = model.family
  if (model.releaseDate) config.release_date = model.releaseDate
  if (model.inputModalities?.length) config.input_modalities = model.inputModalities
  if (model.outputModalities?.length) config.output_modalities = model.outputModalities
  form.value.config = config
  loadVideoPricingFromConfig()

  if (model.inputPrice !== undefined || model.outputPrice !== undefined) {
    tieredPricing.value = {
      tiers: [{
        up_to: null,
        input_price_per_1m: model.inputPrice || 0,
        output_price_per_1m: model.outputPrice || 0,
      }]
    }
  } else {
    tieredPricing.value = null
  }
}

// 清除选择（手动填写）
function clearSelection() {
  selectedModel.value = null
  form.value = defaultForm()
  tieredPricing.value = null
}

// Logo 加载失败处理
function handleLogoError(event: Event) {
  const img = event.target as HTMLImageElement
  img.style.display = 'none'
}

// 重置表单
function resetForm() {
  form.value = defaultForm()
  tieredPricing.value = null
  videoResolutionPrices.value = []
  searchQuery.value = ''
  selectedModel.value = null
  expandedProvider.value = null
}

// 加载模型数据（编辑模式）
function loadModelData() {
  if (!props.model) return
  // 先重置创建模式的残留状态
  selectedModel.value = null
  searchQuery.value = ''
  expandedProvider.value = null

  form.value = {
    name: props.model.name,
    display_name: props.model.display_name,
    default_price_per_request: props.model.default_price_per_request,
    supported_capabilities: [...(props.model.supported_capabilities || [])],
    config: props.model.config ? { ...props.model.config } : { streaming: true },
    is_active: props.model.is_active,
  }
  // 确保 tieredPricing 也被正确设置或重置
  tieredPricing.value = props.model.default_tiered_pricing
    ? JSON.parse(JSON.stringify(props.model.default_tiered_pricing))
    : null
  loadVideoPricingFromConfig()
}

// 使用 useFormDialog 统一处理对话框逻辑
const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.model,
  isLoading: submitting,
  onClose: () => emit('update:open', false),
  loadData: loadModelData,
  resetForm,
})

async function handleSubmit() {
  if (!form.value.name || !form.value.display_name) {
    showError('请填写模型ID和名称')
    return
  }

  const finalTiers = tieredPricingEditorRef.value?.getFinalTiers()
  const finalTieredPricing = finalTiers ? { tiers: finalTiers } : tieredPricing.value

  if (!finalTieredPricing?.tiers?.length) {
    showError('请配置至少一个价格阶梯')
    return
  }

  // Apply billing (video) pricing into config before cleaning/submitting.
  applyVideoPricingToConfig()

  // Auto-infer supported_capabilities from tiered pricing config
  const caps = new Set(form.value.supported_capabilities || [])
  const has1hPricing = finalTieredPricing?.tiers?.some(
    (t: Record<string, unknown>) => Array.isArray(t.cache_ttl_pricing)
      && (t.cache_ttl_pricing as Array<Record<string, unknown>>).some(c => c.ttl_minutes === 60)
  )
  if (has1hPricing) {
    caps.add('cache_1h')
  } else {
    caps.delete('cache_1h')
  }
  form.value.supported_capabilities = caps.size > 0 ? [...caps] : []

  // 清理空的 config
  const cleanConfig = form.value.config && Object.keys(form.value.config).length > 0
    ? form.value.config
    : undefined

  submitting.value = true
  try {
    if (isEditMode.value && props.model) {
      const updateData: GlobalModelUpdate = {
        display_name: form.value.display_name,
        config: cleanConfig || null,
        default_price_per_request: form.value.default_price_per_request ?? null,
        default_tiered_pricing: finalTieredPricing,
        supported_capabilities: form.value.supported_capabilities?.length ? form.value.supported_capabilities : null,
        is_active: form.value.is_active,
      }
      await updateGlobalModel(props.model.id, updateData)
      success('模型更新成功')
    } else {
      const createData: GlobalModelCreate = {
        name: form.value.name ?? '',
        display_name: form.value.display_name ?? '',
        config: cleanConfig,
        default_price_per_request: form.value.default_price_per_request ?? undefined,
        default_tiered_pricing: finalTieredPricing,
        supported_capabilities: form.value.supported_capabilities?.length ? form.value.supported_capabilities : undefined,
        is_active: form.value.is_active,
      }
      await createGlobalModel(createData)
      success('模型创建成功')
      clearSelection()
      emit('success')
      return
    }
    emit('update:open', false)
    emit('success')
  } catch (err: unknown) {
    showError(parseApiError(err, isEditMode.value ? '更新失败' : '创建失败'), isEditMode.value ? '更新失败' : '创建失败')
  } finally {
    submitting.value = false
  }
}
</script>
