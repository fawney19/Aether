<template>
  <div class="min-w-0">
    <div class="flex min-w-0 items-center gap-1">
      <Select
        :model-value="source?.provider_id"
        :disabled="syncing"
        @update:open="emit('open', $event)"
        @update:model-value="emit('select', $event)"
      >
        <SelectTrigger
          class="h-8 min-w-0 flex-1 px-2 text-xs"
          :title="source
            ? t('models.pricingSource.currentTitle', { provider: source.provider_name })
            : t('models.pricingSource.chooseTitle')"
          :aria-label="source
            ? t('models.pricingSource.currentTitle', { provider: source.provider_name })
            : t('models.pricingSource.chooseTitle')"
          :data-testid="`model-pricing-source-${modelId}`"
        >
          <Loader2
            v-if="syncing"
            class="mr-1 h-3 w-3 shrink-0 animate-spin"
          />
          <SelectValue :placeholder="loading ? t('models.pricingSource.loading') : t('models.pricingSource.choose')">
            <span class="truncate">{{ source?.provider_name || t('models.pricingSource.choose') }}</span>
          </SelectValue>
        </SelectTrigger>
        <SelectContent
          class="w-72"
          align="end"
        >
          <SelectItem
            v-if="loading && candidates.length === 0"
            :value="`__loading__:${modelId}`"
            disabled
          >
            {{ t('models.pricingSource.loadingOptions') }}
          </SelectItem>
          <SelectItem
            v-for="candidate in candidates"
            :key="candidate.providerId"
            :value="candidate.providerId"
            :disabled="!isCandidateSyncable(candidate)"
            :text-value="`${candidate.providerName} ${candidate.providerId}`"
          >
            <div class="flex min-w-0 items-center justify-between gap-3">
              <div class="min-w-0">
                <div class="truncate text-xs font-medium">
                  {{ candidate.providerName }}
                </div>
                <div class="truncate font-mono text-[10px] text-muted-foreground">
                  {{ candidate.providerId }}
                </div>
              </div>
              <div class="shrink-0 text-right text-[10px] text-muted-foreground">
                <template v-if="isCandidateSyncable(candidate)">
                  <div>{{ t('models.pricingSource.inputPrice', { price: formatPrice(candidate.inputPrice) }) }}</div>
                  <div>{{ t('models.pricingSource.outputPrice', { price: formatPrice(candidate.outputPrice) }) }}</div>
                </template>
                <span v-else>{{ getUnavailableReason(candidate) }}</span>
              </div>
            </div>
          </SelectItem>
          <SelectItem
            v-if="!loading && candidates.length === 0"
            :value="`__empty__:${modelId}`"
            disabled
          >
            {{ t('models.pricingSource.catalogEmpty') }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Button
        v-if="source"
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0"
        :disabled="syncing"
        :title="t('models.pricingSource.resyncTitle')"
        :aria-label="t('models.pricingSource.resyncTitle')"
        :data-testid="`model-pricing-source-resync-${modelId}`"
        @click="emit('resync')"
      >
        <RefreshCw
          class="h-3.5 w-3.5"
          :class="syncing ? 'animate-spin' : ''"
        />
      </Button>
    </div>
    <p
      v-if="localOnly"
      class="mt-1 text-[10px] text-amber-600 dark:text-amber-400"
    >
      {{ t('models.pricingSource.pendingDatabase') }}
    </p>
  </div>
</template>

<script setup lang="ts">
import { Loader2, RefreshCw } from 'lucide-vue-next'

import type { ModelsDevModelItem } from '@/api/models-dev'
import { useI18n } from '@/i18n'
import {
  Button,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import type { ModelsDevPricingSource } from '../composables/useModelsDevPricingSources'

defineProps<{
  modelId: string
  source: ModelsDevPricingSource | null
  candidates: ModelsDevModelItem[]
  loading: boolean
  syncing: boolean
  localOnly?: boolean
}>()

const emit = defineEmits<{
  open: [value: boolean]
  select: [providerId: string]
  resync: []
}>()

const { t } = useI18n()

function isCandidateSyncable(candidate: ModelsDevModelItem): boolean {
  return !candidate.pricingUnsupportedFields?.length && !!candidate.tieredPricing?.tiers?.length
}

function getUnavailableReason(candidate: ModelsDevModelItem): string {
  if (candidate.pricingUnsupportedFields?.length) return t('models.pricingSource.incompatible')
  return t('models.pricingSource.noTokenPrice')
}

function formatPrice(value?: number): string {
  if (value === undefined) return '-'
  if (value === 0) return '0'
  const precision = value < 0.01 ? 4 : value < 1 ? 3 : 2
  return value.toFixed(precision).replace(/\.?0+$/, '')
}
</script>
