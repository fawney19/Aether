<script setup lang="ts">
import type { ClassValue } from 'clsx'
import { cn } from '@/lib/utils'
import { computed } from 'vue'

interface Props {
  class?: ClassValue
}

const props = defineProps<Props>()

const rowClass = computed(() =>
  cn(
    'border-b transition-colors hover:bg-muted/50 data-[state=selected]:bg-muted',
    props.class
  )
)
</script>

<template>
  <tr :class="rowClass">
    <slot />
  </tr>
</template>
