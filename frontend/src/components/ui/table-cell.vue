<script setup lang="ts">
import type { ClassValue } from 'clsx'
import { cn } from '@/lib/utils'
import { computed } from 'vue'

interface Props {
  class?: ClassValue
}

const props = defineProps<Props>()

const cellClass = computed(() =>
  cn('p-4 align-middle [&:has([role=checkbox])]:pr-0', props.class)
)
</script>

<template>
  <td :class="cellClass">
    <slot />
  </td>
</template>
