<template>
  <Dialog
    :model-value="open"
    :title="isEditing ? '编辑模型配置' : '添加模型'"
    :description="isEditing ? '修改模型价格配置' : '为此 Provider 添加模型实现'"
    :icon="isEditing ? SquarePen : Layers"
    size="xl"
    @update:model-value="handleClose"
  >
    <form
      class="space-y-4"
      @submit.prevent="handleSubmit"
    >
      <!-- 添加模式：选择或手动创建本地全局模型 -->
      <div
        v-if="!isEditing"
        class="space-y-3"
      >
        <div class="flex items-center justify-between gap-3">
          <Label for="global-model">选择已有模型或手动添加 *</Label>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs"
            @click="manualGlobalModelMode = !manualGlobalModelMode"
          >
            {{ manualGlobalModelMode ? '选择已有模型' : '手动添加' }}
          </Button>
        </div>
        <div
          v-if="!manualGlobalModelMode"
          class="space-y-2"
        >
          <Select
            :model-value="form.global_model_id"
            :disabled="loadingGlobalModels"
            @update:model-value="handleGlobalModelSelect"
          >
            <SelectTrigger class="w-full">
              <SelectValue :placeholder="loadingGlobalModels ? '加载中...' : '请选择模型'" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="model in availableGlobalModels"
                :key="model.id"
                :value="model.id"
              >
                {{ model.display_name }} ({{ model.name }})
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div
          v-else
          class="rounded-lg border border-border/60 bg-muted/20 p-3 space-y-3"
        >
          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-1.5">
              <Label
                for="manual-global-model-name"
                class="text-xs"
              >模型ID *</Label>
              <Input
                id="manual-global-model-name"
                v-model="form.manual_global_model_name"
                placeholder="如 gpt-4o-mini"
                @update:model-value="syncManualProviderName"
              />
            </div>
            <div class="space-y-1.5">
              <Label
                for="manual-global-model-display-name"
                class="text-xs"
              >显示名称</Label>
              <Input
                id="manual-global-model-display-name"
                v-model="form.manual_global_model_display_name"
                placeholder="默认使用模型ID"
              />
            </div>
          </div>
          <p class="text-xs text-muted-foreground">
            无法联网获取模型目录时，可直接填写模型ID。保存时会先创建本地全局模型，再添加到当前 Provider。
          </p>
        </div>
        <p
          v-if="availableGlobalModels.length === 0 && !loadingGlobalModels && !manualGlobalModelMode"
          class="text-xs text-muted-foreground"
        >
          没有可选择的本地全局模型。可以切换到“手动添加”继续保存。
        </p>
        <div class="space-y-1.5">
          <Label
            for="provider-model-name"
            class="text-xs"
          >Provider 模型名 *</Label>
          <Input
            id="provider-model-name"
            v-model="form.provider_model_name"
            placeholder="Provider 实际接收的模型名，如 gpt-4o-mini"
          />
          <p class="text-xs text-muted-foreground">
            默认跟随所选模型ID；如内网模型、兼容网关或别名不同，可手动覆盖。
          </p>
        </div>
        <div
          v-if="selectedGlobalModelSupportsEmbedding"
          class="rounded-lg border border-border/60 bg-muted/20 px-3 py-2"
        >
          <div class="text-sm font-medium">
            Embedding
          </div>
          <p class="text-xs text-muted-foreground">
            此模型将继承全局模型的 Embeddings 元数据，不按 Chat 能力处理。
          </p>
        </div>
      </div>

      <!-- 编辑模式：显示模型信息 -->
      <div
        v-else
        class="rounded-lg border bg-muted/30 p-4"
      >
        <div class="flex items-start justify-between">
          <div>
            <p class="font-semibold text-lg">
              {{ editingModel?.global_model_display_name || editingModel?.provider_model_name }}
            </p>
            <p class="text-sm text-muted-foreground font-mono">
              {{ editingModel?.provider_model_name }}
            </p>
            <Badge
              v-if="editingModelSupportsEmbedding"
              variant="secondary"
              class="mt-2 text-xs"
            >
              Embedding
            </Badge>
          </div>
        </div>
      </div>

      <div class="rounded-lg border border-border/60 bg-muted/20 px-3 py-2">
        <div class="flex items-start gap-2">
          <Checkbox
            :checked="isImageGenerationEnabled"
            class="mt-0.5"
            @update:checked="setImageGenerationEnabled"
          />
          <div class="space-y-1">
            <div class="text-sm font-medium">
              图片模型
            </div>
            <p class="text-xs text-muted-foreground">
              启用图片输出计费，并展开尺寸 × 质量矩阵价格。
            </p>
          </div>
        </div>
      </div>

      <!-- 价格配置 -->
      <section class="space-y-3 rounded-lg border bg-card p-4">
        <h4 class="font-medium text-sm">
          选择计费模式
        </h4>
        <Tabs
          v-model="billingMode"
          @update:model-value="handleBillingModeChange"
        >
          <TabsList class="grid w-full grid-cols-4">
            <TabsTrigger value="token">
              Token
            </TabsTrigger>
            <TabsTrigger value="request">
              按次
            </TabsTrigger>
            <TabsTrigger value="image">
              图片
            </TabsTrigger>
            <TabsTrigger value="video">
              视频
            </TabsTrigger>
          </TabsList>

          <TieredPricingEditor
            v-show="billingMode === 'token' || billingMode === 'image'"
            ref="tieredPricingEditorRef"
            v-model="tieredPricing"
            class="mt-3"
            :auto-fill-missing-cache-prices="autoFillMissingCachePrices"
            :show-token-pricing="billingMode === 'token'"
            :show-image-pricing="isImageGenerationEnabled"
            :show-image-editor="billingMode === 'image'"
            :show-processing-tier-multiplier-controls="true"
          />

          <TabsContent
            value="request"
            class="pt-2"
          >
            <div class="rounded-lg border bg-muted/20 p-4 space-y-2">
              <Label class="text-xs">每次请求价格（美元）</Label>
              <Input
                :model-value="form.price_per_request ?? ''"
                type="number"
                step="0.001"
                min="0"
                class="max-w-48"
                placeholder="留空使用全局模型默认值"
                @update:model-value="updatePricePerRequest"
              />
              <p class="text-xs text-muted-foreground">
                按每次 API 请求收取固定费用；未修改时继续继承全局模型。
              </p>
            </div>
          </TabsContent>

          <TabsContent
            value="video"
            class="pt-2"
          >
            <div class="space-y-3 rounded-lg border bg-muted/20 p-4">
              <div>
                <div class="text-sm font-medium">
                  视频计费（分辨率 × 时长）
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  根据输出分辨率配置每秒视频价格；未修改时继续继承全局模型。
                </p>
              </div>

              <div class="flex items-center gap-1.5 flex-wrap">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="() => { fillVideoResolutionPricePreset('common'); configTouched = true }"
                >
                  通用
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="() => { fillVideoResolutionPricePreset('sora'); configTouched = true }"
                >
                  Sora
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="() => { fillVideoResolutionPricePreset('veo'); configTouched = true }"
                >
                  Veo
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="() => { addVideoResolutionPriceRow(); configTouched = true }"
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
                      @update:model-value="() => { configTouched = true }"
                    />
                    <Input
                      :model-value="row.price_per_second ?? ''"
                      type="number"
                      step="0.0001"
                      min="0"
                      class="h-7 text-sm"
                      placeholder="0"
                      @update:model-value="(v) => { row.price_per_second = parseNumberInput(v, { allowFloat: true }); configTouched = true }"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      title="删除"
                      @click="() => { removeVideoResolutionPriceRow(idx); configTouched = true }"
                    >
                      <Trash2 class="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </div>
              </div>
              <div
                v-else
                class="rounded-lg border border-dashed py-8 text-center text-xs text-muted-foreground"
              >
                选择一个价格预设或添加自定义分辨率
              </div>
            </div>
          </TabsContent>
        </Tabs>
      </section>
    </form>

    <template #footer>
      <Button
        variant="outline"
        @click="handleClose(false)"
      >
        取消
      </Button>
      <Button
        :disabled="submitting || (!isEditing && !canSubmitCreate)"
        @click="handleSubmit"
      >
        <Loader2
          v-if="submitting"
          class="w-4 h-4 mr-2 animate-spin"
        />
        {{ isEditing ? '保存' : '添加' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { parseApiError } from '@/utils/errorParser'
import { Loader2, Layers, SquarePen, Plus, Trash2 } from 'lucide-vue-next'
import {
  Dialog,
  Button,
  Input,
  Label,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Badge,
  Checkbox,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { parseNumberInput, sortResolutionEntries } from '@/utils/form'
import { createModel, updateModel, getProviderModels } from '@/api/endpoints/models'
import {
  createGlobalModel,
  getGlobalModel,
  listGlobalModels,
  type GlobalModelResponse,
} from '@/api/global-models'
import TieredPricingEditor from '@/features/models/components/TieredPricingEditor.vue'
import { tieredPricingHasImageOutputPricing } from '@/features/models/utils/tiered-pricing'
import type {
  Model,
  ProviderTieredPricingConfig,
  TieredPricingConfig,
} from '@/api/endpoints'
import {
  buildProviderTieredPricingOverride,
  buildProviderModelCreatePayload,
  buildProviderModelUpdatePayload,
  mergeProviderTieredPricingForEditing,
  modelSupportsEmbedding,
} from './provider-model-form-helpers'

interface Props {
  open: boolean
  providerId: string
  providerName?: string
  editingModel?: Model | null
}

const props = withDefaults(defineProps<Props>(), {
  providerName: '',
  editingModel: null
})

const emit = defineEmits<{
  'update:open': [value: boolean]
  'saved': []
}>()

const { error: showError, success: showSuccess } = useToast()

const tieredPricingEditorRef = ref<InstanceType<typeof TieredPricingEditor> | null>(null)

const isEditing = computed(() => !!props.editingModel)

const selectedGlobalModel = computed(() => {
  return availableGlobalModels.value.find(model => model.id === form.value.global_model_id) || null
})

const selectedGlobalModelSupportsEmbedding = computed(() => modelSupportsEmbedding(selectedGlobalModel.value))
const selectedGlobalModelSupportsImageGeneration = computed(() => modelSupportsImageGeneration(selectedGlobalModel.value))
const editingModelSupportsEmbedding = computed(() => {
  return props.editingModel?.effective_supports_embedding === true
    || modelSupportsEmbedding(props.editingModel)
})
const editingModelSupportsImageGeneration = computed(() => {
  return props.editingModel?.effective_supports_image_generation === true
    || modelSupportsImageGeneration(props.editingModel)
})

const isImageGenerationEnabled = computed(() => {
  if (imageGenerationExplicitOverride.value !== null) {
    return imageGenerationExplicitOverride.value
  }
  if (form.value.supports_image_generation !== undefined) {
    return form.value.supports_image_generation === true
  }
  const supportsImageGeneration = isEditing.value
    ? editingModelSupportsImageGeneration.value
    : selectedGlobalModelSupportsImageGeneration.value
  return supportsImageGeneration || tieredPricingHasImageOutputPricing(tieredPricing.value)
})

// 表单状态
const submitting = ref(false)
const loadingGlobalModels = ref(false)
const availableGlobalModels = ref<GlobalModelResponse[]>([])
const manualGlobalModelMode = ref(false)
const billingMode = ref('token')

// 阶梯计费配置
const tieredPricing = ref<TieredPricingConfig | null>(null)
// 跟踪用户是否修改了阶梯配置（用于判断是否提交）
const tieredPricingModified = ref(false)
// 保存原始配置用于比较
const originalTieredPricing = ref<string>('')
const originalEditorTieredPricing = ref<TieredPricingConfig | null>(null)
const originalProviderTieredPricing = ref<ProviderTieredPricingConfig | null>(null)
const pricePerRequestModified = ref(false)
const originalPricePerRequest = ref<number | undefined>(undefined)

type VideoResolutionPriceRow = { resolution: string; price_per_second: number | undefined }

const configTouched = ref(false)
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

const DEFAULT_MANUAL_GLOBAL_MODEL_PRICING: TieredPricingConfig = {
  tiers: [{ up_to: null, input_price_per_1m: 0, output_price_per_1m: 0 }],
}

const form = ref({
  global_model_id: '',
  provider_model_name: '',
  manual_global_model_name: '',
  manual_global_model_display_name: '',
  price_per_request: undefined as number | undefined,
  config: {} as Record<string, unknown>,
  // 能力配置
  supports_vision: undefined as boolean | undefined,
  supports_function_calling: undefined as boolean | undefined,
  supports_streaming: undefined as boolean | undefined,
  supports_extended_thinking: undefined as boolean | undefined,
  supports_image_generation: undefined as boolean | undefined,
  is_active: true
})
const imageGenerationExplicitOverride = ref<boolean | null>(null)
const autoFillMissingCachePrices = computed(() => !isEditing.value && manualGlobalModelMode.value)

const canSubmitCreate = computed(() => {
  if (isEditing.value) return true
  if (!form.value.provider_model_name.trim()) return false
  if (manualGlobalModelMode.value) return !!form.value.manual_global_model_name.trim()
  return !!form.value.global_model_id
})

// 监听 open 变化
watch(() => props.open, async (newOpen) => {
  if (newOpen) {
    resetForm()
    if (props.editingModel) {
      // 编辑模式：填充表单
      // 使用有效配置（合并全局模型的默认值）供用户查看和编辑
      const effectiveConfig = props.editingModel.effective_config || props.editingModel.config || {}
      form.value = {
        global_model_id: props.editingModel.global_model_id || '',
        provider_model_name: props.editingModel.provider_model_name || '',
        manual_global_model_name: '',
        manual_global_model_display_name: '',
        // 显示有效的按次计费价格（继承自全局模型）
        price_per_request: props.editingModel.effective_price_per_request ?? props.editingModel.price_per_request ?? undefined,
        config: effectiveConfig ? JSON.parse(JSON.stringify(effectiveConfig)) : {},
        supports_vision: props.editingModel.supports_vision ?? undefined,
        supports_function_calling: props.editingModel.supports_function_calling ?? undefined,
        supports_streaming: props.editingModel.supports_streaming ?? undefined,
        supports_extended_thinking: props.editingModel.supports_extended_thinking ?? undefined,
        supports_image_generation: props.editingModel.supports_image_generation ?? undefined,
        is_active: props.editingModel.is_active
      }
      // 从有效配置中加载视频费用
      loadVideoPricingFromConfig(effectiveConfig)
      // Provider 可以只覆盖 processing_tiers。此时后端 effective_tiered_pricing
      // 仍是原始 partial JSON，需要取 GlobalModel 默认目录合成完整编辑视图。
      const providerPricing = props.editingModel.tiered_pricing
        ? JSON.parse(JSON.stringify(props.editingModel.tiered_pricing)) as ProviderTieredPricingConfig
        : null
      let globalDefaultPricing = providerPricing
        ? null
        : props.editingModel.effective_tiered_pricing
      if (providerPricing && props.editingModel.global_model_id) {
        try {
          const globalModel = await getGlobalModel(props.editingModel.global_model_id)
          globalDefaultPricing = globalModel.default_tiered_pricing
        } catch (err: unknown) {
          if (!providerPricing.tiers?.length) {
            showError(parseApiError(err, '加载 GlobalModel 默认价格失败'), '错误')
          }
        }
      }
      const pricing = mergeProviderTieredPricingForEditing(globalDefaultPricing, providerPricing)
        || (props.editingModel.effective_tiered_pricing?.tiers?.length
          ? props.editingModel.effective_tiered_pricing
          : null)
      if (pricing) {
        tieredPricing.value = JSON.parse(JSON.stringify(pricing))
      }
      originalEditorTieredPricing.value = tieredPricing.value
        ? JSON.parse(JSON.stringify(tieredPricing.value))
        : null
      originalProviderTieredPricing.value = providerPricing
      originalTieredPricing.value = JSON.stringify(tieredPricing.value)
      tieredPricingModified.value = false
      originalPricePerRequest.value = form.value.price_per_request
      pricePerRequestModified.value = false
      selectInitialBillingMode()
    } else {
      // 添加模式：加载可用全局模型
      await loadAvailableGlobalModels()
    }
  }
})

// 添加模式：选择全局模型时显示其阶梯计费配置（仅供预览）
// 注意：为保持继承关系，添加时只有用户修改了配置才提交 tiered_pricing
watch(() => form.value.global_model_id, (newId) => {
  if (!isEditing.value && newId) {
    const selectedModel = availableGlobalModels.value.find(m => m.id === newId)
    if (selectedModel && !form.value.provider_model_name.trim()) {
      form.value.provider_model_name = selectedModel.name
    }
    if (selectedModel?.default_tiered_pricing) {
      // 深拷贝阶梯计费配置用于预览
      const pricingCopy = JSON.parse(JSON.stringify(selectedModel.default_tiered_pricing))
      tieredPricing.value = pricingCopy
      originalEditorTieredPricing.value = JSON.parse(JSON.stringify(pricingCopy))
      originalProviderTieredPricing.value = null
      // 保存原始配置用于比较
      originalTieredPricing.value = JSON.stringify(pricingCopy)
    } else {
      tieredPricing.value = null
      originalEditorTieredPricing.value = null
      originalProviderTieredPricing.value = null
      originalTieredPricing.value = JSON.stringify(null)
    }
    tieredPricingModified.value = false
    // 同时继承按次计费（仅供预览）
    form.value.price_per_request = selectedModel?.default_price_per_request ?? undefined
    originalPricePerRequest.value = form.value.price_per_request
    pricePerRequestModified.value = false
    loadVideoPricingFromConfig(selectedModel?.config || {})
    configTouched.value = false
    selectInitialBillingMode()
  }
})

// 监听阶梯配置变化，标记为已修改
watch(tieredPricing, (newValue) => {
  if (originalTieredPricing.value) {
    const newJson = JSON.stringify(newValue)
    tieredPricingModified.value = newJson !== originalTieredPricing.value
  }
}, { deep: true })

// 重置表单
function resetForm() {
  imageGenerationExplicitOverride.value = null
  form.value = {
    global_model_id: '',
    provider_model_name: '',
    manual_global_model_name: '',
    manual_global_model_display_name: '',
    price_per_request: undefined,
    config: {},
    supports_vision: undefined,
    supports_function_calling: undefined,
    supports_streaming: undefined,
    supports_extended_thinking: undefined,
    supports_image_generation: undefined,
    is_active: true
  }
  configTouched.value = false
  videoResolutionPrices.value = []
  tieredPricing.value = null
  tieredPricingModified.value = false
  originalTieredPricing.value = ''
  originalEditorTieredPricing.value = null
  originalProviderTieredPricing.value = null
  pricePerRequestModified.value = false
  originalPricePerRequest.value = undefined
  availableGlobalModels.value = []
  manualGlobalModelMode.value = false
  billingMode.value = 'token'
}

function updatePricePerRequest(value: string | number) {
  form.value.price_per_request = parseNumberInput(value, { allowFloat: true })
  pricePerRequestModified.value = form.value.price_per_request !== originalPricePerRequest.value
}

function handleBillingModeChange(mode: string) {
  billingMode.value = mode
  if (mode === 'image' && !isImageGenerationEnabled.value) {
    setImageGenerationEnabled(true)
  }
}

function selectInitialBillingMode() {
  if (videoResolutionPrices.value.length > 0) {
    billingMode.value = 'video'
  } else if (isImageGenerationEnabled.value) {
    billingMode.value = 'image'
  } else if (form.value.price_per_request !== undefined) {
    billingMode.value = 'request'
  } else {
    billingMode.value = 'token'
  }
}

function handleGlobalModelSelect(value: string) {
  imageGenerationExplicitOverride.value = null
  form.value.supports_image_generation = undefined
  form.value.global_model_id = value
  const selectedModel = availableGlobalModels.value.find(model => model.id === value)
  form.value.provider_model_name = selectedModel?.name || form.value.provider_model_name
}

function modelSupportsImageGeneration(model: {
  supported_capabilities?: string[] | null
  supports_image_generation?: boolean | null
  effective_supports_image_generation?: boolean | null
  default_tiered_pricing?: TieredPricingConfig | null
  tiered_pricing?: ProviderTieredPricingConfig | null
  effective_tiered_pricing?: ProviderTieredPricingConfig | null
  config?: Record<string, unknown> | null
} | null | undefined): boolean {
  if (!model) return false
  if (model.effective_supports_image_generation === true) return true
  if (model.supports_image_generation === true) return true
  const config = model.config || {}
  return model.supported_capabilities?.includes('image_generation') === true
    || config.image_generation === true
    || config.model_type === 'image'
    || (Array.isArray(config.api_formats) && config.api_formats.some((format) => String(format).endsWith(':image')))
    || tieredPricingHasImageOutputPricing(model.default_tiered_pricing)
    || tieredPricingHasImageOutputPricing(model.tiered_pricing)
    || tieredPricingHasImageOutputPricing(model.effective_tiered_pricing)
}

function setImageGenerationEnabled(value: boolean | 'indeterminate') {
  const enabled = value === true
  imageGenerationExplicitOverride.value = enabled
  form.value.supports_image_generation = enabled
}

function syncManualProviderName(value: string | number) {
  const modelName = String(value || '').trim()
  if (!form.value.provider_model_name.trim()) {
    form.value.provider_model_name = modelName
  }
  if (!form.value.manual_global_model_display_name.trim()) {
    form.value.manual_global_model_display_name = modelName
  }
}

function getNested(obj: Record<string, unknown>, path: string): unknown {
  if (!obj || typeof obj !== 'object') return undefined
  const parts = path.split('.').filter(Boolean)
  let cur: unknown = obj
  for (const p of parts) {
    if (!cur || typeof cur !== 'object') return undefined
    cur = (cur as Record<string, unknown>)[p]
  }
  return cur
}

function setNested(obj: Record<string, unknown>, path: string, value: unknown) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur: Record<string, unknown> = obj
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') {
      cur[p] = {}
    }
    cur = cur[p] as Record<string, unknown>
  }
  cur[parts[parts.length - 1]] = value
}

function deleteNested(obj: Record<string, unknown>, path: string) {
  if (!obj || typeof obj !== 'object') return
  const parts = path.split('.').filter(Boolean)
  if (parts.length === 0) return
  let cur: Record<string, unknown> = obj
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i]
    if (!cur[p] || typeof cur[p] !== 'object') return
    cur = cur[p] as Record<string, unknown>
  }
  delete cur[parts[parts.length - 1]]
}

function pruneEmptyBillingConfig(cfg: Record<string, unknown>) {
  const billing = cfg.billing
  if (!billing || typeof billing !== 'object') return
  const billingObj = billing as Record<string, unknown>
  const video = billingObj.video
  if (video && typeof video === 'object' && Object.keys(video).length === 0) {
    delete billingObj.video
  }
  if (Object.keys(billingObj).length === 0) {
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

function loadVideoPricingFromConfig(cfg: Record<string, unknown>) {
  const raw = getNested(cfg, 'billing.video.price_per_second_by_resolution')
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    // 按分辨率从低到高排序
    const sortedEntries = sortResolutionEntries(Object.entries(raw))
    videoResolutionPrices.value = sortedEntries.map(([k, v]) => ({
      resolution: String(k),
      price_per_second: typeof v === 'number' ? v : undefined,
    }))
  } else {
    videoResolutionPrices.value = []
  }
}

function applyVideoPricingToConfig(cfg: Record<string, unknown>) {
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
  pruneEmptyBillingConfig(cfg)
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

function _copyVideoPricingFromSelectedGlobal() {
  const gm = availableGlobalModels.value.find(m => m.id === form.value.global_model_id)
  const cfg = gm?.config || {}
  if (cfg && typeof cfg === 'object') {
    const raw = getNested(cfg, 'billing.video.price_per_second_by_resolution')
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      videoResolutionPrices.value = Object.entries(raw).map(([k, v]) => ({
        resolution: String(k),
        price_per_second: typeof v === 'number' ? v : undefined,
      }))
    }
  }
  configTouched.value = true
}

async function createManualGlobalModel(finalTieredPricing: TieredPricingConfig | null, cleanConfig: Record<string, unknown> | undefined): Promise<GlobalModelResponse> {
  const modelName = form.value.manual_global_model_name.trim()
  const displayName = form.value.manual_global_model_display_name.trim() || modelName
  const supportedCapabilities = [
    form.value.supports_vision === true ? 'vision' : null,
    form.value.supports_function_calling === true ? 'function_calling' : null,
    form.value.supports_streaming === true ? 'streaming' : null,
    form.value.supports_extended_thinking === true ? 'extended_thinking' : null,
    form.value.supports_image_generation === true ? 'image_generation' : null,
  ].filter((capability): capability is string => capability !== null)

  return createGlobalModel({
    name: modelName,
    display_name: displayName,
    default_price_per_request: form.value.price_per_request,
    default_tiered_pricing: finalTieredPricing || DEFAULT_MANUAL_GLOBAL_MODEL_PRICING,
    supported_capabilities: supportedCapabilities.length ? supportedCapabilities : undefined,
    config: cleanConfig,
    is_active: true,
  })
}

// 加载可用的全局模型（排除已添加的）
async function loadAvailableGlobalModels() {
  loadingGlobalModels.value = true
  try {
    const [globalModelsResponse, existingModels] = await Promise.all([
      listGlobalModels({ limit: 1000, is_active: true }),
      getProviderModels(props.providerId)
    ])
    const allGlobalModels = globalModelsResponse.models || []

    // 获取当前 provider 已添加的模型的 global_model_id 列表
    const existingGlobalModelIds = new Set(
      existingModels.map((m: Model) => m.global_model_id)
    )

    // 过滤掉已添加的模型
    availableGlobalModels.value = allGlobalModels.filter(
      (gm: GlobalModelResponse) => !existingGlobalModelIds.has(gm.id)
    )
  } catch (err: unknown) {
    showError(parseApiError(err, '加载模型列表失败'), '错误')
  } finally {
    loadingGlobalModels.value = false
  }
}

// 关闭对话框
function handleClose(value: boolean) {
  if (!submitting.value) {
    emit('update:open', value)
  }
}

// 提交表单
async function handleSubmit() {
  if (submitting.value) return
  if (!isEditing.value && !canSubmitCreate.value) {
    showError(manualGlobalModelMode.value ? '请填写模型ID和 Provider 模型名' : '请选择模型并填写 Provider 模型名', '错误')
    return
  }

  const pricingValidationError = tieredPricingEditorRef.value?.getValidationError()
  if (pricingValidationError) {
    showError(pricingValidationError, '价格配置错误')
    return
  }

  submitting.value = true
  try {
    // 获取包含自动计算缓存价格的最终数据
    const finalTieredPricing = tieredPricingEditorRef.value?.getFinalPricing() ?? tieredPricing.value
    const providerTieredPricingOverride = tieredPricingModified.value
      ? buildProviderTieredPricingOverride(
          finalTieredPricing,
          originalEditorTieredPricing.value,
          originalProviderTieredPricing.value,
        )
      : null
    const supportsImageGeneration = form.value.supports_image_generation

    // Apply billing (video) pricing into config.
    applyVideoPricingToConfig(form.value.config)
    const cleanConfig = form.value.config && Object.keys(form.value.config).length > 0
      ? form.value.config
      : undefined

    if (isEditing.value && props.editingModel) {
      // 编辑模式
      // 仅提交实际修改的 Provider 覆盖；未修改字段继续继承全局模型。
      await updateModel(props.providerId, props.editingModel.id, buildProviderModelUpdatePayload({
        finalTieredPricing: providerTieredPricingOverride,
        tieredPricingModified: tieredPricingModified.value,
        pricePerRequest: form.value.price_per_request,
        pricePerRequestModified: pricePerRequestModified.value,
        cleanConfig,
        configTouched: configTouched.value,
        supportsVision: form.value.supports_vision,
        supportsFunctionCalling: form.value.supports_function_calling,
        supportsStreaming: form.value.supports_streaming,
        supportsExtendedThinking: form.value.supports_extended_thinking,
        supportsImageGeneration,
        isActive: form.value.is_active
      }))
      showSuccess('模型配置已更新')
    } else {
      // 添加模式：只有用户修改了配置才提交 tiered_pricing，否则保持继承关系
      const selectedModel = manualGlobalModelMode.value
        ? await createManualGlobalModel(finalTieredPricing, cleanConfig)
        : availableGlobalModels.value.find(m => m.id === form.value.global_model_id)
      if (!selectedModel) {
        showError('请选择模型，或切换到手动添加后填写模型ID', '错误')
        return
      }
      await createModel(props.providerId, buildProviderModelCreatePayload({
        globalModelId: selectedModel.id,
        providerModelName: form.value.provider_model_name.trim(),
        finalTieredPricing: providerTieredPricingOverride,
        tieredPricingModified: manualGlobalModelMode.value ? false : tieredPricingModified.value,
        pricePerRequest: manualGlobalModelMode.value ? undefined : form.value.price_per_request,
        pricePerRequestModified: manualGlobalModelMode.value ? false : pricePerRequestModified.value,
        cleanConfig,
        configTouched: manualGlobalModelMode.value ? false : configTouched.value,
        supportsVision: form.value.supports_vision,
        supportsFunctionCalling: form.value.supports_function_calling,
        supportsStreaming: form.value.supports_streaming,
        supportsExtendedThinking: form.value.supports_extended_thinking,
        supportsImageGeneration,
        isActive: form.value.is_active
      }))
      showSuccess('模型已添加')
    }
    emit('update:open', false)
    emit('saved')
  } catch (err: unknown) {
    showError(parseApiError(err, isEditing.value ? '更新失败' : '添加失败'), '错误')
  } finally {
    submitting.value = false
  }
}
</script>
