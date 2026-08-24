<template>
  <div class="flex items-center justify-between mb-1">
    <span class="text-[10px] text-muted-foreground">
      {{ title }}
    </span>
    <div class="flex items-center gap-1">
      <button
        v-if="showRefresh"
        type="button"
        class="inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-[9px] font-medium text-primary hover:bg-primary/10 disabled:cursor-not-allowed disabled:opacity-50"
        :disabled="loading"
        data-testid="provider-quota-query-button"
        @click="$emit('query')"
      >
        <RefreshCw
          class="w-3 h-3"
          :class="{ 'animate-spin': loading }"
        />
        查询额度
      </button>
      <RefreshCw
        v-else-if="loading"
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
  showRefresh?: boolean
}>(), {
  loading: false,
  updatedText: null,
  showRefresh: false,
})

defineEmits<{
  query: []
}>()
</script>
