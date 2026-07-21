<template>
  <div
    class="flex min-w-0 max-w-full flex-col gap-0.5"
    :class="shouldStackBadges && stackFullWidth ? 'w-full items-start' : 'items-start'"
    :data-usage-model-layout="shouldStackBadges ? 'stacked' : 'inline'"
    :data-request-detail-model-layout="context === 'detail'
      ? (shouldStackBadges ? 'stacked' : 'inline')
      : undefined"
  >
    <div
      class="flex min-w-0 max-w-full items-center gap-1"
      :class="[modelRowClass, actualModel ? 'flex-wrap' : '']"
    >
      <span
        class="min-w-0 truncate"
        :class="modelClass"
        data-usage-model-source
      >{{ record.model }}</span>
      <template v-if="actualModel">
        <span
          class="order-last basis-full min-w-0 break-all whitespace-normal text-muted-foreground"
          data-usage-model-target
        ><span class="mr-1">-&gt;</span>{{ actualModel }}</span>
      </template>
      <template v-if="!shouldStackBadges">
        <Badge
          v-for="badge in modelBadges"
          :key="badge.key"
          :data-usage-model-badge="badge.key"
          :data-request-detail-model-badge="context === 'detail' ? badge.key : undefined"
          :variant="badge.variant"
          class="h-4 shrink-0 whitespace-nowrap rounded-full px-1.5 text-[10px] leading-4"
          :class="badge.className"
          :title="badge.title"
          :aria-label="badge.ariaLabel"
        >
          {{ badge.label }}
        </Badge>
      </template>
    </div>

    <div
      v-if="shouldStackBadges && modelBadges.length > 0"
      class="flex min-w-0 max-w-full flex-wrap items-center gap-1"
      data-usage-model-badges-row
      :data-request-detail-model-badges-row="context === 'detail' ? '' : undefined"
    >
      <Badge
        v-for="badge in modelBadges"
        :key="badge.key"
        :data-usage-model-badge="badge.key"
        :data-request-detail-model-badge="context === 'detail' ? badge.key : undefined"
        :variant="badge.variant"
        class="h-4 shrink-0 whitespace-nowrap rounded-full px-1.5 text-[10px] leading-4"
        :class="badge.className"
        :title="badge.title"
        :aria-label="badge.ariaLabel"
      >
        {{ badge.label }}
      </Badge>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

import { Badge } from '@/components/ui'
import { isCyberPolicyError } from '../utils/cyberError'
import { formatServiceTierFact } from '../utils/service-tier'

type ModelBadgeKey = 'compact' | 'reasoning' | 'fast' | 'cyber' | 'reasoning_tokens'

interface ModelBadgePresentation {
  key: ModelBadgeKey
  label: string
  variant: 'outline' | 'outline-transparent'
  className: string
  title: string
  ariaLabel: string
}

interface UsageModelDisplayRecord {
  model: string
  target_model?: string | null
  model_version?: string | null
  request_type?: string | null
  requested_reasoning_effort?: string | null
  reasoning_effort?: string | null
  service_tier?: string | null
  reasoning_tokens?: number
  error_message?: string | null
}

const props = withDefaults(defineProps<{
  record: UsageModelDisplayRecord
  modelClass?: string
  modelRowClass?: string
  context?: 'usage' | 'detail'
  cyber?: boolean | null
  stackFullWidth?: boolean
  showServiceTierBadge?: boolean
  showCyberBadge?: boolean
  showReasoningBadge?: boolean
}>(), {
  modelClass: '',
  modelRowClass: '',
  context: 'usage',
  cyber: null,
  stackFullWidth: false,
  showServiceTierBadge: true,
  showCyberBadge: true,
  showReasoningBadge: true,
})

const actualModel = computed(() => {
  const targetModel = normalizeText(props.record.target_model)
  if (targetModel && targetModel !== props.record.model) return targetModel

  const modelVersion = normalizeText(props.record.model_version)
  if (modelVersion && modelVersion !== props.record.model) return modelVersion
  return null
})

const reasoningLabel = computed(() => {
  const requested = normalizeText(props.record.requested_reasoning_effort)
  const actual = normalizeText(props.record.reasoning_effort)
  if (requested && actual && requested.toLowerCase() !== actual.toLowerCase()) {
    return `${requested} -> ${actual}`
  }
  return actual ?? requested
})

const modelBadges = computed<ModelBadgePresentation[]>(() => {
  const badges: ModelBadgePresentation[] = []
  if (normalizeText(props.record.request_type)?.toLowerCase() === 'compact') {
    badges.push({
      key: 'compact',
      label: '会话压缩',
      variant: 'outline',
      className: 'border-sky-500/30 bg-sky-500/5 text-sky-700 dark:text-sky-300',
      title: '会话压缩',
      ariaLabel: '会话压缩',
    })
  }
  if (props.showReasoningBadge && reasoningLabel.value) {
    badges.push({
      key: 'reasoning',
      label: reasoningLabel.value,
      variant: 'outline',
      className: 'border-primary/30 bg-primary/5 text-primary',
      title: `Reasoning: ${reasoningLabel.value}`,
      ariaLabel: `Reasoning: ${reasoningLabel.value}`,
    })
  }

  if (props.showServiceTierBadge && formatServiceTierFact(props.record.service_tier) === 'Fast') {
    badges.push({
      key: 'fast',
      label: 'Fast',
      variant: 'outline-transparent',
      className: 'text-blue-500 dark:text-blue-300',
      title: '上游请求档位：Fast\n计费档位：Fast',
      ariaLabel: '上游请求档位：Fast，计费档位：Fast',
    })
  }

  if (props.showCyberBadge && (props.cyber ?? isCyberPolicyError(props.record.error_message))) {
    badges.push({
      key: 'cyber',
      label: 'Cyber',
      variant: 'outline',
      className: 'border-primary/30 bg-primary/5 text-rose-500 dark:text-rose-300',
      title: '上游 Cyber Policy 拒绝',
      ariaLabel: '上游 Cyber Policy 拒绝',
    })
  }
  if (typeof props.record.reasoning_tokens === 'number' && props.record.reasoning_tokens > 0) {
    badges.push({
      key: 'reasoning_tokens',
      label: `推理 ${formatCompactTokens(props.record.reasoning_tokens)}`,
      variant: 'outline-transparent',
      className: 'text-muted-foreground',
      title: `推理 Token 数：${props.record.reasoning_tokens}`,
      ariaLabel: `推理 Token 数：${props.record.reasoning_tokens}`,
    })
  }
  return badges
})

const shouldStackBadges = computed(() => (
  actualModel.value === null && modelBadges.value.length >= 3
))

function normalizeText(value: string | null | undefined): string | null {
  const normalized = value?.trim()
  return normalized || null
}

function formatCompactTokens(value: number): string {
  if (value < 1000) return `${value} Tokens`
  return `${(value / 1000).toFixed(value >= 10000 ? 0 : 1)}K Tokens`
}
</script>
