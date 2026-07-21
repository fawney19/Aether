<template>
  <div
    v-if="variant === 'mobile'"
    class="rounded-xl border border-border/50 bg-muted/30 px-3 py-2 text-xs"
  >
    <div class="text-muted-foreground mb-1">
      {{ legacyT('配额') }}
    </div>
    <div
      v-if="items.length"
      class="space-y-2"
    >
      <QuotaProgressRows
        :items="items"
        mobile
      />
      <div
        v-if="accountQuotaText"
        class="text-[10px] leading-none text-muted-foreground tabular-nums"
      >
        {{ accountQuotaText }}
      </div>
    </div>
    <div
      v-else-if="accountQuotaText || fallbackText"
      :class="textClass"
    >
      {{ accountQuotaText || fallbackText }}
    </div>
    <div
      v-else
      class="text-muted-foreground"
    >
      -
    </div>
  </div>

  <template v-else>
    <div
      v-if="items.length"
      class="max-w-[208px] space-y-2"
    >
      <QuotaProgressRows :items="items" />
      <div
        v-if="accountQuotaText"
        class="text-[10px] leading-none text-muted-foreground tabular-nums"
      >
        {{ accountQuotaText }}
      </div>
    </div>
    <span
      v-else-if="accountQuotaText || fallbackText"
      :class="textClass"
    >
      {{ accountQuotaText || fallbackText }}
    </span>
    <span
      v-else
      class="text-xs text-muted-foreground"
    >-</span>
  </template>
</template>

<script setup lang="ts">
import { defineComponent, h, type PropType } from 'vue'
import { useI18n } from '@/i18n'

export interface PoolQuotaProgressDisplayItem {
  label: string
  remainingPercent: number
  resetText: string
  meterText: string
  barClass: string
  meterClass: string
}

withDefaults(defineProps<{
  items: PoolQuotaProgressDisplayItem[]
  accountQuotaText?: string | null
  fallbackText?: string | null
  textClass?: string
  variant?: 'desktop' | 'mobile'
}>(), {
  accountQuotaText: null,
  fallbackText: null,
  textClass: '',
  variant: 'desktop',
})

const { legacyT } = useI18n()

const QuotaProgressRows = defineComponent({
  name: 'QuotaProgressRows',
  props: {
    items: {
      type: Array as PropType<PoolQuotaProgressDisplayItem[]>,
      required: true,
    },
    mobile: {
      type: Boolean,
      default: false,
    },
  },
  setup(props) {
    return () => props.items.map((item, idx) => h('div', {
      key: `${item.label}-${idx}`,
      class: props.mobile
        ? 'flex flex-col gap-1 min-w-0'
        : 'flex flex-col gap-1 min-w-[140px] max-w-[208px]',
    }, [
      h('div', { class: 'flex items-center justify-between text-[10px] leading-none' }, [
        h('span', {
          'data-testid': 'pool-quota-period-label',
          class: 'text-muted-foreground font-medium shrink-0',
        }, item.label),
        item.resetText
          ? h('span', {
            'data-testid': 'pool-quota-reset-text',
            class: 'text-muted-foreground/80 tabular-nums truncate',
            title: item.resetText,
          }, item.resetText)
          : null,
      ]),
      h('div', { class: 'flex items-center gap-1.5' }, [
        h('div', { class: 'relative flex-1 h-1.5 rounded-full bg-border overflow-hidden' }, [
          h('div', {
            class: ['absolute left-0 top-0 h-full rounded-full transition-all duration-300', item.barClass],
            style: { width: `${item.remainingPercent}%` },
          }),
        ]),
        h('span', {
          'data-testid': 'pool-quota-meter-text',
          class: ['shrink-0 text-[10px] font-medium tabular-nums leading-none', item.meterClass],
        }, item.meterText),
      ]),
    ]))
  },
})
</script>
