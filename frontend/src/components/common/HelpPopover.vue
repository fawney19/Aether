<template>
  <Popover>
    <PopoverTrigger as-child>
      <button
        type="button"
        class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-muted-foreground/60 transition-colors hover:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background cursor-help"
        :aria-label="label"
        :title="label"
        @click.stop
        @mousedown.stop
      >
        <CircleHelp class="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </PopoverTrigger>
    <PopoverContent
      :side="side"
      :align="align"
      :class="contentClassValue"
    >
      <slot />
    </PopoverContent>
  </Popover>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { CircleHelp } from 'lucide-vue-next'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui'
import { cn } from '@/lib/utils'

const props = withDefaults(defineProps<{
  label: string
  side?: 'top' | 'right' | 'bottom' | 'left'
  align?: 'start' | 'center' | 'end'
  contentClass?: string
}>(), {
  side: 'top',
  align: 'center',
  contentClass: '',
})

const contentClassValue = computed(() =>
  cn(
    'z-[240] max-w-72 px-3 py-2 text-xs leading-5',
    props.contentClass
  )
)
</script>
