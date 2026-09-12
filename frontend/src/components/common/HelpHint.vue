<script setup lang="ts">
import { ref } from 'vue'
import { CircleHelp } from 'lucide-vue-next'

const props = defineProps<{
  label: string
  text: string
}>()

const open = ref(false)
</script>

<template>
  <span class="group relative inline-flex">
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
      class="pointer-events-none invisible absolute left-1/2 top-full z-[230] mt-2 w-max max-w-xs -translate-x-1/2 rounded-md border bg-popover px-3 py-2 text-xs leading-5 text-popover-foreground opacity-0 shadow-md transition-opacity group-hover:visible group-hover:opacity-100 group-focus-within:visible group-focus-within:opacity-100"
      :class="open ? 'visible opacity-100' : ''"
    >
      {{ props.text }}
    </span>
  </span>
</template>
