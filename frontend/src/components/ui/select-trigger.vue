<script setup lang="ts">
import { SelectTrigger as SelectTriggerPrimitive } from 'radix-vue'
import { ChevronDown } from 'lucide-vue-next'
import { cn } from '@/lib/utils'
import { computed } from 'vue'

interface Props {
  class?: string
  disabled?: boolean
}

const props = defineProps<Props>()

const triggerClass = computed(() =>
  cn(
    'flex h-11 w-full items-center justify-between rounded-2xl border border-border/60 bg-card/80 px-4 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary/40 focus:border-primary/60 disabled:cursor-not-allowed disabled:opacity-50 text-foreground cursor-pointer backdrop-blur transition-all',
    props.class
  )
)
</script>

<template>
  <SelectTriggerPrimitive
    v-bind="$attrs"
    :class="triggerClass"
    :disabled="disabled"
  >
    <span class="min-w-0 flex-1 truncate">
      <slot />
    </span>
    <ChevronDown class="h-4 w-4 opacity-50 pointer-events-none flex-shrink-0" />
  </SelectTriggerPrimitive>
</template>
