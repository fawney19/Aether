<template>
  <div class="flex items-center justify-between mb-1">
    <span class="text-[10px] text-muted-foreground">
      {{ title }}
    </span>
    <div class="flex items-center gap-1">
      <RefreshCw
        v-if="loading"
        class="w-3 h-3 text-muted-foreground/70 animate-spin"
        data-testid="provider-quota-header-loading"
      />
      <span
        v-if="updatedText"
        class="text-[9px] text-muted-foreground/70"
        data-testid="provider-quota-header-updated"
      >
        {{ updatedText }}
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { RefreshCw } from 'lucide-vue-next'

withDefaults(defineProps<{
  title: string
  loading?: boolean
  updatedText?: string | null
}>(), {
  loading: false,
  updatedText: null,
})
</script>
