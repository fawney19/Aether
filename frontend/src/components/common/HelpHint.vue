<script setup lang="ts">
import { computed, ref } from 'vue'
import { CircleHelp } from 'lucide-vue-next'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'

const props = defineProps<{
  label: string
  text: string
  portal?: boolean
  /** Extra classes for the tooltip bubble, e.g. a wider max-width for long
   * help texts. */
  contentClass?: string
}>()

const open = ref(false)

const bubbleClass = computed(() =>
  cn('max-w-xs whitespace-pre-line text-xs leading-5', props.contentClass),
)
</script>

<template>
  <TooltipProvider v-if="props.portal">
    <Tooltip v-model:open="open">
      <TooltipTrigger as-child>
        <button
          type="button"
          class="inline-flex shrink-0 items-center justify-center rounded-sm p-0.5 text-muted-foreground/60 transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          :aria-label="`${props.label}说明`"
          :aria-expanded="open"
          @click.stop.prevent="open = !open"
        >
          <CircleHelp class="h-3.5 w-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent
        side="right"
        :class="bubbleClass"
      >
        {{ props.text }}
      </TooltipContent>
    </Tooltip>
  </TooltipProvider>
  <span
    v-else
    class="group relative inline-flex"
  >
    <button
      type="button"
      class="inline-flex items-center justify-center rounded-sm p-0.5 text-muted-foreground/60 transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      :aria-label="`${props.label}说明`"
      :aria-expanded="open"
      :title="props.text"
      @click.stop="open = !open"
    >
      <CircleHelp class="h-3.5 w-3.5" />
    </button>
    <span
      role="tooltip"
      class="pointer-events-none invisible absolute left-1/2 top-full z-[230] mt-2 w-max max-w-xs -translate-x-1/2 whitespace-pre-line rounded-md border bg-popover px-3 py-2 text-xs leading-5 text-popover-foreground opacity-0 shadow-md transition-opacity group-hover:visible group-hover:opacity-100 group-focus-within:visible group-focus-within:opacity-100"
      :class="open ? 'visible opacity-100' : ''"
    >
      {{ props.text }}
    </span>
  </span>
</template>
