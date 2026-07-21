<script setup lang="ts">
import { cva } from 'class-variance-authority'
import { cn } from '@/lib/utils'
import { computed } from 'vue'

const props = withDefaults(defineProps<Props>(), {
  variant: 'default',
  class: undefined,
})

const badgeVariants = cva(
  'inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
  {
    variants: {
      variant: {
        default:
          'border-transparent bg-primary text-primary-foreground hover:bg-primary/80',
        secondary:
          'border-transparent bg-secondary text-secondary-foreground hover:bg-secondary/80',
        destructive:
          'border-transparent bg-destructive text-destructive-foreground hover:bg-destructive/80',
        outline: 'text-foreground border-border bg-card/50',
        'outline-transparent': 'text-foreground border-border bg-transparent',
        success:
          'border-transparent bg-primary text-primary-foreground hover:bg-primary/80',
        warning:
          'border-transparent bg-yellow-500 text-white hover:bg-yellow-600',
        dark:
          'border-transparent bg-foreground text-background hover:bg-foreground/80',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  }
)

interface Props {
  variant?: 'default' | 'secondary' | 'destructive' | 'outline' | 'outline-transparent' | 'success' | 'warning' | 'dark'
  class?: string
}

const badgeClass = computed(() =>
  cn(badgeVariants({ variant: props.variant }), props.class)
)
</script>

<template>
  <div :class="badgeClass">
    <slot />
  </div>
</template>
