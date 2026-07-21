<template>
  <div
    v-if="processingTierEntries.length > 0 && activeEntry"
    class="space-y-3 border-t border-border/60 pt-4"
    data-testid="processing-tier-pricing-summary"
  >
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div>
        <h5 class="text-sm font-medium text-foreground">
          处理层级定价
        </h5>
        <p class="text-xs text-muted-foreground">
          {{ activeEntry.label }}
        </p>
      </div>
      <div
        class="flex max-w-full flex-wrap gap-1"
        role="group"
        aria-label="处理层级定价"
      >
        <Button
          v-for="entry in processingTierEntries"
          :key="entry.key"
          type="button"
          size="sm"
          :variant="activeTierKey === entry.key ? 'secondary' : 'ghost'"
          class="h-8 max-w-full px-2.5"
          :aria-pressed="activeTierKey === entry.key"
          :data-processing-tier="entry.key"
          @click="activeTierKey = entry.key"
        >
          <span class="truncate">{{ entry.label }}</span>
        </Button>
      </div>
    </div>

    <div
      v-if="activePriceMultiplier !== null"
      class="flex items-center justify-between rounded-md border bg-muted/20 px-3 py-2 text-xs"
      data-testid="processing-tier-price-multiplier"
    >
      <span class="text-muted-foreground">相对 Standard</span>
      <span class="font-mono font-medium text-foreground">{{ formatMultiplier(activePriceMultiplier) }}×</span>
    </div>

    <div
      v-if="activeTokenTiers.length > 0"
      class="overflow-x-auto rounded-md border"
    >
      <Table class="min-w-[680px]">
        <TableHeader>
          <TableRow class="bg-muted/30">
            <TableHead class="h-9 text-xs">
              Token 区间
            </TableHead>
            <TableHead class="h-9 text-right text-xs">
              输入 ($/M)
            </TableHead>
            <TableHead class="h-9 text-right text-xs">
              输出 ($/M)
            </TableHead>
            <TableHead class="h-9 text-right text-xs">
              缓存创建
            </TableHead>
            <TableHead class="h-9 text-right text-xs">
              缓存读取
            </TableHead>
            <TableHead class="h-9 text-right text-xs">
              1h 创建
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="(tier, index) in activeTokenTiers"
            :key="index"
            class="text-xs"
            data-testid="processing-token-tier-row"
          >
            <TableCell class="py-2 whitespace-nowrap">
              {{ formatTokenRange(activeTokenTiers, index) }}
            </TableCell>
            <TableCell class="py-2 text-right font-mono">
              {{ formatPrice(tier.input_price_per_1m) }}
            </TableCell>
            <TableCell class="py-2 text-right font-mono">
              {{ formatPrice(tier.output_price_per_1m) }}
            </TableCell>
            <TableCell class="py-2 text-right font-mono text-muted-foreground">
              {{ formatPrice(tier.cache_creation_price_per_1m) }}
            </TableCell>
            <TableCell class="py-2 text-right font-mono text-muted-foreground">
              {{ formatPrice(tier.cache_read_price_per_1m) }}
            </TableCell>
            <TableCell class="py-2 text-right font-mono text-muted-foreground">
              {{ formatPrice(cacheCreationPriceForTtl(tier, 60)) }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <div
      v-if="hasActiveImagePricing"
      class="space-y-2"
      data-testid="processing-image-pricing"
    >
      <div class="flex flex-wrap items-center justify-between gap-2 text-xs">
        <span class="font-medium text-foreground">图片输出</span>
        <span
          v-if="activeImageDefaultPrice !== null"
          class="font-mono text-muted-foreground"
        >默认 {{ formatPrice(activeImageDefaultPrice) }}/张</span>
      </div>

      <div
        v-if="activeImageRows.length > 0"
        class="overflow-x-auto rounded-md border"
      >
        <Table :class="imageTableMinWidthClass">
          <TableHeader>
            <TableRow class="bg-muted/30">
              <TableHead class="h-9 text-xs">
                分辨率
              </TableHead>
              <TableHead
                v-for="quality in activeImageQualities"
                :key="quality"
                class="h-9 text-right text-xs"
              >
                {{ quality }}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="row in activeImageRows"
              :key="row.size"
              class="text-xs"
            >
              <TableCell class="py-2 font-mono whitespace-nowrap">
                {{ formatImageSize(row.size) }}
              </TableCell>
              <TableCell
                v-for="quality in activeImageQualities"
                :key="`${row.size}-${quality}`"
                class="py-2 text-right font-mono"
              >
                {{ formatPrice(row.prices[quality]) }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <div
        v-if="activeImageRangeRows.length > 0"
        class="overflow-x-auto rounded-md border"
      >
        <Table :class="imageTableMinWidthClass">
          <TableHeader>
            <TableRow class="bg-muted/30">
              <TableHead class="h-9 text-xs">
                像素区间
              </TableHead>
              <TableHead
                v-for="quality in activeImageQualities"
                :key="quality"
                class="h-9 text-right text-xs"
              >
                {{ quality }}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="(row, index) in activeImageRangeRows"
              :key="`${row.upToPixels ?? 'unbounded'}-${index}`"
              class="text-xs"
            >
              <TableCell class="py-2 whitespace-nowrap">
                {{ row.label || formatPixelRange(activeImageRangeRows, index) }}
              </TableCell>
              <TableCell
                v-for="quality in activeImageQualities"
                :key="`${index}-${quality}`"
                class="py-2 text-right font-mono"
              >
                {{ formatPrice(row.prices[quality]) }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Button from '@/components/ui/button.vue'
import Table from '@/components/ui/table.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableCell from '@/components/ui/table-cell.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableRow from '@/components/ui/table-row.vue'
import { formatTokens } from '@/utils/format'
import type {
  PricingTier,
  ProcessingTierPricingConfig,
  TieredPricingConfig,
} from '@/api/endpoints/types'
import { comparePricingUpperBounds } from '@/features/models/utils/tiered-pricing'

type ProcessingTierEntry = {
  key: string
  label: string
  config: ProcessingTierPricingConfig
}

type ImagePriceRow = {
  size: string
  prices: Record<string, number>
}

type ImageRangeRow = {
  upToPixels: number | null
  label: string | null
  prices: Record<string, number>
}

const props = defineProps<{
  pricing: TieredPricingConfig | null | undefined
}>()

const KNOWN_PROCESSING_TIERS = [
  { key: 'priority', label: 'Fast（OpenAI）' },
  { key: 'fast', label: 'Fast（Claude）' },
  { key: 'flex', label: 'Flex' },
  { key: 'batch', label: 'Batch' },
] as const
const KNOWN_IMAGE_QUALITIES = ['low', 'medium', 'high'] as const
const activeTierKey = ref('')

const processingTierEntries = computed<ProcessingTierEntry[]>(() => {
  const processingTiers = props.pricing?.processing_tiers
  if (!isRecord(processingTiers)) return []
  const labels = new Map(KNOWN_PROCESSING_TIERS.map(entry => [entry.key, entry.label]))
  const order = new Map(KNOWN_PROCESSING_TIERS.map((entry, index) => [entry.key, index]))
  return Object.entries(processingTiers)
    .filter((entry): entry is [string, ProcessingTierPricingConfig] => (
      isRecord(entry[1]) && processingPricingHasFacts(entry[1])
    ))
    .sort(([left], [right]) => {
      const leftOrder = order.get(left) ?? KNOWN_PROCESSING_TIERS.length
      const rightOrder = order.get(right) ?? KNOWN_PROCESSING_TIERS.length
      return leftOrder - rightOrder || left.localeCompare(right)
    })
    .map(([key, config]) => ({ key, label: labels.get(key) ?? key, config }))
})

watch(processingTierEntries, (entries) => {
  if (!entries.some(entry => entry.key === activeTierKey.value)) {
    activeTierKey.value = entries[0]?.key ?? ''
  }
}, { immediate: true })

const activeEntry = computed(() =>
  processingTierEntries.value.find(entry => entry.key === activeTierKey.value) ?? null,
)
const activeTokenTiers = computed<PricingTier[]>(() =>
  Array.isArray(activeEntry.value?.config.tiers)
    ? activeEntry.value.config.tiers.filter(isRecord) as PricingTier[]
    : [],
)
const activePriceMultiplier = computed(() => {
  const config = activeEntry.value?.config
  if (!config || processingPricingHasExplicitFacts(config)) return null
  const multiplier = config.price_multiplier
  return typeof multiplier === 'number' && Number.isFinite(multiplier) && multiplier >= 0
    ? multiplier
    : null
})
const activeImageDefaultPrice = computed(() =>
  toFiniteNumber(activeEntry.value?.config.image_output_price_default),
)
const activeImageRows = computed<ImagePriceRow[]>(() => {
  const prices = activeEntry.value?.config.image_output_prices
  if (!isRecord(prices)) return []
  return Object.entries(prices)
    .filter((entry): entry is [string, Record<string, unknown>] => isRecord(entry[1]))
    .map(([size, values]) => ({ size, prices: finitePriceRecord(values) }))
    .filter(row => Object.keys(row.prices).length > 0)
    .sort((left, right) => imageSizeArea(left.size) - imageSizeArea(right.size)
      || left.size.localeCompare(right.size))
})
const activeImageRangeRows = computed<ImageRangeRow[]>(() => {
  const ranges = activeEntry.value?.config.image_output_price_ranges
  if (!Array.isArray(ranges)) return []
  return ranges
    .filter(isRecord)
    .map(range => ({
      upToPixels: range.up_to_pixels === null ? null : toFiniteNumber(range.up_to_pixels),
      label: typeof range.label === 'string' && range.label.trim() ? range.label.trim() : null,
      prices: isRecord(range.prices) ? finitePriceRecord(range.prices) : {},
    }))
    .filter(row => Object.keys(row.prices).length > 0)
    .sort((left, right) => comparePricingUpperBounds(left.upToPixels, right.upToPixels))
})
const activeImageQualities = computed(() => {
  const present = new Set<string>()
  for (const row of [...activeImageRows.value, ...activeImageRangeRows.value]) {
    Object.keys(row.prices).forEach(quality => present.add(quality))
  }
  const known = KNOWN_IMAGE_QUALITIES.filter(quality => present.has(quality))
  const custom = [...present]
    .filter(quality => !KNOWN_IMAGE_QUALITIES.includes(quality as typeof KNOWN_IMAGE_QUALITIES[number]))
    .sort((left, right) => left.localeCompare(right))
  return [...known, ...custom]
})
const hasActiveImagePricing = computed(() =>
  activeImageDefaultPrice.value !== null
  || activeImageRows.value.length > 0
  || activeImageRangeRows.value.length > 0,
)
const imageTableMinWidthClass = computed(() =>
  activeImageQualities.value.length > 3 ? 'min-w-[620px]' : 'min-w-[460px]',
)

function processingPricingHasFacts(config: ProcessingTierPricingConfig): boolean {
  if (
    typeof config.price_multiplier === 'number'
    && Number.isFinite(config.price_multiplier)
    && config.price_multiplier >= 0
  ) return true
  return processingPricingHasExplicitFacts(config)
}

function processingPricingHasExplicitFacts(config: ProcessingTierPricingConfig): boolean {
  if (Array.isArray(config.tiers) && config.tiers.length > 0) return true
  if (toFiniteNumber(config.image_output_price_default) !== null) return true
  if (isRecord(config.image_output_prices)) {
    for (const prices of Object.values(config.image_output_prices)) {
      if (isRecord(prices) && Object.keys(finitePriceRecord(prices)).length > 0) return true
    }
  }
  if (Array.isArray(config.image_output_price_ranges)
    && config.image_output_price_ranges.some(range => (
      isRecord(range)
      && isRecord(range.prices)
      && Object.keys(finitePriceRecord(range.prices)).length > 0
    ))) return true
  return [
    'image_output_price_per_image',
    'image_output_price_matrix',
    'image_prices',
  ].some(key => valueHasEntries(config[key]))
}

function valueHasEntries(value: unknown): boolean {
  return (Array.isArray(value) && value.length > 0)
    || (isRecord(value) && Object.keys(value).length > 0)
}

function formatMultiplier(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(6)))
}

function formatTokenRange(tiers: PricingTier[], index: number): string {
  const lower = index === 0 ? 0 : toFiniteNumber(tiers[index - 1]?.up_to)
  const upper = tiers[index]?.up_to === null ? null : toFiniteNumber(tiers[index]?.up_to)
  if (upper === null) return lower && lower > 0 ? `> ${formatTokens(lower)}` : '所有'
  return `${formatTokens(lower ?? 0)} - ${formatTokens(upper)}`
}

function cacheCreationPriceForTtl(tier: PricingTier, ttlMinutes: number): number | null {
  const entry = Array.isArray(tier.cache_ttl_pricing)
    ? tier.cache_ttl_pricing.find(item => item.ttl_minutes === ttlMinutes)
    : undefined
  return toFiniteNumber(entry?.cache_creation_price_per_1m)
}

function formatPixelRange(rows: ImageRangeRow[], index: number): string {
  const lower = index === 0 ? 0 : rows[index - 1]?.upToPixels
  const upper = rows[index]?.upToPixels
  if (upper === null) return lower && lower > 0 ? `> ${formatTokens(lower)} px` : '所有像素'
  return `${formatTokens(lower ?? 0)} - ${formatTokens(upper)} px`
}

function formatPrice(value: unknown): string {
  const price = toFiniteNumber(value)
  if (price === null) return '-'
  return `$${price.toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
    useGrouping: false,
  })}`
}

function formatImageSize(value: string): string {
  return value.replace(/\s*[xX×]\s*/g, ' x ')
}

function imageSizeArea(value: string): number {
  const match = value.match(/^(\d+)\s*[xX×]\s*(\d+)$/)
  return match ? Number(match[1]) * Number(match[2]) : Number.MAX_SAFE_INTEGER
}

function finitePriceRecord(value: Record<string, unknown>): Record<string, number> {
  const entries: Array<[string, number]> = []
  for (const [key, rawPrice] of Object.entries(value)) {
    const price = toFiniteNumber(rawPrice)
    if (price !== null) entries.push([key, price])
  }
  return Object.fromEntries(entries)
}

function toFiniteNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
</script>
