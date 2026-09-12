<template>
  <button
    :class="triggerClass"
    :data-state="isActive ? 'active' : 'inactive'"
    :data-value="props.value"
    type="button"
    @click="handleClick"
  >
    <slot />
  </button>
</template>

<script setup lang="ts">
import { computed, inject, type Ref } from 'vue'
import { cn } from '@/lib/utils'

interface Props {
  value: string
  class?: string
}

const props = defineProps<Props>()

const activeTab = inject<Ref<string>>('activeTab')
const setActiveTab = inject<(value: string) => void>('setActiveTab')

const isActive = computed(() => activeTab?.value === props.value)

const handleClick = () => {
  setActiveTab?.(props.value)
}

const triggerClass = computed(() => {
  return cn(
    'relative z-10 inline-flex shrink-0 items-center justify-center whitespace-nowrap rounded-md px-3 py-1.5 text-sm font-medium [&_svg]:shrink-0 ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50',
    isActive.value
      ? 'text-foreground font-semibold'
      : 'text-muted-foreground hover:text-foreground',
    props.class
  )
})
</script>
