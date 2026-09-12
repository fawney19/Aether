<template>
  <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
    <div class="min-w-0 flex-1">
      <div class="flex min-w-0 items-center gap-3">
        <slot name="icon">
          <div
            v-if="icon"
            class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-primary/10"
          >
            <component
              :is="icon"
              class="h-5 w-5 text-primary"
            />
          </div>
        </slot>

        <div class="min-w-0">
          <h1 class="break-words text-2xl font-semibold text-foreground sm:text-3xl">
            {{ title }}
          </h1>
          <p
            v-if="description"
            class="mt-1 break-words text-sm text-muted-foreground"
          >
            {{ description }}
          </p>
        </div>
      </div>
    </div>

    <div
      v-if="$slots.actions"
      class="flex min-w-0 flex-wrap items-center gap-2 sm:max-w-[50%] sm:justify-end"
    >
      <slot name="actions" />
    </div>
  </div>
</template>

<script setup lang="ts">
import type { Component } from 'vue'

interface Props {
  title: string
  description?: string
  icon?: Component
}

defineProps<Props>()
</script>
