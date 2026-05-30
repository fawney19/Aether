<template>
  <div class="space-y-3">
    <!-- 阶梯列表 -->
    <div
      v-for="(tier, index) in localTiers"
      :key="index"
      class="p-3 border rounded-lg bg-muted/20 space-y-3"
    >
      <!-- 阶梯头部 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2 text-sm">
          <span class="text-muted-foreground">{{ getTierStartLabel(index) }}</span>
          <span class="text-muted-foreground">-</span>
          <template v-if="index < localTiers.length - 1">
            <template v-if="customInputMode[index]">
              <Input
                v-model="customInputValue[index]"
                type="number"
                min="1"
                class="h-7 w-20 text-sm"
                placeholder="K"
                @keyup.enter="confirmCustomInput(index)"
                @blur="confirmCustomInput(index)"
              />
              <span class="text-xs text-muted-foreground">K</span>
            </template>
            <select
              v-else
              :value="getSelectValue(index)"
              class="h-7 px-2 text-sm border rounded bg-background"
              @change="(e) => handleThresholdChange(index, parseInt((e.target as HTMLSelectElement).value))"
            >
              <option
                v-for="opt in getAvailableThresholds(index)"
                :key="opt.value"
                :value="opt.value"
              >
                {{ opt.label }}
              </option>
            </select>
          </template>
          <span
            v-else
            class="font-medium"
          >无上限</span>
        </div>
        <Button
          v-if="localTiers.length > 1"
          variant="ghost"
          size="sm"
          class="h-7 w-7 p-0"
          @click="removeTier(index)"
        >
          <X class="w-4 h-4 text-muted-foreground hover:text-destructive" />
        </Button>
      </div>

      <!-- 价格输入 -->
      <div
        class="grid gap-3"
        :class="[showCache1h ? 'grid-cols-5' : 'grid-cols-4']"
      >
        <div class="space-y-1">
          <Label class="text-xs">输入 ($/M)</Label>
          <Input
            :model-value="tier.input_price_per_1m"
            type="number"
            step="0.01"
            min="0"
            class="h-8"
            placeholder="0"
            @update:model-value="(v) => updateInputPrice(index, parseFloatInput(v))"
          />
        </div>
        <div class="space-y-1">
          <Label class="text-xs">输出 ($/M)</Label>
          <Input
            :model-value="tier.output_price_per_1m"
            type="number"
            step="0.01"
            min="0"
            class="h-8"
            placeholder="0"
            @update:model-value="(v) => updateOutputPrice(index, parseFloatInput(v))"
          />
        </div>
        <div class="space-y-1">
          <Label class="text-xs text-muted-foreground">缓存创建</Label>
          <Input
            :model-value="getCacheCreationDisplay(index)"
            type="number"
            step="0.01"
            min="0"
            class="h-8"
            :placeholder="getCacheCreationPlaceholder(index)"
            @update:model-value="(v) => updateCacheCreation(index, v)"
          />
        </div>
        <div class="space-y-1">
          <Label class="text-xs text-muted-foreground">缓存读取</Label>
          <Input
            :model-value="getCacheReadDisplay(index)"
            type="number"
            step="0.01"
            min="0"
            class="h-8"
            :placeholder="getCacheReadPlaceholder(index)"
            @update:model-value="(v) => updateCacheRead(index, v)"
          />
        </div>
        <div
          v-if="showCache1h"
          class="space-y-1"
        >
          <Label class="text-xs text-muted-foreground">1h 缓存</Label>
          <Input
            :model-value="getCache1hDisplay(index)"
            type="number"
            step="0.01"
            min="0"
            class="h-8"
            :placeholder="getCache1hPlaceholder(index)"
            @update:model-value="(v) => updateCache1h(index, v)"
          />
        </div>
      </div>
    </div>

    <!-- 添加阶梯按钮 -->
    <Button
      variant="outline"
      size="sm"
      class="w-full"
      @click="addTier"
    >
      <Plus class="w-4 h-4 mr-2" />
      添加价格阶梯
    </Button>

    <div
      v-if="showImagePricing"
      class="rounded-lg border bg-muted/10 p-3 space-y-3"
    >
      <div class="flex flex-wrap items-end justify-between gap-3">
        <Label class="text-xs font-medium">图像输出计费 ($/张)</Label>
        <div class="flex items-center gap-2">
          <Label class="text-xs text-muted-foreground">默认价</Label>
          <Input
            :model-value="imageOutputPriceDefault"
            type="number"
            step="0.001"
            min="0"
            class="h-8 w-24"
            placeholder="0"
            @update:model-value="updateImageOutputPriceDefault"
          />
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex items-center justify-between gap-2">
          <Label class="text-xs text-muted-foreground">精确分辨率覆盖</Label>
          <span class="text-[11px] text-muted-foreground">优先匹配 size + quality</span>
        </div>
        <div class="grid grid-cols-[minmax(120px,1.1fr)_repeat(3,minmax(0,1fr))_32px] gap-2 text-xs text-muted-foreground">
          <span>分辨率</span>
          <span>low</span>
          <span>medium</span>
          <span>high</span>
          <span />
        </div>
        <div
          v-for="row in imageOutputPriceRows"
          :key="row.id"
          class="grid grid-cols-[minmax(120px,1.1fr)_repeat(3,minmax(0,1fr))_32px] gap-2 items-center"
        >
          <Input
            :model-value="row.size"
            class="h-8 font-mono text-xs"
            placeholder="1024x1024"
            @update:model-value="(v) => updateImageOutputSize(row.id, v)"
          />
          <Input
            v-for="quality in IMAGE_OUTPUT_QUALITIES"
            :key="`${row.id}-${quality}`"
            :model-value="getImageOutputPrice(row, quality)"
            type="number"
            step="0.001"
            min="0"
            class="h-8"
            placeholder="0"
            @update:model-value="(v) => updateImageOutputPrice(row.id, quality, v)"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-8 w-8 p-0"
            @click="removeImageOutputSizeRow(row.id)"
          >
            <X class="w-4 h-4 text-muted-foreground hover:text-destructive" />
          </Button>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          class="w-full"
          @click="addImageOutputSizeRow"
        >
          <Plus class="w-4 h-4 mr-2" />
          添加分辨率
        </Button>
      </div>

      <div class="space-y-2 border-t pt-3">
        <div class="flex items-center justify-between gap-2">
          <Label class="text-xs text-muted-foreground">像素区间</Label>
          <span class="text-[11px] text-muted-foreground">矩阵未命中时按宽×高落档</span>
        </div>
        <div class="grid grid-cols-[minmax(120px,1.1fr)_repeat(3,minmax(0,1fr))_32px] gap-2 text-xs text-muted-foreground">
          <span>上限像素</span>
          <span>low</span>
          <span>medium</span>
          <span>high</span>
          <span />
        </div>
        <div
          v-for="row in imageOutputPriceRangeRows"
          :key="row.id"
          class="grid grid-cols-[minmax(120px,1.1fr)_repeat(3,minmax(0,1fr))_32px] gap-2 items-center"
        >
          <Input
            :model-value="row.upToPixels"
            type="number"
            min="1"
            class="h-8 font-mono text-xs"
            placeholder="空=无上限"
            @update:model-value="(v) => updateImageOutputRangeLimit(row.id, v)"
          />
          <Input
            v-for="quality in IMAGE_OUTPUT_QUALITIES"
            :key="`${row.id}-${quality}`"
            :model-value="getImageOutputRangePrice(row, quality)"
            type="number"
            step="0.001"
            min="0"
            class="h-8"
            placeholder="0"
            @update:model-value="(v) => updateImageOutputRangePrice(row.id, quality, v)"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-8 w-8 p-0"
            @click="removeImageOutputRangeRow(row.id)"
          >
            <X class="w-4 h-4 text-muted-foreground hover:text-destructive" />
          </Button>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          class="w-full"
          @click="addImageOutputRangeRow"
        >
          <Plus class="w-4 h-4 mr-2" />
          添加像素区间
        </Button>
      </div>
    </div>

    <!-- 验证提示 -->
    <p
      v-if="validationError"
      class="text-xs text-destructive"
    >
      {{ validationError }}
    </p>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, reactive } from 'vue'
import { Plus, X } from 'lucide-vue-next'
import { Button, Input, Label } from '@/components/ui'
import { formatTokens } from '@/utils/format'
import type { TieredPricingConfig, PricingTier, ImageOutputPriceRange } from '@/api/endpoints/types'

type ImageOutputQuality = 'low' | 'medium' | 'high'
type ImageOutputPriceRow = {
  id: string
  size: string
  prices: Partial<Record<ImageOutputQuality, number>>
}
type ImageOutputPriceRangeRow = {
  id: string
  upToPixels: string
  prices: Partial<Record<ImageOutputQuality, number>>
}

const props = defineProps<{
  modelValue?: TieredPricingConfig | null
  showCache1h?: boolean
  showImagePricing?: boolean
}>()
const emit = defineEmits<{
  'update:modelValue': [value: TieredPricingConfig | null]
}>()
const DEFAULT_IMAGE_OUTPUT_SIZES = ['1024x1024', '1536x1024', '1024x1536']
const DEFAULT_IMAGE_OUTPUT_PIXEL_LIMITS = [1_048_576, 1_572_864, 2_097_152]
const IMAGE_OUTPUT_QUALITIES: ImageOutputQuality[] = ['low', 'medium', 'high']

// 本地状态
const localTiers = ref<PricingTier[]>([])
const imageOutputPriceRows = ref<ImageOutputPriceRow[]>([])
const imageOutputPriceRangeRows = ref<ImageOutputPriceRangeRow[]>([])
const imageOutputPriceDefault = ref<string>('')
const lastEmittedPricingJson = ref<string>('')
let imageOutputPriceRowId = 0
let imageOutputPriceRangeRowId = 0

// 跟踪每个阶梯的缓存价格是否被手动设置
const cacheManuallySet = reactive<Record<number, { creation: boolean; read: boolean; cache1h: boolean }>>({})

// 预设的阶梯上限选项
const THRESHOLD_OPTIONS = [
  { value: 64000, label: '64K' },
  { value: 128000, label: '128K' },
  { value: 200000, label: '200K' },
  { value: 500000, label: '500K' },
  { value: 1000000, label: '1M' },
  { value: -1, label: '自定义...' },  // 特殊值表示自定义输入
]

// 跟踪哪些阶梯正在使用自定义输入
const customInputMode = reactive<Record<number, boolean>>({})
const customInputValue = reactive<Record<number, string>>({})

// 初始化
watch(
  () => props.modelValue,
  (newValue) => {
    if (lastEmittedPricingJson.value && JSON.stringify(newValue ?? null) === lastEmittedPricingJson.value) {
      return
    }
    if (newValue?.tiers) {
      localTiers.value = newValue.tiers.map(t => ({ ...t }))
      imageOutputPriceRows.value = createImageOutputPriceRows(newValue.image_output_prices)
      imageOutputPriceRangeRows.value = createImageOutputPriceRangeRows(newValue.image_output_price_ranges)
      imageOutputPriceDefault.value = newValue.image_output_price_default != null
        ? String(newValue.image_output_price_default)
        : ''
      // 如果已有缓存价格，标记为手动设置
      newValue.tiers.forEach((t, i) => {
        const has1hCache = t.cache_ttl_pricing?.some(c => c.ttl_minutes === 60) ?? false
        cacheManuallySet[i] = {
          creation: t.cache_creation_price_per_1m != null,
          read: t.cache_read_price_per_1m != null,
          cache1h: has1hCache,
        }
      })
    } else {
      localTiers.value = [{
        up_to: null,
        input_price_per_1m: 0,
        output_price_per_1m: 0,
      }]
      imageOutputPriceRows.value = createImageOutputPriceRows(null)
      imageOutputPriceRangeRows.value = createImageOutputPriceRangeRows(null)
      imageOutputPriceDefault.value = ''
      cacheManuallySet[0] = { creation: false, read: false, cache1h: false }
    }
  },
  { immediate: true }
)

// 验证错误
const validationError = computed(() => {
  if (localTiers.value.length === 0) {
    return '至少需要一个价格阶梯'
  }

  if (localTiers.value[localTiers.value.length - 1].up_to !== null) {
    return '最后一个阶梯必须是无上限的'
  }

  let prevUpTo = 0
  for (let i = 0; i < localTiers.value.length - 1; i++) {
    const tier = localTiers.value[i]
    if (tier.up_to === null || tier.up_to <= prevUpTo) {
      return `阶梯 ${i + 1} 的上限必须大于前一个阶梯`
    }
    prevUpTo = tier.up_to
  }

  return null
})

// 获取阶梯起始标签
function getTierStartLabel(index: number): string {
  if (index === 0) return '0'
  const prevTier = localTiers.value[index - 1]
  if (prevTier.up_to === null) return '0'
  return formatTokens(prevTier.up_to)
}

// 获取可用的阈值选项
function getAvailableThresholds(index: number) {
  const usedThresholds = new Set<number>()
  localTiers.value.forEach((t, i) => {
    if (i !== index && t.up_to !== null) {
      usedThresholds.add(t.up_to)
    }
  })

  const minValue = index > 0 ? (localTiers.value[index - 1].up_to || 0) : 0
  const currentValue = localTiers.value[index].up_to

  // 过滤可用的预设选项
  const options = THRESHOLD_OPTIONS.filter(opt =>
    (opt.value === -1) ||  // "自定义..."始终显示
    (!usedThresholds.has(opt.value) && opt.value > minValue)
  )

  // 如果当前值是自定义的（不在预设中），添加到选项列表
  if (currentValue !== null && !THRESHOLD_OPTIONS.some(opt => opt.value === currentValue)) {
    options.unshift({ value: currentValue, label: formatTokens(currentValue) })
  }

  return options
}

// 缓存价格自动计算
function getAutoCacheCreation(index: number): number {
  const inputPrice = localTiers.value[index]?.input_price_per_1m || 0
  return parseFloat((inputPrice * 1.25).toFixed(4))
}

function getAutoCacheRead(index: number): number {
  const inputPrice = localTiers.value[index]?.input_price_per_1m || 0
  return parseFloat((inputPrice * 0.1).toFixed(4))
}

function getAutoCache1h(index: number): number {
  const inputPrice = localTiers.value[index]?.input_price_per_1m || 0
  return parseFloat((inputPrice * 2).toFixed(4))
}

function getCacheCreationPlaceholder(index: number): string {
  const auto = getAutoCacheCreation(index)
  return auto > 0 ? String(auto) : '自动'
}

function getCacheReadPlaceholder(index: number): string {
  const auto = getAutoCacheRead(index)
  return auto > 0 ? String(auto) : '自动'
}

function getCache1hPlaceholder(index: number): string {
  const auto = getAutoCache1h(index)
  return auto > 0 ? String(auto) : '自动'
}

function getCacheCreationDisplay(index: number): string | number {
  const tier = localTiers.value[index]
  if (cacheManuallySet[index]?.creation && tier?.cache_creation_price_per_1m != null) {
    // 修复浮点数精度问题
    return parseFloat(tier.cache_creation_price_per_1m.toFixed(4))
  }
  return ''
}

function getCacheReadDisplay(index: number): string | number {
  const tier = localTiers.value[index]
  if (cacheManuallySet[index]?.read && tier?.cache_read_price_per_1m != null) {
    // 修复浮点数精度问题
    return parseFloat(tier.cache_read_price_per_1m.toFixed(4))
  }
  return ''
}

function getCache1hDisplay(index: number): string | number {
  const tier = localTiers.value[index]
  // 只有手动设置过才显示值
  if (cacheManuallySet[index]?.cache1h) {
    const ttl1h = tier?.cache_ttl_pricing?.find(t => t.ttl_minutes === 60)
    if (ttl1h) {
      // 修复浮点数精度问题
      return parseFloat(ttl1h.cache_creation_price_per_1m.toFixed(4))
    }
  }
  return ''
}

// 同步到父组件（只同步用户实际输入的值，不自动填充）
function syncToParent() {
  if (validationError.value) return

  const tiers: PricingTier[] = localTiers.value.map((t, i) => {
    const tier: PricingTier = {
      up_to: t.up_to,
      input_price_per_1m: t.input_price_per_1m,
      output_price_per_1m: t.output_price_per_1m,
    }

    // 缓存创建价格：只有手动设置才同步
    if (cacheManuallySet[i]?.creation && t.cache_creation_price_per_1m != null) {
      tier.cache_creation_price_per_1m = t.cache_creation_price_per_1m
    }

    // 缓存读取价格：只有手动设置才同步
    if (cacheManuallySet[i]?.read && t.cache_read_price_per_1m != null) {
      tier.cache_read_price_per_1m = t.cache_read_price_per_1m
    }

    // 缓存 TTL 价格（1h 缓存）- 只有启用 1h 缓存能力且手动设置时才处理
    if (props.showCache1h && cacheManuallySet[i]?.cache1h && t.cache_ttl_pricing?.length) {
      tier.cache_ttl_pricing = t.cache_ttl_pricing
    }

    return tier
  })

  const value = buildPricingConfig(tiers)
  lastEmittedPricingJson.value = JSON.stringify(value ?? null)
  emit('update:modelValue', value)
}

// 获取最终提交的数据（包含自动计算的缓存价格）
function getFinalTiers(): PricingTier[] {
  return localTiers.value.map((t, i) => {
    const tier: PricingTier = {
      up_to: t.up_to,
      input_price_per_1m: t.input_price_per_1m,
      output_price_per_1m: t.output_price_per_1m,
    }

    // 缓存创建价格：手动设置则用设置值，否则自动计算
    if (cacheManuallySet[i]?.creation && t.cache_creation_price_per_1m != null) {
      tier.cache_creation_price_per_1m = t.cache_creation_price_per_1m
    } else {
      tier.cache_creation_price_per_1m = getAutoCacheCreation(i)
    }

    // 缓存读取价格：手动设置则用设置值，否则自动计算
    if (cacheManuallySet[i]?.read && t.cache_read_price_per_1m != null) {
      tier.cache_read_price_per_1m = t.cache_read_price_per_1m
    } else {
      tier.cache_read_price_per_1m = getAutoCacheRead(i)
    }

    // 缓存 TTL 价格（1h 缓存）- 只有启用 1h 缓存能力时才处理
    if (props.showCache1h) {
      if (cacheManuallySet[i]?.cache1h && t.cache_ttl_pricing?.length) {
        tier.cache_ttl_pricing = t.cache_ttl_pricing
      } else {
        tier.cache_ttl_pricing = [{ ttl_minutes: 60, cache_creation_price_per_1m: getAutoCache1h(i) }]
      }
    }

    return tier
  })
}

function getFinalPricing(): TieredPricingConfig {
  return buildPricingConfig(getFinalTiers())
}

// 暴露给父组件调用
defineExpose({
  getFinalTiers,
  getFinalPricing,
})

function buildPricingConfig(tiers: PricingTier[]): TieredPricingConfig {
  const config: TieredPricingConfig = { tiers }
  if (!props.showImagePricing) {
    return config
  }
  const matrix = normalizedImageOutputPrices()
  if (Object.keys(matrix).length > 0) {
    config.image_output_prices = matrix
  }
  const ranges = normalizedImageOutputPriceRanges()
  if (ranges.length > 0) {
    config.image_output_price_ranges = ranges
  }
  const defaultPrice = parseOptionalFloat(imageOutputPriceDefault.value)
  if (defaultPrice != null) {
    config.image_output_price_default = defaultPrice
  }
  return config
}

function createImageOutputPriceRows(value: TieredPricingConfig['image_output_prices']): ImageOutputPriceRow[] {
  const rows: ImageOutputPriceRow[] = []
  if (!value || typeof value !== 'object') {
    return DEFAULT_IMAGE_OUTPUT_SIZES.map(size => createImageOutputPriceRow(size))
  }
  for (const [size, prices] of Object.entries(value)) {
    if (!prices || typeof prices !== 'object') continue
    const rowPrices: Partial<Record<ImageOutputQuality, number>> = {}
    for (const quality of IMAGE_OUTPUT_QUALITIES) {
      const price = (prices as Record<string, unknown>)[quality]
      if (typeof price === 'number' && Number.isFinite(price)) {
        rowPrices[quality] = price
      }
    }
    rows.push(createImageOutputPriceRow(size, rowPrices))
  }
  if (rows.length > 0) return rows
  return DEFAULT_IMAGE_OUTPUT_SIZES.map(size => createImageOutputPriceRow(size))
}

function createImageOutputPriceRangeRows(value: TieredPricingConfig['image_output_price_ranges']): ImageOutputPriceRangeRow[] {
  const rows: ImageOutputPriceRangeRow[] = []
  if (!Array.isArray(value)) {
    return rows
  }
  for (const range of value) {
    if (!range || typeof range !== 'object') continue
    const rowPrices: Partial<Record<ImageOutputQuality, number>> = {}
    const rawPrices = 'prices' in range && range.prices && typeof range.prices === 'object'
      ? range.prices as Record<string, unknown>
      : range as Record<string, unknown>
    for (const quality of IMAGE_OUTPUT_QUALITIES) {
      const price = rawPrices[quality]
      if (typeof price === 'number' && Number.isFinite(price)) {
        rowPrices[quality] = price
      }
    }
    const upToPixels = 'up_to_pixels' in range && range.up_to_pixels != null
      ? String(range.up_to_pixels)
      : ''
    rows.push(createImageOutputPriceRangeRow(upToPixels, rowPrices))
  }
  return rows
}

function createImageOutputPriceRow(
  size = '',
  prices: Partial<Record<ImageOutputQuality, number>> = {},
): ImageOutputPriceRow {
  imageOutputPriceRowId += 1
  return {
    id: `image-output-size-${imageOutputPriceRowId}`,
    size,
    prices: { ...prices },
  }
}

function createImageOutputPriceRangeRow(
  upToPixels = '',
  prices: Partial<Record<ImageOutputQuality, number>> = {},
): ImageOutputPriceRangeRow {
  imageOutputPriceRangeRowId += 1
  return {
    id: `image-output-range-${imageOutputPriceRangeRowId}`,
    upToPixels,
    prices: { ...prices },
  }
}

function normalizedImageOutputPrices(): Record<string, Record<string, number>> {
  const out: Record<string, Record<string, number>> = {}
  for (const row of imageOutputPriceRows.value) {
    const size = normalizeImageOutputSize(row.size)
    if (!size) continue
    for (const quality of IMAGE_OUTPUT_QUALITIES) {
      const price = row.prices[quality]
      if (price != null && Number.isFinite(price)) {
        out[size] = { ...(out[size] || {}), [quality]: price }
      }
    }
  }
  return out
}

function normalizedImageOutputPriceRanges(): ImageOutputPriceRange[] {
  const ranges: ImageOutputPriceRange[] = []
  for (const row of imageOutputPriceRangeRows.value) {
    const prices: Partial<Record<ImageOutputQuality, number>> = {}
    for (const quality of IMAGE_OUTPUT_QUALITIES) {
      const price = row.prices[quality]
      if (price != null && Number.isFinite(price)) {
        prices[quality] = price
      }
    }
    if (Object.keys(prices).length === 0) continue
    ranges.push({
      up_to_pixels: parseOptionalInteger(row.upToPixels),
      prices,
    })
  }
  return ranges.sort((a, b) => {
    if (a.up_to_pixels == null && b.up_to_pixels == null) return 0
    if (a.up_to_pixels == null) return 1
    if (b.up_to_pixels == null) return -1
    return a.up_to_pixels - b.up_to_pixels
  })
}

function parseOptionalFloat(value: string | number): number | null {
  if (value === '' || value === null || value === undefined) return null
  const number = typeof value === 'string' ? parseFloat(value) : value
  return Number.isFinite(number) ? number : null
}

function parseOptionalInteger(value: string | number): number | null {
  if (value === '' || value === null || value === undefined) return null
  const number = typeof value === 'string' ? parseInt(value, 10) : value
  return Number.isFinite(number) && number > 0 ? Math.trunc(number) : null
}

function normalizeImageOutputSize(size: string): string {
  return String(size || '').trim().replace(/\s*[xX×]\s*/g, 'x')
}

function getImageOutputPrice(row: ImageOutputPriceRow, quality: ImageOutputQuality): string | number {
  return row.prices[quality] ?? ''
}

function getImageOutputRangePrice(row: ImageOutputPriceRangeRow, quality: ImageOutputQuality): string | number {
  return row.prices[quality] ?? ''
}

function updateImageOutputSize(rowId: string, value: string | number) {
  const row = imageOutputPriceRows.value.find(item => item.id === rowId)
  if (!row) return
  row.size = normalizeImageOutputSize(String(value ?? ''))
  imageOutputPriceRows.value = [...imageOutputPriceRows.value]
  syncToParent()
}

function updateImageOutputPrice(rowId: string, quality: ImageOutputQuality, value: string | number) {
  const row = imageOutputPriceRows.value.find(item => item.id === rowId)
  if (!row) return
  const price = parseOptionalFloat(value)
  if (price == null) {
    delete row.prices[quality]
  } else {
    row.prices[quality] = price
  }
  imageOutputPriceRows.value = [...imageOutputPriceRows.value]
  syncToParent()
}

function addImageOutputSizeRow() {
  const usedSizes = new Set(imageOutputPriceRows.value.map(row => normalizeImageOutputSize(row.size)).filter(Boolean))
  const suggestedSize = DEFAULT_IMAGE_OUTPUT_SIZES.find(size => !usedSizes.has(size)) || ''
  imageOutputPriceRows.value = [...imageOutputPriceRows.value, createImageOutputPriceRow(suggestedSize)]
  syncToParent()
}

function removeImageOutputSizeRow(rowId: string) {
  imageOutputPriceRows.value = imageOutputPriceRows.value.filter(row => row.id !== rowId)
  syncToParent()
}

function updateImageOutputRangeLimit(rowId: string, value: string | number) {
  const row = imageOutputPriceRangeRows.value.find(item => item.id === rowId)
  if (!row) return
  row.upToPixels = String(value ?? '')
  imageOutputPriceRangeRows.value = [...imageOutputPriceRangeRows.value]
  syncToParent()
}

function updateImageOutputRangePrice(rowId: string, quality: ImageOutputQuality, value: string | number) {
  const row = imageOutputPriceRangeRows.value.find(item => item.id === rowId)
  if (!row) return
  const price = parseOptionalFloat(value)
  if (price == null) {
    delete row.prices[quality]
  } else {
    row.prices[quality] = price
  }
  imageOutputPriceRangeRows.value = [...imageOutputPriceRangeRows.value]
  syncToParent()
}

function addImageOutputRangeRow() {
  const usedLimits = new Set(imageOutputPriceRangeRows.value.map(row => parseOptionalInteger(row.upToPixels)).filter((value): value is number => value !== null))
  const suggestedLimit = DEFAULT_IMAGE_OUTPUT_PIXEL_LIMITS.find(limit => !usedLimits.has(limit))
  imageOutputPriceRangeRows.value = [...imageOutputPriceRangeRows.value, createImageOutputPriceRangeRow(suggestedLimit ? String(suggestedLimit) : '')]
  syncToParent()
}

function removeImageOutputRangeRow(rowId: string) {
  imageOutputPriceRangeRows.value = imageOutputPriceRangeRows.value.filter(row => row.id !== rowId)
  syncToParent()
}

function updateImageOutputPriceDefault(value: string | number) {
  imageOutputPriceDefault.value = String(value ?? '')
  syncToParent()
}

function parseFloatInput(value: string | number): number {
  const num = typeof value === 'string' ? parseFloat(value) : value
  return isNaN(num) ? 0 : num
}

// 更新输入价格（会触发缓存价格自动更新）
function updateInputPrice(index: number, value: number) {
  localTiers.value[index].input_price_per_1m = value
  syncToParent()
}

function updateOutputPrice(index: number, value: number) {
  localTiers.value[index].output_price_per_1m = value
  syncToParent()
}

// 获取下拉框当前选中值
function getSelectValue(index: number): number {
  const upTo = localTiers.value[index].up_to
  if (upTo === null) return -1
  return upTo  // 直接返回当前值，让下拉框显示对应选项
}

// 处理下拉框选择变化
function handleThresholdChange(index: number, value: number) {
  if (value === -1) {
    // 选择了"自定义..."，进入自定义输入模式
    customInputMode[index] = true
    customInputValue[index] = ''
  } else {
    localTiers.value[index].up_to = value
    syncToParent()
  }
}

// 确认自定义输入
function confirmCustomInput(index: number) {
  const inputK = parseInt(customInputValue[index])
  if (inputK > 0) {
    localTiers.value[index].up_to = inputK * 1000
    syncToParent()
  }
  customInputMode[index] = false
}

function updateCacheCreation(index: number, value: string | number) {
  if (value === '' || value === null || value === undefined) {
    // 清空时恢复自动计算
    cacheManuallySet[index] = { ...cacheManuallySet[index], creation: false }
    localTiers.value[index].cache_creation_price_per_1m = undefined
  } else {
    const numValue = parseFloatInput(value)
    cacheManuallySet[index] = { ...cacheManuallySet[index], creation: true }
    localTiers.value[index].cache_creation_price_per_1m = numValue
  }
  syncToParent()
}

function updateCacheRead(index: number, value: string | number) {
  if (value === '' || value === null || value === undefined) {
    // 清空时恢复自动计算
    cacheManuallySet[index] = { ...cacheManuallySet[index], read: false }
    localTiers.value[index].cache_read_price_per_1m = undefined
  } else {
    const numValue = parseFloatInput(value)
    cacheManuallySet[index] = { ...cacheManuallySet[index], read: true }
    localTiers.value[index].cache_read_price_per_1m = numValue
  }
  syncToParent()
}

function updateCache1h(index: number, value: string | number) {
  const tier = localTiers.value[index]
  if (value === '' || value === null || value === undefined) {
    // 清空时恢复自动计算
    cacheManuallySet[index] = { ...cacheManuallySet[index], cache1h: false }
    tier.cache_ttl_pricing = undefined
  } else {
    const numValue = parseFloatInput(value)
    cacheManuallySet[index] = { ...cacheManuallySet[index], cache1h: true }
    tier.cache_ttl_pricing = [{ ttl_minutes: 60, cache_creation_price_per_1m: numValue }]
  }
  syncToParent()
}

// 阶梯操作
function addTier() {
  if (localTiers.value.length === 0) {
    localTiers.value = [{
      up_to: null,
      input_price_per_1m: 0,
      output_price_per_1m: 0,
    }]
    cacheManuallySet[0] = { creation: false, read: false, cache1h: false }
  } else {
    // 把当前最后一个阶梯（无上限）改为有上限
    const lastTier = localTiers.value[localTiers.value.length - 1]
    const secondLastTier = localTiers.value[localTiers.value.length - 2]
    const minValue = secondLastTier?.up_to || 0
    const availableThresholds = THRESHOLD_OPTIONS.filter(opt => opt.value > minValue)
    const newUpTo = availableThresholds[0]?.value || minValue + 200000

    // 给当前最后一个阶梯设置上限
    lastTier.up_to = newUpTo

    // 添加新的无上限阶梯
    const newIndex = localTiers.value.length
    const newTier: PricingTier = {
      up_to: null,
      input_price_per_1m: 0,
      output_price_per_1m: 0,
    }

    localTiers.value.push(newTier)
    cacheManuallySet[newIndex] = { creation: false, read: false, cache1h: false }
  }

  syncToParent()
}

function removeTier(index: number) {
  if (localTiers.value.length <= 1) return
  localTiers.value.splice(index, 1)
  delete cacheManuallySet[index]

  // 重新整理 cacheManuallySet 的索引
  const newManuallySet: Record<
    number,
    { creation: boolean; read: boolean; cache1h: boolean }
  > = {}
  localTiers.value.forEach((_, i) => {
    newManuallySet[i] =
      cacheManuallySet[i] || { creation: false, read: false, cache1h: false }
  })
  Object.keys(cacheManuallySet).forEach(k => delete cacheManuallySet[Number(k)])
  Object.assign(cacheManuallySet, newManuallySet)

  if (localTiers.value.length > 0) {
    localTiers.value[localTiers.value.length - 1].up_to = null
  }

  syncToParent()
}
</script>
