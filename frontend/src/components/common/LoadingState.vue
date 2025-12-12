<template>
  <div :class="containerClasses">
    <div class="flex flex-col items-center gap-4">
      <Skeleton
        v-if="variant === 'skeleton'"
        :class="skeletonClasses"
      />

      <div
        v-else-if="variant === 'spinner'"
        class="relative"
      >
        <div class="h-12 w-12 animate-spin rounded-full border-4 border-muted border-t-primary" />
      </div>

      <div
        v-else-if="variant === 'pulse'"
        class="flex gap-2"
      >
        <div
          v-for="i in 3"
          :key="i"
          class="h-3 w-3 animate-pulse rounded-full bg-primary"
          :style="{ animationDelay: `${i * 150}ms` }"
        />
      </div>

      <div
        v-if="message"
        class="text-sm text-muted-foreground"
      >
        {{ message }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Skeleton from '@/components/ui/skeleton.vue'

interface Props {
  variant?: 'skeleton' | 'spinner' | 'pulse'
  message?: string
  size?: 'sm' | 'md' | 'lg'
  fullHeight?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  variant: 'spinner',
  message: undefined,
  size: 'md',
  fullHeight: false,
})

const containerClasses = computed(() => {
  const classes = ['flex items-center justify-center']

  if (props.fullHeight) {
    classes.push('min-h-[400px]')
  } else {
    const sizeMap = {
      sm: 'py-8',
      md: 'py-12',
      lg: 'py-16',
    }
    classes.push(sizeMap[props.size])
  }

  return classes.join(' ')
})

const skeletonClasses = computed(() => {
  const sizeMap = {
    sm: 'h-24 w-full',
    md: 'h-48 w-full',
    lg: 'h-64 w-full',
  }

  return sizeMap[props.size]
})
</script>
