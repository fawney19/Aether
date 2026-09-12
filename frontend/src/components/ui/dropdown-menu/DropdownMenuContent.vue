<script setup lang="ts">
import {
  DropdownMenuContent as DropdownMenuContentPrimitive,
  DropdownMenuPortal
} from 'radix-vue'
import { cn } from '@/lib/utils'
import { computed } from 'vue'

interface Props {
  class?: string
  sideOffset?: number
  side?: 'top' | 'right' | 'bottom' | 'left'
  align?: 'start' | 'center' | 'end'
  alignOffset?: number
  avoidCollisions?: boolean
  forceMount?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  class: undefined,
  sideOffset: 4,
  side: 'bottom',
  align: 'start',
  alignOffset: 0,
  avoidCollisions: true
})

const contentClass = computed(() =>
  cn(
    'z-[200] min-w-[8rem] max-w-[calc(100vw-1rem)] max-h-[var(--radix-dropdown-menu-content-available-height)] overflow-y-auto overscroll-contain rounded-2xl border border-border bg-card p-1 text-foreground shadow-2xl backdrop-blur-xl',
    'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
    'data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2',
    props.class
  )
)
</script>

<template>
  <DropdownMenuPortal>
    <DropdownMenuContentPrimitive
      :class="contentClass"
      :side-offset="sideOffset"
      :side="side"
      :align="align"
      :align-offset="alignOffset"
      :avoid-collisions="avoidCollisions"
      :force-mount="forceMount"
    >
      <slot />
    </DropdownMenuContentPrimitive>
  </DropdownMenuPortal>
</template>
